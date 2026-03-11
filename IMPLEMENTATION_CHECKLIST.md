# Froglet Implementation Checklist

Froglet is a bot-specific primitive for creating, discovering, buying, and operating valuable online services across a distributed network. Its job is to give AI agents a small, solid economic and cryptographic core for offers, quotes, deals, receipts, and local ledgers, plus a higher-level bot runtime that makes those primitives easy to use.

Core principles:

- Canonical economic state lives in signed Froglet artifacts and local ledgers, not external discovery networks.
- The core stays small, auditable, and stable; adapters handle discovery, transport, settlement, and runtimes.
- Settlement must be explicit and stronger than local replay protection.
- Bot-facing workflows should be simple and opinionated, but must map cleanly onto the core protocol.
- Metering is adapter-specific and must not be confused with money itself.

Status legend:

- [x] done
- [ ] pending
- [~] in progress

## Phase 1: Core Foundation

- [x] Define signed core artifacts: descriptor, offer, quote, deal, receipt
- [x] Persist artifacts, quotes, deals, and receipts in the local SQLite ledger
- [x] Expose core HTTP endpoints for descriptor, offers, feed, quotes, deals, and receipt verification
- [ ] Define normative artifact field requirements from `SPEC.md`
- [ ] Define and enforce the normative deal and settlement state machines from `SPEC.md`
- [ ] Define ledger archival, pruning, and storage invariants independent of SQLite
- [x] Replace ad hoc JSON hashing and signing bytes with RFC 8785 JCS canonicalization
- [x] Add content-addressed artifact fetch by hash
- [x] Add explicit replication cursor semantics and pagination guarantees
- [x] Define and implement a signed rejection form for deal admission or capacity failures

## Phase 2: Bot Runtime Surface

- [x] Create a bot-facing implementation plan and track it in this checklist
- [x] Add local runtime authentication for localhost control endpoints
- [x] Add basic bot-runtime provider snapshot endpoints for status/start/publish
- [ ] Separate `provider/start` lifecycle behavior from `services/publish` advertisement behavior
- [x] Add high-level service-buy workflow that wraps quote plus deal plus optional wait
- [ ] Add bot-runtime search workflow over configured discovery adapters
- [x] Add local wallet status surface for settlement drivers
- [ ] Extend the current driver-backed wallet endpoint with real balance retrieval once wallet-backed settlement exists

## Phase 3: Settlement Hardening

- [x] Introduce a settlement-driver abstraction beyond local token replay protection
- [~] Add stronger Cashu driver semantics for reservation, commit, cancel, and verification
- [x] Model first-class settlement lifecycle states for `reserved`, `committed`, `released`, and `expired`
- [x] Add support for mint allowlists and driver capability reporting
- [ ] Model claim-first semantics explicitly where supported
- [ ] Include settlement references in signed receipts
- [x] Record reserved and committed settlement amounts distinctly in receipts when they differ

## Phase 4: Runtime and Sandbox Adapters

- [ ] Add executor adapter abstraction separate from protocol nouns
- [ ] Add richer result models for adapters that expose stdout/stderr
- [x] Add per-deal wall-clock execution timeout enforcement to long-running deal execution
- [ ] Add metered offer model for adapter-specific usage metrics
- [ ] Extend metered receipts with `units_used`, `unit_price`, `max_reserved_amount`, `committed_amount`, and `meter_version`
- [ ] Add Python-in-Wasm and JavaScript-in-Wasm adapter planning and interfaces

## Phase 5: Discovery and Marketplace

- [ ] Keep the existing marketplace path working while decoupling it from core assumptions
- [ ] Add discovery-adapter abstraction in the runtime
- [ ] Add Nostr discovery adapter that publishes descriptor and offer summaries or hashes
- [ ] Define Froglet-native indexer and broker roles over artifact feeds
- [ ] Define Froglet-native catalog and reputation service roles over artifact feeds
- [ ] Add requester-side artifact persistence and export for reputation/indexing workflows

## Phase 6: Local Security and Ops

- [x] Require auth for all privileged localhost runtime endpoints
- [x] Persist local auth material with strict filesystem permissions
- [ ] Add OS-scoped IPC options or document the future interface boundary
- [ ] Expand runtime and core observability around deals, receipts, and adapter failures
- [ ] Align marketplace health and operational responses with the node's JSON API conventions

## Phase 7: Specification Convergence

- [ ] Promote `SPEC.md` from draft to a normative v0.2 protocol and localhost runtime document
- [ ] Align runtime endpoint docs with the implemented localhost API
- [ ] Align implementation docs and examples for non-compute workload hashing
- [ ] Revisit crate boundaries for `froglet-core`, `froglet-node`, and runtime adapters

## Recent Milestones

- [x] Land the first authenticated bot-runtime endpoints on top of the existing core
- [x] Add integration tests for runtime auth and `services/buy`
- [x] Move artifact and workload hashing onto RFC 8785 JCS canonicalization
- [x] Introduce the settlement-driver boundary and move the current Cashu verifier flow behind it
- [x] Route node capability and runtime wallet metadata through settlement-driver descriptors
- [x] Add artifact fetch-by-hash and explicit feed cursor semantics for replication
- [x] Emit signed terminal rejection receipts for capacity admission failures
- [x] Add explicit receipt settlement fields for reserved and committed amounts
- [x] Add Cashu mint allowlists, capability reporting, and optional NUT-07 checkstate verification
- [x] Enforce configurable wall-clock execution timeouts for Lua and Wasm workloads
- [x] Emit signed restart-recovery receipts for interrupted deals
- [x] Preserve explicit `released` and `expired` settlement states instead of deleting reservations

## Confirmed Gaps

- [ ] Wallet balance endpoint is driver-backed now, but still reports `balance_known = false` for the current Cashu verifier driver
- [ ] Cashu handling now supports mint allowlists and optional mint state checks, but still lacks mint-backed commit/cancel semantics and wallet-backed balance reporting
- [ ] Runtime discovery/search flow is not implemented yet
- [ ] Provider snapshot endpoints are present, but provider lifecycle and publish semantics are still conflated
- [ ] The implementation is still SQLite-specific; storage invariants and archival policy are not yet explicit beyond the current DB module
- [ ] Marketplace `/health` still returns plain text instead of the JSON shape used by the node API
- [ ] Restart recovery still marks interrupted jobs failed without signed terminal receipts
- [ ] Metered receipt fields described in `SPEC.md` are not implemented yet
- [ ] Catalog and reputation marketplace roles are represented in the plan, but not implemented yet

## Next Execution Order

- [~] Tighten Cashu settlement from local verifier mode to wallet or mint-backed driver semantics, including commit/cancel proofs and settlement references
- [ ] Emit signed recovery receipts for interrupted jobs or collapse the compatibility job API fully onto deals
- [ ] Separate `provider/start` lifecycle behavior from `services/publish` advertisement behavior
- [ ] Add runtime-side discovery abstraction and a first search endpoint shape
- [ ] Promote `SPEC.md` into the normative protocol and localhost runtime spec
