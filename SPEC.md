# Froglet Bot Runtime and Economic Core Specification

Status: draft

## 1. Goal

Froglet is a bot-specific tool for creating, buying, and operating online services.

It is built as a two-layer system:

- Froglet Core: a small economic and cryptographic primitive for signed offers, quotes, deals, receipts, and a local logical append-only ledger.
- Froglet Bot Runtime: a higher-level sidecar that bots use through a localhost API for discovery, wallet access, service publishing, service buying, and execution.

OpenClaw can be one client of this system, but the system is not OpenClaw-specific.

## 2. Design Principles

- Canonical economic state lives in signed Froglet artifacts and local ledgers.
- Discovery networks are adapters, not the source of truth.
- Settlement is pluggable and must be stronger than local replay protection.
- Metering is adapter-specific and should never be confused with money itself.
- Canonical serialization and hashing must be deterministic and versioned.
- External adapter identities must be explicitly anchored to the Froglet core identity.
- The bot-facing runtime should be highly opinionated and easy to use.
- The bot runtime is a convenience and control layer over the core, not a second state model.
- The core should stay small, auditable, and stable.

## 3. Layered Architecture

### Layer A: Froglet Core

The core protocol is defined around five signed artifact types:

- Descriptor: who the provider is, how it can be reached, what protocol versions and adapters it supports.
- Offer: what resource is being sold, under what constraints, with what settlement methods.
- Quote: a short-lived signed commitment to price and terms for a specific workload shape.
- Deal: an accepted quote plus workload hash, payment lock or reservation reference, and deadline.
- Receipt: the terminal signed result of the deal, including success or failure information and optional metering details.

The core also maintains:

- a local logical append-only ledger of signed artifacts
- a content-addressed artifact retrieval surface
- a pull-based feed or replication surface for other Froglet services
- deal and settlement state machines
- a verification surface for signed receipts

The core must support both compute and data-like services.

### 3.1 Deal and settlement state machines

Core implementations must treat deal lifecycle and settlement lifecycle as separate concerns.

Canonical deal states for version 1 are:

- `accepted`
- `running`
- `rejected`
- `succeeded`
- `failed`

Terminal deal states are:

- `rejected`
- `succeeded`
- `failed`

The only valid deal transitions are:

- `accepted -> running`
- `accepted -> rejected`
- `accepted -> failed`
- `running -> succeeded`
- `running -> failed`

Settlement state is adapter-specific, but version 1 implementations should map local settlement progression onto the following states when applicable:

- `none`
- `reserved`
- `committed`
- `released`
- `expired`

A terminal receipt must reflect the final deal outcome and, when settlement was involved, enough settlement information to determine the final settlement outcome independently of the deal state alone.

### Layer B: Froglet Bot Runtime

Bots do not talk directly in low-level protocol terms unless they want to.

The bot runtime is a localhost sidecar that:

- manages wallet access
- manages discovery adapters
- manages transport adapters
- exposes simple buy and sell workflows
- translates high-level bot requests into quote, deal, and receipt operations on the core

This sidecar is the primary product surface for bot developers.

## 4. Protocol Invariants

The following invariants should hold across all implementations:

- Every economic artifact is signed by a stable Froglet core identity.
- Adapter-specific identities such as Nostr keys or transport endpoints are subordinate to and bound by the core descriptor.
- Canonical serialization, hashing, and signature rules are part of the protocol version and must not be implementation-defined.
- Quotes are short-lived and immutable once issued.
- A deal references exactly one accepted quote.
- Deal lifecycle and settlement lifecycle must be modeled separately.
- Only the defined deal transitions are valid.
- A receipt is terminal and immutable. It must represent either success or failure, never an in-between state.
- Artifacts should be content-addressed and retrievable by hash.
- Workloads, results, and settlement references should be hash-addressed whenever possible.
- The bot runtime may cache, batch, or automate flows, but it must not invent economic state that is not representable in the core.

For version 1, signed JSON artifacts should use RFC 8785 JCS as the canonical pre-hash serialization format.
If a future protocol version adopts a binary codec, that change should be explicit and versioned.

## 5. Canonical State Model

Canonical state is not Nostr, not Tor, and not any external gossip system.

Canonical state is:

- the provider's signed artifacts
- the provider's local ledger for artifacts emitted by that provider
- independently verifiable receipts and deal chains

External networks may carry summaries, pointers, or mirrors of that state, but they are not authoritative.

This does not imply a single global database. It means that the authoritative record for a provider's economic actions is the signed artifact set that provider emitted, not what a relay, broker, or crawler happened to observe.

Replication should happen by pulling Froglet artifact feeds and resolving artifact hashes, not by trusting lossy discovery summaries.

Requesters should also persist and replicate the quote, deal, and receipt artifacts relevant to their own transactions.
Provider disappearance should not erase the requester's cryptographic proof of the interaction.

Append-only is a logical property of the ledger, not a requirement to keep one ever-growing database file forever.
Implementations may archive, snapshot, compact, or prune terminal records after settlement finality, as long as hashes, signatures, and required audit chains remain reproducible and independently verifiable.
Open deals, unsettled payment records, and locally required accountability evidence must not be pruned away.

The protocol does not mandate SQLite or any specific storage engine.
Implementations must still provide atomic artifact persistence, durable deal and receipt updates, content-addressed retrieval, and restart recovery semantics equivalent to a local ledger.

This is required for:

- deterministic auditability
- stable economic verification
- private deal execution
- replayable marketplace indexing

## 6. Discovery Adapters

Froglet should support multiple discovery adapters.

### 6.1 Froglet-native discovery

Marketplace and indexing services can be built on Froglet itself by consuming provider descriptors, offers, and receipts.

### 6.2 Nostr discovery

Nostr is useful as a decentralized discovery and announcement layer.

It may be used to publish:

- descriptor summaries
- offer summaries
- descriptor hashes or artifact hashes
- transport endpoints or endpoint hashes
- settlement hints such as accepted mints
- reputation references

Nostr must not be treated as the canonical economic state layer.

A Nostr adapter should publish compact summaries or hashes that point back to Froglet artifacts.

If a separate Nostr identity is used, the Froglet descriptor must explicitly bind that Nostr identity to the Froglet core identity.

### 6.3 Direct or curated discovery

Bots may also rely on:

- local allowlists
- curated catalogs
- private brokers
- direct onion or clearnet endpoints

## 7. Settlement Model

Settlement must be modeled explicitly.

The core should expose a pluggable settlement interface with operations equivalent to:

- prepare or reserve
- verify reservation or proof
- commit
- cancel or expire
- verify receipt or proof

Settlement progression is not inferred from deal status alone.
A deal may fail after funds were reserved, and version 1 implementations should preserve enough state to distinguish `released` or `expired` settlement outcomes from deals that never reserved funds at all.

Cashu is one settlement driver.

### 7.1 Cashu driver requirements

A production Cashu driver should support:

- mint whitelisting
- real mint interaction or wallet interaction
- spend verification stronger than local replay protection
- provider-side proof of settlement
- trust-minimized reservation flows when supported by the Cashu feature set, including spend conditions or pubkey-gated proofs such as NUT-10 and NUT-11

Claim-first execution may be supported by the Cashu driver, but it is only one settlement policy.
If claim-first is used, the driver must specify the exact refund, failure, and receipt semantics rather than presenting the flow as trustless fair exchange.

The protocol must also support:

- max-price reservation followed by actual commit
- fixed-price execution
- policy-specific failure semantics
- receipts that record both reserved and committed amounts when those differ
- machine-verifiable settlement references when the driver supports them

### 7.2 Quote admission and capacity

Quotes guarantee price and terms.
They do not guarantee that provider capacity will still be available when a requester attempts to open a deal.

Providers must be allowed to reject admission safely if local CPU, memory, concurrency, or policy limits were consumed between quote issuance and deal opening.

These rejections should be machine-readable and auditable.
They may be represented as a signed terminal rejection receipt or another signed rejection form that references the quote hash and rejection reason.

Capacity policy should be explicit in offers when practical.
Bots must not assume that requesting many quotes reserves scarce resources.

## 8. Pricing and Metering

Money is not fuel.

The protocol should keep settlement amounts in sats or settlement-native units.

Metering is optional and adapter-specific.

Offers should support at least:

- pricing_model = fixed
- pricing_model = metered

Metered offers should declare:

- meter_type
- unit price
- max charge or reservation requirements
- execution policy on exhaustion or timeout
- how usage is reported in terminal receipts

Examples:

- wasm_fuel.v1
- network_bytes.v1
- rows_returned.v1
- runtime_seconds.v1

Wasm fuel is a good metering input for Wasm execution.
It is not a universal economic unit for all Froglet services.

Metered terminal receipts should include, when applicable:

- units_used
- unit_price
- max_reserved_amount
- committed_amount
- meter_version

## 9. Transport Adapters

Froglet should support multiple transport adapters.

### 9.1 Tor-first

Tor is a strong default for providers who want zero-config ingress and location privacy.

### 9.2 Clearnet

Clearnet remains important for:

- low latency deployments
- data center nodes
- easier debugging and enterprise use

Transports belong in descriptors and offers, not in the canonical state model itself.

Transport endpoints may rotate without rotating the Froglet core identity, as long as the current descriptor binds the active endpoints.

## 10. Executor Adapters

Execution environments are adapters, not protocol nouns.

Useful adapters include:

- raw Wasm
- Python via preloaded Wasm interpreter
- JavaScript via preloaded Wasm interpreter
- Lua
- data or query services

Each executor adapter should publish its own:

- runtime name
- capability set
- safety constraints
- pricing model
- metering support
- result model such as stdout or structured JSON when relevant

Regardless of resource type, `workload_hash` is always the hash of a canonical request object under the protocol's canonical serialization rules.

Examples of data-like workload objects include:

- SQL-style query service:
  `{"workload_type":"sql.query.v1","dialect":"sqlite","query":"SELECT * FROM listings WHERE tag = ?","params":["scrape"],"snapshot":"dataset-epoch-123"}`
- content-addressed retrieval service:
  `{"workload_type":"cid.fetch.v1","cid":"bafy...","range":null}`
- HTTP extraction service:
  `{"workload_type":"http.extract.v1","url":"https://example.com","method":"GET","selector":"article"}`

These are hashed the same way as compute workloads.

## 11. Sandbox Requirements

Sandbox policy must be explicit and default-deny.

Minimum expectations:

- memory caps
- concurrency caps
- filesystem denial by default
- network denial by default
- per-execution wall-clock timeout
- compute caps such as Wasm fuel or Lua instruction counts
- isolated stdout and stderr capture where relevant

Optional capabilities such as outbound HTTP should be exposed as priced offer variants, not hidden side effects.

Timeouts, meter exhaustion, and policy denials should surface as machine-readable terminal failure reasons in receipts.

## 12. Bot-Facing Local API

The localhost sidecar API should be bot-oriented.

Illustrative endpoints:

- GET /wallet/balance
- POST /provider/start
- POST /services/publish
- POST /services/search
- POST /services/buy
- GET /deals/:id
- POST /receipts/verify

These endpoints are convenience surfaces.
They compile down to core operations around descriptors, offers, quotes, deals, and receipts.

The localhost API must not be treated as implicitly trusted just because it binds to localhost.
The runtime should require a local auth secret, macaroon, or equivalent capability token for all privileged requests, with strict filesystem permissions on the credential material.

Where possible, the runtime should also support OS-scoped IPC mechanisms such as Unix domain sockets or equivalent local transports with stronger process-level isolation.

The local API should support both:

- async workflows that return a `deal_id`
- convenience workflows that wait for a terminal receipt when the caller prefers synchronous behavior

## 13. Marketplace on Froglet

The long-term marketplace should itself be composed of Froglet services.

Examples include:

- indexers that crawl descriptors, offers, and receipts
- brokers that request quotes and route deals
- catalog services that curate subsets of the network
- reputation services that score providers from receipt history

These services consume the same core artifacts as any other Froglet participant.
They are not privileged protocol actors.

Froglet proves who signed what, not whether a signed claim is factually true.
Fraud disputes, arbitration, slashing, and reputation interpretation are off-core marketplace concerns.

## 14. Example Service Lifecycle

1. A provider bot starts the Froglet runtime.
2. The runtime opens transport adapters and publishes provider offers.
3. Discovery adapters announce offer summaries.
4. A consumer bot searches for a matching service.
5. The runtime requests a quote.
6. The runtime opens a deal with an explicit settlement flow.
7. The provider executes the workload or serves the data.
8. The provider emits a signed receipt.
9. Brokers, indexers, and marketplaces consume receipts and artifacts to build higher-level services.

## 15. What Belongs in Core

Core should contain:

- artifact schemas
- signing and verification
- ledger persistence
- deal state machine
- settlement interfaces
- receipt verification
- storage invariants, not a mandatory database choice

## 16. What Does Not Belong in Core

Core should not hardwire:

- Nostr as the source of truth
- Tor as the only transport
- Cashu as the only settlement method
- Wasm fuel as the universal pricing unit
- Python or JavaScript runtimes as mandatory protocol features
- a single global marketplace design
- a mandatory storage engine

## 17. Product Direction

The intended product is:

- a small, stable Froglet core
- a highly ergonomic bot runtime built on top of it
- multiple discovery, settlement, runtime, and marketplace adapters

This allows Froglet to remain a real primitive while still feeling like a bot-specific tool.
