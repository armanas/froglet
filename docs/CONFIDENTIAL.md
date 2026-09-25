# Confidential Execution

> **Unavailable in this build.** Froglet currently contains protocol types and
> mock providers for tests, but no hardware-backed attestation verifier or
> evidence-gated external key-release implementation. Setting
> `FROGLET_CONFIDENTIAL_POLICY_PATH` fails node startup. The node does not
> advertise any `tee.*` runtime.

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

The reserved provider API shape is:

- `GET /v1/provider/confidential/profiles/:artifact_hash`
- `POST /v1/provider/confidential/sessions`
- `GET /v1/provider/confidential/sessions/:session_id`

Bots still initiate confidential work through the local runtime. The provider confidential routes are provider-facing primitives, not the primary bot API.

## Policy

The file [../examples/confidential_policy.example.toml](../examples/confidential_policy.example.toml)
is a protocol/test fixture, not an enablement configuration. Production support
requires a real attestation verifier and key-release provider before this path
can be enabled.

## Client Helpers

Confidential helpers above the raw provider routes are intentionally treated as
client- or SDK-level surfaces rather than part of the core Froglet node
contract. The signed artifact shapes are retained for interoperability work;
provider routes are not a runnable production capability in this build.
