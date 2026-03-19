# Higher-Layer Repo Strategy

This document is the canonical public/private boundary for product-layer work.
It replaces the older `private/` incubation idea as the active planning model.

## Public Components

Keep public:

- `SPEC.md` and protocol-facing docs
- the Froglet node/runtime
- adapters and verification surfaces
- SDKs, conformance fixtures, and reference examples
- the public OpenClaw integration
- the reference discovery service

These pieces exist to maximize interoperability, trust, and ecosystem
adoption.

## Private Components

Keep private or extract later:

- the commercial marketplace product
- indexers and catalog projections
- broker and routing logic
- ranking and reputation systems
- ownership and issuer overlays
- hosted operator/control-plane tooling
- first-party OpenClaw integration helpers for private product surfaces

These systems should compete on data, curation, routing quality, trust policy,
operations, and user experience rather than on protocol secrecy.

## In-Repo Staging Rule

While this work is still incubated in the public Froglet repo, stage it under:

- `higher_layers/marketplace/`
- `higher_layers/indexer/`
- `higher_layers/broker/`
- `higher_layers/trust/`
- `higher_layers/operator/`
- `higher_layers/openclaw/`

Do not reintroduce a hidden `private/` source tree as the primary plan.

## Boundary Rules

Higher-layer code must:

- consume public Froglet HTTP APIs, signed artifacts, or documented external
  contracts
- avoid direct reads from Froglet SQLite databases
- avoid imports from non-public internal Rust modules as shortcuts
- avoid shaping `SPEC.md` around private-product convenience
- document any new required public contract under `higher_layers/` before
  depending on it privately

Public code must not depend on ignored private code or assume it exists.

## Extraction Trigger

Move each service into its own repository once:

- the service interface is defined
- data ownership is clear
- the service has its own release cadence
- development speed would otherwise pressure the Froglet core boundary

Until then, keep the boundary explicit and easy to extract.
