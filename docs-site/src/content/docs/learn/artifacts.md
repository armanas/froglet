---
title: "Signed Artifacts"
description: "Froglet's atomic unit — the signed envelope."
---

Everything in froglet is a **signed artifact**. It's a JSON envelope that wraps a payload with identity and integrity proof:

<div class="learn-sequence four">
  <div class="learn-sequence-step">
    <strong>Payload</strong>
    <small>The protocol-specific data, canonicalized with JCS.</small>
  </div>
  <div class="learn-sequence-step">
    <strong>Payload hash</strong>
    <small>Commits to the exact canonical bytes of that payload.</small>
  </div>
  <div class="learn-sequence-step">
    <strong>Artifact hash</strong>
    <small>Commits to signer, type, timestamp, and payload hash.</small>
  </div>
  <div class="learn-sequence-step">
    <strong>Signature</strong>
    <small>Proves the holder of the private key authored the envelope.</small>
  </div>
</div>

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

<div class="learn-grid two">
  <div class="learn-card">
    <span class="learn-kicker">Verification pipeline</span>
    <ol>
      <li>Canonicalize the <code>payload</code> using JCS.</li>
      <li>SHA-256 the canonical bytes and compare to <code>payload_hash</code>.</li>
      <li>Build signing bytes from type, schema version, signer, timestamp, and payload hash.</li>
      <li>SHA-256 those signing bytes and compare to <code>hash</code>.</li>
      <li>Verify the BIP340 Schnorr signature against the signer key.</li>
    </ol>
  </div>
  <div class="learn-card">
    <span class="learn-kicker">What you learn if it passes</span>
    <ul>
      <li>The payload has not been tampered with.</li>
      <li>The artifact came from the holder of the signer's private key.</li>
      <li>The timestamp and payload hash were committed at signing time.</li>
    </ul>
  </div>
</div>

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
