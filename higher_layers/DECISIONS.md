# Higher-Layer Decisions

Date anchored: 2026-03-15

These decisions capture the current boundary for work that should not keep
growing the Froglet core repo sideways.

## 1. Core Repo Boundary

Decision:

- This repo remains focused on the Froglet kernel, node/runtime, adapters,
  SDKs, conformance fixtures, and reference examples.

Implication:

- Product-layer services may be prototyped here temporarily, but the intended
  long-term home is outside this repo.
- Public planning for those services should live under `higher_layers/`.
- Private early-stage incubation may temporarily live under an ignored local
  `private/` directory, but that must not create hidden coupling back into the
  public core.

## 2. Marketplace Boundary

Decision:

- The real marketplace is an ordinary Froglet-consuming service, not a
  privileged protocol actor.

Implication:

- Marketplace behavior must not become part of canonical economic state.
- Discovery, indexing, ranking, broker logic, and reputation logic stay above
  the kernel.
- The existing in-repo `marketplace` binary should be treated as a reference
  discovery service, not as the long-term full marketplace product.

## 3. Integration Boundary

Decision:

- Protocol and integration tooling should remain public, including the
  OpenClaw integration.

Implication:

- SDKs and integration adapters should optimize for visibility, portability,
  and third-party use.
- The public Froglet OpenClaw plugin may include reference marketplace
  discovery plus documented local runtime helpers, but first-party
  marketplace-product integrations should stay in separate private packages.
- Commercial differentiation should live in higher-layer services and data, not
  in hidden protocol connectors.

## 4. Commercial Higher-Layer Boundary

Decision:

- The official marketplace, indexers, brokers, ranking/reputation systems, and
  related service-layer products may remain closed source.

Implication:

- Those products must compete through data, curation, routing, trust policy,
  and operations rather than through protocol secrecy.
- Closed higher-layer services should consume public Froglet interfaces and
  signed artifacts rather than private core internals.

## 5. Ownership Boundary

Decision:

- No kernel change is required now for ownership-style linkage.

Implication:

- Froglet `provider_id` remains the operational protocol identity.
- Ownership, issuer, brand, or company-level claims should be modeled as
  higher-layer attestations and/or linked identities, not as changes to quote,
  deal, or receipt semantics.

## 6. Exchange / Stock-Like Products

Decision:

- Any future market for ownership, issuer activity, or stock-like instruments
  is a separate product layer beyond the core marketplace.

Implication:

- Do not mix listing, capitalization, issuance, transfer, or corporate-action
  logic into Froglet core.
- Do not block core freeze on capital-market features.

## 7. Extraction Rule

Decision:

- Work in this directory should be written so it can be moved into a new repo
  with minimal churn.

Implication:

- Keep boundaries explicit.
- Avoid assumptions that depend on private files, private DB schema, or
  unstable internal functions.
