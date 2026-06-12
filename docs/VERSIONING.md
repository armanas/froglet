# Versioning & Stability Policy

This document defines what is frozen, what may change, and how change happens.
It exists so that anyone building a second implementation, archiving receipts,
or depending on signed froglet artifacts knows exactly what they can rely on.

## The core promise

**Signed `froglet/v1` artifacts verify forever.** A receipt produced today must
still verify, byte-for-byte, in any future conforming implementation. Nothing
in this policy ever permits a change that alters the verification outcome of
an existing signed artifact.

## What is frozen (kernel, `froglet/v1`)

The following are immutable for the lifetime of `froglet/v1`:

- The signed artifact envelope and its **signing bytes** construction
  (RFC 8785 JCS canonicalization, SHA-256 hashing, BIP340 Schnorr over
  secp256k1).
- The six artifact types — Descriptor, Offer, Quote, Deal, InvoiceBundle,
  Receipt — their required fields, hash-chain rules, and verification rules
  as specified in [KERNEL.md](./KERNEL.md).
- The canonical deal, execution, and settlement states and their terminal
  semantics.
- The semantics of every settlement method identifier that has shipped in a
  tagged release. A method's wire behavior is frozen the moment it appears in
  the conformance vectors.

## What may be added (additive change)

Additions are allowed within `froglet/v1` when they cannot change the
verification outcome of any existing artifact:

- **New settlement method identifiers** (e.g. `lightning.prepaid.v1` was added
  alongside `lightning.base_fee_plus_success_fee.v1`). Old methods keep
  working; verifiers that don't recognize a new method reject deals that use
  it, never previously-valid artifacts.
- **New artifact types** under new `artifact_type` values.
- **New companion specifications** (transport adapters, confidential
  execution, storage profiles) layered above the kernel.

Every addition ships with conformance vectors in
[`conformance/kernel_v1.json`](../conformance/kernel_v1.json) (or a sibling
fixture) in the same release. An addition without vectors is not part of the
protocol.

## What is experimental

Surfaces labeled **experimental** may change or be removed without a major
version: currently the **x402 (`x402_usdc`) settlement driver** — implemented
at the daemon level, not covered by conformance vectors, and not exposed on
the marketplace publish path. Do not build on experimental surfaces for
anything that must outlive a release cycle.

## What may change with notice (implementation, pre-1.0)

The reference node (`froglet-node`), its HTTP APIs, CLI, configuration
variables, MCP/OpenClaw integrations, and operational scripts follow the
implementation version (currently 0.x). Pre-1.0, these may change in any
release with a CHANGELOG entry; behavior-breaking changes (e.g. a new
fail-closed default) are called out explicitly in the release notes. The
kernel wire format does **not** participate in this looseness.

## How `froglet/v2` would happen

A change that cannot be made additively (altering signing bytes, envelope
shape, or verification rules) requires a new schema version string:

1. A written proposal in `docs/` describing the change, the migration path,
   and why it cannot be additive.
2. New conformance vectors for v2 before any implementation merges.
3. **Permanent parallel verification:** conforming implementations must verify
   `froglet/v1` artifacts forever. v2 changes what new artifacts look like,
   never what old ones mean.

There is no v2 proposal today, and none is planned.

## Deprecation

Settlement methods and companion surfaces may be deprecated (documented as
discouraged for new deals, with warnings in the reference implementation), but
their verification rules are never removed. Deprecation affects what new
artifacts *should* use, not what old artifacts *mean*.

## Conformance

An implementation is conforming if it passes every vector in
[`conformance/kernel_v1.json`](../conformance/kernel_v1.json). The Rust
harness ([`tests/kernel_conformance_vectors.rs`](../tests/kernel_conformance_vectors.rs))
and the Python harness ([`python/tests/test_conformance_vectors.py`](../python/tests/test_conformance_vectors.py))
are worked examples of consuming the fixture from two languages. Vector
coverage and the second-implementation guide live in the
[spec section of the docs site](https://froglet.dev/spec/conformance/).
