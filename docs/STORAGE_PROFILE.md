# Froglet Storage Profile

Status: non-normative supporting document

The kernel does not mandate a storage engine.
This document records the logical invariants and the current reference SQLite profile used by the implementation.

## 1. Logical Invariants

Implementations should preserve four logical classes of data:

- immutable artifact documents
- an append-only local feed log
- mutable query indexes derived from retained evidence
- settlement and execution evidence needed to justify terminal receipts

The important invariants are:

- artifact immutability
- durability before acknowledgment
- logical append-only feed sequencing
- rebuildable derived indexes
- receipt accountability preservation
- pruning and archival safety
- engine-neutral exportability

Any engine is acceptable if those invariants hold.

## 2. Reference SQLite Split

The reference implementation currently maps those invariants into three explicit storage classes:

- `artifact_documents`
- `artifact_feed`
- `execution_evidence`

The same implementation may also keep mutable convenience tables such as:

- `jobs`
- `quotes`
- `deals`
- `payment_tokens`
- `events`

Those convenience tables are not the authoritative evidence layer.

## 3. Evidence Retention

For every locally opened or locally served deal, the implementation should retain enough material to reconstruct:

- the signed artifacts by hash
- local feed order for retained entries
- the quote -> deal -> receipt chain
- settlement identifiers and final settlement states
- timestamps relevant to expiry and accountability

## 4. Export and Archival

The reference implementation may expose archive/export helpers for local accountability and migration.

Those exports should preserve, at minimum:

- retained artifact documents
- corresponding feed entries
- retained execution evidence
- retained settlement transport material such as `invoice_bundle` records

Archive layout is intentionally not part of the kernel contract.
