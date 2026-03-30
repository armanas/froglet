# Froglet Adapters

Status: non-normative supporting document

This document captures adapter-level behavior that is intentionally outside the
kernel defined in [`KERNEL.md`](KERNEL.md).

## 1. Transport

Version 1 keeps direct HTTPS as the baseline transport and Tor as an optional transport.

Transport choice must not change kernel semantics.
A `Quote`, `Deal`, `invoice_bundle`, or `Receipt` must mean the same thing whether it moved over clearnet HTTPS or an onion service.

`Descriptor.transport_endpoints[]` is the kernel binding for reachability.
How clients prioritize, retry, or rotate across those endpoints is adapter behavior.

## 2. Execution Material Delivery

The kernel carries a workload hash and signed economic state, but it does not
hardwire one transport wrapper for execution material.

During the current cutover, Froglet should be understood as supporting one
primitive that may be bound to:

- provider-defined named services
- provider-defined data services
- open-ended compute supplied by the requester

The execution material for those bindings may eventually be delivered as:

- module uploads
- interpreted source bundles
- archive bundles such as zip files
- container or image references
- other runtime-specific submission wrappers

Current implementation note:

- the checked-in reference implementation still uses Wasm-oriented submission
  wrappers for the current execution profiles
- those wrappers are reference adapters, not the permanent product boundary

Longer-term delivery formats may include interpreted source bundles, archive
bundles such as zip files, and container or image references as first-class
execution profiles over the same Froglet primitive.

## 3. Invoice Bundle Delivery

`invoice_bundle` verification rules are part of the kernel.
How a requester retrieves the signed bundle is not.

Implementations may deliver the bundle through:

- direct HTTP endpoints
- runtime payment-intent helpers
- transport relays
- private broker infrastructure

The delivery method must not change the signed `invoice_bundle` bytes or the validation rules from [`KERNEL.md`](KERNEL.md).

## 4. Settlement Drivers

Settlement driver choice is an adapter boundary.

Examples:

- mock Lightning mode for local development
- LND REST for real Lightning interaction

These drivers may differ operationally, but they must preserve:

- the same `invoice_bundle` commitments
- the same leg-state meanings
- the same gating rule before execution
- the same receipt semantics

Mock drivers are acceptable for local testing.
They are not production settlement finality.

## 5. Discovery Bootstrap

Direct peers, allowlists, curated lists, private catalogs, private brokers, and
marketplace services are all valid discovery adapters.

One useful bootstrap format is a signed `curated_list` object:

- `schema_version`: `froglet/v1`
- `list_type`: `curated_list`
- `curator_id`: Froglet identity of the curator
- `list_id`: curator-generated identifier
- `created_at`: Unix timestamp in seconds
- `expires_at`: Unix timestamp in seconds
- `entries`: array of objects with:
  - `provider_id`
  - `descriptor_hash`
  - `tags`
  - `note`
- signed with the same kernel envelope semantics, but used only as discovery metadata

Curated lists are signed recommendations.
They are not canonical economic state.

A marketplace is not a special protocol actor.
It is just another Froglet node or service that consumes signed Froglet
artifacts and republishes higher-layer discovery information.
Marketplace implementations may live in separate repos, local ignored
incubation outside the public release surface, or closed-source services. The
protocol boundary stays the same either way.

## 6. Deployment Adapters

How a provider or operator stack is deployed is also an adapter boundary.

Examples:

- local Docker Compose or systemd deployments
- Kubernetes or Nomad packaging
- native cloud implementations for AWS, GCP, OVH, or similar providers

These deployment adapters may choose their own storage, secret management,
image, and networking conventions, but they must preserve:

- the same kernel semantics
- the same signed artifact verification rules
- the same `/v1/froglet/*` bot/operator contract
- the same clearnet or Tor reachability semantics once endpoints are published
