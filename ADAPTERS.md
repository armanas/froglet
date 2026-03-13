# Froglet Adapters

Status: non-normative supporting document

This document captures adapter-level behavior that is intentionally outside the kernel frozen in `SPEC.md`.

## 1. Transport

Version 1 keeps direct HTTPS as the baseline transport and Tor as an optional transport.

Transport choice must not change kernel semantics.
A `Quote`, `Deal`, `invoice_bundle`, or `Receipt` must mean the same thing whether it moved over clearnet HTTPS or an onion service.

`Descriptor.transport_endpoints[]` is the kernel binding for reachability.
How clients prioritize, retry, or rotate across those endpoints is adapter behavior.

## 2. Transport-level `wasm_submission`

The kernel freezes the canonical `compute.wasm.v1` workload object, but not the upload wrapper used to carry execution material.

The reference transport object is `wasm_submission`:

- `schema_version`: `froglet/v1`
- `submission_type`: `wasm_submission`
- `workload`: canonical `compute.wasm.v1` workload object
- `module_bytes_hex`: hex-encoded Wasm module bytes
- `input`: JSON value whose JCS hash must equal `workload.input_hash`

Before execution, providers should verify:

- `SHA256(module_bytes) == workload.module_hash`
- `SHA256(JCS(input)) == workload.input_hash`
- `workload.abi_version` is supported
- `requested_capabilities` is acceptable under the active offer and quote

Cached-module flows, alternate upload encodings, or multipart delivery are adapter decisions as long as they satisfy the same workload hash.

## 3. Invoice Bundle Delivery

`invoice_bundle` verification rules are part of the kernel.
How a requester retrieves the signed bundle is not.

Implementations may deliver the bundle through:

- direct HTTP endpoints
- runtime payment-intent helpers
- transport relays
- private broker infrastructure

The delivery method must not change the signed `invoice_bundle` bytes or the validation rules from `SPEC.md`.

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

Direct peers, allowlists, curated lists, private catalogs, and private brokers are all valid discovery adapters.

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
