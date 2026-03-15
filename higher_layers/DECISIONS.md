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

## 3. Ownership Boundary

Decision:

- No kernel change is required now for ownership-style linkage.

Implication:

- Froglet `provider_id` remains the operational protocol identity.
- Ownership, issuer, brand, or company-level claims should be modeled as
  higher-layer attestations and/or linked identities, not as changes to quote,
  deal, or receipt semantics.

## 4. Exchange / Stock-Like Products

Decision:

- Any future market for ownership, issuer activity, or stock-like instruments
  is a separate product layer beyond the core marketplace.

Implication:

- Do not mix listing, capitalization, issuance, transfer, or corporate-action
  logic into Froglet core.
- Do not block core freeze on capital-market features.

## 5. Extraction Rule

Decision:

- Work in this directory should be written so it can be moved into a new repo
  with minimal churn.

Implication:

- Keep boundaries explicit.
- Avoid assumptions that depend on private files, private DB schema, or
  unstable internal functions.
