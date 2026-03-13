# Froglet Operator Guide

Status: practical operating guidance for the current runtime

This document describes the operational steps that matter for the bot-facing alpha surface.

## 1. Minimal Local Setup

For local bot development, the easiest path is mock Lightning:

```bash
FROGLET_PRICE_EXEC_WASM=10 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet
```

That gives you:

- a priced deal flow
- runtime-issued payment intents
- deterministic local settlement control through the mock-Lightning runtime route

For free local development:

```bash
cargo run --bin froglet
```

## 2. Runtime Auth Token

Privileged runtime calls require a bearer token.
The default token path is:

- `./data/runtime/auth.token`

The token is local operational state, not a protocol artifact.
Bots should read it from disk rather than hardcode it.

The current runtime bootstrap snapshot is:

- `POST /v1/runtime/provider/start`

That returns the current descriptor, current offers, and the runtime token path that the node is using.

## 3. Wallet Modes

Supported operating modes today:

- `FROGLET_PAYMENT_BACKEND=none`
  - free local execution only
- `FROGLET_PAYMENT_BACKEND=lightning` with `FROGLET_LIGHTNING_MODE=mock`
  - preferred for local bot development and examples
- `FROGLET_PAYMENT_BACKEND=lightning` with `FROGLET_LIGHTNING_MODE=lnd_rest`
  - real Lightning boundary

For `lnd_rest`, configure:

- `FROGLET_LIGHTNING_REST_URL`
- `FROGLET_LIGHTNING_TLS_CERT_PATH`
- `FROGLET_LIGHTNING_MACAROON_PATH`
- `FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS`
- `FROGLET_LIGHTNING_SYNC_INTERVAL_MS`

Use `GET /v1/runtime/wallet/balance` to confirm the runtime sees the configured wallet backend.

## 4. Bot Runtime Workflow

The intended local operator workflow is:

1. Start Froglet.
2. Hand the bot the runtime auth token path.
3. Let the bot call `RuntimeClient.buy_service(...)` or equivalent.
4. Let the bot wait on `payment_pending`, `result_ready`, or a terminal state.
5. Let the bot call `accept_result(...)` only when it is ready to release the success-fee preimage.
6. Verify the terminal receipt.

For local mock-Lightning flows, the runtime also exposes:

- `POST /v1/runtime/lightning/invoice-bundles/:session_id/state`

That route is useful for development, examples, and deterministic local testing.

## 5. Archive Export

Use:

- `GET /v1/runtime/archive/deal/:deal_id`
- `GET /v1/runtime/archive/job/:job_id`

The archive export is the main operator-facing evidence surface.
It retains:

- artifact documents
- artifact feed entries
- execution evidence
- retained Lightning invoice-bundle material

Use archive export when:

- debugging a failed deal
- preserving evidence before cleanup
- comparing local state with a third-party verifier

## 6. Restart and Recovery

Current recovery behavior:

- if the node restarts while a deal is locally `accepted` or `running`, Froglet emits a signed terminal failure receipt for that interrupted deal
- Lightning-backed `payment_pending` and `result_ready` deals are reconciled by the settlement watcher after restart

Operationally, after a restart:

1. inspect the deal status
2. inspect the terminal receipt if one exists
3. export the runtime archive if the interaction matters

## 7. Publication and Discovery

The runtime can build local publication intents through:

- `POST /v1/runtime/services/publish`
- `GET /v1/runtime/nostr/publications/provider`
- `GET /v1/runtime/nostr/publications/deals/:deal_id/receipt`

Relay publication stays outside the core node.
Use [froglet_nostr_adapter.py](froglet_nostr_adapter.py) when relay interaction is needed.

## 8. Recommended Alpha Defaults

For the current bot/runtime alpha:

- use `mock` Lightning for local bot integration
- use `lnd_rest` only when you intentionally want real settlement testing
- keep the bot on the runtime surface rather than lower-level compatibility endpoints
- use archive export as the first debugging tool when a deal does not land where expected

Runnable examples are in [examples/README.md](examples/README.md).
