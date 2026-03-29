# Implementation Plan: Production Contract Hardening

## Overview

This plan implements a pre-stable protocol and documentation cleanup for Froglet. It is not just documentation hardening — it deliberately defines and normalizes a new canonical `method = "none"` shape across offers, quotes, receipts, tests, and conformance vectors, and it aligns the reference implementation with the newly specified protocol.

The work spans three categories:
- **(a) Pure documentation/specification**: writing `docs/KERNEL.md` and `docs/SERVICE_BINDING.md` as the current authoritative contract materials (temporary, may later be removed or folded elsewhere), fixing cross-references, defining the four-layer contract model
- **(b) Implementation behavior changes**: normalizing settlement method strings, fixing offer/quote generation for free services, ensuring receipts are only emitted after settlement is terminal
- **(c) Protocol-level additions**: defining the canonical `method = "none"` artifact shapes, adding free-service conformance vectors

## Tasks

- [x] 1. Documentation deliverables (Phase 4.3)
  - [x] 1.1 Create `docs/KERNEL.md` — current authoritative normative kernel specification
    - Add status line: `Status: normative kernel specification (current authoritative contract material — temporary, may later be removed or folded elsewhere)`
    - Sections: signed envelope format and verification, all six artifact type payload schemas, settlement method definitions (`none` and `lightning.base_fee_plus_success_fee.v1`), invoice bundle immutability model (signed payload leg states are issuance-time only), receipt terminal-only semantics (receipts MUST NOT be emitted until the deal has reached a terminal state; for Lightning deals this means settlement must be terminal before receipt emission), receipt settlement state values (`settled`, `canceled`, `expired`, `none` — no `pending` value exists in the protocol), expiry ordering constraints (cross-artifact ordering from design §2.2.6 — quote validity, admission gating, and success-fee payment lifetime are distinct concerns), linked identity challenge format, non-kernel exclusion boundary
    - Content source: design §2.2 (Kernel Contract), requirements 1–6, 9, 12, 14, historical SPEC.md (commit `b11880d7`)
    - Use normative language (MUST, MUST NOT, SHOULD, MAY) for all protocol rules
    - _Requirements: 7.1, 7.2, 7.6, 8.2, 8.4, 9.1, 9.2, 9.3, 9.4, 9.5, 12.1, 12.2, 12.3, 12.4_

  - [x] 1.2 Create `docs/SERVICE_BINDING.md` — current authoritative normative service binding specification
    - Add status line: `Status: normative service binding specification (current authoritative contract material — temporary, may later be removed or folded elsewhere)`
    - Sections: `service_id` / `offer_id` / `offer_kind` / `resource_kind` relationships, three product shapes (named services, data services, direct compute) and their kernel reduction, discovery record structure (required and optional fields), `invoke_service` resolution from service manifest to workload spec and deal parameters
    - Explicitly exclude: local project layout, file layout, build pipelines, host-specific API shapes
    - Use normative language for interoperability-critical rules
    - _Requirements: 7.1, 7.3, 11.1, 11.2, 11.3, 11.4, 11.5_

  - [x] 1.3 Update cross-references in six files to replace `SPEC.md` with new authoritative docs
    - `README.md`: replace `[SPEC.md](SPEC.md)` with `[docs/KERNEL.md](docs/KERNEL.md)`
    - `docs/ADAPTERS.md`: replace `[../SPEC.md](../SPEC.md)` with `[KERNEL.md](KERNEL.md)`
    - `docs/ARCHITECTURE.md`: replace `[../SPEC.md](../SPEC.md)` with `[KERNEL.md](KERNEL.md)`
    - `docs/NOSTR.md`: replace `[../SPEC.md](../SPEC.md)` with `[KERNEL.md](KERNEL.md)`
    - `CONTRIBUTING.md`: replace `SPEC.md` references with `docs/KERNEL.md`
    - `AGENTS.md`: replace `SPEC.md` references with `docs/KERNEL.md`
    - _Requirements: 8.3, 8.5_


- [x] 2. Checkpoint — Documentation review
  - Ensure all new docs are created, all cross-references resolve to existing files, and no broken `SPEC.md` links remain
  - Verify `docs/KERNEL.md` and `docs/SERVICE_BINDING.md` are clearly marked as the current authoritative contract materials
  - Ask the user if questions arise

- [-] 3. Implementation behavior changes — define and normalize canonical `method = "none"` shape and settlement alignment (Phase 4.1)
  - [-] 3.1 Define and implement canonical free-service offer shape
    - File: `src/api.rs` (~line 4618, `payload_from_provider_offer_definition`)
    - Change: when `price_sats == 0`, set `settlement_method: "none"` instead of unconditionally using `"lightning.base_fee_plus_success_fee.v1"`
    - This is a deliberate pre-stable protocol decision, not just matching existing behavior
    - _Requirements: 9.2, 10.2_

  - [x] 3.2 Define and implement canonical free-service quote settlement_terms shape
    - File: `src/api.rs` (~line 4303, `quoted_settlement_terms`)
    - Change: when `quoted_lightning_settlement_terms` returns `None` (free service or no Lightning backend), emit canonical free-service settlement terms: `method: "none"`, `base_fee_msat: 0`, `success_fee_msat: 0`, `destination_identity: ""` (empty string, consistent with receipt `settlement_refs` shape)
    - Do NOT fall through to the Lightning method string with zero fees
    - The `destination_identity: ""` choice is a deliberate canonical decision — the production fallback currently uses the provider's compressed public key, which is wrong for free services
    - _Requirements: 1.3, 10.2_

  - [x] 3.3 Normalize free-service settlement method string in test fixtures
    - File: `tests/runtime_routes.rs`
    - Replace all occurrences of `"none.v1"` with `"none"` in test fixtures
    - Update free-service receipt fixture shapes to match the new canonical `method = "none"` shape: `state: "canceled"`, `amount_msat: 0`, `invoice_hash: ""`, `payment_hash: ""`, `destination_identity: ""`
    - _Requirements: 10.1_

  - [x] 3.4 Ensure receipts are emitted only after settlement is terminal
    - File: `src/api.rs` (receipt emission paths)
    - Verify that the provider does NOT emit a signed receipt while Lightning settlement legs are still in `Open` or `Accepted` state
    - For Lightning deals: receipt emission must wait until success_fee leg reaches a terminal state (`settled`, `canceled`, or `expired`)
    - For free deals: no settlement gating needed; receipt can be emitted after execution completes
    - The internal `settlement_state_from_bundle` helper may still return non-terminal values for tracking purposes, but those values MUST NOT appear in signed receipts
    - _Requirements: 6.4, 6.5, 10.3_

  - [x] 3.5 Verify `empty_receipt_leg` uses `Canceled` state (no code change expected)
    - File: `src/api.rs` (`empty_receipt_leg` function, ~line 8068)
    - Verify production code already uses `ReceiptLegState::Canceled` for zero-valued legs
    - If test fixtures disagree, update them (covered in task 3.3)
    - _Requirements: 10.4, 1.5, 1.6_

- [x] 4. Checkpoint — Implementation behavior changes
  - Run `cargo fmt --all --check`, `CARGO_INCREMENTAL=0 RUSTFLAGS="-D warnings" cargo test --all-targets`, and `cargo clippy --all-targets -- -D warnings`
  - Ensure all tests pass. Ask the user if questions arise.

- [x] 5. Conformance vector additions (Phase 4.2)
  - [x] 5.1 Add free-service round-trip conformance vectors to `conformance/kernel_v1.json`
    - Add valid Offer with `settlement_method: "none"`, `price_schedule: { base_fee_msat: 0, success_fee_msat: 0 }`
    - Add valid Quote with `settlement_terms: { method: "none", base_fee_msat: 0, success_fee_msat: 0, destination_identity: "" }`
    - Add valid Deal referencing the free-service quote (note: `success_payment_hash` is still present in the Deal payload since it is a required field, but it is unused for free deals)
    - Add valid Receipt with `settlement_state: "none"`, `settlement_refs: { method: "none", bundle_hash: null, destination_identity: "", base_fee: { amount_msat: 0, invoice_hash: "", payment_hash: "", state: "canceled" }, success_fee: { amount_msat: 0, invoice_hash: "", payment_hash: "", state: "canceled" } }`
    - Add tampered variants for each new artifact (at minimum: tampered hash for each)
    - All vectors must use proper JCS canonical serialization, SHA-256 hashing, and secp256k1 signing
    - _Requirements: 1.7, 13.1, 13.2_

  - [x] 5.2 Verify existing InvoiceBundle conformance vector coverage
    - Check that `conformance/kernel_v1.json` already includes valid and tampered InvoiceBundle vectors
    - If missing, add valid InvoiceBundle with correct cross-artifact references and a tampered variant
    - _Requirements: 13.1_

- [ ] 6. Test updates and property-based tests (Phase 4.4)
  - [x] 6.1 Update `tests/runtime_routes.rs` free-service test fixtures
    - Ensure all free-service test fixtures use `settlement_method: "none"` (not `"none.v1"`)
    - Ensure receipt shapes in test fixtures match the new canonical `method = "none"` shape
    - This may overlap with task 3.3 — verify completeness here
    - _Requirements: 10.1, 10.4_

  - [x] 6.2 Write property test: Settlement Method String Canonicality
    - **Property 1**: For all artifacts (offers, quotes, receipts, test fixtures, conformance vectors), the `settlement_method` / `method` field is either exactly `"none"` or exactly `"lightning.base_fee_plus_success_fee.v1"`. No other strings appear.
    - Tag: `// Feature: production-contract-hardening, Property 1: Settlement Method String Canonicality`
    - **Validates: Requirements 9.1, 10.1**

  - [x] 6.3 Write property test: Free-Deal Receipt Shape Invariant
    - **Property 2**: For all receipts where `settlement_refs.method == "none"`: `bundle_hash` is null, `destination_identity` is empty string, and both fee legs have `amount_msat: 0`, empty `invoice_hash: ""`, empty `payment_hash: ""`, and `state: "canceled"`.
    - Tag: `// Feature: production-contract-hardening, Property 2: Free-Deal Receipt Shape Invariant`
    - **Validates: Requirements 1.5, 1.6, 6.3**

  - [x] 6.4 Write property test: Funds_Locked Gating Invariant
    - **Property 3**: For all Lightning-settled deals and all combinations of `base_state` and `success_state`, execution starts if and only if `base_fee.state == settled AND success_fee.state in {accepted, settled}`. In particular, `base_fee.state == settled AND success_fee.state == open` MUST NOT gate execution.
    - Tag: `// Feature: production-contract-hardening, Property 3: Funds_Locked Gating Invariant`
    - **Validates: Requirements 2.4, 2.8**

  - [x] 6.5 Write property test: Invoice Bundle Immutability
    - **Property 4**: For all signed invoice bundles, `verify_artifact(bundle)` returns true at issuance time and continues to return true regardless of externally observed leg state changes. The bundle hash and signature are stable across the bundle's lifetime.
    - Tag: `// Feature: production-contract-hardening, Property 4: Invoice Bundle Immutability`
    - **Validates: Requirements 4.1, 4.2, 4.3**

  - [x] 6.6 Write property test: Receipt Settlement State Consistency
    - **Property 5**: For all terminal receipts: if `settlement_refs.method == "lightning.base_fee_plus_success_fee.v1"`, then `settlement_state` is in `{settled, canceled, expired}` (never `"none"`), and `settlement_refs` includes non-null `bundle_hash`, non-empty `destination_identity`, and fee legs with actual invoice/payment hashes. If `settlement_refs.method == "none"`, then `settlement_state == "none"`.
    - Tag: `// Feature: production-contract-hardening, Property 5: Receipt Settlement State Consistency`
    - **Validates: Requirements 6.1, 6.2, 6.4, 6.5**

  - [ ]* 6.7 Write property test: Expiry Ordering (deal deadline chain only)
    - **Property 6**: For all deals: `deal.admission_deadline < deal.completion_deadline < deal.acceptance_deadline`. This property covers only the deal-internal deadline ordering, which is well-defined. The cross-artifact expiry relationships between `invoice_bundle.expires_at`, `deal.admission_deadline`, and `quote.expires_at` are documented in `docs/KERNEL.md` but the exact constraints for the success-fee hold invoice lifetime need further specification before they can be tested as a property.
    - Tag: `// Feature: production-contract-hardening, Property 6: Expiry Ordering`
    - **Validates: Requirements 5.1, 5.2, 5.3, 5.4**

  - [x] 6.8 Write property test: Conformance Vector Round-Trip
    - **Property 7**: For all artifacts in `conformance/kernel_v1.json`: `verify_artifact(artifact)` returns `expected_valid`. Canonical signing bytes produce the expected hash. Signature verification matches expected outcome. Existing vectors remain valid after implementation changes.
    - Tag: `// Feature: production-contract-hardening, Property 7: Conformance Vector Round-Trip`
    - **Validates: Requirements 14.1, 14.2, 14.3, 14.4**

  - [x] 6.9 Write property test: Cross-Reference Validity
    - **Property 8**: For all markdown files in the repository, every relative link to a `.md` file resolves to an existing file in the repository.
    - Tag: `// Feature: production-contract-hardening, Property 8: Cross-Reference Validity`
    - **Validates: Requirements 8.3, 8.5**

  - [x] 6.10 Write property test: Free-Deal Admission Behavior
    - **Property 9**: For all deals where `settlement_method == "none"`, no invoice bundle is generated and no payment gating occurs before execution. Free deals skip payment gating but still follow normal provider admission semantics (the provider still admits the deal based on identity, capacity, and policy — the deal does not automatically start in `accepted` status without provider admission).
    - Tag: `// Feature: production-contract-hardening, Property 9: Free-Deal Admission Behavior`
    - **Validates: Requirements 1.2, 1.4**

  - [x] 6.11 Write property test: Success Payment Hash Linkage
    - **Property 10**: For all Lightning-settled deals that have an invoice bundle, `deal.success_payment_hash == invoice_bundle.success_fee.payment_hash`.
    - Tag: `// Feature: production-contract-hardening, Property 10: Success Payment Hash Linkage`
    - **Validates: Requirements 2.7**

  - [x] 6.12 Write property test: Free-Service Offer Settlement Method
    - **Property 11**: For all offers where `price_schedule.base_fee_msat == 0` and `price_schedule.success_fee_msat == 0`, the `settlement_method` field is `"none"`.
    - Tag: `// Feature: production-contract-hardening, Property 11: Free-Service Offer Settlement Method`
    - **Validates: Requirements 9.2, 10.2**

  - [x] 6.13 Verify existing conformance vector tests pass unchanged
    - Run existing conformance vector tests to confirm no regressions from implementation changes
    - Existing Lightning round-trip vectors must still pass
    - _Requirements: 14.1, 14.2, 14.3, 14.4, 14.5_

- [-] 7. Final checkpoint — Full validation
  - Run `./scripts/strict_checks.sh` (full repo validation matrix)
  - Ensure all Rust tests, Clippy, formatting, Python tests, OpenClaw plugin checks, MCP server checks pass
  - Ensure all tests pass, ask the user if questions arise.

## Notes

- Only task 6.7 (Expiry Ordering) is marked optional (`*`) — the cross-artifact expiry model for success-fee hold invoice lifetime needs further specification before it can be fully tested as a property. The deal-internal deadline ordering is testable now.
- `docs/KERNEL.md` and `docs/SERVICE_BINDING.md` are the current authoritative contract materials. They are temporary and may later be removed or folded elsewhere, but they are the canonical reference for now.
- The `destination_identity` for free-service quotes and receipts is canonically `""` (empty string). This is a deliberate pre-stable protocol decision.
- The `settlement_state_from_bundle` internal helper may still use non-terminal values for tracking, but those values MUST NOT appear in signed receipts. The enforcement mechanism is that receipts are only emitted after settlement is terminal.
- Each task references specific requirements for traceability.
- Checkpoints ensure incremental validation using the project's established validation commands from AGENTS.md.
