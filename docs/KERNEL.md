# Froglet v1 Kernel Specification

Status: normative kernel specification (current authoritative contract material — temporary, may later be removed or folded elsewhere)

This document is normative for the irreversible Froglet kernel only.

It defines:

- the signed artifact envelope
- canonical hashing and signing bytes
- the six artifact types: Descriptor, Offer, Quote, Deal, InvoiceBundle, and Receipt
- the hash-chain and verification rules between those artifacts
- the canonical deal, execution, and settlement states
- the two standardized v1 settlement methods: `none` and `lightning.base_fee_plus_success_fee.v1`
- the invoice bundle immutability model
- receipt terminal-only semantics
- expiry ordering constraints
- linked identity challenge format

The following are intentionally outside the kernel and are defined in companion docs:

- layered product architecture: `ARCHITECTURE.md`
- transport and discovery adapters: `ADAPTERS.md`
- bot-facing localhost runtime: `RUNTIME.md`
- confidential execution companion extensions: `CONFIDENTIAL.md`
- Nostr publication behavior: `NOSTR.md`
- storage and archival profiles: `STORAGE_PROFILE.md`
- service binding (service_id, offer_kind, discovery records): `SERVICE_BINDING.md`

## 1. Scope

Froglet version 1 is a small economic primitive for short-lived, bounded,
fixed-price resource deals.

A Froglet deal may represent:

- a predefined named service
- a predefined data service
- open-ended compute supplied by the requester

These are product-layer distinctions over the same signed economic primitive.
They are not different deal types in the kernel.

`provider_id` and `requester_id` in this specification describe the role played
in a specific interaction. They are not node classes. A single Froglet node MAY
act as both provider and requester across different deals.

The kernel defines a signed evidence chain of six artifact types:

- `descriptor`
- `offer`
- `quote`
- `deal`
- `invoice_bundle`
- `receipt`

Everything else is an adapter or product surface layered on top of that evidence
chain.

## 2. Canonical Encodings and Identities

### 2.1 Global Constants

- `schema_version` is always `froglet/v1` for this specification
- All hashes are lowercase hex SHA-256 digests
- All timestamps are Unix seconds
- Canonical JSON serialization is RFC 8785 JCS
- Froglet application identities are 32-byte secp256k1 x-only public keys encoded as lowercase hex
- Froglet signatures are 64-byte lowercase hex BIP340 Schnorr signatures

For Lightning settlement identities in this kernel:

- `destination_identity` is the 33-byte compressed secp256k1 public key encoded as lowercase hex

### 2.2 Signed Envelope

All six artifact types use the same signed envelope:

- `artifact_type`: one of `descriptor`, `offer`, `quote`, `deal`, `invoice_bundle`, or `receipt`
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

Verification order:

1. Verify that `schema_version` is `froglet/v1`.
2. Recompute `payload_hash` from `payload`.
3. Recompute `artifact_bytes` exactly as above.
4. Recompute `artifact_hash = SHA256(artifact_bytes)`.
5. Verify `signature` as a BIP340 Schnorr signature by `signer` over the raw `artifact_bytes`.

`artifact_hash` is a derived content address.
It is not part of the in-band signed envelope.
Transport surfaces MAY include it as out-of-band metadata for lookup and indexing.

### 2.3 Linked Publication Identities

`Descriptor.payload.linked_identities[]` is how a provider links optional
publication identities to the Froglet application identity.

The v1 kernel only normatively defines `identity_kind = nostr`.
Other linked identity kinds are adapter-level extensions and are outside this
specification.

A linked identity entry MUST contain:

- `identity_kind`: currently only `nostr`
- `identity`: linked public identity string
- `scope`: array of scope strings
- `created_at`: Unix timestamp in seconds
- `expires_at`: Unix timestamp in seconds or `null`
- `signature_algorithm`: for v1 Nostr linkage this MUST be `secp256k1_schnorr_bip340`
- `linked_signature`: signature by the linked identity over the exact challenge bytes below

For `identity_kind = nostr`:

- `identity` MUST be a 32-byte x-only public key encoded as lowercase hex
- `scope` MUST contain only publication scopes, such as `publication.nostr`

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

For v1 Nostr linkage, `linked_signature` is a BIP340 Schnorr signature over the
raw UTF-8 challenge bytes above.

## 3. Artifact Payloads

### 3.1 Descriptor Payload

`Descriptor.payload` MUST contain:

- `provider_id`: Froglet application identity of the provider; MUST equal `signer`
- `descriptor_seq`: monotonically increasing unsigned integer per provider
- `protocol_version`: MUST be `froglet/v1`
- `linked_identities`: array of linked publication identities (see §2.3)
- `transport_endpoints`: array of transport endpoint objects
- `capabilities`: object with:
  - `service_kinds`: provider-defined identifiers for the kinds of resources this node serves
  - `execution_runtimes`: provider-defined runtime families such as `wasm`, `python`, or `container`

`Descriptor.payload` MAY additionally contain:

- `expires_at`: Unix timestamp in seconds or `null`
- `max_concurrent_deals`: integer or `null`

Each `transport_endpoints[]` entry MUST contain:

- `transport`: `http`, `https`, or `tor`
- `uri`: canonical endpoint URI
- `created_at`: Unix timestamp in seconds
- `priority`: lower values are preferred first
- `features`: array of feature strings such as `quote_http`, `artifact_fetch`, or `receipt_poll`

Each `transport_endpoints[]` entry MAY contain:

- `expires_at`: Unix timestamp in seconds or `null`

`Descriptor` is the root artifact for provider identity, transport reachability,
and optional publication-key linkage.

### 3.2 Offer Payload

`Offer.payload` MUST contain:

- `provider_id`: Froglet application identity of the provider; MUST equal `signer`
- `offer_id`: stable provider-chosen identifier for the offer
- `descriptor_hash`: `artifact_hash` of the descriptor the offer is published under
- `offer_kind`: service kind identifier chosen by the provider for the published resource
- `settlement_method`: settlement method string (see §5)
- `quote_ttl_secs`: maximum lifetime of quotes issued from this offer
- `execution_profile`: object with runtime-specific execution parameters
- `price_schedule`: object with:
  - `base_fee_msat`
  - `success_fee_msat`

`Offer.payload` MAY additionally contain:

- `expires_at`: Unix timestamp in seconds or `null`
- `terms_hash`: hash of additional machine-readable or human-readable policy text

Settlement method rules for offers:

- An Offer for a free service (where `price_schedule.base_fee_msat == 0` AND `price_schedule.success_fee_msat == 0`) MUST use `settlement_method: "none"`.
- An Offer for a paid service MUST use `settlement_method: "lightning.base_fee_plus_success_fee.v1"`.
- An Offer MUST NOT use a paid settlement method string when fees are zero, and MUST NOT use `"none"` when fees are non-zero.

### 3.3 Quote Payload

`Quote.payload` MUST contain:

- `provider_id`: Froglet application identity of the provider; MUST equal `signer`
- `requester_id`: Froglet application identity of the intended requester
- `descriptor_hash`: descriptor hash used when the quote was issued
- `offer_hash`: `artifact_hash` of the referenced offer
- `expires_at`: Unix timestamp in seconds
- `workload_kind`: workload kind identifier for the requested execution or service invocation
- `workload_hash`: hash of the canonical workload object
- `settlement_terms`: settlement terms object (see below)
- `execution_limits`: object with:
  - `max_input_bytes`
  - `max_runtime_ms`
  - `max_memory_bytes`
  - `max_output_bytes`
  - `fuel_limit`

For `settlement_method: "lightning.base_fee_plus_success_fee.v1"`, `settlement_terms` MUST contain:

- `method`: `"lightning.base_fee_plus_success_fee.v1"`
- `destination_identity`: 33-byte compressed secp256k1 public key (lowercase hex)
- `base_fee_msat`: base fee amount in millisatoshis
- `success_fee_msat`: success fee amount in millisatoshis
- `max_base_invoice_expiry_secs`: maximum expiry for the base-fee invoice
- `max_success_hold_expiry_secs`: maximum expiry for the success-fee hold invoice
- `min_final_cltv_expiry`: minimum CLTV expiry delta for Lightning invoices

For `settlement_method: "none"`, `settlement_terms` MUST contain:

- `method`: `"none"`
- `destination_identity`: `""` (empty string)
- `base_fee_msat`: `0`
- `success_fee_msat`: `0`

Quote validation rules:

- `provider_id` MUST match the referenced offer and descriptor
- `expires_at` MUST be no later than `created_at + Offer.quote_ttl_secs`
- `workload_kind` MUST be compatible with `Offer.offer_kind`
- `execution_limits` MUST be less than or equal to the maxima advertised in `Offer.execution_profile`
- `settlement_terms.method` MUST equal `Offer.settlement_method`
- `settlement_terms.base_fee_msat` and `settlement_terms.success_fee_msat` MUST equal `Offer.price_schedule` values

### 3.4 Deal Payload

`Deal.payload` MUST contain:

- `requester_id`: Froglet application identity of the requester; MUST equal `signer`
- `provider_id`: Froglet application identity of the quoted provider
- `quote_hash`: `artifact_hash` of the accepted quote
- `workload_hash`: hash of the canonical workload object to be executed
- `success_payment_hash`: lowercase hex SHA-256 of the requester-chosen success-fee preimage
- `admission_deadline`: latest time the provider may admit or reject the deal
- `completion_deadline`: latest time the provider may finish the work
- `acceptance_deadline`: latest time the requester may release the success-fee preimage

Deal validation rules:

- `provider_id` MUST match `Quote.payload.provider_id`
- `requester_id` MUST match `Quote.payload.requester_id`
- `workload_hash` MUST match `Quote.payload.workload_hash`
- `admission_deadline` MUST be less than or equal to `Quote.payload.expires_at`
- `completion_deadline` MUST be strictly greater than `admission_deadline`
- `acceptance_deadline` MUST be greater than or equal to `completion_deadline`

### 3.5 InvoiceBundle Payload

`InvoiceBundle` is a signed artifact that uses the same envelope defined in §2.2
with `artifact_type = invoice_bundle`. It is issued by the provider for
Lightning-settled deals only. Free deals (`settlement_method: "none"`) MUST NOT
have an invoice bundle.

`InvoiceBundle.payload` MUST contain:

- `provider_id`: Froglet application identity of the provider; MUST equal `signer`
- `requester_id`: Froglet application identity of the requester
- `quote_hash`: `artifact_hash` of the referenced quote
- `deal_hash`: `artifact_hash` of the referenced deal
- `expires_at`: Unix timestamp in seconds
- `destination_identity`: 33-byte compressed secp256k1 public key (lowercase hex)
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

Leg `state` values at issuance time:

- `success_fee.state` MUST be `open`
- `base_fee.state` MUST be `open`, except that it MAY be `settled` immediately when `base_fee.amount_msat = 0`

See §4 for immutability rules and §6.3 for requester-side validation.

### 3.6 Receipt Payload

`Receipt.payload` MUST contain:

- `provider_id`: Froglet application identity of the provider; MUST equal `signer`
- `requester_id`: Froglet application identity of the requester from the deal
- `deal_hash`: `artifact_hash` of the referenced deal
- `quote_hash`: `artifact_hash` of the referenced quote
- `started_at`: Unix timestamp in seconds or `null`
- `finished_at`: Unix timestamp in seconds
- `deal_state`: terminal deal state; one of `rejected`, `succeeded`, `failed`, or `canceled`
- `execution_state`: terminal execution state; one of `not_started`, `succeeded`, or `failed`
- `settlement_state`: terminal settlement state (see §7)
- `result_hash`: hash of the canonical result object or `null`
- `result_format`: result format identifier or `null`
- `executor`: object with:
  - `runtime`
  - `runtime_version`
  - `abi_version`
  - `module_hash`
- `limits_applied`: object with:
  - `max_input_bytes`
  - `max_runtime_ms`
  - `max_memory_bytes`
  - `max_output_bytes`
  - `fuel_limit`
- `settlement_refs`: settlement references object (see §7)

`Receipt.payload` MAY additionally contain:

- `failure_code`
- `failure_message`
- `result_ref`

Receipt validation rules:

- `provider_id` MUST equal the quote provider and the receipt signer
- `requester_id` MUST equal the deal requester
- `quote_hash` MUST equal `Deal.payload.quote_hash`
- `finished_at` MUST be greater than or equal to `started_at` when `started_at` is present
- `result_hash` and `result_format` MUST both be present if and only if `execution_state = succeeded`
- If `deal_state = rejected`, then `execution_state` MUST be `not_started`
- If `deal_state = succeeded`, then `execution_state` MUST be `succeeded` and `settlement_state` MUST be consistent with the settlement method: `none` for free deals and `settled` for Lightning-settled deals
- If `deal_state = failed`, then `execution_state` MUST be `failed`
- If `deal_state = canceled`, then `execution_state` MAY be `not_started` or `succeeded`

`Receipt` is the authoritative terminal artifact.

## 4. Invoice Bundle Immutability Model

The signed `invoice_bundle` artifact is immutable once created:

- The signed bytes, hash, and payload fields MUST NOT change after issuance.
- The `state` field in each `InvoiceBundleLeg` within the signed payload is an issuance-time field only, set at bundle creation (typically `"open"`).
- Later observed states of the underlying Lightning invoices (`accepted`, `settled`, `canceled`, `expired`) are NOT mutations of the signed bundle payload; they are tracked externally by the provider and requester.
- Implementations MUST NOT re-sign, re-hash, or alter the `invoice_bundle` artifact bytes to reflect leg state changes.
- The signed bundle is evidence of issuance, not a live state document.

When an `invoice_bundle` expires (`expires_at` is past), the provider SHALL treat any externally-observed `open` legs as `expired` and SHALL NOT gate execution on expired bundles.

## 5. Settlement Methods

### 5.1 Overview

`settlement_method` is a string field on Offer, Quote `settlement_terms`, and Receipt `settlement_refs`. v1 standardizes exactly two methods:

- `"none"` — free service, no payment required
- `"lightning.base_fee_plus_success_fee.v1"` — paid service via Lightning Network

Future settlement methods (Stripe-backed flows, B2B rails, ACH/wire/invoice, custom credit systems) are architecturally expected but MUST NOT be presented as v1 interoperable unless explicitly standardized in a future version.

If a requester encounters an unrecognized `settlement_method` in an Offer, the requester SHALL treat the offer as unsupported rather than attempting settlement.

### 5.2 Settlement Method: `"none"` (Free Service)

When `settlement_method` is `"none"`:

- The provider MUST NOT generate an invoice bundle.
- The requester MUST NOT initiate any payment flow.
- Deal admission proceeds directly to execution without payment gating. The deal starts in `accepted` status, skipping `payment_pending`.
- The provider still admits the deal based on identity, capacity, and policy — the deal does not automatically start without provider admission.

Canonical free-service quote `settlement_terms`:

```json
{
  "method": "none",
  "destination_identity": "",
  "base_fee_msat": 0,
  "success_fee_msat": 0
}
```

Canonical free-service receipt `settlement_refs`:

```json
{
  "method": "none",
  "bundle_hash": null,
  "destination_identity": "",
  "base_fee": {
    "amount_msat": 0,
    "invoice_hash": "",
    "payment_hash": "",
    "state": "canceled"
  },
  "success_fee": {
    "amount_msat": 0,
    "invoice_hash": "",
    "payment_hash": "",
    "state": "canceled"
  }
}
```

The fee leg state `"canceled"` indicates no invoice was ever issued. The states `open`, `accepted`, `settled`, and `expired` are invalid for zero-valued legs in free deals.

Receipt `settlement_state` for free deals: `"none"`.

### 5.3 Settlement Method: `"lightning.base_fee_plus_success_fee.v1"` (Paid Service)

The v1 paid settlement method uses a two-leg Lightning model:

- `base_fee`: a standard BOLT11 invoice (the non-conditional fee leg)
- `success_fee`: a hold invoice, released only after requester acceptance

The provider issues a signed `invoice_bundle` before the deal `admission_deadline`.

#### Execution Gating (Funds_Locked)

The provider MUST NOT admit or execute the deal until:

- The base-fee leg is `settled`
- The success-fee leg is `accepted` (or `settled`)

Both conditions are required. Base-fee settlement alone (without success-fee hold acceptance) is NOT sufficient to gate execution.

#### Success-Fee Acceptance Flow

The **requester** controls the success-fee preimage:

1. The requester generates a random secret `s` and computes `success_payment_hash = SHA256(s)`.
2. The requester places `success_payment_hash` in the Deal artifact.
3. The provider creates a hold invoice with that `payment_hash`.
4. `deal.success_payment_hash` MUST equal `invoice_bundle.success_fee.payment_hash`.
5. After execution succeeds and the requester reviews the result, the **requester** releases `s` (via `release_deal_preimage`) to settle the success-fee hold invoice.
6. The provider's node then calls `settle_invoice(s)` on the Lightning backend.
7. The requester — not the provider — decides whether to accept the result and release payment.

On execution failure or deal cancellation, the provider cancels the success-fee hold invoice.

#### Receipt Settlement for Lightning Deals

For `lightning.base_fee_plus_success_fee.v1`:

- `Receipt.payload.settlement_state` is the terminal state of the success-fee leg
- The base-fee leg state is recorded in `Receipt.payload.settlement_refs.base_fee.state`
- The success-fee leg state is recorded in `Receipt.payload.settlement_refs.success_fee.state`

Terminal receipt `settlement_state` values for Lightning deals:

| `settlement_state` | Meaning |
|---|---|
| `"settled"` | Both payment legs reached terminal settled state |
| `"canceled"` | Success-fee hold invoice was canceled |
| `"expired"` | Success-fee hold invoice expired before completion |

The value `"none"` is forbidden on terminal receipts for deals that used `lightning.base_fee_plus_success_fee.v1`.

Examples:

- base fee `settled`, success fee `settled` → `settlement_state = "settled"`
- base fee `settled`, success fee `canceled` → `settlement_state = "canceled"`
- base fee `settled`, success fee `expired` → `settlement_state = "expired"`

## 6. Lightning Settlement Binding

### 6.1 Signed InvoiceBundle

The `invoice_bundle` uses the same signed envelope defined in §2.2 with
`artifact_type = invoice_bundle`. See §3.5 for the payload schema and §4 for
immutability rules.

### 6.2 Requester-Side InvoiceBundle Validation

Before paying either invoice, the requester MUST reject the bundle unless all of
the following hold:

- The `invoice_bundle` envelope verifies (§2.2)
- `provider_id`, `requester_id`, `quote_hash`, and `deal_hash` match the current interaction
- `destination_identity` equals `Quote.payload.settlement_terms.destination_identity`
- `base_fee.amount_msat` and `success_fee.amount_msat` equal the quoted settlement terms
- `success_fee.payment_hash` equals `Deal.payload.success_payment_hash`
- `min_final_cltv_expiry` equals the quoted settlement constraint
- `expires_at` does not exceed the quote deadline
- `invoice_hash = SHA256(UTF-8(invoice_bolt11))` for both legs
- The decoded BOLT11 amount, payment hash, payee identity, and expiry match the signed bundle fields
- The decoded success-fee invoice does not advertise a smaller `min_final_cltv_expiry` than the quoted constraint
- Neither decoded invoice expiry exceeds the quoted settlement constraints or the quote deadline

### 6.3 Requester-Controlled Release

The intended paid flow is:

1. The provider issues a signed `Quote`.
2. The requester issues a signed `Deal` containing `success_payment_hash`.
3. The provider issues a signed `invoice_bundle`.
4. The requester validates the `invoice_bundle` and pays the base fee and hold invoice.
5. The provider observes `funds_locked` and executes the work.
6. The provider returns result material or a `result_ref`.
7. The requester releases `s` to accept the success fee, or the provider cancels or lets the hold expire.
8. The provider emits the final signed `Receipt` only after the success-fee leg is terminal.

## 7. Receipt Terminal-Only Semantics

Receipts are terminal-only artifacts. A provider MUST NOT emit a signed receipt
until the deal has reached a terminal state (`succeeded`, `failed`, `canceled`,
`rejected`).

For Lightning-settled deals, this additionally means settlement MUST be terminal
before receipt emission. If the success-fee leg has not reached a terminal state
(`settled`, `canceled`, or `expired`), the provider simply does not emit a
receipt yet.

There is no `"pending"` value in the receipt `settlement_state` enumeration.
Internal implementation helpers MAY track non-terminal settlement states for
operational purposes, but those values MUST NOT appear in signed receipts.

### 7.1 Receipt Settlement State Values

Complete enumeration of valid `settlement_state` values for terminal receipts:

| Value | Meaning | Valid For |
|---|---|---|
| `"settled"` | All payment legs reached terminal settled state | Lightning-settled deals only |
| `"canceled"` | Success-fee hold invoice was canceled | Lightning-settled deals only |
| `"expired"` | Payment expired before completion | Lightning-settled deals only |
| `"none"` | No settlement involved (free deal) | `settlement_method: "none"` only |

Invariant: A terminal receipt for a deal using `settlement_method: "lightning.base_fee_plus_success_fee.v1"` MUST have `settlement_state` in `{settled, canceled, expired}`. The value `"none"` is forbidden on terminal receipts for paid deals.

Invariant: A terminal receipt for a deal using `settlement_method: "none"` MUST have `settlement_state == "none"`.

## 8. Canonical State Model

### 8.1 Deal State

Canonical deal states are:

- `opened`
- `admitted`
- `rejected`
- `succeeded`
- `failed`
- `canceled`

Allowed transitions:

- `opened → admitted`
- `opened → rejected`
- `opened → canceled`
- `admitted → succeeded`
- `admitted → failed`
- `admitted → canceled`

Semantics:

- `opened`: the requester has emitted a signed `Deal`, but the provider has not yet admitted it
- `admitted`: the provider has observed the required settlement preconditions and accepted the work
- `rejected`: the provider refused admission before execution began
- `succeeded`: execution completed successfully and the success-dependent settlement leg reached `settled`
- `failed`: execution failed after admission
- `canceled`: the interaction ended non-successfully for a non-execution reason, including requester/provider aborts and requester refusal or failure to release the success fee after result staging

For Lightning-settled deals: if the deal has not been admitted by `admission_deadline` (invoices expire or are canceled before funding), the deal transitions to `deal_state: "canceled"` (NOT `"failed"`, which is reserved for post-admission execution failures).

For free deals (`settlement_method: "none"`): admission is immediate — the deal starts in `accepted` status, skipping `payment_pending`. The `admission_deadline` question is only relevant for Lightning deals.

### 8.2 Execution State

Canonical execution states are:

- `not_started`
- `running`
- `succeeded`
- `failed`

Rejected deals MUST NOT enter `running`.

### 8.3 Settlement State

Canonical settlement states are:

- `none`
- `invoice_open`
- `funds_locked`
- `settled`
- `canceled`
- `expired`

For `lightning.base_fee_plus_success_fee.v1`, the meanings are:

- `none`: no settlement bundle exists for the deal
- `invoice_open`: an `invoice_bundle` exists, but the success-fee leg is still open and the provider MUST NOT execute
- `funds_locked`: the base-fee leg is settled and the success-fee hold leg is accepted
- `settled`: the success-fee leg is settled
- `canceled`: the success-fee leg is canceled
- `expired`: the success-fee leg expired

The provider MUST NOT begin execution until settlement is at least `funds_locked`.

### 8.4 Runtime-Local States

Implementations MAY expose runtime-local deal statuses such as `payment_pending` or `result_ready`.

Those are projections over canonical state and are not part of the signed protocol surface.

For v1:

- `payment_pending` projects to canonical `deal_state = opened`
- `result_ready` projects to canonical `deal_state = admitted`, `execution_state = succeeded`, and `settlement_state = funds_locked`

Only canonical states belong in signed receipts.

## 9. Hash Chains and Cross-Artifact Validation

The core commitment chain is:

- `Offer` commits to `descriptor_hash`
- `Quote` commits to `descriptor_hash`, `offer_hash`, and `workload_hash`
- `Deal` commits to `quote_hash`, `workload_hash`, and `success_payment_hash`
- `InvoiceBundle` commits to `quote_hash`, `deal_hash`, and `success_fee.payment_hash`
- `Receipt` commits to `deal_hash`, `quote_hash`, `result_hash`, and settlement references

A verifier checking a full paid interaction MUST validate, in order:

1. `Descriptor`
2. `Offer`
3. `Quote`
4. `Deal`
5. `InvoiceBundle`
6. `Receipt`

The full chain is valid only if:

- Every envelope verifies independently (§2.2)
- Every hash reference resolves to the expected prior artifact
- Actor identities line up across the chain
- The workload hash is unchanged from quote to deal
- The success payment hash is unchanged from deal to `invoice_bundle` and receipt
- The settlement destination and fee amounts are unchanged from quote to `invoice_bundle` and receipt

For free deals (`settlement_method: "none"`), the chain is:

1. `Descriptor`
2. `Offer`
3. `Quote`
4. `Deal`
5. `Receipt`

No `InvoiceBundle` exists in the free-deal chain.

## 10. Expiry Ordering Constraints

Each expiry field constrains a specific phase of the deal lifecycle. These MUST NOT be conflated.

### 10.1 Expiry Field Definitions

**`quote.expires_at`** — Quote validity window. After this time, the quote MUST NOT be used to create new deals. This is the outermost deadline for deal creation.

**`deal.admission_deadline`** — The deadline by which the provider must admit the deal. For Lightning deals, this means the provider must issue the `invoice_bundle` and the requester must fund it (reach Funds_Locked) before this time. For free deals, admission is immediate. If a Lightning deal is not admitted by this deadline (invoices expire or are canceled), the deal transitions to `deal_state: "canceled"` with failure code `payment_expired` or `payment_canceled`.

**`invoice_bundle.expires_at`** — The payment window for the base-fee invoice and the success-fee hold acceptance. This constrains only the initial funding phase: the requester must pay the base_fee and the success_fee hold must be accepted within this window. This MUST be `<= deal.admission_deadline` and `<= quote.expires_at`. Critically, the success-fee hold invoice itself must survive beyond `invoice_bundle.expires_at` — it must remain valid through execution AND requester acceptance (up to `deal.acceptance_deadline`). The `invoice_bundle.expires_at` constrains when the hold must be *accepted*, not when it must be *settled*.

**`deal.completion_deadline`** — The deadline by which execution MUST complete. MUST be `> deal.admission_deadline`.

**`deal.acceptance_deadline`** — The deadline by which the requester MUST accept or reject the result (release or withhold the success-fee preimage). For Lightning deals, the success-fee hold invoice must remain valid until at least this deadline. MUST be `> deal.completion_deadline`.

### 10.2 Cross-Artifact Ordering

```
offer.expires_at (if present)  >= quote.expires_at

quote.expires_at               >= deal.admission_deadline

deal.admission_deadline        >= invoice_bundle.expires_at  (Lightning deals)

deal.admission_deadline        <  deal.completion_deadline
                               <  deal.acceptance_deadline
```

The success-fee hold invoice expiry (the BOLT11 invoice's own expiry) is a separate concern from `invoice_bundle.expires_at`. The hold invoice must survive through `deal.acceptance_deadline` to allow the requester time to review and accept. The `max_success_hold_expiry_secs` in `Quote.settlement_terms` constrains this.

### 10.3 Expiry Validation Rules

- Expired offers (where `expires_at` is present and past) are invalid for new quotes. A provider MUST NOT issue a quote referencing an expired offer, and a requester MUST NOT request a quote against an expired offer.
- Expired transport endpoints in `Descriptor.transport_endpoints` affect reachability only, not artifact validity. A signed Descriptor with expired transport endpoints remains a valid signed artifact, but clients SHOULD NOT expect those endpoints to be reachable.
- Quote expiry past means no new deals from that quote.
- A provider receiving a deal submission after `quote.expires_at` SHALL reject the deal.

## 11. Non-Kernel Exclusion Boundary

The following are explicitly NOT part of the kernel. Implementations MUST NOT treat these as protocol commitments:

**Product surfaces:**
- OpenClaw, NemoClaw, MCP tool contracts
- Marketplace search, registration, and indexing
- Project authoring, build, test, publish flows
- Runtime payment-intent helpers

**Higher layers:**
- Marketplaces, catalogs, brokers
- Ranking, reputation, policy systems
- Indexers, curated lists, private catalogs

**Execution profiles:**
- Execution runtime choice (Wasm, Python, container, confidential) is an execution profile concern, not a kernel commitment. The kernel defines the signed economic primitive; execution profiles define how work is performed within that primitive.

**Adapters:**
- Transport choice (HTTPS, Tor, Nostr relay) is an adapter concern that MUST NOT change kernel artifact semantics.
- Settlement driver choice (Mock Lightning, LND REST, future drivers) MUST preserve invoice_bundle commitments, leg-state meanings, gating rules, and receipt semantics.
- Discovery bootstrap (direct peers, allowlists, curated lists, private catalogs, brokers) is an adapter concern.
- Execution material delivery (module uploads, source bundles, archives, container references) is an adapter concern.
- Deployment (Docker Compose, Kubernetes, cloud-native) MUST preserve kernel semantics and artifact verification.

**Storage:**
- Storage engine choice is an implementation concern that MUST preserve artifact immutability and evidence retention invariants.

**Future extensions (not part of v1):**
- Additional settlement methods (Stripe, B2B, ACH, credit systems)
- Long-running batch orchestration
- Native cloud deployment adapters
- Archive/zip packaging as first-class execution format
- Marketplace, ranking, reputation, broker policy as protocol actors

These are architecturally expected but MUST NOT be presented as v1 interoperable.

## 12. Conformance

Conformance is verified against the canonical test vectors in `conformance/kernel_v1.json`.

A conformant implementation MUST:

- Correctly verify all artifact envelopes (§2.2)
- Correctly verify linked identity challenges (§2.3)
- Correctly validate the full hash chain (§9)
- Correctly enforce settlement gating rules (§5.3)
- Correctly enforce expiry ordering constraints (§10)
- Correctly enforce receipt terminal-only semantics (§7)
- Pass all verification cases in `conformance/kernel_v1.json` with the expected outcomes
