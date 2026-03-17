# Froglet v1 Economic Kernel Specification

Status: draft v1 kernel freeze

This document is normative for the irreversible Froglet kernel only.

It freezes:

- the signed artifact envelope
- canonical hashing and signing bytes
- the Descriptor, Offer, Quote, Deal, and Receipt payloads
- the hash-chain and verification rules between those artifacts
- the canonical deal, execution, and settlement states
- the Lightning settlement binding rules that a receipt relies on
- the canonical `compute.wasm.v1` workload object and the `froglet.wasm.run_json.v1` / `froglet.wasm.host_json.v1` ABIs

The following are intentionally moved out of the kernel and are defined in companion docs:

- layered product architecture: `docs/ARCHITECTURE.md`
- transport and discovery adapters: `docs/ADAPTERS.md`
- bot-facing localhost runtime: `docs/RUNTIME.md`
- Nostr publication behavior: `docs/NOSTR.md`
- storage and archival profiles: `docs/STORAGE_PROFILE.md`

## 1. Scope

Froglet version 1 is a small economic primitive for short-lived, bounded, fixed-price deals.

The kernel defines a signed evidence chain:

- `Descriptor`
- `Offer`
- `Quote`
- `Deal`
- `Receipt`

The kernel also normatively defines one signed transport object because terminal receipt verification depends on it:

- `invoice_bundle`

Everything else is an adapter or product surface layered on top of that evidence chain.

## 2. Canonical Encodings and Identities

### 2.1 Global constants

- `schema_version` is always `froglet/v1` for this specification
- all hashes are lowercase hex SHA-256 digests
- all timestamps are Unix seconds
- canonical JSON serialization is RFC 8785 JCS
- Froglet application identities are 32-byte secp256k1 x-only public keys encoded as lowercase hex
- Froglet signatures are 64-byte lowercase hex BIP340 Schnorr signatures

For Lightning settlement identities in this kernel:

- `destination_identity` is the 33-byte compressed secp256k1 public key encoded as lowercase hex

### 2.2 Signed envelope

All five core artifacts and the `invoice_bundle` transport object use the same signed envelope:

- `artifact_type`: one of `descriptor`, `offer`, `quote`, `deal`, `receipt`, or `invoice_bundle`
- `schema_version`: always `froglet/v1`
- `signer`: Froglet application identity of the signer
- `created_at`: Unix timestamp in seconds
- `payload_hash`: lowercase hex SHA-256 of `JCS(payload)`
- `payload`: the canonical JSON payload for the artifact type
- `signature`: BIP340 Schnorr signature by `signer` over the exact `artifact_bytes` defined below

The exact signing and artifact-hash preimage is:

```text
artifact_bytes = JCS([
  schema_version,
  artifact_type,
  signer,
  created_at,
  payload_hash,
  payload
])
```

The exact derived values are:

- `payload_hash = SHA256(JCS(payload))`
- `artifact_hash = SHA256(artifact_bytes)`

The combination of `schema_version` and `artifact_type` is the domain separator.
There is no separate mutable domain field inside the envelope.

Validation order is:

1. Verify that `schema_version` is `froglet/v1`.
2. Recompute `payload_hash` from `payload`.
3. Recompute `artifact_bytes` exactly as above.
4. Recompute `artifact_hash = SHA256(artifact_bytes)`.
5. Verify `signature` as a BIP340 Schnorr signature by `signer` over the raw `artifact_bytes`.

`artifact_hash` is a derived content address.
It is not part of the in-band signed envelope.
Transport surfaces may include it as out-of-band metadata for lookup and indexing.

### 2.3 Linked publication identities

`Descriptor.payload.linked_identities[]` is how a provider links optional publication identities to the Froglet application identity.

The v1 kernel only normatively defines `identity_kind = nostr`.
Other linked identity kinds are adapter-level extensions and are outside this specification.

A linked identity entry has these fields:

- `identity_kind`: currently only `nostr`
- `identity`: linked public identity string
- `scope`: array of scope strings
- `created_at`: Unix timestamp in seconds
- `expires_at`: Unix timestamp in seconds or `null`
- `signature_algorithm`: for v1 Nostr linkage this must be `secp256k1_schnorr_bip340`
- `linked_signature`: signature by the linked identity over the exact challenge bytes below

For `identity_kind = nostr`:

- `identity` must be a 32-byte x-only public key encoded as lowercase hex
- `scope` must contain only publication scopes, such as `publication.nostr`

The exact linkage challenge bytes are:

```text
froglet:identity_link:v1
<provider_id>
<identity_kind>
<identity>
<scope_hash>
<created_at>
<expires_at_or_dash>
```

Where:

- `provider_id` is `Descriptor.payload.provider_id`
- `scope_hash = SHA256(JCS(scope))`
- `expires_at_or_dash` is the decimal expiry timestamp or `-`

For v1 Nostr linkage, `linked_signature` is a BIP340 Schnorr signature over the raw UTF-8 challenge bytes above.

## 3. Artifact Payloads

### 3.1 Descriptor payload

`Descriptor.payload` must contain:

- `provider_id`: Froglet application identity of the provider; must equal `signer`
- `descriptor_seq`: monotonically increasing unsigned integer per provider
- `protocol_version`: must be `froglet/v1`
- `expires_at`: Unix timestamp in seconds or `null`
- `linked_identities`: array of linked publication identities
- `transport_endpoints`: array of transport endpoint objects
- `capabilities`: object with:
  - `service_kinds`: array such as `compute.wasm.v1`
  - `execution_runtimes`: array; public v1 remote execution should only advertise `wasm`
  - `max_concurrent_deals`: integer or `null`

Each `transport_endpoints[]` entry must contain:

- `transport`: `http`, `https`, or `tor`
- `uri`: canonical endpoint URI
- `created_at`: Unix timestamp in seconds
- `expires_at`: Unix timestamp in seconds or `null`
- `priority`: lower values are preferred first
- `features`: array of feature strings such as `quote_http`, `artifact_fetch`, or `receipt_poll`

`Descriptor` is the root artifact for provider identity, transport reachability, and optional publication-key linkage.

### 3.2 Offer payload

`Offer.payload` must contain:

- `provider_id`: Froglet application identity of the provider; must equal `signer`
- `offer_id`: stable provider-chosen identifier for the offer
- `descriptor_hash`: `artifact_hash` of the descriptor the offer is published under
- `expires_at`: Unix timestamp in seconds or `null`
- `offer_kind`: service kind identifier; public v1 remote compute uses `compute.wasm.v1`
- `settlement_method`: must be `lightning.base_fee_plus_success_fee.v1` for v1 paid remote execution
- `quote_ttl_secs`: maximum lifetime of quotes issued from this offer
- `execution_profile`: object with:
  - `runtime`: must be `wasm` for public v1 remote compute
  - `abi_version`: supported v1 public compute ABIs are `froglet.wasm.run_json.v1` and `froglet.wasm.host_json.v1`
  - `capabilities`: array of capability strings; `froglet.wasm.run_json.v1` offers must publish `[]`, while `froglet.wasm.host_json.v1` offers may publish a provider-defined allowlist such as `net.http.fetch`, `net.http.fetch.auth.<profile>`, or `db.sqlite.query.read.<handle>`
  - `max_input_bytes`
  - `max_runtime_ms`
  - `max_memory_bytes`
  - `max_output_bytes`
  - `fuel_limit`
- `price_schedule`: object with:
  - `base_fee_msat`
  - `success_fee_msat`

`Offer.payload` may additionally contain:

- `terms_hash`: hash of additional machine-readable or human-readable policy text

### 3.3 Quote payload

`Quote.payload` must contain:

- `provider_id`: Froglet application identity of the provider; must equal `signer`
- `requester_id`: Froglet application identity of the intended requester
- `descriptor_hash`: descriptor hash used when the quote was issued
- `offer_hash`: `artifact_hash` of the referenced offer
- `expires_at`: Unix timestamp in seconds
- `workload_kind`: workload kind identifier; public v1 remote compute uses `compute.wasm.v1`
- `workload_hash`: hash of the canonical workload object
- `capabilities_granted`: capability strings granted for this quoted execution; must be a subset of both `Offer.execution_profile.capabilities` and the workload's `requested_capabilities`
- `settlement_terms`: object with:
  - `method`: must be `lightning.base_fee_plus_success_fee.v1`
  - `destination_identity`
  - `base_fee_msat`
  - `success_fee_msat`
  - `max_base_invoice_expiry_secs`
  - `max_success_hold_expiry_secs`
  - `min_final_cltv_expiry`
- `execution_limits`: object with:
  - `max_input_bytes`
  - `max_runtime_ms`
  - `max_memory_bytes`
  - `max_output_bytes`
  - `fuel_limit`

Quote validation rules:

- `provider_id` must match the referenced offer and descriptor
- `expires_at` must be no later than `created_at + Offer.quote_ttl_secs`
- `workload_kind` must be compatible with `Offer.offer_kind`
- `execution_limits` must be less than or equal to the maxima advertised in `Offer.execution_profile`
- `capabilities_granted` must be a subset of `Offer.execution_profile.capabilities`
- `settlement_terms.method` must equal `Offer.settlement_method`
- `settlement_terms.base_fee_msat` and `settlement_terms.success_fee_msat` must equal `Offer.price_schedule`

### 3.4 Deal payload

`Deal.payload` must contain:

- `requester_id`: Froglet application identity of the requester; must equal `signer`
- `provider_id`: Froglet application identity of the quoted provider
- `quote_hash`: `artifact_hash` of the accepted quote
- `workload_hash`: hash of the canonical workload object to be executed
- `success_payment_hash`: lowercase hex SHA-256 of the requester-chosen success-fee preimage
- `admission_deadline`: latest time the provider may admit or reject the deal
- `completion_deadline`: latest time the provider may finish the work
- `acceptance_deadline`: latest time the requester may release the success-fee preimage

Deal validation rules:

- `provider_id` must match `Quote.payload.provider_id`
- `requester_id` must match `Quote.payload.requester_id`
- `workload_hash` must match `Quote.payload.workload_hash`
- `admission_deadline` must be less than or equal to `Quote.payload.expires_at`
- `completion_deadline` must be strictly greater than `admission_deadline`
- `acceptance_deadline` must be greater than or equal to `completion_deadline`

### 3.5 Receipt payload

`Receipt.payload` must contain:

- `provider_id`: Froglet application identity of the provider; must equal `signer`
- `requester_id`: Froglet application identity of the requester from the deal
- `deal_hash`: `artifact_hash` of the referenced deal
- `quote_hash`: `artifact_hash` of the referenced quote
- `started_at`: Unix timestamp in seconds or `null`
- `finished_at`: Unix timestamp in seconds
- `deal_state`: terminal deal state; one of `rejected`, `succeeded`, `failed`, or `canceled`
- `execution_state`: terminal execution state; one of `not_started`, `succeeded`, or `failed`
- `settlement_state`: terminal settlement state; one of `none`, `settled`, `canceled`, or `expired`
- `result_hash`: hash of the canonical result object or `null`
- `result_format`: result format identifier or `null`
- `executor`: object with:
  - `runtime`
  - `runtime_version`
  - `abi_version`
  - `module_hash`
  - `capabilities_granted`
- `limits_applied`: object with:
  - `max_input_bytes`
  - `max_runtime_ms`
  - `max_memory_bytes`
  - `max_output_bytes`
  - `fuel_limit`
- `settlement_refs`: object with:
  - `method`
  - `bundle_hash`: `artifact_hash` of the `invoice_bundle`, or `null` when settlement was not used
  - `destination_identity`
  - `base_fee`: object with `amount_msat`, `invoice_hash`, `payment_hash`, and `state`
  - `success_fee`: object with `amount_msat`, `invoice_hash`, `payment_hash`, and `state`

`Receipt.payload` may additionally contain:

- `failure_code`
- `failure_message`
- `result_ref`

Receipt validation rules:

- `provider_id` must equal the quote provider and the receipt signer
- `requester_id` must equal the deal requester
- `quote_hash` must equal `Deal.payload.quote_hash`
- `Receipt.executor.capabilities_granted` must be a subset of `Quote.payload.capabilities_granted`
- `finished_at` must be greater than or equal to `started_at` when `started_at` is present
- `result_hash` and `result_format` must both be present if and only if `execution_state = succeeded`
- if `deal_state = rejected`, then `execution_state` must be `not_started`
- if `deal_state = succeeded`, then `execution_state` must be `succeeded` and `settlement_state` must be `settled`
- if `deal_state = failed`, then `execution_state` must be `failed`
- if `deal_state = canceled`, then `execution_state` may be `not_started` or `succeeded`

`Receipt` is the authoritative terminal artifact.
For requester-controlled success-fee release, the success preimage is not evidence by itself because the requester knew it before payment.

## 4. Canonical State Model

### 4.1 Deal state

Canonical deal states are:

- `opened`
- `admitted`
- `rejected`
- `succeeded`
- `failed`
- `canceled`

Allowed transitions are:

- `opened -> admitted`
- `opened -> rejected`
- `opened -> canceled`
- `admitted -> succeeded`
- `admitted -> failed`
- `admitted -> canceled`

Semantics:

- `opened`: the requester has emitted a signed `Deal`, but the provider has not yet admitted it
- `admitted`: the provider has observed the required settlement preconditions and accepted the work
- `rejected`: the provider refused admission before execution began
- `succeeded`: execution completed successfully and the success-dependent settlement leg reached `settled`
- `failed`: execution failed after admission
- `canceled`: the interaction ended non-successfully for a non-execution reason, including requester/provider aborts and requester refusal or failure to release the success fee after result staging

### 4.2 Execution state

Canonical execution states are:

- `not_started`
- `running`
- `succeeded`
- `failed`

Rejected deals must not enter `running`.

### 4.3 Settlement state

Canonical settlement states are:

- `none`
- `invoice_open`
- `funds_locked`
- `settled`
- `canceled`
- `expired`

For `lightning.base_fee_plus_success_fee.v1`, the meanings are:

- `none`: no settlement bundle exists for the deal
- `invoice_open`: an `invoice_bundle` exists, but the success-fee leg is still open and the provider must not execute
- `funds_locked`: the base-fee leg is settled and the success-fee hold leg is accepted
- `settled`: the success-fee leg is settled
- `canceled`: the success-fee leg is canceled
- `expired`: the success-fee leg expired

The provider must not begin execution until settlement is at least `funds_locked`.

### 4.4 Runtime-local states are not canonical protocol states

Implementations may expose runtime-local deal statuses such as `payment_pending` or `result_ready`.

Those are projections over canonical state and are not part of the signed protocol surface.

For v1:

- `payment_pending` projects to canonical `deal_state = opened`
- `result_ready` projects to canonical `deal_state = admitted`, `execution_state = succeeded`, and `settlement_state = funds_locked`

Only canonical states belong in signed receipts.

## 5. Hash Chains and Cross-Artifact Validation

The core commitment chain is:

- `Offer` commits to `descriptor_hash`
- `Quote` commits to `descriptor_hash`, `offer_hash`, and `workload_hash`
- `Deal` commits to `quote_hash`, `workload_hash`, and `success_payment_hash`
- `Receipt` commits to `deal_hash`, `quote_hash`, `result_hash`, and settlement references

A verifier checking a full paid interaction must validate, in order:

1. `Descriptor`
2. `Offer`
3. `Quote`
4. `Deal`
5. `invoice_bundle`
6. `Receipt`

The full chain is valid only if:

- every envelope verifies independently
- every hash reference resolves to the expected prior artifact
- actor identities line up across the chain
- the workload hash is unchanged from quote to deal
- the success payment hash is unchanged from deal to `invoice_bundle` and receipt
- the settlement destination and fee amounts are unchanged from quote to `invoice_bundle` and receipt

## 6. Lightning Settlement Binding

### 6.1 Settlement method

The v1 paid settlement method is:

- `lightning.base_fee_plus_success_fee.v1`

It has two Lightning legs:

- `base_fee`: the non-conditional fee leg that must reach `settled` before admission; implementations may realize it as either a normal invoice or a hold invoice so long as the signed bundle fields and observed leg states remain consistent
- `success_fee`: hold invoice, released only after requester acceptance

### 6.2 Signed `invoice_bundle`

`invoice_bundle` is not a sixth core artifact type.
It is a signed transport object that uses the same envelope defined in section 2.2 with `artifact_type = invoice_bundle`.

`invoice_bundle.payload` must contain:

- `provider_id`: Froglet application identity of the provider; must equal `signer`
- `requester_id`: Froglet application identity of the requester
- `quote_hash`
- `deal_hash`
- `expires_at`: Unix timestamp in seconds
- `destination_identity`
- `base_fee`: object with:
  - `amount_msat`
  - `invoice_bolt11`
  - `invoice_hash`
  - `payment_hash`
  - `state`
- `success_fee`: object with:
  - `amount_msat`
  - `invoice_bolt11`
  - `invoice_hash`
  - `payment_hash`
  - `state`
- `min_final_cltv_expiry`

Leg `state` values are:

- `open`
- `accepted`
- `settled`
- `canceled`
- `expired`

At issuance time:

- `success_fee.state` must be `open`
- `base_fee.state` must be `open`, except that it may be `settled` immediately when `base_fee.amount_msat = 0`

### 6.3 Requester-side `invoice_bundle` validation

Before paying either invoice, the requester must reject the bundle unless all of the following hold:

- the `invoice_bundle` envelope verifies
- `provider_id`, `requester_id`, `quote_hash`, and `deal_hash` match the current interaction
- `destination_identity` equals `Quote.payload.settlement_terms.destination_identity`
- `base_fee.amount_msat` and `success_fee.amount_msat` equal the quoted settlement terms
- `success_fee.payment_hash` equals `Deal.payload.success_payment_hash`
- `min_final_cltv_expiry` equals the quoted settlement constraint
- `expires_at` does not exceed the quote deadline
- `invoice_hash = SHA256(UTF-8(invoice_bolt11))` for both legs
- the decoded BOLT11 amount, payment hash, payee identity, and expiry match the signed bundle fields
- the decoded success-fee invoice does not advertise a smaller `min_final_cltv_expiry` than the quoted constraint
- neither decoded invoice expiry exceeds the quoted settlement constraints or the quote deadline

### 6.4 Execution gating

The provider must not admit or execute the deal until:

- the base-fee leg is `settled`
- the success-fee leg is `accepted`

That is the canonical `funds_locked` point.

### 6.5 Requester-controlled release

The requester chooses a secret `s` and places `success_payment_hash = SHA256(s)` in the `Deal`.

The intended paid flow is:

1. The provider issues a signed `Quote`.
2. The requester issues a signed `Deal` containing `success_payment_hash`.
3. The provider issues a signed `invoice_bundle`.
4. The requester validates the `invoice_bundle` and pays the base fee and hold invoice.
   If the provider realizes the base-fee leg as a hold invoice, the requester still funds that leg first and the provider must settle it before admission.
5. The provider observes `funds_locked` and executes the work.
6. The provider returns result material or a `result_ref`.
7. The requester releases `s` to accept the success fee, or the provider cancels or lets the hold expire.
8. The provider emits the final signed `Receipt` only after the success-fee leg is terminal.

### 6.6 Receipt settlement semantics

For `lightning.base_fee_plus_success_fee.v1`:

- `Receipt.payload.settlement_state` is the terminal state of the success-fee leg, or `none` if no `invoice_bundle` existed
- the base-fee leg is interpreted from `Receipt.payload.settlement_refs.base_fee.state`
- the success-fee leg is interpreted from `Receipt.payload.settlement_refs.success_fee.state`

This avoids ambiguity in mixed two-leg outcomes.

Examples:

- base fee `settled`, success fee `settled` -> `settlement_state = settled`
- base fee `settled`, success fee `canceled` -> `settlement_state = canceled`
- base fee `settled`, success fee `expired` -> `settlement_state = expired`
- no `invoice_bundle` -> `settlement_state = none`

## 7. Canonical `compute.wasm.v1` Workload

The canonical public compute workload kind for v1 is `compute.wasm.v1`.

The canonical workload object must contain:

- `schema_version`: `froglet/v1`
- `workload_kind`: `compute.wasm.v1`
- `abi_version`: `froglet.wasm.run_json.v1` or `froglet.wasm.host_json.v1`
- `module_format`: `application/wasm`
- `module_hash`: lowercase hex SHA-256 of the raw Wasm bytes
- `input_format`: `application/json+jcs`
- `input_hash`: lowercase hex SHA-256 of `JCS(input_json_value)`
- `requested_capabilities`: array of capability strings; `froglet.wasm.run_json.v1` workloads must use `[]`, while `froglet.wasm.host_json.v1` workloads may request a provider-offered subset such as `net.http.fetch`, `net.http.fetch.auth.<profile>`, or `db.sqlite.query.read.<handle>`

The canonical workload hash is:

- `workload_hash = SHA256(JCS(workload_object))`

The canonical workload object does not include:

- inline Wasm bytes
- transport URLs
- cache keys
- provider-local file paths
- other transport-only submission material

Those belong to adapters and runtime surfaces, not to the kernel.

## 8. Wasm ABIs

### 8.1 `froglet.wasm.run_json.v1`

The pure-compute v1 Wasm ABI is `froglet.wasm.run_json.v1`.

Modules implementing it must satisfy all of the following:

- the module format must be a core WebAssembly binary module
- the module must define one exported linear memory named `memory`
- the module must export `alloc(len: i32) -> i32`
- the module must export `run(input_ptr: i32, input_len: i32) -> i64`
- the module may export `dealloc(ptr: i32, len: i32) -> ()`
- the module must not require WASI, filesystem, network, clock, or other undeclared host imports
- public v1 providers should reject all imported functions, globals, tables, and memories before instantiation
- public v1 providers should reject shared memories, 64-bit memories, and memory declarations whose minimum or declared maximum exceed the active memory cap

Host execution sequence:

1. Canonicalize the requester input with RFC 8785 JCS.
2. Verify `SHA256(JCS(input)) == workload.input_hash`.
3. Call `alloc(input_len)`.
4. Copy the JCS input bytes into guest memory.
5. Call `run(input_ptr, input_len)`.
6. Interpret the returned `i64` as:
   - upper 32 bits: `result_ptr`
   - lower 32 bits: `result_len`
7. Verify the slice lies inside guest memory and respects the output limit.
8. Copy the result bytes.
9. Parse the result bytes as UTF-8 JSON.
10. Canonicalize the resulting JSON value with RFC 8785 JCS to derive `result_hash`.

Execution fails if any of the following occur:

- missing required exports
- invalid guest memory ranges
- trap during execution
- fuel exhaustion
- timeout
- invalid UTF-8 output
- invalid JSON output
- output that exceeds the quoted limit

For public v1 remote execution:

- `requested_capabilities` must be `[]`
- `capabilities_granted` in receipts must be `[]`
- providers must not expose ambient filesystem, network, clock, randomness, or process APIs

### 8.2 `froglet.wasm.host_json.v1`

The capability-enabled v1 Wasm ABI is `froglet.wasm.host_json.v1`.

It uses the same exported `memory`, `alloc`, `run`, and optional `dealloc` contract as `froglet.wasm.run_json.v1`, but it may additionally import:

- `froglet_host::call_json(input_ptr: i32, input_len: i32) -> i64`

Providers using this ABI must:

- publish the offered capability allowlist in `Offer.execution_profile.capabilities`
- sign the accepted subset in `Quote.payload.capabilities_granted`
- repeat the accepted subset in `Receipt.executor.capabilities_granted`
- enforce provider-local policy for outbound HTTP, named database handles, auth profiles, call-count ceilings, and private-network access

The initial capability namespace is:

- `net.http.fetch`
- `net.http.fetch.auth.<profile>`
- `db.sqlite.query.read.<handle>`

## 9. Conformance Expectations

Milestone 1 is only complete when the frozen kernel is backed by golden vectors for:

- envelope signing and `artifact_hash` derivation
- Nostr linked-identity challenge signing
- `Quote -> Deal -> Receipt` verification
- `invoice_bundle` verification
- `compute.wasm.v1` workload hashing and receipt `result_hash` derivation

Those vectors are intentionally separate from this specification so they can evolve into an executable conformance suite without changing the kernel contract.

The initial checked-in bundle for this repository is `conformance/kernel_v1.json`.
