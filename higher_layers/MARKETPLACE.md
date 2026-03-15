# Marketplace Scope

The marketplace should be built as a set of higher-layer services that consume
Froglet, not as an extension of the Froglet trust boundary.

## 1. Roles

The marketplace should be split into distinct roles:

- indexer: ingests signed Froglet artifacts and builds queryable projections
- catalog: presents curated provider and service discovery on top of indexed
  data
- broker: aggregates or routes quotes without becoming canonical state
- reputation service: interprets receipt history and other observed signals
- issuer/ownership profile service: maps providers to brands, operators,
  issuers, or organizations

These roles may live together early on, but they should remain conceptually
separate.

## 2. What the Marketplace Consumes

The marketplace should consume:

- signed Froglet descriptors
- signed Froglet offers
- signed curated lists
- signed receipts or receipt summaries
- optional Nostr summaries
- optional operator-provided metadata layered above signed artifacts

The marketplace should not require:

- private access to Froglet SQLite databases
- privileged runtime-only state to establish canonical truth
- custom settlement shortcuts
- changes to quote, deal, or receipt semantics

## 3. What the Marketplace Must Not Become

The marketplace must not become:

- the source of canonical economic truth
- a required broker for normal Froglet operation
- a replacement for direct peer discovery
- the owner of protocol verification semantics

Froglet artifacts remain primary. Marketplace projections are derived views.

## 4. Suggested Build Order

1. Minimal indexer
2. Searchable catalog
3. Ownership / issuer profiles
4. Broker and quote-routing experiments
5. Reputation and ranking policy
6. Any future exchange/capital-market layer

## 5. Temporary In-Repo Incubation Rule

While this work lives in this repo:

- keep it under `higher_layers/`
- do not mix it into `SPEC.md`
- do not make the core node depend on it
- prefer documentation and boundary definitions before implementation

## 6. Move-Out Trigger

This work should move into a separate repo once:

- the first external service interface is defined
- data ownership is clear
- the service has its own release cadence
- development speed would otherwise pressure the Froglet core boundary
