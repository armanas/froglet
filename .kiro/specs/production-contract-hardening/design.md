# Design Document

## Overview

This design hardens the Froglet production contract by auditing the current state, redesigning the contract layer model, creating temporary helper documentation, and planning follow-up implementation work. The work is organized into four phases that can be executed sequentially.

This is a pre-stable protocol and documentation cleanup, not just clarification of existing behavior. It deliberately defines and normalizes a new canonical `method = "none"` shape across offers, quotes, receipts, tests, and conformance vectors. Offer/quote/receipt shapes and conformance vectors will change as a result.

### Honesty About Change Categories

This work spans three distinct categories, and the design is explicit about which items fall where:

**(a) Pure documentation/specification work** (no code behavior changes):
- Writing `docs/KERNEL.md` and `docs/SERVICE_BINDING.md` to formalize what the code already does
- Updating broken `SPEC.md` cross-references to point to new authoritative documents
- Defining the four-layer contract model (Kernel, Service Binding, Node/Tool Surface, Higher Layer)
- Specifying expiry ordering constraints that the code already enforces
- Specifying invoice bundle immutability semantics that the code already implements
- Documenting the non-kernel exclusion boundary

**(b) Implementation behavior changes** that align code with the newly specified protocol:
- Normalizing `"none.v1"` → `"none"` in test fixtures (`tests/runtime_routes.rs`)
- Changing offer generation to emit `settlement_method: "none"` for free services instead of unconditionally emitting `"lightning.base_fee_plus_success_fee.v1"` (`src/api.rs`)
- Defining and implementing canonical free-service quote `settlement_terms` with `method: "none"`, `destination_identity: ""` instead of falling through to the Lightning method string (`src/api.rs`)
- Ensuring receipts are only emitted after settlement reaches a terminal state for Lightning deals (no `pending` value in signed receipts)
- Updating free-service test fixture receipt shapes to match the new canonical `method = "none"` shape (`tests/runtime_routes.rs`)

**(c) Protocol-level additions** (new conformance surface):
- Adding free-service conformance vectors to `conformance/kernel_v1.json` (new canonical test cases that did not previously exist)
- Defining the exact canonical serialization shape for free-service artifacts (a design decision, since the current code's production shape and test fixture shape disagree — see Finding 1.5)

## Phase 1: Production Alignment Audit

### Audit Scope

Systematically compare the codebase against the requirements to identify every gap, contradiction, and undefined behavior.

### Audit Findings

#### Finding 1.1: Settlement Method String Inconsistency

**Source:** `src/api.rs` line ~4618, `tests/runtime_routes.rs` line ~396
**Issue:** Production offer generation unconditionally sets `settlement_method: "lightning.base_fee_plus_success_fee.v1"` regardless of price. Test code uses `"none.v1"` for free services. Receipt construction in `api.rs` uses `"none"` (without `.v1`).
**Impact:** Three different strings for the free-service settlement method across the codebase. No single canonical value.
**Resolution:** Normalize to `"none"` everywhere. Update offer generation to use `"none"` when `price_sats == 0`.

#### Finding 1.2: settlement_state "none" Overloaded

**Source:** `src/api.rs` `settlement_state_from_bundle` function (~line 8108)
**Issue:** Returns `"none"` for both free deals (bundle=None) AND non-terminal Lightning payment states (Open, Accepted). This means the internal state tracker conflates "free deal, no payment involved" with "paid deal, payment not yet terminal."
**Impact:** If a receipt were emitted while Lightning settlement is still non-terminal, it would carry `settlement_state: "none"`, which is semantically wrong. The correct fix is not to add a `"pending"` value to signed receipts (that would be a protocol change), but to ensure receipts are only emitted after settlement reaches a terminal state.
**Resolution:** Enforce that receipts are terminal-only artifacts: the provider MUST NOT emit a signed receipt until settlement is terminal for Lightning deals. The internal `settlement_state_from_bundle` helper may use any internal tracking values, but those values never appear in signed receipts.

#### Finding 1.3: Execution Gating Definition

**Source:** `src/settlement.rs` `lightning_bundle_is_funded` function (~line 943)
**Issue:** The code correctly implements Funds_Locked as `base_state == Settled AND success_state in {Accepted, Settled}`. This is the correct two-condition gating. However, the previous requirements document (Requirement 2, criterion 4) described gating as "base_fee payment confirmation" only, omitting the success_fee hold acceptance requirement.
**Impact:** An implementer reading only the old spec might gate on base_fee alone, which is insufficient.
**Resolution:** Requirements now explicitly define Funds_Locked as both conditions. Design matches code.

#### Finding 1.4: Invoice Bundle Leg State Semantics

**Source:** `src/protocol.rs` `InvoiceBundleLeg` struct, `src/settlement.rs` `LightningInvoiceBundleSession`
**Issue:** The `InvoiceBundleLeg` struct in the signed payload includes a `state` field. The `LightningInvoiceBundleSession` tracks `base_state` and `success_state` separately from the signed bundle. The code correctly treats the signed payload as immutable and tracks observed states externally. However, the struct definition could mislead implementers into thinking the signed payload's state field is mutable.
**Impact:** Ambiguity about whether the signed bundle's leg states are live or issuance-time snapshots.
**Resolution:** Specification must explicitly state that signed payload leg states are issuance-time only. External tracking is the correct pattern.

#### Finding 1.5: Empty Receipt Leg Shape — Production vs Test Fixture Disagreement

**Source:** `src/api.rs` `empty_receipt_leg` function (~line 8068), `tests/runtime_routes.rs` (~line 518)
**Issue:** The production code (`empty_receipt_leg`) produces free-deal receipt legs with `state: Canceled`, `amount_msat: 0`, empty `invoice_hash: ""`, empty `payment_hash: ""`, and `destination_identity: ""` (empty string). However, the test fixtures in `tests/runtime_routes.rs` use a different shape: `state: Settled`, `invoice_hash: "00".repeat(32)`, `payment_hash: success_payment_hash`, and `destination_identity: provider_id`.

**This is a real disagreement between production code and test code.** The test fixtures were written with `"none.v1"` as the settlement method and use non-canonical shapes that don't match what `settlement_refs_from_bundle(None)` actually produces.

**Impact:** Independent implementations cannot determine the canonical free-service receipt shape from the codebase alone — production code says one thing, tests say another.

**Design decision required:** The canonical v1 shape for free-service receipt `settlement_refs` must be explicitly chosen and documented. The production code shape (empty strings, `Canceled` state) is the recommended canonical form because:
1. It is what the production `settlement_refs_from_bundle(None)` path actually emits
2. `Canceled` correctly conveys "no invoice was ever issued" (as opposed to `Settled` which implies payment occurred)
3. Empty strings for `invoice_hash` and `payment_hash` correctly convey "no invoice exists"

The test fixtures must be updated to match the chosen canonical shape.

#### Finding 1.6: Missing SPEC.md References

**Source:** `README.md`, `docs/ADAPTERS.md`, `docs/ARCHITECTURE.md`, `docs/NOSTR.md`, `CONTRIBUTING.md`, `AGENTS.md`
**Issue:** Six files reference `SPEC.md` which was deleted from the repository (last available at commit `b11880d7`). All cross-references are broken.
**Impact:** Developers following documentation links hit dead ends. No authoritative kernel specification exists in the repo.
**Resolution:** Create temporary helper documentation and update all cross-references.

#### Finding 1.7: Expiry Cross-Artifact Relationships Unspecified

**Source:** `src/settlement.rs` `effective_bundle_expiry_secs`, protocol.rs `InvoiceBundlePayload.expires_at`
**Issue:** The code enforces `invoice_bundle.expires_at <= quote.expires_at` implicitly through the bundle expiry calculation, but the relationship between bundle expiry, quote expiry, and deal admission_deadline is not formally documented.
**Impact:** Independent implementations might set bundle expiry beyond quote expiry, creating invalid payment windows.
**Resolution:** Formally specify the cross-artifact expiry ordering constraints.

#### Finding 1.8: Offer/Descriptor Expiry and Quote Validity Unspecified

**Source:** `src/protocol.rs` `OfferPayload.expires_at` (Option<i64>)
**Issue:** Offers have an optional `expires_at` field, but there is no specification of whether expired offers are valid for new quote requests.
**Impact:** Implementations might accept quote requests against expired offers.
**Resolution:** Specify that expired offers are invalid for new quotes.

#### Finding 1.9: Deal State for Unadmitted Lightning Deals

**Source:** `src/api.rs` `lightning_settlement_failure_details` function (~line 7294)
**Issue:** When a Lightning deal in `payment_pending` status has its invoices expire or get canceled before funding, the code transitions the deal to `deal_state: "canceled"` (not `"failed"`). The `"failed"` state is reserved for post-admission execution failures. Free deals skip `payment_pending` entirely (they go directly to `accepted`), so the admission_deadline question is only relevant for Lightning deals.
**Impact:** The previous design incorrectly stated "Deal admission_deadline past without admission means deal failure." The actual behavior is `deal_state: "canceled"` with failure codes `payment_expired` or `payment_canceled`.
**Resolution:** Correct the design to reflect the actual code behavior: unadmitted Lightning deals transition to `canceled`, not `failed`.

## Phase 2: Contract Redesign

### 2.1 Contract Layer Model

The specification is reorganized into five distinct contract surfaces with clear normative boundaries:

```
┌─────────────────────────────────────────────────┐
│  Higher Layers (non-normative)                  │
│  marketplaces, brokers, ranking, reputation     │
├─────────────────────────────────────────────────┤
│  Node/Tool Surface (stable product surface)     │
│  /v1/froglet/*, OpenClaw, NemoClaw, MCP         │
├─────────────────────────────────────────────────┤
│  Service Binding (normative for interop)        │
│  service_id, offer_kind, discovery records      │
├─────────────────────────────────────────────────┤
│  Adapters (non-normative, kernel-preserving)    │
│  transport, settlement drivers, deployment      │
├─────────────────────────────────────────────────┤
│  Kernel (normative, frozen)                     │
│  signed envelope, artifacts, state machines,    │
│  settlement methods, conformance vectors        │
└─────────────────────────────────────────────────┘
```

### 2.2 Kernel Contract

The kernel contract is the smallest irreversible protocol surface. All rules use normative language (MUST, MUST NOT, SHOULD, MAY).

#### 2.2.1 Signed Envelope

No changes. The canonical signing bytes format remains:
`[schema_version, artifact_type, signer, created_at, payload_hash, payload]`

Serialization: JCS. Hashing: SHA-256. Signing: secp256k1 Schnorr (BIP-340).

#### 2.2.2 Artifact Types

Six artifact types, unchanged:
- **Descriptor**: Provider identity, capabilities, transport endpoints, linked identities
- **Offer**: Service terms, pricing, execution profile, settlement method
- **Quote**: Provider-signed commitment to execute at specific terms
- **Deal**: Requester-signed acceptance of a quote with deadlines
- **InvoiceBundle**: Provider-signed Lightning invoices for payment (Lightning-settled deals only)
- **Receipt**: Provider-signed evidence of deal outcome

#### 2.2.3 Settlement Methods

Two standardized v1 methods:

**`"none"` (free service)**:
- No invoice bundle generated
- No payment gating
- Deal proceeds directly to execution after admission (initial status: `accepted`, skipping `payment_pending`)
- **Quote `settlement_terms`**: The canonical v1 form for free-service quote settlement_terms is: `method: "none"`, `base_fee_msat: 0`, `success_fee_msat: 0`, `destination_identity: ""` (empty string). This is a deliberate pre-stable protocol decision. The current production code falls through to the Lightning method string with zero fees when `quoted_lightning_settlement_terms` returns `None`, and the test fixtures use `method: "none.v1"` — neither is the intended canonical form. The implementation must set the canonical shape explicitly.
- **Receipt `settlement_refs`**: Production code (`settlement_refs_from_bundle(None)`) emits: `{ method: "none", bundle_hash: null, destination_identity: "", base_fee: { amount_msat: 0, invoice_hash: "", payment_hash: "", state: "canceled" }, success_fee: { amount_msat: 0, invoice_hash: "", payment_hash: "", state: "canceled" } }`. This is the canonical v1 shape.
- Receipt `settlement_state`: `"none"`
- Fee leg state `"canceled"` indicates no invoice was ever issued

**`"lightning.base_fee_plus_success_fee.v1"` (paid service)**:
- Two-leg model: base_fee (standard BOLT11) + success_fee (hold invoice)
- Provider issues signed `invoice_bundle` before `deal.admission_deadline`
- Execution gating (Funds_Locked): `base_fee.state == settled AND success_fee.state in {accepted, settled}`
- Base_fee settlement alone is NOT sufficient
- **Success-fee acceptance flow**: The **requester** controls the success-fee acceptance preimage. The requester generates a random secret `s`, computes `success_payment_hash = SHA256(s)`, and places `success_payment_hash` in the Deal artifact. The provider creates a hold invoice with that `payment_hash`. After execution succeeds and the requester reviews the result, the **requester** releases `s` (via `release_deal_preimage`) to settle the success_fee hold invoice. The provider's node then calls `settle_invoice(s)` on the Lightning backend. The requester — not the provider — decides whether to accept the result and release payment.
- On execution failure or deal cancellation: provider cancels the success_fee hold invoice
- `deal.success_payment_hash == invoice_bundle.success_fee.payment_hash`
- Receipt `settlement_state` for terminal receipts: `"settled"`, `"canceled"`, or `"expired"` (never `"none"`)

#### 2.2.4 Invoice Bundle Immutability Model

The signed `invoice_bundle` artifact is immutable once created:
- Signed bytes, hash, and payload fields MUST NOT change after issuance
- The `state` field in each `InvoiceBundleLeg` within the signed payload is an issuance-time field (typically `"open"`)
- Later observed states (`accepted`, `settled`, `canceled`, `expired`) are tracked externally by provider and requester
- Implementations MUST NOT re-sign, re-hash, or alter bundle bytes to reflect state changes
- The signed bundle is evidence of issuance, not a live state document

#### 2.2.5 Receipt Settlement State Values

Complete enumeration of valid `settlement_state` values for terminal receipts:

| Value | Meaning | Valid For |
|-------|---------|-----------|
| `"settled"` | All payment legs reached terminal settled state | Lightning-settled deals only |
| `"canceled"` | Success_fee hold invoice was canceled | Lightning-settled deals only |
| `"expired"` | Payment expired before completion | Lightning-settled deals only |
| `"none"` | No settlement involved (free deal) | `settlement_method: "none"` only |

Invariant: A terminal receipt for a deal using `settlement_method: "lightning.base_fee_plus_success_fee.v1"` MUST have `settlement_state` in `{settled, canceled, expired}`. The value `"none"` is forbidden on terminal receipts for paid deals.

**Non-terminal receipts**: Receipts are terminal-only artifacts. A provider MUST NOT emit a signed receipt until the deal has reached a terminal state (`succeeded`, `failed`, `canceled`, `rejected`) AND settlement has reached a terminal state for Lightning deals. If settlement has not reached a terminal state, the provider simply does not emit a receipt yet. There is no `"pending"` value in the receipt `settlement_state` enumeration. Internal implementation helpers may track non-terminal settlement states for operational purposes, but those values MUST NOT appear in signed receipts.

#### 2.2.6 Expiry Semantics

Each expiry field constrains a specific phase of the deal lifecycle. These MUST NOT be conflated:

**`quote.expires_at`** — Quote validity window. After this time, the quote MUST NOT be used to create new deals. This is the outermost deadline for deal creation.

**`deal.admission_deadline`** — The deadline by which the provider must admit the deal. For Lightning deals, this means the provider must issue the `invoice_bundle` and the requester must fund it (reach Funds_Locked) before this time. For free deals, admission is immediate (deal starts in `accepted` status). If a Lightning deal is not admitted by this deadline (invoices expire or are canceled), the deal transitions to `deal_state: "canceled"` with failure code `payment_expired` or `payment_canceled` — NOT `"failed"` (which is reserved for post-admission execution failures).

**`invoice_bundle.expires_at`** — The payment window for the base-fee invoice and the success-fee hold acceptance. This constrains only the initial funding phase: the requester must pay the base_fee and the success_fee hold must be accepted within this window. This MUST be `<= deal.admission_deadline` and `<= quote.expires_at`. Critically, the success-fee hold invoice itself must survive beyond `invoice_bundle.expires_at` — it must remain valid through execution AND requester acceptance (up to `deal.acceptance_deadline`). The `invoice_bundle.expires_at` constrains when the hold must be *accepted*, not when it must be *settled*.

**`deal.completion_deadline`** — The deadline by which execution MUST complete. Must be `> deal.admission_deadline`.

**`deal.acceptance_deadline`** — The deadline by which the requester MUST accept or reject the result (release or withhold the success-fee preimage). For Lightning deals, the success-fee hold invoice must remain valid until at least this deadline, since the requester may not release the preimage until after reviewing the result. Must be `> deal.completion_deadline`.

Cross-artifact ordering constraints:

```
offer.expires_at (if present)  >= quote.expires_at

quote.expires_at               >= deal.admission_deadline

deal.admission_deadline        >= invoice_bundle.expires_at (for Lightning deals)

deal.admission_deadline        <  deal.completion_deadline
                               <  deal.acceptance_deadline
```

The success-fee hold invoice expiry (the BOLT11 invoice's own expiry) is a separate concern from `invoice_bundle.expires_at`. The hold invoice must survive through `deal.acceptance_deadline` to allow the requester time to review and accept. The `max_success_hold_expiry_secs` in `QuoteSettlementTerms` constrains this.

Rules:
- Expired offers (where `expires_at` is present and past) are invalid for new quotes
- Expired transport endpoints affect reachability only, not artifact validity
- Quote expiry past means no new deals from that quote

#### 2.2.7 Settlement Method Extensibility

- `settlement_method` is a string field on Offer, Quote settlement_terms, and Receipt settlement_refs
- v1 standardizes exactly `"none"` and `"lightning.base_fee_plus_success_fee.v1"`
- Future methods are architecturally expected but MUST NOT claim v1 interoperability
- Unrecognized methods: requester treats offer as unsupported

### 2.3 Service Binding Contract

Normative for interoperability-critical rules. Defines how service discovery and invocation reduce to kernel deal fields.

**Scope:**
- `service_id`, `offer_id`, `offer_kind`, `resource_kind` relationships
- Three product shapes: named services, data services, direct compute
- Service record structure from discovery (required and optional fields)
- `invoke_service` resolution from service manifest to workload spec and deal parameters

**Explicitly excluded:**
- Local project layout, file layout, build pipelines
- Host-specific API shapes
- Project authoring workflows

### 2.4 Node/Tool Surface Contract

The `/v1/froglet/*` operator control API and the bot-facing `froglet` tool contract (OpenClaw/NemoClaw/MCP) are a **stable product surface**. While not kernel-normative (they do not affect signed artifact semantics or cross-implementation interoperability), bots and integrations are meant to reliably depend on these interfaces.

**Stability commitment:** The node/tool surface is treated as a supported product contract. Breaking changes to `/v1/froglet/*` routes or the `froglet` tool schema require explicit migration guidance, even though they are not part of the kernel protocol. The "non-normative" label means these interfaces are not part of the interoperable protocol specification — it does NOT mean they are unstable or unsupported.

Covers:
- `/v1/froglet/*` operator control API routes
- `froglet` tool contract (OpenClaw/NemoClaw/MCP)
- Local project authoring, build, test, publish flows
- Runtime payment-intent helpers
- Status, logs, restart operations

### 2.5 Non-Normative Architecture and Adapters

Adapter boundaries that MUST preserve kernel semantics but MAY vary operationally:
- **Transport**: HTTPS, Tor, Nostr relay. Transport choice MUST NOT change artifact semantics.
- **Settlement drivers**: Mock Lightning, LND REST, future drivers. MUST preserve invoice_bundle commitments, leg-state meanings, gating rules, receipt semantics.
- **Discovery bootstrap**: Direct peers, allowlists, curated lists, private catalogs, brokers.
- **Execution material delivery**: Module uploads, source bundles, archives, container references.
- **Deployment**: Docker Compose, Kubernetes, cloud-native. MUST preserve kernel semantics and artifact verification.

### 2.6 Future Extension Boundaries

Explicitly documented as NOT part of v1:
- Additional settlement methods (Stripe, B2B, ACH, credit systems)
- Long-running batch orchestration
- Native cloud deployment adapters
- Archive/zip packaging as first-class execution format
- Marketplace, ranking, reputation, broker policy as protocol actors

These are architecturally expected but MUST NOT be presented as v1 interoperable.

## Phase 3: Temporary Helper Documentation Structure

### 3.1 Documentation Strategy

Rather than restoring the deleted `SPEC.md` file, create focused helper contract documents that serve as the current authoritative contract materials. `docs/KERNEL.md` is the current authoritative normative kernel specification. `docs/SERVICE_BINDING.md` is the current authoritative normative service binding specification. Both are temporary and may later be removed or folded elsewhere, but they are the canonical reference for now. All cross-references must resolve to these actual authoritative documents.

### 3.2 Proposed Documentation Structure

```
docs/
├── KERNEL.md              # Normative kernel contract (new)
│   ├── Signed envelope
│   ├── Artifact types and payloads
│   ├── Settlement methods (none, lightning.v1)
│   ├── Invoice bundle immutability model
│   ├── Receipt settlement state semantics
│   ├── Expiry ordering constraints
│   ├── Linked identity challenge format
│   └── Non-kernel exclusion boundary
├── SERVICE_BINDING.md     # Normative service binding (new)
│   ├── service_id / offer_id / offer_kind mapping
│   ├── Product shapes → kernel reduction
│   └── Discovery record structure
├── ARCHITECTURE.md        # Updated, non-normative
│   └── References → docs/KERNEL.md
├── ADAPTERS.md            # Updated, non-normative
│   └── References → docs/KERNEL.md
├── NOSTR.md               # Updated, non-normative
│   └── References → docs/KERNEL.md
├── OPERATOR.md            # Unchanged, non-normative
└── ... (other existing docs unchanged)
```

### 3.3 Cross-Reference Updates

All files currently referencing `SPEC.md` must be updated:

| File | Current Reference | Updated Reference |
|------|-------------------|-------------------|
| `README.md` | `[SPEC.md](SPEC.md)` | `[docs/KERNEL.md](docs/KERNEL.md)` |
| `docs/ADAPTERS.md` | `[../SPEC.md](../SPEC.md)` | `[KERNEL.md](KERNEL.md)` |
| `docs/ARCHITECTURE.md` | `[../SPEC.md](../SPEC.md)` | `[KERNEL.md](KERNEL.md)` |
| `docs/NOSTR.md` | `[../SPEC.md](../SPEC.md)` | `[KERNEL.md](KERNEL.md)` |
| `CONTRIBUTING.md` | `SPEC.md` | `docs/KERNEL.md` |
| `AGENTS.md` | `SPEC.md` | `docs/KERNEL.md` |

### 3.4 Normative Status Markers

Each document must carry a status line:
- `Status: normative kernel specification` (for KERNEL.md)
- `Status: normative service binding specification` (for SERVICE_BINDING.md)
- `Status: non-normative supporting document` (existing docs already have this)

## Phase 4: Production Follow-Up Work Plan

### 4.1 Implementation Behavior Changes (Code Changes)

These are codebase-specific changes that align the reference implementation with the newly specified protocol. They are behavior changes — not pure documentation — and are tracked separately from protocol specification work.

#### 4.1.1 Settlement Method String Normalization

**File:** `tests/runtime_routes.rs`
**Change:** Replace `"none.v1"` with `"none"` in all test fixtures. Update receipt fixture shapes to match production `empty_receipt_leg` output (empty strings, `Canceled` state).
**Category:** (b) implementation behavior change

#### 4.1.2 Offer Generation Settlement Method

**File:** `src/api.rs` (~line 4618, `payload_from_provider_offer_definition`)
**Change:** Conditionally set `settlement_method` based on pricing:
- `price_sats == 0` → `settlement_method: "none"`
- `price_sats > 0` → `settlement_method: "lightning.base_fee_plus_success_fee.v1"`

Currently the code unconditionally sets `settlement_method: "lightning.base_fee_plus_success_fee.v1"` regardless of price.
**Category:** (b) implementation behavior change

#### 4.1.3 Quote Settlement Terms for Free Services

**File:** `src/api.rs` (~line 4303, `quoted_settlement_terms`)
**Change:** When `quoted_lightning_settlement_terms` returns `None` (free service or no Lightning backend), the fallback currently emits `method: "lightning.base_fee_plus_success_fee.v1"` with zero fees. This must be changed to emit `method: "none"` with appropriate zero-fee terms.
**Category:** (b) implementation behavior change

#### 4.1.4 Receipt Terminal-Only Enforcement

**File:** `src/api.rs` (receipt emission paths)
**Change:** Ensure the provider does NOT emit a signed receipt while Lightning settlement legs are still in `Open` or `Accepted` state. For Lightning deals, receipt emission must wait until the success_fee leg reaches a terminal state (`settled`, `canceled`, or `expired`). For free deals, no settlement gating is needed. The internal `settlement_state_from_bundle` helper may still use any internal tracking values, but those values never appear in signed receipts.
**Category:** (b) implementation behavior change

#### 4.1.5 Empty Receipt Leg State Verification

**File:** `src/api.rs` `empty_receipt_leg` function (~line 8068)
**Status:** Already uses `ReceiptLegState::Canceled`. No change needed. Verify test fixtures match.
**Category:** (a) verification only

### 4.2 Conformance Vector Additions

**Category:** (c) protocol-level addition

#### 4.2.1 Free-Service Round-Trip Vector

Add to `conformance/kernel_v1.json`:
- Valid Offer with `settlement_method: "none"`, `price_schedule: { base_fee_msat: 0, success_fee_msat: 0 }`
- Valid Quote with `settlement_terms: { method: "none", base_fee_msat: 0, success_fee_msat: 0 }`
- Valid Deal referencing the free-service quote
- Valid Receipt with `settlement_state: "none"`, `settlement_refs.method: "none"`, zero-valued canceled legs
- Tampered variants for each

#### 4.2.2 InvoiceBundle Artifact Vectors

The existing conformance suite includes an InvoiceBundle artifact. Verify coverage includes:
- Valid InvoiceBundle with correct cross-artifact references
- Tampered InvoiceBundle (signature, hash, payload modification)

### 4.3 Documentation Deliverables

**Category:** (a) pure documentation/specification work

#### 4.3.1 docs/KERNEL.md

New file. Normative kernel specification covering:
- Signed envelope format and verification
- All six artifact type payload schemas
- Settlement method definitions (none, lightning.v1)
- Invoice bundle immutability model
- Receipt settlement state semantics
- Expiry ordering constraints
- Linked identity challenge format (migrated from SPEC.md content)
- Non-kernel exclusion boundary

Content source: Extract from historical `SPEC.md` (commit `b11880d7`), updated to match current code behavior and the requirements in this spec.

#### 4.3.2 docs/SERVICE_BINDING.md

New file. Normative service binding specification covering:
- service_id / offer_id / offer_kind / resource_kind relationships
- Three product shapes and their kernel reduction
- Discovery record structure
- invoke_service resolution

#### 4.3.3 Cross-Reference Updates

Update all six files listed in Phase 3.3 to point to the new authoritative documents.

### 4.4 Test Updates

- Update `tests/runtime_routes.rs` free-service test fixtures to use `"none"` instead of `"none.v1"` and align receipt shapes with the new canonical `method = "none"` shape
- Add integration test verifying that free-service offers use `settlement_method: "none"`
- Add integration test verifying that receipts are not emitted while Lightning settlement is non-terminal
- Verify existing conformance vector tests pass unchanged (no kernel changes to existing vectors)

## Correctness Properties

*A property is a characteristic or behavior that should hold true across all valid executions of a system — essentially, a formal statement about what the system should do. Properties serve as the bridge between human-readable specifications and machine-verifiable correctness guarantees.*

### Property 1: Settlement Method String Canonicality

*For all* artifacts in the system (offers, quotes, receipts, test fixtures, conformance vectors), the `settlement_method` / `method` field is either exactly `"none"` or exactly `"lightning.base_fee_plus_success_fee.v1"`. No other strings (including `"none.v1"`) appear.

**Validates: Requirements 9.1, 10.1**

### Property 2: Free-Deal Receipt Shape Invariant

*For all* receipts where `settlement_refs.method == "none"`: `bundle_hash` is null (JSON null or absent), `destination_identity` is empty string, and both fee legs have `amount_msat: 0`, empty `invoice_hash: ""`, empty `payment_hash: ""`, and `state: "canceled"`.

**Validates: Requirements 1.5, 1.6, 6.3**

### Property 3: Funds_Locked Gating Invariant

*For all* Lightning-settled deals and all combinations of `base_state` and `success_state`, execution starts if and only if `base_fee.state == settled AND success_fee.state in {accepted, settled}`. In particular, `base_fee.state == settled AND success_fee.state == open` MUST NOT gate execution.

**Validates: Requirements 2.4, 2.8**

### Property 4: Invoice Bundle Immutability

*For all* signed invoice bundles, `verify_artifact(bundle)` returns true at issuance time and continues to return true regardless of externally observed leg state changes. The bundle hash and signature are stable across the bundle's lifetime.

**Validates: Requirements 4.1, 4.2, 4.3**

### Property 5: Receipt Settlement State Consistency

*For all* terminal receipts: if `settlement_refs.method == "lightning.base_fee_plus_success_fee.v1"`, then `settlement_state` is in `{settled, canceled, expired}` (never `"none"`), and `settlement_refs` includes a non-null `bundle_hash`, a non-empty `destination_identity`, and fee legs with actual invoice/payment hashes. If `settlement_refs.method == "none"`, then `settlement_state == "none"`.

**Validates: Requirements 6.1, 6.2, 6.4, 6.5**

### Property 6: Expiry Ordering

*For all* valid deal chains with Lightning settlement: `invoice_bundle.expires_at <= deal.admission_deadline` and `invoice_bundle.expires_at <= quote.expires_at`. For all deals: `deal.admission_deadline < deal.completion_deadline < deal.acceptance_deadline`.

**Validates: Requirements 4.5, 5.9**

### Property 7: Conformance Vector Round-Trip

*For all* artifacts in `conformance/kernel_v1.json`: `verify_artifact(artifact)` returns `expected_valid`. Canonical signing bytes produce the expected hash. Signature verification matches expected outcome. Existing vectors remain valid after implementation changes.

**Validates: Requirements 14.1, 14.2, 14.3, 14.4**

### Property 8: Cross-Reference Validity

*For all* markdown files in the repository, every relative link to a `.md` file resolves to an existing file in the repository.

**Validates: Requirements 8.3, 8.5**

### Property 9: Free-Deal Admission Behavior

*For all* deals where `settlement_method == "none"`, no invoice bundle is generated and no payment gating occurs before execution. Free deals skip payment gating but still follow normal provider admission semantics (the provider still admits the deal based on identity, capacity, and policy).

**Validates: Requirements 1.2, 1.4**

### Property 10: Success Payment Hash Linkage

*For all* Lightning-settled deals that have an invoice bundle, `deal.success_payment_hash == invoice_bundle.success_fee.payment_hash`.

**Validates: Requirements 2.7**

### Property 11: Free-Service Offer Settlement Method

*For all* offers where `price_schedule.base_fee_msat == 0` and `price_schedule.success_fee_msat == 0`, the `settlement_method` field is `"none"`.

**Validates: Requirements 9.2, 10.2**

## Error Handling

### Settlement Errors

- **Unrecognized settlement method**: Requester treats offer as unsupported and does not attempt deal creation.
- **Invoice bundle expiry**: Provider treats externally-observed `open` legs as `expired` after `invoice_bundle.expires_at`. Lightning deals in `payment_pending` with expired invoices transition to `deal_state: "canceled"` with failure code `payment_expired`.
- **Invoice cancellation before funding**: Lightning deals in `payment_pending` with canceled invoices transition to `deal_state: "canceled"` with failure code `payment_canceled`.
- **Completion deadline elapsed**: Deals that exceed `completion_deadline` transition to `deal_state: "failed"` with failure code `completion_deadline_elapsed_during_recovery`.
- **Acceptance deadline elapsed**: Requester preimage release is rejected after `acceptance_deadline`. Success-fee hold may expire, resulting in `deal_state: "canceled"` with failure code `success_fee_expired_before_release`.

### Artifact Verification Errors

- **Tampered artifacts**: `verify_artifact` returns false. Implementations MUST reject tampered artifacts.
- **Expired quotes**: Deal creation against expired quotes is rejected with HTTP 410 Gone.
- **Cross-artifact hash mismatches**: Deal `quote_hash` must match the actual quote artifact hash. Invoice bundle `deal_hash` must match the deal artifact hash.

## Testing Strategy

### Dual Testing Approach

Both unit tests and property-based tests are required for comprehensive coverage:

- **Unit tests**: Specific examples, edge cases, error conditions, integration points
- **Property-based tests**: Universal properties across all valid inputs

### Property-Based Testing Configuration

- **Library**: Use a Rust property-based testing library (e.g., `proptest` or `quickcheck`)
- **Minimum iterations**: 100 per property test
- **Tag format**: Each test must include a comment referencing the design property:
  `// Feature: production-contract-hardening, Property {N}: {title}`
- **Each correctness property** from the design MUST be implemented by a SINGLE property-based test

### Unit Test Focus Areas

- Conformance vector verification (existing + new free-service vectors)
- `settlement_state_from_bundle` with all `InvoiceBundleLegState` combinations
- `empty_receipt_leg` shape verification
- `lightning_bundle_is_funded` with all state combinations
- Cross-artifact expiry validation edge cases
- Deal deadline validation (admission, completion, acceptance)
- Free-service offer generation with `price_sats == 0`
- Markdown cross-reference link checker
