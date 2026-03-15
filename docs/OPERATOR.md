# Froglet Operator Guide

Status: practical operating guidance for the current runtime

This document describes the operational steps that matter for the bot-facing alpha surface.

Guarantee boundary:

- treat the signed artifact kernel and durable local state as the stable primitive
- treat the runtime routes, marketplace flows, Tor publication, and wallet integrations as supported operational layers above that primitive
- do not assume the alpha runtime surface is frozen in the same way as [../SPEC.md](../SPEC.md)

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
- the default split local topology:
  - provider API on `127.0.0.1:8080`
  - runtime API on `127.0.0.1:8081`

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

For SDK callers, this means:

- `RuntimeClient` should point at the runtime listener
- `ProviderClient` should point at the public provider listener
- the runtime auth token should never be sent to the public provider listener

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

## 4. Transport Modes

Tor support now runs through an external `tor` sidecar process rather than an in-process Rust Tor stack.

Relevant transport settings:

- `FROGLET_NETWORK_MODE=clearnet|tor|dual`
- `FROGLET_TOR_BINARY=tor`
- `FROGLET_TOR_BACKEND_LISTEN_ADDR=127.0.0.1:8082`
- `FROGLET_TOR_STARTUP_TIMEOUT_SECS=90`

Keep `FROGLET_TOR_BACKEND_LISTEN_ADDR` on loopback. It is the local HTTP backend that the Tor sidecar exposes as the onion service.
That backend carries the same public provider routes as `FROGLET_LISTEN_ADDR`, but it is not intended for direct clients or reverse proxies.

Practical startup behavior:

1. Froglet starts the public listener when `clearnet` or `dual` mode is enabled.
2. Froglet starts the local runtime listener on `FROGLET_RUNTIME_LISTEN_ADDR`.
3. In `tor` or `dual` mode, Froglet starts a local Tor backend listener on `FROGLET_TOR_BACKEND_LISTEN_ADDR`.
4. Froglet then launches the external `tor` sidecar and waits for bootstrap plus a hidden-service hostname before reporting Tor transport `up`.

If the `tor` sidecar exits later:

- Tor transport status falls back to `down`
- in `tor` mode the node treats that as fatal
- in `dual` mode the clearnet listener keeps serving, but the onion transport is gone until restart

Recommended checks in `dual` mode:

1. Verify the public API is reachable on `FROGLET_LISTEN_ADDR`.
2. Verify the runtime API is reachable on `FROGLET_RUNTIME_LISTEN_ADDR`.
3. Read `/v1/node/capabilities` from the public API and confirm `transports.tor.status == "up"` plus a non-empty `onion_url`.

## 5. Bot Runtime Workflow

The intended local operator workflow is:

1. Start Froglet.
2. Hand the bot the runtime auth token path plus the correct runtime and provider base URLs.
3. Let the bot call `RuntimeClient.buy_service(...)` or equivalent.
4. Let the bot wait on `payment_pending`, `result_ready`, or a terminal state.
5. Let the bot call `accept_result(...)` only when it is ready to release the success-fee preimage.
6. Verify the terminal receipt.

For local mock-Lightning flows, the runtime also exposes:

- `POST /v1/runtime/lightning/invoice-bundles/:session_id/state`

That route is useful for development, examples, and deterministic local testing.

## 6. Archive Export

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

## 7. Restart and Recovery

Current recovery behavior:

- if the node restarts while a deal is locally `accepted` or `running`, Froglet emits a signed terminal failure receipt for that interrupted deal
- Lightning-backed `payment_pending` and `result_ready` deals are reconciled by the settlement watcher after restart

Operationally, after a restart:

1. inspect the deal status
2. inspect the terminal receipt if one exists
3. export the runtime archive if the interaction matters

## 8. Publication and Discovery

The runtime can build local publication intents through:

- `POST /v1/runtime/services/publish`
- `GET /v1/runtime/nostr/publications/provider`
- `GET /v1/runtime/nostr/publications/deals/:deal_id/receipt`

Relay publication stays outside the core node.
Use [../python/froglet_nostr_adapter.py](../python/froglet_nostr_adapter.py) when relay interaction is needed.

## 9. Recommended Alpha Defaults

For the current bot/runtime alpha:

- use `mock` Lightning for local bot integration
- use `lnd_rest` only when you intentionally want real settlement testing
- keep the bot on the runtime surface rather than lower-level compatibility endpoints
- use archive export as the first debugging tool when a deal does not land where expected

Runnable examples are in [../examples/README.md](../examples/README.md).
