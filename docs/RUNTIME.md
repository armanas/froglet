# Runtime

The runtime is the requester-side controller.

It owns:

- provider resolution
- quote fetch and verification
- requester deal signing
- remote provider submission
- requester-side deal state
- payment intent exposure
- result acceptance

It does not expose the provider or discovery contracts as the primary bot API.

## Local State

Runtime-local state is requester state, not provider execution state.

Important runtime statuses:

- `payment_pending`
- `result_ready`
- terminal `succeeded`, `failed`, `rejected`

Those are operational requester views. Signed artifacts remain the kernel truth.

## Identity

Requester identity comes from the runtime’s managed node identity.

The plugin and high-level SDK do not send requester seed material or success preimages.

## Provider Relationship

The runtime submits deals to remote providers under `/v1/provider/*`.
The provider remains authoritative for execution, receipts, and settlement.
