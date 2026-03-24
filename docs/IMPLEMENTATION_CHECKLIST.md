# Froglet Implementation Checklist

This checklist is reset around the narrowed version 1 direction.
Prototype features outside that path are not treated as roadmap anchors.

Froglet version 1 should be a small, solid economic primitive for AI agents:
signed artifacts, Lightning-backed conditional settlement, optional Tor
transport, optional Nostr publication, and one execution/resource primitive that
can back named services, data services, and open-ended compute.

Status legend:

- [x] done
- [~] current focus
- [ ] pending

This checklist is the only freeze-status authority for the Froglet core.
It spans both spec-closure work and reference-implementation work.
Detailed planning for higher-layer discovery, broker, trust, operator, and
OpenClaw boundary work now lives under `../higher_layers/` so the core
checklist can stay focused on the kernel/runtime boundary.

## Guiding Constraints

- Keep the hard core small enough to audit.
- Keep settlement, identity, discovery, and execution cleanly separated.
- Optimize for short-lived, bounded jobs before broader market features.
- Prefer one obvious happy path for agent clients over feature breadth.
- Keep adapters optional unless they strengthen the hard core directly.

## Phase 1: Freeze the Version 1 Protocol Surface

- [x] Reset `SPEC.md` and this checklist around the narrowed v1 direction
- [x] Split the kernel contract from supporting docs: `ARCHITECTURE.md`, `ADAPTERS.md`, `RUNTIME.md`, `NOSTR.md`, and `STORAGE_PROFILE.md`
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

- [x] Specify `Descriptor` linkage proofs for optional publication identities without hardwiring settlement-adapter proof formats into the kernel
- [x] Decide and freeze the v1 Froglet application signature scheme
- [x] Specify optional Nostr identity linkage without making Nostr authoritative
- [x] Define endpoint rotation rules for HTTPS and onion endpoints
- [x] Define signed curated-list format for bootstrap discovery
- [x] Implement secp256k1 x-only node identity plus BIP340 Schnorr signing for artifacts and discovery flows

## Phase 3: Lightning Settlement Core

- [x] Finalize the Lightning-only mainline v1 payment direction
- [x] Design the Lightning settlement driver around normal invoices plus hold invoices
- [x] Define `base_fee_plus_success_fee` quote fields and validation rules
- [x] Implement a mock-backed Lightning invoice-bundle/session layer for protocol development and persistence testing
- [x] Implement requester-supplied `payment_hash` flow for the success-fee hold invoice
- [x] Validate returned invoice material against quote and deal commitments before payment
- [x] Persist invoice identifiers, destination pubkey, payment hash, expiry, CLTV data, and settlement state
- [x] Implement the requester preimage-release path and non-terminal `result_ready` state for Lightning-backed deals
- [x] Implement automatic cancel, expiry, and restart-recovery behavior
- [x] Emit Lightning settlement references in signed receipts
- [x] Define conservative maximum job duration and chunking guidance for longer work
- [x] Add adversarial tests for requester preimage withholding and provider cancel failures

## Phase 4: Current Reference Execution Profiles

- [x] Keep the current reference implementation available while the broader
      execution-profile cutover remains under review
- [x] Remove Lua from the v1 code and protocol surface
- [x] Define the current Wasm ABI and canonical workload object
- [x] Align API request/response shapes with the current reference execution profiles
- [x] Enforce memory, fuel, epoch, output-size, and wall-clock limits
- [x] Make host calls explicit, capability-scoped, and time-bounded by rejecting all public v1 host imports
- [x] Emit executor metadata in receipts
- [x] Add determinism guidance for clocks, RNG, filesystem, and network access
- [x] Add abuse tests for infinite loops, memory pressure, and blocking host calls

## Phase 5: Transport and Discovery Edges

- [x] Keep HTTPS as the baseline direct transport
- [x] Keep Tor as an optional transport with identical protocol semantics
- [x] Advertise clearnet and onion endpoints through `Descriptor` without coupling them to identity
- [x] Define Nostr publication adapter for descriptor, offer, and receipt hashes or summaries
- [x] Keep Nostr out of the deal execution and settlement critical path
- [x] Define direct-peer and signed curated-list discovery flows before broker or indexer work

## Phase 6: Bot Runtime and OpenClaw Usability

- [x] Reduce the bot-facing flow to `search -> quote -> deal -> wait -> accept/reject -> receipt`
- [x] Keep local auth mandatory for all privileged runtime endpoints
- [x] Add wallet-facing abstractions around Lightning settlement instead of raw invoice plumbing
- [x] Add agent-friendly client SDK helpers for quote, deal, and receipt flows
- [x] Ensure the happy path hides relay, transport, and invoice details unless requested
- [x] Plan for eventual full remote agent execution on top of the same deal primitive without widening v1

## Phase 7: Higher-Layer Boundary

- [x] Keep discovery and commercial layers out of the core trust model
- [x] Treat signed curated lists as the first bootstrap discovery primitive
- [x] Move detailed higher-layer and addon planning under `higher_layers/`

## Immediate Next Steps

1. [x] Retire inline paid query and job paths in favor of explicit Lightning-era protocol deals.
2. [x] Validate returned invoice material against quote and deal commitments before payment instead of treating the mock bundle as already trusted.
3. [x] Add an LND REST adapter boundary and settlement-destination resolution without weakening the current mock-backed safety model.
4. [x] Implement the requester preimage-release path needed to complete real hold-invoice settlement without faking finality.
5. [x] Add active Lightning settlement watchers so `payment_pending` and `result_ready` deals recover and progress without relying on request-driven sync points.
6. [x] Add synthetic LND REST settlement integration coverage with real BOLT11 invoices so the `lnd_rest` driver is exercised independently of the older mock bundle path.
7. [x] Finish the remaining Wasm hardening gaps: host-call time bounds, stronger memory-pressure tests, and explicit determinism guidance.
8. [x] Add LND-backed regtest integration coverage for hold-invoice create, accept, settle, cancel, and restart recovery.
9. [x] Tighten local wallet-facing abstractions so the deal engine does not depend on raw invoice-plumbing details outside the settlement driver.
10. [x] Define conservative maximum job duration and chunking guidance for longer work.
11. [x] Add adversarial tests for requester preimage withholding and provider cancel failures.
12. [x] Exercise Tor as an optional transport with descriptor parity and protocol-surface checks, behind an env-gated integration path.
13. [x] Define and implement the first Nostr publication adapter surface without making Nostr authoritative for execution or settlement.
14. [x] Keep relay publishing outside the core node, add a distinct linked Nostr publication key, and sign local Nostr summary events with that linked key instead of the Froglet node key.
15. [x] Build the external Nostr relay publisher/consumer adapter around the local publication surfaces, keeping relay policy out of the core node.
16. [x] Add relay-auth, retry/backoff, and relay-list policy support to the external Nostr adapter without pulling those concerns back into the node.

## Clear Roadmap

### Milestone 1: Freeze the Hard Version 1 Core

- [~] Tighten `SPEC.md` until the hard core is reimplementable from docs alone
- [x] Pin exact artifact signing bytes, artifact hashing, and Nostr linkage challenge bytes unambiguously
- [x] Shrink the kernel by freezing direct quote, deal, receipt, and settlement commitments instead of extra hashed helper objects
- [x] Cleanly separate canonical protocol states from local runtime-only states such as `payment_pending` and `result_ready`
- [x] Migrate the Rust implementation, verifier logic, and persistence assumptions to the frozen kernel artifact shapes
- [x] Add protocol test vectors and conformance-style verification cases for artifact, linkage, and invoice-bundle validation

Exit criteria:
- The economic core is precise enough that another implementation could verify and reproduce hashes, signatures, and terminal receipts without reading this codebase, and the reference implementation uses those same frozen kernel shapes

### Milestone 2: Stabilize Froglet as a Bot Runtime

- [x] Turn the current runtime into a clearly documented bot-facing alpha surface
- [x] Add operator documentation for wallet setup, auth token handling, archive export, and recovery flows
- [x] Add one or two end-to-end example bot integrations using the Python client helpers
- [x] Decide which runtime endpoints and helper shapes are part of the supported v1 bot product surface
- [x] Plan the future full remote-agent execution layer without widening the hard v1 primitive

Exit criteria:
- A bot developer can discover, quote, buy, wait, accept, and verify receipts through the runtime without needing to understand transport or settlement internals first

### Higher-Layer Follow-On Work

Discovery services, broker/reputation layers, ownership/issuer products, and
other post-v1 additions are intentionally tracked outside the core freeze
checklist under `../higher_layers/`.
