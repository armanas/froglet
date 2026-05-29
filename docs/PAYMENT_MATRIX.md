# Payment Verification Matrix

Status: living document.
Last refreshed: 2026-05-15 (v0.2.0 cut).

This is the single source of truth for **which payment rails Froglet
supports, in which modes, with which test coverage, and how to re-run any
cell**. It exists so that "does Froglet support X?" has one answer, and so
that regressions in a specific rail × mode cell fail a release gate rather
than a launch post.

### Publish-path posture (free + Lightning)

The publish path (the `marketplace_publish` MCP / `froglet-node publish` CLI
narrative) supports **free (`none`) and Lightning** settlement:

- `froglet-protocol::manifest::validate_settlement_method` accepts
  `settlement.method` in {`"none"`, `"lightning"`} and fails closed on any
  other value. Lightning is settled via the daemon's hold-invoice escrow
  (base fee + success fee, preimage as proof). Publishing a paid Lightning
  service requires a Lightning backend on the node
  (`FROGLET_PAYMENT_BACKEND=lightning`).
- Stripe and x402 are defined in the protocol kernel and supported by their
  settlement drivers at runtime, but are **not yet exposed on the publish
  path**: fiat cannot produce a third-party-verifiable settlement proof, so
  Stripe is deferred pending a labeled-proof design. Self-hosted operators
  can still issue those offers via the lower-level provider API.

The hosted Lightning + Stripe rows below are evidence that the **code is
ready**; Lightning is now on the publish path, Stripe/x402 are not yet.

## 1. Supported rails and modes

Four payment backends live in [src/config.rs](../src/config.rs)'s
`PaymentBackend` enum. Each has its own settlement driver in
[src/settlement/](../src/settlement/).

| Backend | Driver | Modes | Purpose |
| --- | --- | --- | --- |
| `None` | [none.rs](../src/settlement/none.rs) | — | Free-only deals. Used in local compose smoke, conformance tests, and the public `try.froglet.dev` demo catalog. |
| `Lightning` | [lightning.rs](../src/settlement/lightning.rs) | `Mock`, `LndRest` | BOLT11 invoices for local/self-hosted nodes. `Mock` is deterministic + in-memory for unit tests; `LndRest` talks to any LND REST endpoint (regtest, signet, or mainnet). |
| `X402` | [x402.rs](../src/settlement/x402.rs) | — | Local/self-hosted HTTP 402 challenge/response; a lightweight cryptographic settlement rail suitable for agent-to-agent calls. |
| `Stripe` | [stripe.rs](../src/settlement/stripe.rs) | — | Local/self-hosted fiat via Stripe PaymentIntents (Multi-Party Payments / Stripe Connect). |

"Modes" are a property of the Lightning backend; the other backends are
single-mode. The `Mock` Lightning mode is **for tests only** — production
operators set `LightningMode::LndRest`.

## 2. Verification matrix

Columns are verification modes; rows are rails. Each cell states the current
status and how to re-run it. `gate` columns map to
[scripts/release_gate.sh](../scripts/release_gate.sh) flags.

Legend: **🟢 covered** / **🟡 partial** / **⬜ not covered** / **— not applicable**.

| Rail / mode | Unit (Rust `#[test]`) | Local integration | Hosted sandbox | Hosted live | Failure injection | Restart recovery | Observability |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `None` | 🟢 covered in `payments_and_discovery.rs` + most `api/mod.rs` tests; verified 2026-05-15 | 🟢 local compose smoke (`release_gate.sh --compose`) | 🟢 `marketplace.froglet.dev` /v1/providers /v1/offers /v1/stats verified 2026-05-15 via `scripts/hosted_smoke.sh` (5/5) | 🟢 v0.2 launch posture: free hosted catalog is the headline product; `marketplace_publish` MCP + CLI tested via 25-test Phase 4 harness | 🟢 `PaymentBackend::None` is the default fallback when rails misconfigure | — | 🟢 settlement state reads as "free" via MCP `get_settlement_state` |
| `Lightning::Mock` | 🟢 Mock-Lightning logic exercised via `tests/payments_and_discovery.rs` integration suite (6/6 pass, verified 2026-05-15); stripe/x402 in-file tests also exercise Mock-Lightning as their settlement substrate | 🟢 `payments_and_discovery.rs` 6/6 pass (verified 2026-05-15): mock invoice bundle persists across reload, quote/deal commitment validation, randomized invoice-bundle validation reports targeted issues, payments_enforce_all_error_paths | — | — | 🟢 mock can be forced to return failure in tests | 🟢 Mock state persists in sqlite between restarts (covered by `lightning_mock_invoice_bundle_persists_and_updates_state`) | 🟢 settlement state via MCP |
| `Lightning::LndRest` | 🟢 unit coverage of bundle builder, quote expiry, WALLET INTENT in lightning.rs; verified 2026-05-15 | 🟢 6/6 fake-LND-REST integration tests pass (`tests/lnd_rest_settlement.rs`, verified 2026-05-15) covering BOLT11 invoice issuance, bundle cancellation, backend cancellation reflection, orphaned-materialization recovery, and issue-delay tolerance. **Real-LND regtest** (`python/tests/test_lnd_regtest.py::test_lnd_regtest_hold_invoice_flow_and_restart_recovery`) passed 2026-05-15 in 78.8s end-to-end: Docker + bitcoind + 2 LND nodes (alice + bob, `lightninglabs/lnd:v0.20.0-beta`), hold-invoice issued by bob, paid by alice, success-fee settled through the Froglet provider, restart-recovery semantics verified. See [§ 7. Regtest run log](#7-regtest-run-log). | 🟡 mainnet test harness in place (`python/tests/test_lnd_mainnet.py`, double-gated on `FROGLET_RUN_LND_MAINNET=1` + `~/.froglet/voltage/lightning.env`); blocked on inbound channel liquidity on the Voltage node (`channel_remote_sats = 0` as of 2026-05-15). See [§ 8. Mainnet run log](#8-mainnet-run-log) — empty until first real-money settlement clears. | ⬜ v0.3 publish-path follow-up; daemon supports mainnet Lightning today (`test_lnd_mainnet.py` will prove it once channels open), `marketplace_publish` does not yet | 🟢 timeout + cancellation tested in fake-LND-REST integration; restart-recovery exercised in the live regtest run | 🟢 invoice bundle state + preimage persistence verified across process restart in both `tests/lnd_rest_settlement.rs` and the live regtest (`test_lnd_regtest_hold_invoice_flow_and_restart_recovery`, 2026-05-15) | 🟢 settlement state + invoice-bundle status via MCP |
| `X402` | 🟢 9 tests in [x402.rs `mod tests`](../src/settlement/x402.rs) covering token parsing, amount/network checks, facilitator verify/settle response handling, and driver receipts (verified 2026-05-15) | 🟡 local driver path covered with mock facilitator tests; no live facilitator or compose-paid smoke today | ⬜ v0.3 follow-up | ⬜ v0.3 follow-up | 🟡 invalid amount/network and facilitator rejection are tested; replay/nonce and flaky-peer behavior are not simulated | 🟢 challenge state is stateless per-request; no restart state to recover | 🟢 settlement state via MCP |
| `Stripe` (MPP/Connect) | 🟢 6 tests in [stripe.rs `mod tests`](../src/settlement/stripe.rs) covering intent creation, capture, refund, error mapping (verified 2026-05-15) | 🟡 Stripe driver tested against a **local mock HTTP server**; one operator-run Stripe sandbox smoke on 2026-04-30: local `/v1/node/events/query` returned `stripe_mpp` receipt status `committed` with a `pi_` PaymentIntent reference. Webhook signature verification and event-id dedupe covered in `python/tests/test_payments.py` | 🟡 public VM-backed `paid-staging.froglet.dev` smoke passed on 2026-04-30 (last refresh); evidence above is point-in-time and has not been re-run for v0.2. The hosted endpoint is in the private `froglet-services` workspace; re-running requires deployment access | ⬜ v0.3 publish-path follow-up; production live-money Stripe not yet wired to `marketplace_publish` | 🟡 API error mapping exercised; webhook signature failure + duplicate delivery tested locally and on paid-staging as of 2026-04-30 | 🟡 VM-backed restart replay passed 2026-04-30 (replaying `evt_froglet_restart_1777551288` returned `duplicate:true`); not re-verified for v0.2 | 🟢 settlement state via MCP |

Hosted paid cells are intentionally separate from `try.froglet.dev`. The public
hosted proof stays free-only; Stripe hosted-sandbox evidence comes from
`paid-staging.froglet.dev` and
`../froglet-services/ops/paid_staging_stripe_smoke.sh`.

## 3. How to re-run a cell

Every cell in the matrix has one canonical entrypoint.

```bash
# All unit + in-file tests for the settlement drivers:
./scripts/release_gate.sh  # runs strict_checks.sh, which runs `cargo test --all-targets`

# Local Lightning regtest (requires a reachable LND regtest node):
./scripts/release_gate.sh --lnd-regtest

# Local compose smoke for the free/default path:
./scripts/release_gate.sh --compose

# Hosted Stripe sandbox smoke, after paid-staging is deployed:
(cd ../froglet-services && \
  FROGLET_PAID_STAGING_URL=https://paid-staging.froglet.dev \
  ./ops/paid_staging_stripe_smoke.sh)

# Hosted Stripe synthetic webhook probe, after the probe Worker is deployed:
curl -fsS https://paid-staging-stripe-probe.froglet.dev/health
```

For a single rail + single cell, invoke the underlying test directly:

```bash
# Stripe unit tests only:
cargo test --lib settlement::stripe

# x402 unit tests only:
cargo test --lib settlement::x402

# LND regtest integration tests only:
FROGLET_RUN_LND_REGTEST=1 cargo test --test lnd_rest_settlement
python3 -W error -m unittest python.tests.test_lnd_regtest
```

## 4. Observability contract

Every rail exposes settlement state through the same MCP surface:

- **MCP action `get_settlement_state`** returns the current settlement-driver
  verdict for a deal (invoice status for Lightning, challenge status for
  X402, intent status for Stripe, "free" for None). Landed in commit
  `2ca1aa3` ("Expose settlement state to the MCP tool surface").
- **Structured logs** — every settlement-driver call logs an event with the
  rail, deal id, operation, and outcome. Inspectable via `docker logs` on
  local compose; hosted log aggregation is planned separately.
- **Capabilities endpoint** — `/v1/node/capabilities` reports which rails are
  configured as `payment_backends` in the running node, so operators can verify
  at a glance that the rail they configured is actually enabled. `/health` only
  reports process health.

## 5. Known gaps (explicit, not deferred-by-accident)

These are intentionally outside the current matrix. They are listed here so
a reviewer can see they are known and tracked, not forgotten.

- **No multi-rail concurrent test** — a single deal that tries to settle on
  Lightning first and fall back to Stripe is not currently exercised.
  Decision pending: is multi-rail-per-deal a supported product shape, or is
  rail-per-deal the contract? The codebase today assumes rail-per-deal.
- **No chaos testing** — no broad fault injection at the network layer (e.g.
  dropped Stripe webhook deliveries beyond signature validation and the
  paid-staging synthetic webhook probe, LND channel force-close
  mid-settlement).
- **No cross-rail dispute resolution** — dispute handling comes from the
  arbiter service, which operates at the marketplace layer, not inside a
  single-rail settlement driver.
- **No load testing** — throughput characteristics per rail are not
  measured. Should exist before any launch that claims "production ready";
  for a permissionless-alpha launch, deferring is OK if documented.
- **No paid async job settlement** — `/v1/node/jobs` accepts only free async
  jobs today. Priced Lightning work must use the signed quote/deal flow, while
  Stripe/x402 token-settled work must use synchronous `/v1/node/events/query`
  or `/v1/node/execute/wasm` until durable async payment reservations exist.
- **PayPal** is out of scope for the current release scope.

## 6. What changes require an update to this matrix

- Adding or removing a row in `PaymentBackend` or `LightningMode`.
- Adding a new verification column (e.g. "Hosted live" when the first public
  instance is up).
- Moving a cell from 🟡 / ⬜ to 🟢, or vice versa, in either direction.
- Landing or closing any hosted-payment milestone that changes the matrix.

The release gate runs the unit + local-integration cells on every PR; hosted
cells light up through the separate first-party operator smoke once real URLs
exist.

## 7. Regtest run log

This is the canonical evidence trail for `Lightning::LndRest` "local
integration". Each row records a real-LND regtest run, not a fake-LND-REST
unit test.

| Date | Test | Duration | Result | Notes |
| --- | --- | --- | --- | --- |
| 2026-05-15 | `python.tests.test_lnd_regtest::test_lnd_regtest_hold_invoice_flow_and_restart_recovery` | 78.8s | ✅ pass | First real-LND regtest run post v0.2.0 cut. Docker `lightninglabs/lnd:v0.20.0-beta` + `ruimarinho/bitcoin-core:24`, 2 LND nodes (alice + bob), real BOLT11 hold invoice, restart-recovery verified. Containers cleaned up automatically. |

### Reproduce the latest run

```bash
# Requires Docker daemon running. First run pulls ~600MB of images.
FROGLET_RUN_LND_REGTEST=1 python3 -m unittest -v python.tests.test_lnd_regtest
```

What it does end-to-end:

1. Spins up a docker network with bitcoind (`ruimarinho/bitcoin-core:24`)
   and two LND nodes (alice + bob, `lightninglabs/lnd:v0.20.0-beta`).
2. Mines initial blocks, opens a payment channel alice → bob.
3. Builds a Froglet provider configured to use bob's LND REST endpoint for
   settlement (`PaymentBackend::Lightning` / `LightningMode::LndRest`).
4. Creates a signed Froglet quote + deal through the provider API; the
   provider materialises a real BOLT11 hold invoice on bob.
5. Alice pays the invoice; bob observes `ACCEPTED` state (hold invoice
   intermediate).
6. Provider settles the success-fee leg with the preimage; bob observes
   `SETTLED`.
7. Provider process is restarted with the same data directory; bundle
   state is verified to survive the restart (preimage + settled state
   intact).
8. Cluster is torn down (containers stopped + removed, temp data dir
   cleaned).

Adding a row here for each scheduled regtest run keeps the
"local integration: 🟢" claim honest. Stale-dated rows are a signal that
the test hasn't been re-run; if more than a release cycle passes without
a fresh row, downgrade the cell to 🟡.

## 8. Mainnet run log

This is the evidence trail for the `Lightning::LndRest` "Hosted live"
column. The test is `python.tests.test_lnd_mainnet` and exercises a real
mainnet Lightning settlement against a Voltage LND node — provider issues
a real BOLT11 hold invoice, operator pays it from a separate funding
wallet, provider releases the preimage, receipt is durable.

| Date | Test | Amount | Result | Notes |
| --- | --- | --- | --- | --- |
| — | _no mainnet runs yet_ | — | — | The test is in place (`python/tests/test_lnd_mainnet.py`). First run is blocked on inbound channel liquidity on the Voltage node (`channel_remote_sats = 0` as of 2026-05-15). |

### Reproduce a mainnet run

The test is **double-gated**: it skips unless `FROGLET_RUN_LND_MAINNET=1`
is set AND `~/.froglet/voltage/lightning.env` exists. The env file is
written by `ops/voltage_lnd.sh materialize` in the `froglet-services`
repo from Keychain-stored Voltage credentials. The test never moves
funds itself; the operator pays the invoice externally.

```bash
# In the froglet-services repo, refresh the Voltage env + confirm inbound:
cd ~/Projects/github.com/armanas/froglet-services
./ops/voltage_lnd.sh materialize
./ops/voltage_lnd.sh balance   # confirm channel_remote_sats > 50000

# In the froglet repo, run the gated test:
cd ~/Projects/github.com/armanas/froglet
FROGLET_RUN_LND_MAINNET=1 python3 -m unittest -v python.tests.test_lnd_mainnet
```

What the test does end-to-end:

1. Parses `~/.froglet/voltage/lightning.env` for the Voltage REST URL,
   macaroon path, and TLS cert path.
2. Probes the Voltage node via `GET /v1/getinfo` to confirm credentials
   work and at least one active channel exists. Fails fast if not.
3. Builds a local Froglet provider configured with the Voltage
   credentials (`PaymentBackend::Lightning` / `LightningMode::LndRest`).
4. Creates a signed Froglet quote + deal; provider materialises a real
   mainnet BOLT11 hold invoice on Voltage.
5. Prints the invoice and the sat amount to stdout and **waits up to
   10 minutes** for the operator to pay it from a separate Lightning
   wallet (Phoenix, Wallet of Satoshi, Alby, another LND, etc.).
6. Polls the local provider's `/v1/provider/deals/{id}` until the
   provider observes the invoice ACCEPTED via Voltage (deal status
   flips to `result_ready`).
7. Releases the preimage via `POST /v1/provider/deals/{id}/accept`.
8. Verifies the deal lands in `succeeded` and is durable in the
   provider DB.

Typical invoice amount per run: 1-5 sats (~$0.005 USD at $60k BTC). The
test prints the amount before asking for payment so the operator can
refuse if it looks wrong.

After a successful run, append a row to the table above with the date,
sat amount, and any operator notes. Stale-dated rows are a signal that
the cell should downgrade.
