# Higher-Layer Repo Strategy

This document captures the intended split between the public Froglet core and
private product-layer services.

## 1. Public Components

The following should remain public and permissively licensed:

- `SPEC.md` and protocol-facing docs
- the Froglet node/runtime
- SDKs and integration tooling
- the OpenClaw integration
- conformance fixtures and reference examples
- the reference discovery service

These pieces exist to maximize interoperability, trust, and ecosystem
adoption.

## 2. Private Components

The following may remain private:

- the official marketplace product
- indexers and catalog projections
- broker and routing logic
- ranking and reputation systems
- ownership / issuer overlays
- seed-service catalog data and policy
- hosted control-plane or operator tooling

Those systems should compete on data, curation, routing quality, trust policy,
operations, and user experience rather than on protocol secrecy.

## 3. License Position

The public Froglet repo should stay under Apache-2.0 unless there is a concrete
distribution reason to change it.

Why:

- it is permissive enough for broad protocol and SDK adoption
- it keeps integration tooling easy to consume
- it includes an explicit patent grant, which is valuable for protocol and
  infrastructure code

Switching to MIT would simplify the text, but it would not materially improve
the intended open-core / closed-service split.

## 4. Temporary In-Repo Private Incubation

During early alignment, private higher-layer work may live in an ignored local
`private/` directory in this repo.

Suggested layout:

- `private/marketplace/`
- `private/indexer/`
- `private/broker/`
- `private/openclaw/` only for non-public operational helpers if needed

This is a temporary incubation aid, not the long-term home for those services.

## 5. Boundary Rules

Private higher-layer code must:

- consume public Froglet HTTP APIs, signed artifacts, or explicitly documented
  external contracts
- avoid direct reads from Froglet SQLite databases
- avoid imports from non-public internal Rust modules as shortcuts
- avoid shaping `SPEC.md` around private-product convenience
- document any new required public contract under `higher_layers/` before
  depending on it privately

Public code must not depend on ignored `private/` code or assume it exists.

## 6. Extraction Trigger

Move each private service into its own repository once:

- the service interface is defined
- data ownership is clear
- the service has its own release cadence
- development speed would otherwise pressure the Froglet core boundary

Until then, keep the boundary explicit and easy to extract.
