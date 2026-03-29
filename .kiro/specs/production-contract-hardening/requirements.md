# Requirements Document

## Introduction

This is a pre-stable protocol and documentation cleanup for the Froglet protocol, node, and bot-tool system. It eliminates documentation contradictions, undefined economic behavior, misleading interoperability claims, and unclear boundaries between kernel, service bindings, node/tool surfaces, and future extensions.

This work does not change the signed envelope format, hashing algorithm, signing algorithm, or existing artifact payload schemas. However, it deliberately defines and normalizes a new canonical `method = "none"` shape across offers, quotes, receipts, tests, and conformance vectors, and it includes implementation behavior changes that align code with the newly specified protocol (e.g., normalizing settlement method strings, fixing offer generation for free services, ensuring receipts are only emitted after settlement is terminal). It also adds new conformance surface (free-service conformance vectors). The design document explicitly categorizes each work item as (a) pure documentation, (b) implementation behavior change, or (c) protocol-level addition.

## Glossary

- **Kernel**: The smallest irreversible protocol surface: signed envelope, canonical serialization, hashing, signing, artifact types (Descriptor, Offer, Quote, Deal, Receipt, InvoiceBundle), cross-artifact commitments, deal/execution/settlement state machines, and the two standardized v1 settlement methods (`none` and `lightning.base_fee_plus_success_fee.v1`).
- **Service_Binding**: The interoperable layer that maps service discovery and invocation into kernel workload/deal fields. Includes `service_id`, `offer_id`, `offer_kind`, `resource_kind`, service records, and the reduction from service invocation to kernel deal flow.
- **Node_Surface**: The `/v1/froglet/*` operator control API and the bot-facing tool contract (`froglet` tool via OpenClaw/NemoClaw/MCP). Product surface, not protocol commitment.
- **Settlement_Method**: A string identifier in Offer, Quote, and Receipt artifacts that declares the economic settlement mechanism for a deal. v1 standardizes exactly two: `none` and `lightning.base_fee_plus_success_fee.v1`.
- **Invoice_Bundle**: A signed artifact (`artifact_type: invoice_bundle`) issued by the provider for Lightning-settled deals, containing base_fee and success_fee legs with BOLT11 invoices. The signed payload is immutable once created; leg states are observed externally, not mutations of the signed bytes.
- **Funds_Locked**: The point at which a Lightning-settled deal is considered funded for execution gating purposes. Defined as: base_fee invoice is `settled` AND success_fee hold invoice is `accepted` (or `settled`). Both conditions are required.
- **Conformance_Vector**: A test case in `conformance/kernel_v1.json` that provides canonical artifact bytes, hashes, signatures, and expected validation outcomes for interoperability testing.
- **Adapter**: A replaceable implementation component (transport, settlement driver, discovery bootstrap, execution material delivery, deployment) that MUST preserve kernel semantics but MAY vary operationally.
- **Higher_Layer**: Any system above the kernel and service binding layers: marketplaces, brokers, ranking, reputation, policy, catalogs, indexers. NOT part of the interoperable protocol contract.
- **Provider**: A Froglet node role that publishes resources and serves deals. Per-deal role, not a node class.
- **Requester**: A Froglet node role that discovers, quotes, and invokes remote resources. Per-deal role, not a node class.
- **Signed_Envelope**: The canonical wrapper for all kernel artifacts: `[schema_version, artifact_type, signer, created_at, payload_hash, payload]` serialized as JCS, hashed with SHA-256, signed with secp256k1.

## Requirements

### Requirement 1: Explicit Free Service Settlement Method

**User Story:** As a protocol implementer, I want `settlement_method = "none"` to be explicitly defined as a standardized v1 settlement method with fully specified serialization semantics, so that free services have unambiguous interoperable behavior.

#### Acceptance Criteria

1. THE Kernel specification SHALL define `none` as a standardized v1 settlement method for free services where no payment is required.
2. WHEN `settlement_method` is `none`, THE Provider SHALL skip invoice bundle generation, and THE Requester SHALL skip payment flow entirely.
3. WHEN `settlement_method` is `none`, THE Quote SHALL carry `settlement_terms` with `method: "none"`, `base_fee_msat: 0`, `success_fee_msat: 0`, and `destination_identity: ""` (empty string). This is a deliberate pre-stable protocol decision — the current production code falls through to the Lightning method string with zero fees, and the test fixtures use `"none.v1"`, neither of which is the intended canonical form.
4. WHEN `settlement_method` is `none`, THE Deal admission flow SHALL proceed directly to execution without waiting for payment gating.
5. WHEN `settlement_method` is `none`, THE Receipt `settlement_refs` SHALL carry the following exact shape:
   - `method: "none"`
   - `bundle_hash: null` (JSON null)
   - `destination_identity: ""` (empty string)
   - `base_fee: { amount_msat: 0, invoice_hash: "", payment_hash: "", state: "canceled" }`
   - `success_fee: { amount_msat: 0, invoice_hash: "", payment_hash: "", state: "canceled" }`
6. WHEN `settlement_method` is `none`, THE free-deal fee legs SHALL only be in state `canceled` (indicating no invoice was ever issued). The states `open`, `accepted`, `settled`, and `expired` are invalid for zero-valued legs in free deals.
7. THE Conformance_Vector suite SHALL include at least one valid free-service round-trip (Offer → Quote → Deal → Receipt) using `settlement_method: "none"` with the exact serialization shapes specified above.

### Requirement 2: Lightning Settlement Method Specification

**User Story:** As a protocol implementer, I want the Lightning settlement method to be precisely specified including the exact execution gating point, so that independent implementations can interoperate on paid deals.

#### Acceptance Criteria

1. THE Kernel specification SHALL define `lightning.base_fee_plus_success_fee.v1` as the only standardized v1 paid settlement method.
2. THE Kernel specification SHALL define the two-leg model: a base_fee invoice (standard BOLT11) and a success_fee invoice (hold invoice), both bundled in a signed `invoice_bundle` artifact.
3. WHEN a Deal uses Lightning settlement, THE Provider SHALL issue a signed `invoice_bundle` before the deal `admission_deadline`.
4. WHEN a Deal uses Lightning settlement, THE Provider SHALL gate execution start on the Funds_Locked point: base_fee invoice state is `settled` AND success_fee hold invoice state is `accepted` (or `settled`). Both conditions are required before execution begins.
5. WHEN execution succeeds and the requester accepts the result, THE Requester SHALL release the success-fee preimage (via `release_deal_preimage`) to settle the success_fee hold invoice. The provider's node then settles the hold invoice on the Lightning backend using the revealed preimage.
6. WHEN execution fails or the deal is rejected, THE Provider SHALL cancel the success_fee hold invoice.
7. THE Kernel specification SHALL define that `deal.success_payment_hash` MUST equal `invoice_bundle.success_fee.payment_hash` for Lightning-settled deals.
8. THE Kernel specification SHALL state that base_fee settlement alone (without success_fee hold acceptance) is NOT sufficient to gate execution.

### Requirement 3: Settlement Method Framework Extensibility

**User Story:** As a protocol designer, I want the settlement method framework to be explicitly extensible, so that future payment rails can be added without breaking v1 interoperability.

#### Acceptance Criteria

1. THE Kernel specification SHALL state that `settlement_method` is a string field on Offer, Quote settlement_terms, and Receipt settlement_refs.
2. THE Kernel specification SHALL state that v1 standardizes exactly two methods: `none` and `lightning.base_fee_plus_success_fee.v1`.
3. THE Kernel specification SHALL state that future settlement methods (Stripe-backed flows, B2B rails, ACH/wire/invoice, custom credit systems) are architecturally expected but MUST NOT be presented as v1 interoperable unless explicitly standardized in a future version.
4. IF a Requester encounters an unrecognized `settlement_method` in an Offer, THEN THE Requester SHALL treat the offer as unsupported rather than attempting settlement.

### Requirement 4: Invoice Bundle Immutability and Lifecycle

**User Story:** As a protocol implementer, I want invoice bundle immutability and lifecycle rules to be unambiguous, so that implementations handle bundle observation correctly.

#### Acceptance Criteria

1. THE Kernel specification SHALL state that a signed `invoice_bundle` artifact is immutable once created: the signed bytes, hash, and payload fields MUST NOT change after issuance.
2. THE Kernel specification SHALL state that the `state` field within each `InvoiceBundleLeg` in the signed payload is an issuance-time field only, set at bundle creation (typically `open`). Later observed states of the underlying Lightning invoices (`accepted`, `settled`, `canceled`, `expired`) are NOT mutations of the signed bundle payload; they are tracked externally by the provider and requester.
3. THE Kernel specification SHALL state that implementations MUST NOT re-sign, re-hash, or alter the `invoice_bundle` artifact bytes to reflect leg state changes. The signed bundle is evidence of issuance, not a live state document.
4. WHEN an `invoice_bundle` expires (bundle `expires_at` is past), THE Provider SHALL treat any externally-observed `open` legs as `expired` and SHALL NOT gate execution on expired bundles.
5. THE Kernel specification SHALL state that `invoice_bundle.expires_at` MUST NOT exceed `quote.expires_at` and MUST NOT exceed `deal.admission_deadline`.

### Requirement 5: Expiry Validation Rules

**User Story:** As a protocol implementer, I want expiry validation rules to be explicit across all artifacts including cross-artifact relationships, so that implementations reject stale artifacts consistently.

#### Acceptance Criteria

1. THE Kernel specification SHALL define that `quote.expires_at` is the deadline after which the quote MUST NOT be used to create new deals.
2. THE Kernel specification SHALL define that `deal.admission_deadline` is the deadline by which the provider MUST admit the deal (complete payment gating for paid deals, or accept for free deals).
3. THE Kernel specification SHALL define that `deal.completion_deadline` is the deadline by which execution MUST complete.
4. THE Kernel specification SHALL define that `deal.acceptance_deadline` is the deadline by which the requester MUST accept or reject the result.
5. WHEN a Provider receives a deal submission after `quote.expires_at`, THE Provider SHALL reject the deal.
6. WHEN a Lightning-settled deal has not been admitted by `deal.admission_deadline` (invoices expire or are canceled before funding), THE Provider SHALL transition the deal to `deal_state: "canceled"` (not `"failed"`, which is reserved for post-admission execution failures). For free deals, admission is immediate and this criterion does not apply.
7. THE Kernel specification SHALL define that expired Descriptors and Offers (where an `expires_at` field is present and past) are invalid for generating new Quotes. A Provider SHALL NOT issue a Quote referencing an expired Offer, and a Requester SHALL NOT request a Quote against an expired Offer.
8. THE Kernel specification SHALL define that expired transport endpoints in `Descriptor.transport_endpoints` affect reachability only, not artifact validity. A signed Descriptor with expired transport endpoints remains a valid signed artifact, but clients SHOULD NOT expect those endpoints to be reachable.
9. THE Kernel specification SHALL define that `invoice_bundle.expires_at` MUST be less than or equal to `deal.admission_deadline`, and MUST be less than or equal to `quote.expires_at`. The bundle expiry represents the window within which the requester must complete the initial payment (base-fee settlement and success-fee hold acceptance) to gate execution. It does NOT constrain the success-fee hold invoice's own expiry, which must survive through execution and requester acceptance (up to `deal.acceptance_deadline`).

### Requirement 6: Receipt Settlement State Semantics

**User Story:** As a protocol implementer, I want receipt `settlement_state` to be unambiguous for all deal outcomes, so that receipt verification is consistent and terminal receipts for paid deals never carry misleading state.

#### Acceptance Criteria

1. THE Receipt `settlement_refs` SHALL always include a `method` field matching the settlement method used for the deal.
2. WHEN the deal used Lightning settlement, THE Receipt `settlement_refs` SHALL include `bundle_hash` referencing the signed `invoice_bundle` artifact hash, `destination_identity` matching the bundle's destination, and `base_fee` and `success_fee` legs with actual invoice hashes, payment hashes, and terminal states.
3. WHEN the deal used `method: "none"`, THE Receipt `settlement_refs` SHALL carry the exact shape specified in Requirement 1 acceptance criterion 5.
4. THE Receipt `settlement_state` SHALL be one of the following values with these exact semantics. Receipts are terminal-only artifacts — a provider MUST NOT emit a signed receipt until the deal has reached a terminal state:
   - `settled`: All payment legs reached terminal settled state. Valid only for Lightning-settled deals where both legs settled.
   - `canceled`: Payment was canceled (success_fee hold invoice canceled). Valid only for Lightning-settled deals.
   - `expired`: Payment expired before completion. Valid only for Lightning-settled deals.
   - `none`: No settlement was involved because the deal used `settlement_method: "none"` (free deal). This value MUST NOT appear on terminal receipts for deals that used a paid settlement method.
5. IF a terminal Receipt is issued for a deal that used `settlement_method: "lightning.base_fee_plus_success_fee.v1"`, THEN THE Receipt `settlement_state` SHALL be one of `settled`, `canceled`, or `expired`. The value `none` is forbidden on terminal receipts for paid deals.
6. WHEN the deal used Lightning settlement and the Receipt `settlement_refs` contains exact `base_fee` and `success_fee` shapes, THE `destination_identity` field SHALL match the `invoice_bundle.destination_identity` and the `quote.settlement_terms.destination_identity`.

### Requirement 7: Contract Layer Boundary Documentation

**User Story:** As a developer or integrator, I want clear documentation of what belongs to each contract layer, so that I know which behaviors are stable protocol commitments versus product-level decisions.

#### Acceptance Criteria

1. THE documentation model SHALL define exactly four layers: Kernel, Service_Binding, Node_Surface, and Higher_Layer.
2. THE Kernel layer documentation SHALL use normative language (MUST, MUST NOT, SHOULD, MAY) for all protocol rules.
3. THE Service_Binding layer documentation SHALL define the interoperable mapping from service discovery and invocation to kernel deal fields, using normative language for interoperability-critical rules.
4. THE Node_Surface layer documentation SHALL be marked as non-normative product documentation.
5. THE Higher_Layer documentation SHALL be marked as non-normative and SHALL NOT make claims about kernel-level interoperability.
6. THE documentation SHALL explicitly list which current claims are kernel commitments versus adapter/product decisions.

### Requirement 8: Authoritative Helper Contract Documentation

**User Story:** As a developer, I want all documentation references to point to actual authoritative material, so that I can trust the docs as a reliable reference.

#### Acceptance Criteria

1. WHEN code behavior and documentation disagree, THE documentation SHALL be updated to match the code behavior, unless an explicit compatibility-impacting change is justified.
2. THE repository SHALL contain authoritative helper contract documentation that covers the kernel specification content previously referenced by other documents (invoice_bundle verification rules, linkage challenge format, kernel contract definition, settlement bindings).
3. ALL documentation cross-references (in `README.md`, `docs/ADAPTERS.md`, `docs/ARCHITECTURE.md`, `docs/NOSTR.md`, `CONTRIBUTING.md`, `AGENTS.md`, and any other files) that currently point to missing or stale specification files SHALL be updated to reference the actual authoritative material, whatever files are created to house it.
4. THE authoritative helper documentation SHALL be clearly marked with its normative status (normative for kernel rules, non-normative for architecture/adapter guidance).
5. THE documentation structure SHALL ensure that no cross-reference in the repository points to a non-existent file.

### Requirement 9: Protocol Contract Requirements (Standalone)

**User Story:** As a protocol implementer, I want protocol-level contract requirements to stand alone without being mixed with implementation cleanup tasks, so that the interoperable specification is clear regardless of any single codebase's state.

#### Acceptance Criteria

1. THE Kernel specification SHALL define exactly one canonical string per settlement method: `"none"` for free services and `"lightning.base_fee_plus_success_fee.v1"` for Lightning-settled services.
2. THE Kernel specification SHALL define that `settlement_method` on Offer artifacts MUST accurately reflect the actual settlement mechanism for the deal. An Offer for a free service (zero fees) MUST use `settlement_method: "none"`, not a paid settlement method string.
3. THE Kernel specification SHALL define the complete set of valid `settlement_state` values for Receipt artifacts and their exact semantics as specified in Requirement 6.
4. THE Kernel specification SHALL define the exact serialization shape of `settlement_refs` for each standardized settlement method as specified in Requirements 1 and 6.
5. THE Kernel specification SHALL define the Funds_Locked gating rule for Lightning settlement as specified in Requirement 2.

### Requirement 10: Implementation Cleanup Tasks

**User Story:** As a developer working on the Froglet codebase, I want implementation-specific cleanup tasks tracked separately from protocol requirements, so that codebase normalization does not block or confuse protocol specification work.

#### Acceptance Criteria

1. THE codebase SHALL normalize the free-service settlement method string to the single canonical value `"none"` across all production code, test code, and test fixtures. The variant `"none.v1"` (currently used in `tests/runtime_routes.rs`) SHALL be replaced with `"none"`.
2. THE codebase SHALL ensure that Offer generation uses `settlement_method: "none"` for free services (where `price_sats = 0`) instead of unconditionally setting `settlement_method: "lightning.base_fee_plus_success_fee.v1"` (as currently done in `src/api.rs` offer construction).
3. THE codebase SHALL ensure that the provider does NOT emit signed receipts while Lightning settlement legs are still in non-terminal states (`Open`, `Accepted`). For Lightning deals, receipt emission MUST wait until the success_fee leg reaches a terminal state (`settled`, `canceled`, or `expired`). The internal `settlement_state_from_bundle` helper may use any internal tracking values, but those values MUST NOT appear in signed receipts.
4. THE codebase SHALL ensure that `empty_receipt_leg` in `src/api.rs` uses state `canceled` (not `settled`) for zero-valued legs in free deals, consistent with the protocol specification that no invoice was ever issued.

### Requirement 11: Service Binding Interoperability

**User Story:** As a bot or integration developer, I want the service binding model to be specified enough for interoperability, so that different implementations can discover and invoke services consistently.

#### Acceptance Criteria

1. THE Service_Binding specification SHALL define the relationship between `service_id`, `offer_id`, `offer_kind`, and `resource_kind`.
2. THE Service_Binding specification SHALL define the three product shapes: named services, data services, and direct compute, and how each reduces to a kernel workload and deal.
3. THE Service_Binding specification SHALL define the service record structure returned by discovery, including required and optional fields.
4. THE Service_Binding specification SHALL define how `invoke_service` resolves a service manifest into the correct underlying workload spec and deal parameters.
5. THE Service_Binding specification SHALL NOT freeze local project layout, file layout, build pipelines, or host-specific API shapes.

### Requirement 12: Non-Kernel Exclusion Boundary

**User Story:** As a protocol implementer, I want explicit documentation of what is NOT part of the kernel, so that I do not accidentally treat product-level behavior as protocol commitments.

#### Acceptance Criteria

1. THE Kernel specification SHALL explicitly state that OpenClaw, NemoClaw, MCP, `/v1/froglet/*` routes, project authoring, marketplaces, catalogs, brokers, ranking, reputation, policy systems, deployment adapters, and payment-provider integrations are NOT part of the kernel.
2. THE Kernel specification SHALL explicitly state that execution runtime choice (Wasm, Python, container, confidential) is an execution profile concern, not a kernel commitment.
3. THE Kernel specification SHALL explicitly state that transport choice (HTTPS, Tor, Nostr relay) is an adapter concern that MUST NOT change kernel artifact semantics.
4. THE Kernel specification SHALL explicitly state that storage engine choice is an implementation concern that MUST preserve artifact immutability and evidence retention invariants.

### Requirement 13: Conformance Vector Coverage

**User Story:** As a protocol implementer, I want conformance vectors to cover all standardized settlement methods, so that I can verify my implementation against canonical test cases.

#### Acceptance Criteria

1. THE Conformance_Vector suite SHALL include valid and tampered test cases for all six artifact types: Descriptor, Offer, Quote, Deal, Receipt, and InvoiceBundle.
2. THE Conformance_Vector suite SHALL include at least one complete round-trip using `settlement_method: "none"` (free service) with the exact serialization shapes specified in Requirement 1.
3. THE Conformance_Vector suite SHALL include at least one complete round-trip using `settlement_method: "lightning.base_fee_plus_success_fee.v1"` (paid service), which already exists.
4. WHEN a new standardized settlement method is added in a future version, THE Conformance_Vector suite SHALL be extended with corresponding test cases.

### Requirement 14: Signed Envelope and Hashing Preservation

**User Story:** As a protocol implementer, I want assurance that the signed envelope, hashing, and artifact chain semantics are preserved, so that existing signed artifacts remain valid.

#### Acceptance Criteria

1. THE production-hardening work SHALL NOT change the canonical signing bytes format: `[schema_version, artifact_type, signer, created_at, payload_hash, payload]`.
2. THE production-hardening work SHALL NOT change the SHA-256 hashing algorithm used for payload hashes and artifact hashes.
3. THE production-hardening work SHALL NOT change the secp256k1 signature algorithm used for artifact signing.
4. THE production-hardening work SHALL NOT change the JCS (JSON Canonicalization Scheme) used for canonical serialization.
5. IF any compatibility-impacting change to signing, hashing, or artifact structure is proposed, THEN THE change SHALL require explicit interoperability justification and SHALL update `conformance/kernel_v1.json`.
