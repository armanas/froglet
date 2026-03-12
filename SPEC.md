# Froglet Economic Core and Bot Runtime Specification

Status: draft v0.2 direction

## 1. Goal

Froglet is a small economic primitive for AI agents to exchange valuable compute and data across a distributed network.

Version 1 is intentionally narrow. It optimizes for short-lived, bounded, fixed-price deals backed by Lightning settlement, a very small signed artifact model, and a Wasm-first public execution surface. Optional discovery and transport adapters may exist, but the core must remain small, auditable, and stable.

OpenClaw can be one client of this system, but Froglet is not OpenClaw-specific.

## 2. Design Principles

- One hard core, many optional edges.
- Canonical economic state lives in signed Froglet artifacts and local ledgers.
- Identity, discovery, transport, execution, and settlement are distinct concerns.
- Version 1 optimizes for safety, auditability, and restart recovery over flexibility.
- Settlement must be stronger than local replay protection and must be modeled explicitly.
- The public network execution surface should be as small as possible.
- Discovery networks are adapters, not the source of truth.
- The bot runtime may automate flows, but it must not invent economic state that is not representable in the core.
- Froglet does not claim perfect fair exchange for arbitrary off-chain compute or data delivery. It provides signed deal semantics plus conditional settlement.

## 3. Layered Architecture

### Layer A: Froglet Core

The core protocol is defined around five signed artifact types:

- Descriptor: who the provider is, how it can be reached, which protocol version it speaks, and which subordinate identities and adapters it binds.
- Offer: what resource is being sold, under what constraints, and with which settlement and transport options.
- Quote: a short-lived signed commitment to price and terms for a specific workload shape.
- Deal: an accepted quote plus workload hash, settlement lock reference, deadlines, and requester acceptance parameters.
- Receipt: the terminal signed result of the deal, including success or failure, result hashes, execution metadata, and settlement references.

The core also maintains:

- a local logical append-only ledger of emitted and accepted artifacts
- content-addressed artifact retrieval by hash
- feed and replication surfaces for other Froglet services
- explicit deal, execution, and settlement state machines
- receipt verification independent of any external marketplace

The public remote execution target for version 1 is raw Wasm only.
Other local or experimental adapters may exist, but they are not part of the public v1 economic promise.

### Layer B: Froglet Bot Runtime

Bots do not need to speak low-level protocol terms unless they want to.

The bot runtime is a localhost sidecar that:

- manages wallet access
- manages discovery adapters
- manages transport adapters
- exposes simple buy and sell workflows
- translates high-level bot requests into quote, deal, and receipt operations on the core

The bot runtime is the primary product surface for agent developers.

## 4. Identity Model

Every economic artifact is signed by a stable Froglet application identity.

That application identity is not the same thing as:

- a Lightning node identity
- a Nostr identity
- a TLS certificate
- a Tor onion address

Version 1 requires explicit binding, in `Descriptor`, between the Froglet application identity and:

- one or more settlement identities, such as a Lightning node public key or invoice destination identity
- zero or more Nostr publication identities
- one or more transport endpoints, such as HTTPS URLs or onion endpoints

The purpose of this separation is stability.
A provider must be able to rotate transport infrastructure or settlement plumbing without changing the meaning of previously signed Froglet artifacts, as long as the current descriptor publishes the new linkage.

Version 1 freezes the Froglet application identity to `secp256k1` x-only public keys with BIP340 Schnorr signatures.

The Froglet application identity encoding rules are:

- `provider_id`, `requester_id`, and `signing.signer` are 32-byte x-only public keys encoded as lowercase hex
- `signing.algorithm` for Froglet-signed artifacts is always `secp256k1_schnorr_bip340`
- Froglet Schnorr signatures are 64-byte lowercase hex values

What is mandatory in version 1 is:

- deterministic canonical serialization before hashing and signing
- explicit domain separation for every signed artifact type
- explicit linkage proofs between Froglet identity and subordinate identities

For Froglet-signed artifacts, the exact signing procedure is:

1. Canonicalize the artifact payload with RFC 8785 JCS.
2. Compute `payload_hash = SHA256(JCS(payload))`.
3. Build the signing message bytes as:

```text
<schema_version>
<domain>
<payload_hash>
```

4. Compute `message_digest = SHA256(signing_message_bytes)`.
5. Sign `message_digest` with BIP340 Schnorr using the Froglet application key.

Verification uses the same digest derivation and BIP340 verification.

The reason for freezing this scheme in version 1 is straightforward:

- native alignment with Bitcoin and Lightning key material
- straightforward optional interop with Nostr publication identities
- one compact, well-understood application signature format across all core artifacts

### 4.1 Linked identity proof format

For identities that can sign messages, version 1 uses an explicit linkage object inside `Descriptor`.

`linked_identities[]` entries must contain:

- `identity_kind`: `lightning_node` or `nostr`
- `identity`: serialized public identity string for that ecosystem
- `scope`: array of strings describing allowed use, such as `settlement.receive` or `publication.nostr`
- `created_at`: unix timestamp in seconds
- `expires_at`: unix timestamp in seconds or `null`
- `signature_algorithm`: algorithm identifier understood for that linked identity
- `linked_signature`: signature by the linked identity over the linkage challenge

The linkage challenge bytes are:

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

- `provider_id` is the Froglet application identity published by the descriptor
- `scope_hash` is the lowercase hex SHA-256 of `JCS(scope)`
- `expires_at_or_dash` is the decimal expiry timestamp or `-`

The enclosing descriptor signature is the Froglet-side authorization for the binding.
The `linked_signature` is the subordinate identity's acknowledgement of that binding.

Identity-specific encoding rules are:

- for `identity_kind = nostr`, `identity` must be the 32-byte x-only public key encoded as lowercase hex
- for `identity_kind = lightning_node`, `identity` must be the 33-byte compressed secp256k1 public key encoded as lowercase hex

For `identity_kind = nostr`, `signature_algorithm` should be `secp256k1_schnorr_bip340` unless a future protocol version explicitly allows another scheme.
For `identity_kind = lightning_node`, `signature_algorithm` is provider-implementation-specific for version 1 and must be named explicitly in the linkage object.

### 4.2 Transport endpoint binding format

Transport endpoints are bound by the enclosing descriptor signature.
They do not carry their own signatures.

`transport_endpoints[]` entries must contain:

- `transport`: `https` or `tor`
- `uri`: canonical endpoint URI
- `created_at`: unix timestamp in seconds
- `expires_at`: unix timestamp in seconds or `null`
- `priority`: lower values are preferred first
- `features`: array of transport feature strings, such as `quote_http`, `artifact_fetch`, or `receipt_poll`

## 5. Artifact, Hashing, and Ledger Rules

Version 1 uses JSON artifacts canonically serialized with RFC 8785 JCS before hashing and signing.

Every signed payload must be domain-separated, for example with stable prefixes such as:

- `froglet:descriptor:v1`
- `froglet:offer:v1`
- `froglet:quote:v1`
- `froglet:deal:v1`
- `froglet:receipt:v1`

Artifacts must be hash-chained:

- `Quote` commits to `OfferHash` and `RequestHash`.
- `Deal` commits to `QuoteHash`, `WorkloadHash`, and the settlement lock reference.
- `Receipt` commits to `DealHash`, `ResultHash`, and settlement proof references.

### 5.1 Signed artifact envelope

All five core artifacts use the same signed envelope:

- `artifact_type`: `descriptor`, `offer`, `quote`, `deal`, or `receipt`
- `schema_version`: version string, initially `froglet/v1`
- `payload`: canonical JSON payload for that artifact type
- `payload_hash`: lowercase hex SHA-256 of `JCS(payload)`
- `signing`: object with:
  - `domain`: artifact domain string, such as `froglet:quote:v1`
  - `algorithm`: must be `secp256k1_schnorr_bip340` in version 1
  - `signer`: Froglet application public identity of the signer
  - `signature`: signature over the signing message

The signing message is the UTF-8 bytes of:

```text
<schema_version>
<domain>
<payload_hash>
```

Validation order is:

1. Canonicalize `payload` with RFC 8785 JCS.
2. Recompute `payload_hash`.
3. Verify that `schema_version` is understood and that `signing.domain` matches `artifact_type`.
4. Compute `message_digest = SHA256(signing_message_bytes)`.
5. Verify `signing.signature` as a BIP340 Schnorr signature over `message_digest`.
6. Compute `artifact_hash = SHA256(JCS(envelope_without_artifact_hash_field))`.

`artifact_hash` is the content address.
It should not be stored as a mutable in-band field that could create hashing ambiguity.

### 5.2 Descriptor payload

`Descriptor.payload` must contain:

- `provider_id`: Froglet application identity of the provider; must equal `signing.signer`
- `descriptor_seq`: monotonically increasing unsigned integer per provider
- `created_at`: unix timestamp in seconds
- `expires_at`: unix timestamp in seconds or `null`
- `protocol_version`: currently `froglet/v1`
- `linked_identities`: array of linkage objects from section 4.1
- `transport_endpoints`: array of transport endpoint bindings from section 4.2
- `capabilities`: object with:
  - `service_kinds`: array such as `compute.wasm.v1` or bounded data-service kinds
  - `execution_runtimes`: array; version 1 public remote execution should only advertise `wasm`
  - `max_concurrent_deals`: integer or `null`
- `feed_endpoints`: object with:
  - `feed_url`: artifact feed URL
  - `artifact_base_url`: base URL for artifact fetch by hash

`Descriptor.payload` may additionally contain:

- `display_name`: optional human label
- `metadata`: optional display-only map

### 5.3 Offer payload

`Offer.payload` must contain:

- `provider_id`: Froglet application identity of the provider; must equal `signing.signer`
- `descriptor_hash`: content hash of the descriptor the offer is published under
- `created_at`: unix timestamp in seconds
- `expires_at`: unix timestamp in seconds or `null`
- `offer_kind`: service kind identifier, such as `compute.wasm.v1`
- `pricing_model`: must be `fixed` in version 1
- `settlement_method`: must be `lightning.base_fee_plus_success_fee.v1` in version 1
- `request_schema_hash`: hash of the canonical request schema or request-shape definition
- `quote_ttl_secs`: maximum quote lifetime the provider will issue from this offer
- `execution_profile`: object with:
  - `runtime`: must be `wasm` for public version 1 compute offers
  - `abi_version`: must be `froglet.wasm.run_json.v1` for public version 1 compute offers
  - `capabilities`: array of capability strings; must be empty for `froglet.wasm.run_json.v1`
  - `max_input_bytes`: upper bound provider is willing to quote
  - `max_runtime_ms`: upper bound provider is willing to quote
  - `max_memory_bytes`: upper bound provider is willing to quote
  - `max_output_bytes`: upper bound provider is willing to quote
- `price_schedule`: object with:
  - `base_fee_msat`
  - `success_fee_msat`

`Offer.payload` may additionally contain:

- `result_schema_hash`: hash of an advertised result schema
- `terms_hash`: hash of additional policy text or machine-readable policy

### 5.4 Quote payload

`Quote.payload` must contain:

- `provider_id`: Froglet application identity of the provider; must equal `signing.signer`
- `requester_id`: Froglet application identity of the intended requester
- `descriptor_hash`: descriptor hash used when the quote was issued
- `offer_hash`: content hash of the referenced offer
- `quote_id`: provider-generated unique identifier
- `request_hash`: hash of the canonical quote request object
- `created_at`: unix timestamp in seconds
- `expires_at`: unix timestamp in seconds
- `payment_mode`: must be `base_fee_plus_success_fee` in version 1
- `settlement_terms`: object with:
  - `destination_identity`: linked Lightning settlement identity expected on returned invoices
  - `base_fee_msat`
  - `success_fee_msat`
  - `base_invoice_expiry_secs`
  - `success_hold_expiry_secs`
  - `min_final_cltv_expiry`
- `execution_limits`: object with:
  - `max_input_bytes`
  - `max_runtime_ms`
  - `max_memory_bytes`
  - `max_output_bytes`
  - `fuel_limit`
- `completion_criteria_hash`: hash of the canonical completion-criteria object

### 5.5 Deal payload

`Deal.payload` must contain:

- `deal_id`: requester-generated unique identifier
- `requester_id`: Froglet application identity of the requester; must equal `signing.signer`
- `provider_id`: Froglet application identity of the quoted provider
- `quote_hash`: content hash of the accepted quote
- `workload_hash`: hash of the canonical workload object to be executed or served
- `payment_hash`: lowercase hex SHA-256 of the requester-chosen success-fee secret
- `created_at`: unix timestamp in seconds
- `admission_deadline`: latest time the provider may admit or reject the deal
- `completion_deadline`: latest time the provider may finish the work
- `acceptance_deadline`: latest time the requester may release the success-fee secret

`Deal.payload` may additionally contain:

- `client_reference`: requester-local idempotency or trace identifier
- `result_delivery`: optional result-delivery hint object

### 5.6 Receipt payload

`Receipt.payload` must contain:

- `provider_id`: Froglet application identity of the provider; must equal `signing.signer`
- `requester_id`: Froglet application identity of the requester from the deal
- `deal_hash`: content hash of the referenced deal
- `quote_hash`: content hash of the referenced quote
- `created_at`: unix timestamp in seconds
- `started_at`: unix timestamp in seconds or `null`
- `finished_at`: unix timestamp in seconds
- `deal_outcome`: exactly one of `succeeded`, `failed`, `rejected`, or `canceled`
- `execution_state`: exactly one of `not_started`, `succeeded`, or `failed`
- `settlement_state`: exactly one of `none`, `settled`, `canceled`, or `expired`
- `result_hash`: hash of the canonical result object or `null`
- `result_format`: result format identifier or `null`
- `executor`: object with:
  - `runtime`: `wasm` or another local-only adapter name
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
  - `destination_identity`
  - `base_fee`: object with `amount_msat`, `invoice_hash`, `payment_hash`, and `state`
  - `success_fee`: object with `amount_msat`, `invoice_hash`, `payment_hash`, and `state`

`Receipt.payload` may additionally contain:

- `failure_code`: machine-readable failure code
- `failure_message`: human-readable failure message
- `result_ref`: content-addressed pointer or encrypted payload pointer
- `settlement_finalized_at`: unix timestamp in seconds or `null`

Admission refusal is represented as a receipt with `deal_outcome = rejected` and `execution_state = not_started`.
Requester or provider abort is represented as a receipt with `deal_outcome = canceled`.

Artifacts should be content-addressed and retrievable by hash.
Requesters must persist the quote, deal, and receipt artifacts relevant to their own transactions so a provider disappearing later does not erase the cryptographic evidence of the interaction.

Append-only is a logical property, not a storage-engine mandate.
Implementations may compact or archive terminal records, but they must preserve enough material to reproduce hashes, signatures, and required audit chains.
Open deals, unsettled payment records, and accountability evidence must not be pruned away.

### 5.7 Storage and archival invariants

Version 1 does not mandate SQLite or any other storage engine.
It does mandate the logical properties that storage must preserve.

Every Froglet implementation must preserve four logical classes of data:

- immutable artifact documents
- an append-only local feed log
- mutable query indexes derived from those records
- settlement and execution evidence needed to justify terminal receipts

The required invariants are:

- **Artifact immutability**
  - once an `artifact_hash` is associated with a signed artifact envelope, that mapping must never change
  - implementations may store parsed fields alongside raw content, but they must retain enough canonical material to recompute `payload_hash`, signing bytes, and `artifact_hash`
  - if a stored artifact cannot be revalidated against its hash and signature after restart, it must be treated as corrupted evidence

- **Durability before acknowledgment**
  - a node must not acknowledge creation, acceptance, or publication of an artifact until the artifact document and the minimal related state transition are durably recorded
  - a node must not expose an artifact through `/v1/feed` or `/v1/artifacts/:hash` before that durability condition is met

- **Logical append-only feed**
  - every first local observation of an artifact must receive a strictly increasing local feed sequence
  - feed cursors are local, monotonic, and exclusive-after; once assigned, a feed sequence number must never be reused or renumbered
  - compaction may move older feed entries to colder storage, but it must not change cursor semantics for retained history

- **Derived-index rebuildability**
  - mutable query tables, caches, and search indexes are not canonical state
  - deal views, provider views, and discovery indexes must be rebuildable from retained artifact documents plus retained settlement and execution evidence
  - implementations may optimize query paths however they want, but they must not make verification depend on opaque mutable rows that cannot be reconstructed

- **Receipt accountability preservation**
  - for every locally opened or locally served deal, the implementation must retain the associated quote, deal, and terminal receipt artifacts
  - if a receipt references non-core transport evidence, such as an `invoice_bundle`, result package, or external settlement identifier, the implementation must retain either that evidence itself or enough canonical material to verify the referenced hashes and identifiers later
  - restart recovery must never destroy the evidence required to explain why a receipt ended in `succeeded`, `failed`, `rejected`, `canceled`, `settled`, `expired`, or `canceled`

- **Pruning and archival safety**
  - implementations must not prune:
    - non-terminal deals
    - unsettled or not-yet-expired payment records
    - the latest valid descriptor chain needed to interpret active offers and active endpoints
    - the only retained copy of a quote, deal, receipt, or settlement record for a transaction the node participated in
  - implementations may archive:
    - superseded descriptors
    - expired offers and quotes
    - terminal deals and receipts whose evidence is already sealed
    - old feed segments
  - archival is only valid if offline verification of retained hashes, signatures, hash chains, and terminal receipt claims remains possible

- **Export independence**
  - an implementation should be able to export its retained evidence into an engine-neutral archive form
  - that archive form must preserve enough information to reconstruct:
    - artifact documents by hash
    - local feed order for retained entries
    - quote -> deal -> receipt hash chains
    - settlement references and final settlement states
    - timestamps relevant to accountability and expiry

SQLite, Postgres, flat files, object storage, or replicated logs are all acceptable if these invariants hold.
The storage engine is an implementation detail.
The invariants above are part of the protocol contract.

## 6. Canonical State Model

Core implementations must model deal state, execution state, and settlement state separately.

### 6.1 Deal state

Canonical deal states for version 1 are:

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

### 6.2 Execution state

Canonical execution states for version 1 are:

- `not_started`
- `running`
- `succeeded`
- `failed`

Execution state is only relevant for deals that were admitted.
Rejected or canceled deals should not enter `running`.

### 6.3 Settlement state

Canonical settlement states for version 1 are:

- `none`
- `invoice_open`
- `funds_locked`
- `settled`
- `canceled`
- `expired`

Settlement progression is not inferred from deal state alone.
A requester may lock funds for a deal that later fails, and a provider may emit a failure receipt while the settlement leg is later canceled or expired.

A terminal receipt must include enough information to determine:

- the final deal outcome
- the final execution outcome
- the final settlement outcome or outcomes relevant to the deal

## 7. Transport and Discovery

### 7.1 Transport

Version 1 keeps direct HTTPS as the baseline transport and Tor as an optional transport.

Transport choice must not change protocol semantics.
A `Quote`, `Deal`, or `Receipt` must mean the same thing whether it moved over clearnet HTTPS or an onion service.

Transport endpoints belong in `Descriptor`.
They are hints about reachability, not identity.
A provider identity must not be derived from an IP address, DNS record, or onion address.

### 7.2 Direct peers and curated lists

The version 1 discovery baseline is deliberately simple:

- direct peer configuration
- local allowlists
- signed curated lists
- private brokers or private catalogs

This keeps discovery out of the core trust model while still making the network usable.

### 7.3 Nostr publication and discovery

Nostr remains valuable as an optional dissemination and discovery layer.

It may be used to publish:

- descriptor summaries
- offer summaries
- artifact hashes
- transport endpoint hints
- receipt hashes or reputation references
- signed curated peer lists

Nostr must not be treated as canonical economic state.
It is an optional publication and indexing fabric, not a required execution or settlement dependency.

If a provider uses a Nostr identity, that identity must be explicitly linked from the provider descriptor.

### 7.4 Endpoint rotation rules

Endpoint rotation must not invalidate provider identity or previously signed economic artifacts.

Version 1 endpoint rotation rules are:

- `Descriptor.descriptor_seq` must increase monotonically for a given `provider_id`
- requesters should prefer the valid descriptor with the highest `descriptor_seq`, breaking ties by `created_at`
- a transport endpoint is considered active only while both the enclosing descriptor and the endpoint binding itself are unexpired
- removing an endpoint from a newer descriptor is the canonical way to deprecate it
- rotating transport endpoints must not require rotating the Froglet application identity
- rotating settlement or publication identities requires new linkage proofs in the new descriptor

Older descriptors may remain useful for audit, but requesters should not initiate fresh deals against expired descriptors or expired endpoint bindings.

### 7.5 Signed curated-list bootstrap format

Signed curated lists are bootstrap discovery objects, not canonical economic state.
They are intended for direct peers, private networks, and early marketplace bootstrapping.

A `curated_list` object must contain:

- `schema_version`: `froglet/v1`
- `list_type`: `curated_list`
- `curator_id`: Froglet identity of the curator
- `list_id`: curator-generated unique identifier
- `created_at`: unix timestamp in seconds
- `expires_at`: unix timestamp in seconds
- `entries`: array of objects with:
  - `provider_id`
  - `descriptor_hash`
  - `tags`: optional array of short discovery labels
  - `note`: optional display-only string
- `signing`: same signature fields and signing procedure as Froglet-signed artifacts, but with `domain = froglet:curated_list:v1`

Consumers of a curated list must treat it as a signed recommendation set, not as proof that the listed providers are online, honest, or currently reachable.

## 8. Settlement Model

Version 1 settlement is Lightning-first.
Cashu and other settlement drivers are intentionally out of the mainline v1 path.
Future settlement drivers may exist, but they must not shape the hard core until the Lightning path is solid.

### 8.1 Lightning as the version 1 settlement rail

The version 1 settlement rail is built around two Lightning payment legs:

- `base_fee`: a normal immediately settled invoice used to compensate admission and anti-griefing cost
- `success_fee`: a hold-invoice-backed conditional payment leg used for the main result-dependent settlement

Offers and quotes should default to `base_fee_plus_success_fee`.
In trusted or private environments, `base_fee` may be zero, but open marketplaces should assume providers need a non-zero base fee.

### 8.2 Quote and deal binding

A `Quote` must commit to the settlement terms required to safely validate any returned invoice material, including:

- settlement destination identity, such as Lightning node public key
- `base_fee_msat`
- `success_fee_msat`
- invoice expiry constraints
- final-hop CLTV constraints when relevant
- `payment_mode`
- maximum job duration

A `Deal` must commit to:

- `QuoteHash`
- `WorkloadHash`
- `payment_hash` for the success fee leg
- request and acceptance deadlines

### 8.3 Invoice bundle binding

After a provider accepts a deal, it must return invoice material through a signed transport object called an `invoice_bundle`.
An `invoice_bundle` is not a new core artifact type.
It is a transport-level signed object that binds Lightning invoices to the already-signed `Quote` and `Deal`.

`invoice_bundle` must contain:

- `schema_version`: `froglet/v1`
- `bundle_type`: `invoice_bundle`
- `provider_id`
- `requester_id`
- `quote_hash`
- `deal_hash`
- `created_at`
- `expires_at`
- `destination_identity`
- `base_invoice_bolt11`
- `base_invoice_payment_hash`
- `base_fee_msat`
- `success_hold_invoice_bolt11`
- `success_payment_hash`
- `success_fee_msat`
- `min_final_cltv_expiry`
- `signing`: same signature fields and signing procedure as Froglet-signed artifacts, but with `domain = froglet:invoice_bundle:v1`

The requester must validate the returned `invoice_bundle` before paying either invoice.
At minimum, the requester must reject the bundle if:

- `provider_id`, `requester_id`, `quote_hash`, or `deal_hash` do not match the current interaction
- `destination_identity` does not match `Quote.settlement_terms.destination_identity`
- `base_fee_msat` or `success_fee_msat` do not match the quote
- `success_payment_hash` does not equal `Deal.payment_hash`
- `expires_at` exceeds the quote lifetime or the encoded invoice expiries exceed the quoted constraints
- the Froglet signature over the bundle is invalid

The requester should also verify that the decoded BOLT11 invoices are internally consistent with the bundle fields before any payment is attempted.

### 8.4 Requester-controlled success-fee release

For the version 1 success-fee leg, the requester chooses a secret `s` and includes `payment_hash = SHA256(s)` in the deal.
The provider creates a hold invoice bound to that hash.

The intended sequence is:

1. The requester asks for a quote.
2. The provider returns a signed quote.
3. The requester opens a deal that includes `payment_hash = SHA256(s)`.
4. The provider returns a signed `invoice_bundle` bound to the quote and deal.
5. The requester validates the `invoice_bundle` and pays the base fee invoice and success-fee hold invoice.
6. The provider observes that the base fee is settled and the success fee is locked.
7. The provider executes the work.
8. The provider returns result material, a `result_ref`, or another transport-level result package bound to the deal.
9. The requester reveals `s` to accept the success fee, or the provider cancels the hold or allows it to expire.
10. After the settlement outcome is terminal, the provider emits the final signed `Receipt`.

This flow intentionally gives the requester leverage over the success-fee release.
That is acceptable for version 1, but it means Froglet does not provide perfect fair exchange for arbitrary computation or data delivery.

### 8.5 Receipt semantics under requester-controlled release

In requester-controlled success-fee mode, the secret `s` is not proof of payment by itself.
The requester knew it before payment.

Therefore, the authoritative terminal artifact is the final signed `Receipt`, emitted only after the settlement outcome is terminal, and anchored by:

- the provider signature
- the deal hash
- the result hash
- execution metadata
- Lightning settlement references and final observed settlement state

### 8.6 Time bounds and operational limits

Hold invoices lock liquidity and therefore constrain the kind of work Froglet can safely sell.
Version 1 should support short-lived jobs measured in seconds, not long-running opaque sessions.

If a job does not fit within conservative hold windows, it should be:

- chunked into multiple deals
- converted into staged payments
- or treated as out of scope for version 1

Providers must automate cancellation and expiry handling.
Manual operator intervention is not acceptable as the normal failure path.

### 8.7 Admission, capacity, and rejection

Quotes guarantee price and terms.
They do not reserve provider capacity.

Providers must be allowed to reject admission if local CPU, memory, concurrency, policy, or wallet constraints were exhausted between quote issuance and deal opening.
These rejections should be machine-readable and signed as rejection receipts.

## 9. Pricing and Workload Model

Version 1 pricing is fixed-price only.
Metered billing is intentionally out of the hard v1 core.

A `Quote` must bind the full resource envelope, including:

- input schema or workload type
- maximum runtime
- memory cap
- maximum output size
- completion criteria
- total settlement amounts
- expiry

`WorkloadHash` is always the hash of a canonical request object under the protocol's canonical serialization rules.

The core supports both compute and data-like services.
For version 1, that means:

- remotely supplied code execution is Wasm-only
- data-like services remain valid, but they must still expose bounded request and response contracts with signed quotes and receipts

### 9.1 Canonical `compute.wasm.v1` workload object

The canonical public compute workload kind for version 1 is `compute.wasm.v1`.

The canonical workload object must contain:

- `schema_version`: `froglet/v1`
- `workload_kind`: `compute.wasm.v1`
- `abi_version`: `froglet.wasm.run_json.v1`
- `module_format`: `application/wasm`
- `module_hash`: lowercase hex SHA-256 of the raw Wasm binary bytes
- `input_format`: `application/json+jcs`
- `input_hash`: lowercase hex SHA-256 of the RFC 8785 JCS bytes of the input JSON value
- `requested_capabilities`: array of capability strings; must be empty in version 1

The canonical `WorkloadHash` is:

- `SHA256(JCS(workload_object))`

The canonical workload object deliberately does **not** include:

- inline Wasm bytes
- transport URLs
- local cache keys
- provider-specific file paths
- non-verifiable execution hints

Those belong to transport-level request objects, not the economic core.

Version 1 therefore separates:

- the economic workload identity: `module_hash + input_hash + abi_version + capability request`
- the transport submission material: inline bytes or externally fetched blobs that satisfy those hashes

This separation keeps the signed economic contract small and stable while still allowing convenient HTTP submission flows.

### 9.2 Transport-level Wasm submission object

To make `compute.wasm.v1` executable over HTTP without bloating the core artifact model, version 1 defines a transport object called `wasm_submission`.

`wasm_submission` is not a sixth core artifact type.
It is a request/response transport object used when a requester needs to provide execution material.

`wasm_submission` must contain:

- `schema_version`: `froglet/v1`
- `submission_type`: `wasm_submission`
- `workload`: canonical `compute.wasm.v1` workload object
- `module_bytes`: raw Wasm binary bytes encoded for transport, or an equivalent field whose decoding is unambiguous
- `input`: the JSON input value whose JCS hash must equal `workload.input_hash`

Before execution, the provider must verify:

- `SHA256(module_bytes) == workload.module_hash`
- `SHA256(JCS(input)) == workload.input_hash`
- `workload.abi_version` is supported
- `requested_capabilities` is acceptable under the active offer and quote

Providers may support cached execution by accepting an already-known `module_hash` without resending bytes, but that is a transport optimization, not part of the canonical workload identity.

## 10. Execution Model

The public network execution target for version 1 is raw Wasm.

Version 1 should define a small, stable Wasm execution contract with:

- a fixed ABI
- explicit input object hashing
- explicit capability grants
- strict resource limits
- deterministic receipt metadata

Lua is not part of version 1.
The version 1 execution primitive is Wasm only.

### 10.1 `froglet.wasm.run_json.v1` ABI

The version 1 public Wasm ABI is `froglet.wasm.run_json.v1`.

Its purpose is to keep remote execution interoperable and minimal:

- input is canonical JSON bytes
- output is JSON bytes
- there is one required entrypoint
- there are no ambient host capabilities
- execution occurs inside one short-lived sandbox invocation

Modules implementing `froglet.wasm.run_json.v1` must satisfy all of the following:

- the module format must be a core WebAssembly binary module, not a component-model package
- the module must define one exported linear memory named `memory`
- the module must export `alloc(len: i32) -> i32`
- the module must export `run(input_ptr: i32, input_len: i32) -> i64`
- the module may export `dealloc(ptr: i32, len: i32) -> ()`, but providers are not required to call it in version 1
- the module must not require WASI, filesystem, network, clock, or other undeclared host imports

The host execution sequence is:

1. Canonicalize the requester input with RFC 8785 JCS.
2. Verify the resulting bytes hash to `workload.input_hash`.
3. Call `alloc(input_len)` to obtain writable memory for the input bytes.
4. Copy the JCS input bytes into `memory[input_ptr..input_ptr + input_len)`.
5. Call `run(input_ptr, input_len)`.
6. Interpret the returned `i64` as:
   - upper 32 bits: `result_ptr`
   - lower 32 bits: `result_len`
7. Validate that the returned slice lies within the exported memory and that `result_len` does not exceed the active output limit.
8. Copy the result bytes from guest memory.
9. Parse the result bytes as UTF-8 JSON.
10. Canonicalize the resulting JSON value with RFC 8785 JCS for `result_hash` and receipt generation.

Execution fails if any of the following occur:

- missing required exports
- invalid memory range from `alloc` or `run`
- trap during module execution
- meter exhaustion
- timeout
- invalid UTF-8 output
- invalid JSON output
- output that exceeds the quoted limit

The `run` function represents a successful execution only if it returns a valid pointer/length pair for a JSON result.
Application-level success or failure inside the business logic is represented in the JSON result itself.
Sandbox or ABI failures are represented as Froglet execution failures and must appear in terminal receipts.

### 10.2 Capability and determinism profile

The version 1 public capability profile for `froglet.wasm.run_json.v1` is deliberately empty.

That means:

- `requested_capabilities` must be `[]`
- `capabilities_granted` in receipts must be `[]`
- providers must not expose ambient filesystem, network, clock, randomness, or process APIs to public version 1 compute workloads

Future ABI versions may define explicit capability-scoped imports, but version 1 keeps the public interoperable surface pure and bounded.

The version 1 determinism profile is:

- module identity is the raw Wasm binary hash
- input identity is the JCS hash of the input JSON value
- result identity is the JCS hash of the output JSON value
- no clock, RNG, filesystem, or network access is part of the public ABI

This does not guarantee universal reproducibility across every engine or future CPU target.
It does guarantee that Froglet receipts can name exactly which module, input, ABI, and result were involved in a completed deal.

## 11. Sandbox Requirements

Sandbox policy must be explicit and default-deny.

Minimum requirements for version 1 are:

- memory caps
- fuel or equivalent compute accounting
- wall-clock execution timeout
- output size caps
- filesystem denial by default
- network denial by default
- explicit capability-scoped host calls only
- bounded or timed host-call behavior

Timeouts, policy denials, meter exhaustion, and runtime traps must surface as machine-readable failure reasons in receipts.

## 12. Receipt Requirements

A terminal receipt is immutable and must represent exactly one terminal outcome.

At minimum, a version 1 receipt should include:

- `deal_hash`
- provider identity
- deal outcome
- `result_hash`
- result format metadata
- executor type and version
- code or module hash when relevant
- resource-limit profile actually applied
- settlement references
- final terminal settlement state
- timestamp
- provider signature

Without this metadata, receipts are only claims, not useful accountability artifacts.

## 13. Bot-Facing Local Runtime

The localhost bot runtime should make Froglet immediately usable by agents.

The happy path should feel like:

- search
- quote
- deal
- wait
- accept or reject
- receipt

The runtime should hide transport, invoice, and discovery details on the happy path, while still allowing advanced callers to inspect them when needed.

The localhost runtime must require local authentication for all privileged requests.
Binding to localhost is not a sufficient trust boundary.

The runtime may expose:

- async workflows that return a `deal_id`
- convenience workflows that wait for a terminal receipt
- wallet and descriptor inspection
- provider lifecycle helpers

Long-lived full-agent execution, session leasing, and checkpointed remote agents are future layers on top of the same primitive, not reasons to widen version 1.

## 14. Marketplace on Froglet

The long-term marketplace should itself be composed of Froglet services.

Examples include:

- indexers that crawl descriptors, offers, and receipts
- brokers that aggregate quotes and route deals
- catalog services that publish curated subsets of the network
- reputation services that interpret receipt history

These services consume the same artifacts as any other participant.
They are not privileged protocol actors.

Froglet proves who signed what.
It does not prove that every signed claim is factually true.
Fraud interpretation, arbitration, slashing, and reputation are higher-layer marketplace concerns.

## 15. What Belongs in Core

Core should contain:

- artifact schemas
- canonical serialization and hashing rules
- signing and verification
- ledger invariants
- deal, execution, and settlement state machines
- settlement interfaces
- receipt verification
- content-addressed artifact retrieval

## 16. What Does Not Belong in Core

Core should not hardwire:

- Cashu as a settlement dependency in version 1
- Nostr as a source of truth
- Tor as the only transport
- Lua as a public network execution requirement
- metered pricing in the hard v1 surface
- a single privileged marketplace design
- a mandatory storage engine
- long-running opaque remote sessions as a version 1 requirement

## 17. Product Direction

The intended product direction is:

- a very small, stable Froglet core
- a Lightning-first economic kernel
- a Wasm-first public execution surface
- optional Tor transport and optional Nostr publication
- a bot runtime that OpenClaw can use immediately
- higher-layer marketplaces built on top of the same primitive

That keeps Froglet usable now without turning it into a bloated platform.
