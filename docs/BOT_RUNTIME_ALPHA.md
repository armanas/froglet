# Froglet Bot Runtime Alpha

Status: supported bot-facing product surface

The only supported bot contract is the local requester runtime.

Bots do not:

- call provider routes directly
- call discovery routes directly
- manage requester seed material
- manage success preimages client-side

## Supported Runtime Routes

- `GET /v1/runtime/wallet/balance`
- `POST /v1/runtime/search`
- `GET /v1/runtime/providers/:provider_id`
- `POST /v1/runtime/deals`
- `GET /v1/runtime/deals/:deal_id`
- `GET /v1/runtime/deals/:deal_id/payment-intent`
- `POST /v1/runtime/deals/:deal_id/accept`
- `GET /v1/runtime/archive/:subject_kind/:subject_id`

## Supported SDK Surface

Primary bot surface in [../python/froglet_client.py](../python/froglet_client.py):

- `RuntimeClient`
- `DealHandle`

Low-level, non-primary surfaces:

- `ProviderClient`
- `DiscoveryClient`

## Supported Bot Flow

1. search through the local runtime
2. inspect a provider through the local runtime
3. create a deal through the local runtime
4. poll the local runtime for status
5. inspect payment intent if present
6. accept through the local runtime when required
7. export archive if needed

## Verification Surface

These routes remain supported because bots and operators still need verification:

- `POST /v1/invoice-bundles/verify`
- `POST /v1/curated-lists/verify`
- `POST /v1/nostr/events/verify`
- `POST /v1/receipts/verify`

## Out of Scope

These are not part of the bot-facing alpha contract:

- internal storage layout
- direct provider quote/deal orchestration from bots
- direct discovery coupling from bots
- private broker/ranking/catalog layers
