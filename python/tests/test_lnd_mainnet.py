"""End-to-end Lightning settlement test against a real mainnet LND node.

This is the mainnet sibling of ``test_lnd_regtest.py``. The regtest version
exercises the full flow against two LND containers in Docker; this one
exercises the same flow against a real Voltage LND node on mainnet.

The test is gated TWICE so it can never run by accident:

  1. ``FROGLET_RUN_LND_MAINNET=1`` must be set. Without it the entire
     class is ``unittest.skipUnless``-skipped.
  2. ``~/.froglet/voltage/lightning.env`` must exist (the operator runs
     ``ops/voltage_lnd.sh materialize`` in the ``froglet-services`` repo
     to populate this from the Keychain-stored Voltage credentials).
     Without it the setUp raises a clear error before any network I/O.

The test never moves funds itself. It creates a small signed Froglet quote
and deal, asks the local provider to materialize a real BOLT11 hold
invoice on Voltage, prints the invoice to stdout, and **waits for the
operator to pay it from a separate Lightning wallet** (Phoenix, Wallet
of Satoshi, Blue Wallet, Alby, another LND, etc.). Once the deal status
flips to ``result_ready`` (the provider has seen the invoice ACCEPTED on
Voltage), the test releases the preimage via ``/v1/provider/deals/.../accept``
and verifies the deal lands in ``succeeded``.

Sat amount is intentionally tiny (the offer is priced via
``FROGLET_PRICE_EXEC_WASM=30`` msat per WASM compute unit, matching the
regtest test). At current rates a typical test invoice is ~1-5 sats
(<$0.005 USD). The exact amount is printed before the invoice prompt
so the operator can refuse if it looks wrong.

Operator runbook:

  1. Ensure Voltage credentials are in Keychain via
     ``ops/voltage_lnd.sh ingest`` (one-time setup).
  2. Refresh the local env file:
     ``ops/voltage_lnd.sh materialize``
  3. Confirm channel liquidity:
     ``ops/voltage_lnd.sh balance``
     Look for an active channel with inbound receive capacity. For a
     comfortable test, target ``channel_remote_sats > 50000``; this is not
     a requirement to fund the node with 1M sats.
  4. Have a separate Lightning wallet ready with ~5,000 sats.
  5. From the ``froglet`` repo:
       ``FROGLET_RUN_LND_MAINNET=1 python3 -m unittest -v python.tests.test_lnd_mainnet``
  6. When the test prints ``PAYMENT REQUIRED:`` and a BOLT11 invoice,
     paste the invoice into your funding wallet and pay it.
  7. Test polls the provider's deal status; once it sees ``result_ready``
     it releases the preimage automatically and verifies ``succeeded``.

Run log lives in ``docs/PAYMENT_MATRIX.md`` § 7 (regtest run log) — add
a new ``Mainnet run log`` subsection after each successful run, mirroring
the regtest entries so stale-dated rows downgrade the cell to 🟡.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import sys
import time
import unittest
from pathlib import Path

import aiohttp

from test_support import (
    VALID_WASM_HEX,
    FrogletAsyncTestCase,
    build_wasm_request,
    create_protocol_deal,
    create_protocol_quote,
    generate_schnorr_signing_key,
    sha256_hex,
)


VOLTAGE_ENV_PATH = Path(os.path.expanduser(
    os.environ.get("FROGLET_VOLTAGE_ENV_PATH", "~/.froglet/voltage/lightning.env")
))

# Operator may need a few minutes to copy/paste and approve the payment
# in their wallet. 10-minute cap is generous but bounded so a forgotten
# test doesn't hang CI if someone misconfigures the gate.
PAYMENT_WAIT_SECS = 600.0

# Same offer pricing the regtest test uses, so a typical invoice is
# ~1-5 sats. Keep this in sync with test_lnd_regtest.py.
PRICE_EXEC_WASM_MSAT = "30"

# Same poll cadence as wait_for_deal_status_in_db internally uses.
POLL_INTERVAL_SECS = 2.0


def _load_voltage_env(env_path: Path) -> dict[str, str]:
    """Parse a Voltage lightning.env file written by ops/voltage_lnd.sh
    materialize.

    The file is a simple ``KEY=VALUE`` env file (no quoting, no shell
    expansion). We do NOT shell out to ``source`` for two reasons:

      1. The macaroon path the env file points at is in
         ``~/.froglet/voltage/`` with mode 0600 — only the operator can
         read it. The test process must inherit the operator's uid; if
         it doesn't (e.g. CI), it fails at probe time, which is the
         right failure mode.
      2. We never print the file contents back to stdout. The operator
         can audit the file directly with ``cat ~/.froglet/voltage/lightning.env``
         if they want.
    """
    if not env_path.exists():
        raise RuntimeError(
            f"Voltage env file not found at {env_path}.\n"
            "Run `ops/voltage_lnd.sh materialize` in the froglet-services "
            "repo to generate it from Keychain. If you have not stored "
            "credentials yet, run `ops/voltage_lnd.sh ingest --rest-url ... "
            "--macaroon ... --tls-cert ...` first."
        )
    env: dict[str, str] = {}
    for line in env_path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise RuntimeError(f"malformed line in {env_path}: {line!r}")
        key, _, value = line.partition("=")
        env[key.strip()] = value.strip()
    required = {
        "FROGLET_PAYMENT_BACKEND",
        "FROGLET_LIGHTNING_MODE",
        "FROGLET_LIGHTNING_REST_URL",
        "FROGLET_LIGHTNING_TLS_CERT_PATH",
        "FROGLET_LIGHTNING_MACAROON_PATH",
    }
    missing = required - env.keys()
    if missing:
        raise RuntimeError(
            f"{env_path} is missing required keys: {sorted(missing)}. "
            "Re-run `ops/voltage_lnd.sh materialize`."
        )
    if env["FROGLET_PAYMENT_BACKEND"] != "lightning":
        raise RuntimeError(
            f"{env_path} sets FROGLET_PAYMENT_BACKEND={env['FROGLET_PAYMENT_BACKEND']!r}, "
            "expected 'lightning'. Did you point the test at the wrong env file?"
        )
    if env["FROGLET_LIGHTNING_MODE"] != "lnd_rest":
        raise RuntimeError(
            f"{env_path} sets FROGLET_LIGHTNING_MODE={env['FROGLET_LIGHTNING_MODE']!r}, "
            "expected 'lnd_rest'. Voltage path requires lnd_rest mode."
        )
    return env


@unittest.skipUnless(
    os.getenv("FROGLET_RUN_LND_MAINNET") == "1",
    "requires FROGLET_RUN_LND_MAINNET=1 and a configured Voltage LND node",
)
class LndMainnetIntegrationTests(FrogletAsyncTestCase):
    """Exercise a real Lightning hold-invoice settlement against Voltage.

    The test runs ONE real-money round-trip:

      1. Spawn a local froglet-node configured with Voltage LndRest.
      2. Create a signed Froglet quote + deal.
      3. Provider materializes a real BOLT11 hold invoice on Voltage.
      4. Print the invoice to stdout. Wait for operator to pay it.
      5. Poll the deal status until ``result_ready``.
      6. Release the preimage via the provider's ``/accept`` endpoint.
      7. Verify the deal lands in ``succeeded``.

    Restart-recovery is NOT exercised here. That's covered by the regtest
    suite and doesn't need a real mainnet payment to prove behavior.
    """

    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()

        env = _load_voltage_env(VOLTAGE_ENV_PATH)

        # Pre-flight probe: make sure the local Voltage credentials
        # actually reach a healthy LND node before we burn time on the
        # full setup. Failure here is cheaper than starting the provider.
        await self._probe_voltage_getinfo(env)

        self.node_env = {
            **env,
            "FROGLET_PRICE_EXEC_WASM": PRICE_EXEC_WASM_MSAT,
        }
        self.node = await self.start_provider(extra_env=self.node_env)

    async def _probe_voltage_getinfo(self, env: dict[str, str]) -> None:
        """Sanity-check that the macaroon and TLS cert reach a node that
        responds to ``GET /v1/getinfo`` before we start the provider."""
        rest_url = env["FROGLET_LIGHTNING_REST_URL"].rstrip("/")
        tls_cert_path = env["FROGLET_LIGHTNING_TLS_CERT_PATH"]
        macaroon_path = env["FROGLET_LIGHTNING_MACAROON_PATH"]
        try:
            macaroon_hex = Path(macaroon_path).read_bytes().hex()
        except OSError as e:
            raise RuntimeError(
                f"cannot read macaroon at {macaroon_path}: {e}. "
                "Re-run `ops/voltage_lnd.sh materialize`."
            ) from e

        import ssl
        ssl_ctx = ssl.create_default_context(cafile=tls_cert_path)
        conn = aiohttp.TCPConnector(ssl=ssl_ctx)
        timeout = aiohttp.ClientTimeout(total=10.0)
        async with aiohttp.ClientSession(connector=conn, timeout=timeout) as session:
            async with session.get(
                f"{rest_url}/v1/getinfo",
                headers={"Grpc-Metadata-macaroon": macaroon_hex},
            ) as resp:
                if resp.status != 200:
                    body = await resp.text()
                    raise RuntimeError(
                        f"Voltage probe failed: GET {rest_url}/v1/getinfo -> "
                        f"HTTP {resp.status}\n{body[:500]}"
                    )
                info = await resp.json()

        # Print only non-secret metadata so the operator can see the
        # test is talking to the expected node.
        print(
            f"\nVoltage probe OK:\n"
            f"  alias:           {info.get('alias')!r}\n"
            f"  identity_pubkey: {info.get('identity_pubkey', '?')[:16]}...\n"
            f"  chains:          {info.get('chains')}\n"
            f"  synced_to_chain: {info.get('synced_to_chain')}\n"
            f"  num_active_channels: {info.get('num_active_channels')}",
            flush=True,
        )

        num_active = info.get("num_active_channels", 0)
        if num_active == 0:
            raise RuntimeError(
                "Voltage node has 0 active channels. The provider needs "
                "inbound liquidity to receive a Lightning payment. Source "
                "inbound capacity through a liquidity marketplace, LSP, "
                "peer-opened channel, or swap/rebalance flow, then rerun. "
                "Confirm with `ops/voltage_lnd.sh balance` that "
                "channel_remote_sats is comfortably above the tiny test "
                "invoice amount; >50000 sats is the conservative test target."
            )

    async def test_lnd_mainnet_hold_invoice_flow(self) -> None:
        """Open a real Lightning deal, prompt operator to pay, release
        preimage, verify ``succeeded``.

        Total expected wall-clock time: ~10-60 seconds for the Froglet
        plumbing + however long the operator takes to copy/paste and
        confirm the payment in their wallet (typically <30 seconds).
        """
        label = f"mainnet-{int(time.time())}"
        preimage = hashlib.sha256(f"froglet-lnd-mainnet-{label}".encode("utf-8")).digest()
        success_payment_hash = sha256_hex(preimage)
        requester_key = generate_schnorr_signing_key()

        async with aiohttp.ClientSession() as session:
            quote = await create_protocol_quote(
                session,
                self.node,
                offer_id="execute.compute",
                request=build_wasm_request(VALID_WASM_HEX),
                requester_secret_key=requester_key,
            )

            deal = await create_protocol_deal(
                session,
                self.node,
                quote=quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_secret_key=requester_key,
                idempotency_key=f"lnd-mainnet-{label}",
                success_payment_hash=success_payment_hash,
            )

            async with session.get(
                self.node.url(f"/v1/provider/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

        success_leg = bundle["bundle"]["payload"]["success_fee"]
        invoice = success_leg["invoice_bolt11"]
        amount_msat = int(success_leg.get("amount_msat") or 0)
        amount_sats = amount_msat / 1000.0

        self.assertFalse(
            invoice.startswith("lnmock-"),
            "mainnet hold invoice must be a real BOLT11, not a mock invoice",
        )
        self.assertTrue(
            invoice.startswith("lnbc") or invoice.startswith("LNBC"),
            f"expected mainnet BOLT11 prefix lnbc..., got: {invoice[:10]!r}",
        )

        # The whole point of this test is operator visibility. Make the
        # invoice and the amount unmissable on stdout.
        bar = "=" * 72
        print(
            f"\n{bar}\nPAYMENT REQUIRED ({amount_sats:.0f} sats / {amount_msat} msat)\n"
            f"deal_id:       {deal['deal_id']}\n"
            f"payment_hash:  {success_leg['payment_hash']}\n\n"
            f"Paste this BOLT11 into your funding wallet and pay it:\n\n"
            f"  {invoice}\n\n"
            f"Waiting up to {int(PAYMENT_WAIT_SECS)}s for the invoice to be "
            f"ACCEPTED on the Voltage node...\n{bar}\n",
            flush=True,
        )

        deal_id = deal["deal_id"]
        deadline = asyncio.get_event_loop().time() + PAYMENT_WAIT_SECS
        last_status = None
        while True:
            now = asyncio.get_event_loop().time()
            if now >= deadline:
                raise AssertionError(
                    f"deal {deal_id} did not reach result_ready within "
                    f"{int(PAYMENT_WAIT_SECS)}s. Last observed status: "
                    f"{last_status!r}. Check `ops/voltage_lnd.sh balance` for "
                    "channel state and confirm the invoice was paid; if the "
                    "invoice is paid but the provider didn't observe it, "
                    "inspect the local provider's stderr/log."
                )
            async with aiohttp.ClientSession() as session:
                async with session.get(self.node.url(f"/v1/provider/deals/{deal_id}")) as resp:
                    self.assertEqual(resp.status, 200)
                    deal_state = await resp.json()
            current = deal_state.get("status")
            if current != last_status:
                remaining = int(deadline - now)
                print(
                    f"  [{remaining:>3}s left] deal status: {current!r}",
                    flush=True,
                )
                last_status = current
            if current == "result_ready":
                break
            if current == "failed":
                raise AssertionError(
                    f"deal {deal_id} transitioned to failed before reaching "
                    "result_ready. Inspect provider logs."
                )
            await asyncio.sleep(POLL_INTERVAL_SECS)

        print(f"  invoice ACCEPTED. Releasing preimage to settle...", flush=True)

        async with aiohttp.ClientSession() as session:
            async with session.post(
                self.node.url(f"/v1/provider/deals/{deal_id}/accept"),
                json={"success_preimage": preimage.hex()},
            ) as resp:
                self.assertEqual(resp.status, 200)
                released = await resp.json()

        self.assertEqual(released["status"], "succeeded")

        # Final poll to confirm the deal is durable in the provider DB.
        status = await self.wait_for_deal_status_in_db(
            self.node,
            deal_id,
            {"succeeded"},
            timeout=60.0,
        )
        self.assertEqual(status, "succeeded")

        print(
            f"\n{bar}\nMAINNET SETTLEMENT PASS\n"
            f"  deal_id:       {deal_id}\n"
            f"  amount:        {amount_sats:.0f} sats\n"
            f"  payment_hash:  {success_leg['payment_hash']}\n"
            f"  preimage:      {preimage.hex()}\n\n"
            f"Add this run to docs/PAYMENT_MATRIX.md § 7 'Mainnet run log'.\n{bar}\n",
            flush=True,
        )


if __name__ == "__main__":
    unittest.main()
