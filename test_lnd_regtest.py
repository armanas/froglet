import asyncio
import hashlib
import os
import unittest

import aiohttp

from test_support import (
    VALID_WASM_HEX,
    FrogletAsyncTestCase,
    build_wasm_request,
    create_protocol_deal,
    create_protocol_quote,
    generate_schnorr_signing_key,
    sha256_hex,
    start_lnd_regtest_cluster,
)


@unittest.skipUnless(
    os.getenv("FROGLET_RUN_LND_REGTEST") == "1",
    "requires FROGLET_RUN_LND_REGTEST=1 and Docker",
)
class LndRegtestIntegrationTests(FrogletAsyncTestCase):
    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()
        self.cluster = await start_lnd_regtest_cluster()
        self.addAsyncCleanup(self.cluster.stop)
        self.node_data_dir = self.cluster.temp_root / "froglet-node-data"
        self.node_env = {
            **self.cluster.lightning_env("bob"),
            "FROGLET_PRICE_EXEC_WASM": "30",
        }
        self.node = await self.start_node(
            data_dir=self.node_data_dir,
            extra_env=self.node_env,
        )

    async def _open_lightning_deal(self, label: str):
        preimage = hashlib.sha256(label.encode("utf-8")).digest()
        success_payment_hash = sha256_hex(preimage)
        requester_key = generate_schnorr_signing_key()

        async with aiohttp.ClientSession() as session:
            quote = await create_protocol_quote(
                session,
                self.node,
                offer_id="execute.wasm",
                request=build_wasm_request(VALID_WASM_HEX),
                requester_secret_key=requester_key,
            )

            deal = await create_protocol_deal(
                session,
                self.node,
                quote=quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_secret_key=requester_key,
                idempotency_key=f"lnd-regtest-{label}",
                success_payment_hash=success_payment_hash,
            )

            async with session.get(
                self.node.url(f"/v1/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

        success_leg = bundle["bundle"]["payload"]["success_fee"]
        self.assertFalse(
            success_leg["invoice_bolt11"].startswith("lnmock-"),
            "regtest hold invoice should be real BOLT11, not a mock invoice",
        )
        pay_proc = self.cluster.pay_invoice_async("alice", success_leg["invoice_bolt11"])
        await self.cluster.wait_invoice_state(
            "bob",
            success_leg["payment_hash"],
            "ACCEPTED",
            timeout=60.0,
        )
        await self.wait_for_deal_status_in_db(
            self.node,
            deal["deal_id"],
            {"result_ready"},
            timeout=60.0,
        )
        return {
            "quote": quote,
            "deal": deal,
            "bundle": bundle,
            "preimage_hex": preimage.hex(),
            "pay_proc": pay_proc,
        }

    async def test_lnd_regtest_hold_invoice_flow_and_restart_recovery(self) -> None:
        settled = await self._open_lightning_deal("settle")

        async with aiohttp.ClientSession() as session:
            async with session.post(
                self.node.url(f"/v1/deals/{settled['deal']['deal_id']}/release-preimage"),
                json={"success_preimage": settled["preimage_hex"]},
            ) as resp:
                self.assertEqual(resp.status, 200)
                released = await resp.json()

        self.assertEqual(released["status"], "succeeded")
        await self.cluster.wait_invoice_state(
            "bob", settled["bundle"]["bundle"]["payload"]["success_fee"]["payment_hash"],
            "SETTLED",
            timeout=60.0,
        )
        settle_code, settle_out, settle_err = await self.cluster.wait_payment_process(
            settled["pay_proc"], timeout=30.0
        )
        self.assertEqual(settle_code, 0, f"stdout:\n{settle_out}\nstderr:\n{settle_err}")

        canceled = await self._open_lightning_deal("cancel")
        await self.cluster.cancel_hold_invoice(
            "bob", canceled["bundle"]["bundle"]["payload"]["success_fee"]["payment_hash"],
        )
        await self.cluster.wait_invoice_state(
            "bob", canceled["bundle"]["bundle"]["payload"]["success_fee"]["payment_hash"],
            "CANCELED",
            timeout=60.0,
        )
        canceled_deal = await self.wait_for_deal(
            self.node,
            canceled["deal"]["deal_id"],
            timeout=60.0,
        )
        self.assertEqual(canceled_deal["status"], "failed")
        cancel_code, _, _ = await self.cluster.wait_payment_process(
            canceled["pay_proc"], timeout=30.0
        )
        self.assertNotEqual(cancel_code, 0)

        recovered = await self._open_lightning_deal("restart-recovery")
        await self.node.stop()
        await self.cluster.settle_hold_invoice("bob", recovered["preimage_hex"])
        await self.cluster.wait_invoice_state(
            "bob", recovered["bundle"]["bundle"]["payload"]["success_fee"]["payment_hash"],
            "SETTLED",
            timeout=60.0,
        )
        restart_code, restart_out, restart_err = await self.cluster.wait_payment_process(
            recovered["pay_proc"], timeout=30.0
        )
        self.assertEqual(restart_code, 0, f"stdout:\n{restart_out}\nstderr:\n{restart_err}")

        self.node = await self.start_node(
            data_dir=self.node_data_dir,
            extra_env=self.node_env,
        )
        status = await self.wait_for_deal_status_in_db(
            self.node,
            recovered["deal"]["deal_id"],
            {"succeeded"},
            timeout=60.0,
        )
        self.assertEqual(status, "succeeded")

        async with aiohttp.ClientSession() as session:
            async with session.get(
                self.node.url(f"/v1/deals/{recovered['deal']['deal_id']}")
            ) as resp:
                self.assertEqual(resp.status, 200)
                recovered_deal = await resp.json()

        self.assertEqual(recovered_deal["status"], "succeeded")
