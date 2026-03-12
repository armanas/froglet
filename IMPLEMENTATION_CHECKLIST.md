# Froglet Implementation Checklist

This checklist is reset around the narrowed version 1 direction.
Prototype features outside that path are not treated as roadmap anchors.

Froglet version 1 should be a small, solid economic primitive for AI agents: signed artifacts, Lightning-backed conditional settlement, a Wasm-first public execution surface, optional Tor transport, and optional Nostr publication.

Status legend:

- [x] done
- [~] current focus
- [ ] pending

## Guiding Constraints

- Keep the hard core small enough to audit.
- Keep settlement, identity, discovery, and execution cleanly separated.
- Optimize for short-lived, bounded jobs before broader market features.
- Prefer one obvious happy path for agent clients over feature breadth.
- Keep adapters optional unless they strengthen the hard core directly.

## Phase 1: Freeze the Version 1 Protocol Surface

- [x] Reset `SPEC.md` and this checklist around the narrowed v1 direction
- [x] Freeze the normative artifact fields for `Descriptor`, `Offer`, `Quote`, `Deal`, and `Receipt`
- [x] Freeze RFC 8785 JCS plus domain-separated signing payload rules
- [x] Freeze hash-chaining rules for quote, deal, and receipt verification
- [x] Define separate deal, execution, and settlement state machines normatively
- [x] Define receipt requirements for `result_hash`, executor metadata, and settlement references
- [x] Define signed rejection artifacts for admission or capacity failures
- [x] Define storage and archival invariants independent of SQLite

## Phase 1.5: Reference Storage Profile

- [x] Define the reference SQLite split between artifact documents, feed ordering, and execution evidence
- [x] Keep `jobs`, `quotes`, and `deals` as derived query state rather than the only retained evidence
- [x] Persist workload/result/failure accountability material in a dedicated execution-evidence store
- [x] Preserve local feed order independently from artifact document storage
- [x] Reconstruct quote/deal/job views directly from retained artifact/evidence references instead of convenience JSON copies
- [x] Add engine-neutral export/archive surface over retained artifacts, feed order, and execution evidence

## Phase 2: Identity and Trust Bindings

- [x] Specify `Descriptor` linkage proofs between Froglet application identity and Lightning settlement identity
- [x] Decide and freeze the v1 Froglet application signature scheme
- [x] Specify optional Nostr identity linkage without making Nostr authoritative
- [x] Define endpoint rotation rules for HTTPS and onion endpoints
- [x] Define signed curated-list format for bootstrap discovery
- [x] Implement secp256k1 x-only node identity plus BIP340 Schnorr signing for artifacts and marketplace flows

## Phase 3: Lightning Settlement Core

- [x] Remove Cashu from the mainline v1 design direction
- [~] Design the Lightning settlement driver around normal invoices plus hold invoices
- [x] Define `base_fee_plus_success_fee` quote fields and validation rules
- [x] Implement a mock-backed Lightning invoice-bundle/session layer for protocol development and persistence testing
- [x] Implement requester-supplied `payment_hash` flow for the success-fee hold invoice
- [ ] Validate returned invoice material against quote and deal commitments before payment
- [x] Persist invoice identifiers, destination pubkey, payment hash, expiry, CLTV data, and settlement state
- [~] Implement automatic cancel, expiry, and restart-recovery behavior
- [x] Emit Lightning settlement references in signed receipts
- [ ] Define conservative maximum job duration and chunking guidance for longer work
- [ ] Add adversarial tests for requester preimage withholding and provider cancel failures

## Phase 4: Wasm-First Execution Core

- [x] Collapse the public remote execution surface to Wasm only
- [x] Remove Lua from the v1 code and protocol surface
- [x] Define the v1 Wasm ABI and canonical workload object
- [x] Align API request/response shapes with `compute.wasm.v1` and `wasm_submission`
- [~] Enforce memory, fuel, epoch, output-size, and wall-clock limits
- [ ] Make host calls explicit, capability-scoped, and time-bounded
- [x] Emit executor metadata in receipts
- [ ] Add determinism guidance for clocks, RNG, filesystem, and network access
- [ ] Add abuse tests for infinite loops, memory pressure, and blocking host calls

## Phase 5: Transport and Discovery Edges

- [ ] Keep HTTPS as the baseline direct transport
- [ ] Keep Tor as an optional transport with identical protocol semantics
- [ ] Advertise clearnet and onion endpoints through `Descriptor` without coupling them to identity
- [ ] Define Nostr publication adapter for descriptor, offer, and receipt hashes or summaries
- [ ] Keep Nostr out of the deal execution and settlement critical path
- [ ] Define direct-peer and signed curated-list discovery flows before broker or indexer work

## Phase 6: Bot Runtime and OpenClaw Usability

- [ ] Reduce the bot-facing flow to `search -> quote -> deal -> wait -> accept/reject -> receipt`
- [ ] Keep local auth mandatory for all privileged runtime endpoints
- [ ] Add wallet-facing abstractions around Lightning settlement instead of raw invoice plumbing
- [ ] Add agent-friendly client SDK helpers for quote, deal, and receipt flows
- [ ] Ensure the happy path hides relay, transport, and invoice details unless requested
- [ ] Plan for eventual full remote agent execution on top of the same deal primitive without widening v1

## Phase 7: Marketplace on Froglet

- [ ] Keep the marketplace out of the core trust model
- [ ] Define indexer role over artifact feeds
- [ ] Define broker role over quote aggregation and routing
- [ ] Define catalog and reputation roles as separate Froglet services
- [ ] Treat signed curated lists as the first bootstrap marketplace primitive
- [ ] Delay open adversarial marketplace mechanics until the deal primitive is proven

## Immediate Next Steps

1. [x] Replace the remaining Cashu-only paid query and job paths with explicit Lightning-era demotion from the v1 primitive.
2. [x] Validate returned invoice material against quote and deal commitments before payment instead of treating the mock bundle as already trusted.
3. Finish the remaining Wasm hardening gaps: host-call time bounds, stronger memory-pressure tests, and explicit determinism guidance.
