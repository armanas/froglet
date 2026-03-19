# Confidential Execution

Confidential execution is an additive extension on top of the normal Froglet topology:

- local requester runtime
- remote provider
- remote discovery

It does not change the rule that bots talk only to the local runtime.

## Workload Classes

- `confidential.service.v1`
- `compute.wasm.attested.v1`

The first is for provider-defined attested services over provider-private data.
The second is for requester-supplied attested Wasm over requester-owned or public data.

## Artifact Additions

Confidential mode adds:

- `confidential_profile`
- `confidential_session`
- `encrypted_envelope`

Offers may reference `confidential_profile_hash`.
Quotes, deals, and receipts may reference `confidential_session_hash`.
Receipts may also reference `result_envelope_hash`.

## Provider Routes

When `FROGLET_CONFIDENTIAL_POLICY_PATH` is set, the provider exposes:

- `GET /v1/provider/confidential/profiles/:artifact_hash`
- `POST /v1/provider/confidential/sessions`
- `GET /v1/provider/confidential/sessions/:session_id`

Bots still initiate confidential work through the local runtime. The provider confidential routes are provider-facing primitives, not the primary bot API.

## Policy

Start from [../examples/confidential_policy.example.toml](../examples/confidential_policy.example.toml).

Enable confidential execution on the provider:

```bash
FROGLET_CONFIDENTIAL_POLICY_PATH=./examples/confidential_policy.example.toml \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet-provider
```

## Python Helpers

[../python/froglet_client.py](../python/froglet_client.py) exposes the low-level confidential helpers:

- `ProviderClient.confidential_profile(...)`
- `ProviderClient.open_confidential_session(...)`
- `ProviderClient.confidential_session(...)`
- `generate_confidential_keypair()`
- `encrypt_confidential_payload(...)`
- `decrypt_confidential_envelope(...)`
- `verify_confidential_session_bundle(...)`

These are low-level confidential building blocks. They do not change the supported requester-runtime topology.
