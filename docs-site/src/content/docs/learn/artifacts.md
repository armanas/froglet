---
title: "Signed Artifacts"
description: "Froglet's atomic unit — the signed envelope."
---

Everything in froglet is a **signed artifact**. It's a JSON envelope that wraps a payload with identity and integrity proof:

```json
{
  "artifact_type": "offer",
  "schema_version": "froglet/v1",
  "signer": "02a8d6...your_public_key",
  "created_at": 1700000000,
  "payload_hash": "sha256_of_canonical_payload",
  "hash": "sha256_of_canonical_signing_bytes",
  "payload": { ... },
  "signature": "bip340_schnorr_signature"
}
```

## Verification

1. Canonicalize the `payload` using JCS
2. SHA-256 the canonical bytes — must match `payload_hash`
3. Build signing bytes: `artifact_type + schema_version + signer + created_at + payload_hash`
4. SHA-256 the signing bytes — must match `hash`
5. Verify BIP340 Schnorr: `verify(signer, signature, hash)`

If all checks pass:

- The payload has not been tampered with
- The artifact was produced by the holder of the private key corresponding to `signer`
- The timestamp was committed at signing time

## Six artifact types

| Type | Signed by | Purpose |
|------|-----------|---------|
| **Descriptor** | Provider | Declares identity, capabilities, transport |
| **Offer** | Provider | Declares a specific service with pricing |
| **Quote** | Provider | Prices a workload for a specific requester |
| **Deal** | Requester | Commits to the quote |
| **InvoiceBundle** | Provider | Lightning payment instructions |
| **Receipt** | Provider | Proof of execution outcome and settlement |

:::tip[This is the foundation of everything]
Every descriptor, offer, quote, deal, invoice, and receipt is a signed artifact. The entire protocol is a chain of these artifacts referencing each other by hash. No central authority — just math.
:::
