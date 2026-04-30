# Payment Verification Matrix

Status: living document.
Last refreshed: 2026-04-24.

This is the single source of truth for **which payment rails Froglet
supports, in which modes, with which test coverage, and how to re-run any
cell**. It exists so that "does Froglet support X?" has one answer, and so
that regressions in a specific rail × mode cell fail a release gate rather
than a launch post.

For v0.1.0, `try.froglet.dev` is not a hosted paid-rail proof. The public
hosted trial is a free `PaymentBackend::None` catalog: `demo.add` is the
canonical proof, while witness/hash/notarize demos can provide stronger URL or
content-hash evidence. Lightning, Stripe, and x402 are local/self-hosted launch
adapters in this release. Stripe MPP has a separate paid-staging path under
`paid-staging.froglet.dev`, with sandbox evidence recorded separately from the
free hosted proof.

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
| `None` | 🟢 covered in `payments_and_discovery.rs` + most `api/mod.rs` tests | 🟢 local compose smoke (`release_gate.sh --compose`) | — | 🟡 manual hosted trial smoke for the free demo catalog; evidence is outside this matrix | 🟢 `PaymentBackend::None` is the default fallback when rails misconfigure | — | 🟢 settlement state reads as "free" via MCP `get_settlement_state` |
| `Lightning::Mock` | 🟢 lightning.rs in-file tests + stripe/x402 tests use Mock Lightning | 🟢 `payments_and_discovery.rs` | — | — | 🟢 mock can be forced to return failure in tests | 🟢 Mock state persists in sqlite between restarts | 🟢 settlement state via MCP |
| `Lightning::LndRest` | 🟢 unit coverage of bundle builder, quote expiry, WALLET INTENT in lightning.rs | 🟢 regtest E2E via `python/tests/test_lnd_regtest.py` and `tests/lnd_rest_settlement.rs` (gated by `FROGLET_RUN_LND_REGTEST=1` or `release_gate.sh --lnd-regtest`) | ⬜ deferred to v0.2 hosted paid rails | ⬜ deferred to v0.2 hosted paid rails | 🟡 timeout + cancellation tested in regtest; on-chain dispute path not exercised | 🟢 invoice bundle state + preimage persistence verified across process restart in the regtest test | 🟢 settlement state + invoice-bundle status via MCP |
| `X402` | 🟢 9 tests in [x402.rs `mod tests`](../src/settlement/x402.rs) covering token parsing, amount/network checks, facilitator verify/settle response handling, and driver receipts | 🟡 local driver path covered with mock facilitator tests; no live facilitator or compose-paid smoke today | ⬜ deferred to v0.2 hosted paid rails | ⬜ deferred to v0.2 hosted paid rails | 🟡 invalid amount/network and facilitator rejection are tested; replay/nonce and flaky-peer behavior are not simulated | 🟢 challenge state is stateless per-request; no restart state to recover | 🟢 settlement state via MCP |
| `Stripe` (MPP/Connect) | 🟢 6 tests in [stripe.rs `mod tests`](../src/settlement/stripe.rs) covering intent creation, capture, refund, error mapping | 🟡 Stripe driver tested against a **local mock HTTP server** plus one operator-run Stripe sandbox smoke on 2026-04-30: local `/v1/node/events/query` returned `stripe_mpp` receipt status `committed` with a `pi_` PaymentIntent reference. Webhook signature verification and event-id dedupe are covered in `python/tests/test_payments.py` | 🟢 public VM-backed `paid-staging.froglet.dev` smoke passed on 2026-04-30: `/health`, `/v1/node/capabilities`, signed webhook accept, duplicate idempotency, forged-signature rejection, fresh test SPT creation, token-settled `/v1/node/events/query` with a `pi_` settlement reference, Stripe Dashboard-origin `payment_intent.succeeded` delivery returning HTTP 200, Stripe CLI resend delivery reaching `pending_webhooks=0`, and synthetic signed-webhook probe returning `status:"ok"` from `paid-staging-stripe-probe.froglet.dev` | ⬜ production/live-money Stripe not covered | 🟡 API error mapping exercised; webhook signature failure and duplicate delivery are tested locally and on paid-staging; Stripe Dashboard-origin success delivery, Stripe CLI resend, and the Cloudflare synthetic webhook probe are observed, but the external monitor alert-fire/clear drill is not yet observed | 🟢 VM-backed restart replay passed on 2026-04-30: the previous Lightsail Container Service redeploy replay failed for `evt_froglet_restart_1777543048`, then the hostname was moved to a Lightsail VM with persistent `/opt/froglet/data`; replaying `evt_froglet_restart_1777551288` after recreating the Froglet app container returned `duplicate:true` | 🟢 settlement state via MCP |

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
