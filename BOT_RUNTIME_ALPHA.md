# Froglet Bot Runtime Alpha

Status: supported alpha product surface

This document defines the bot-facing surface that Froglet intends to support in version 1 alpha.
It sits above the economic kernel in [SPEC.md](SPEC.md) and is narrower than the full internal node API.

## Scope

The supported alpha happy path is:

- discover provider state
- request a quote or use the authenticated runtime buy flow
- open a deal
- wait for `payment_pending`, `result_ready`, or a terminal receipt
- accept the result when required
- verify the terminal receipt
- optionally export the retained archive bundle

The runtime should hide:

- raw invoice-bundle parsing on the happy path
- relay behavior and relay auth
- internal deal signing details for local bot callers

The runtime does not hide:

- signed artifacts
- `artifact_hash` values
- receipt verification
- optional archive export for debugging or audit

## Supported Python Surface

The supported alpha Python entrypoint is [froglet_client.py](froglet_client.py).

Supported helpers:

- `RuntimeClient`
- `ProviderClient`
- `MarketplaceClient`
- `generate_requester_seed()`
- `requester_id_from_seed(seed)`
- `runtime_requester_fields(seed, success_preimage)`

Supported helper shapes:

- `DealHandle`
  - `quote`
  - `deal`
  - `terminal`
  - `payment_intent_path`
  - optional `payment_intent`

These helpers are part of the alpha bot surface because they let a local bot complete the default flow without manually signing deals or parsing settlement internals first.

## Supported Runtime Routes

These localhost routes are part of the supported alpha runtime surface:

- `GET /v1/runtime/wallet/balance`
- `POST /v1/runtime/provider/start`
- `POST /v1/runtime/services/publish`
- `POST /v1/runtime/services/buy`
- `GET /v1/runtime/deals/:deal_id/payment-intent`
- `GET /v1/runtime/archive/:subject_kind/:subject_id`
- `POST /v1/runtime/discovery/curated-lists/issue`
- `GET /v1/runtime/nostr/publications/provider`
- `GET /v1/runtime/nostr/publications/deals/:deal_id/receipt`

The mock-Lightning state mutation route remains alpha-only and test-oriented:

- `POST /v1/runtime/lightning/invoice-bundles/:session_id/state`

It is useful for local bot development and examples, but it is not a public-network workflow.

## Supported Verification Routes

These node routes are intentionally part of the alpha bot product because bots still need explicit verification:

- `POST /v1/invoice-bundles/verify`
- `POST /v1/curated-lists/verify`
- `POST /v1/nostr/events/verify`
- `POST /v1/receipts/verify`

## Explicitly Out of the Alpha Bot Surface

These endpoints may continue to exist, but they are not the primary bot-facing alpha contract:

- direct legacy Cashu inline-payment helpers
- free-only compatibility execution endpoints
- raw feed replication as the first integration path
- internal storage layout or SQLite details
- relay publishing policy

Bots can still use the lower-level node routes, but the supported product path is the runtime plus verification helpers above.

## State Model for Bot Callers

The runtime may expose these local statuses:

- `payment_pending`
- `result_ready`
- terminal `succeeded`, `failed`, or `rejected`

Bot callers should treat `payment_pending` and `result_ready` as operational states, not signed protocol facts.
Receipt verification still depends on the terminal signed receipt artifact.

## Recommended Integration Pattern

For a local bot:

1. Read the runtime auth token from `./data/runtime/auth.token`.
2. Use `RuntimeClient.buy_service(...)` with `runtime_requester_fields(...)`.
3. If the returned handle is non-terminal, inspect or follow `payment_intent`.
4. Wait for `result_ready` or a terminal state.
5. Call `accept_result(...)` when the success-fee preimage should be released.
6. Call `verify_receipt(...)`.
7. Export `archive_subject(...)` if the interaction needs retention or debugging.

Runnable examples are in [examples/README.md](examples/README.md).
Planned evolution beyond this alpha runtime surface is described in [REMOTE_AGENT_LAYER.md](REMOTE_AGENT_LAYER.md).
