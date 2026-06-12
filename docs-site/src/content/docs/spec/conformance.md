---
title: "Conformance: implement froglet in your language"
description: The canonical test vectors, what they cover, and how to build a second implementation against them.
---

Froglet is specified by two things: the prose rules in the
[kernel spec](/spec/kernel/) and the canonical test vectors in
[`conformance/kernel_v1.json`](https://github.com/armanas/froglet/blob/main/conformance/kernel_v1.json).
When they could possibly disagree, **the vectors win** — they are what every
conforming implementation must reproduce byte-for-byte.

If you want froglet to exist in your language, this page is the entry point.
You do not need to read the Rust source.

## What the fixture covers

`conformance/kernel_v1.json` (one self-contained JSON file) contains:

| Section | Contents |
|---|---|
| `keys` | Fixed secp256k1 seeds and derived ids for a test provider and requester — so every implementation produces identical signatures |
| `artifacts` | A complete paid deal chain — `descriptor`, `offer`, `quote`, `deal`, `invoice_bundle`, `receipt` — each a full signed envelope with expected hashes |
| `artifacts` (free) | A complete free-service chain — `free_offer`, `free_quote`, `free_deal`, `free_receipt` — settlement method `none` |
| `conformance_path` | The required artifact verification order for the six-artifact paid chain |
| `free_service_conformance_path` | The five-artifact order for free deals |
| `artifact_verification_cases` | 14 accept/reject cases: tampered payloads, wrong signers, broken hash chains, schema violations |
| `invoice_bundle_validation_cases` | 5 cases for invoice-bundle immutability and binding rules |
| `linked_identity` | The linked-identity challenge format (Nostr publication scope), with expected signature |

## What a conforming implementation must do

1. **Canonicalize** payloads with RFC 8785 JCS and hash with SHA-256.
2. **Rebuild signing bytes** exactly as the kernel spec defines and verify
   BIP340 Schnorr signatures over secp256k1.
3. **Reproduce every hash in `artifacts`** — for each artifact, your
   canonicalization + hashing must yield the fixture's `payload_hash` and
   `hash` byte-for-byte.
4. **Verify the chains** in `conformance_path` order, enforcing the
   cross-artifact hash references.
5. **Match every verdict** in `artifact_verification_cases` and
   `invoice_bundle_validation_cases` — accept what must verify, reject what
   must not.

Pass all of that and your implementation is conforming. There is no
certification process beyond the vectors.

## Worked examples in two languages

The repo ships two independent harnesses that consume the same fixture —
use whichever is closer to your target language as a reference:

- **Rust**: [`tests/kernel_conformance_vectors.rs`](https://github.com/armanas/froglet/blob/main/tests/kernel_conformance_vectors.rs)
- **Python**: [`python/tests/test_conformance_vectors.py`](https://github.com/armanas/froglet/blob/main/python/tests/test_conformance_vectors.py)

The Python harness is deliberately dependency-light and is the better
starting point for a port.

## Stability

The vectors are governed by the
[versioning policy](https://github.com/armanas/froglet/blob/main/docs/VERSIONING.md):
`froglet/v1` signing bytes and verification rules are frozen, additions ship
with new vectors in the same release, and signed artifacts must verify
forever. Anything not covered by vectors (notably the experimental x402
driver) is not part of the conformance surface.

## Scope boundary

Conformance covers the **kernel**: envelopes, hashing, signing, the six
artifact types, chain verification, and settlement-state semantics. It does
not cover the reference node's HTTP API, the marketplace, or payment-rail
drivers — those are implementation surfaces, documented separately and free
to differ between implementations.

Building a port? Open an issue on
[GitHub](https://github.com/armanas/froglet/issues) — interoperability reports
from second implementations get priority attention.
