---
title: "Canonical Serialization"
description: Why everyone must agree on the same bytes.
---

To hash or sign a data structure, you must first convert it to bytes. But different systems might serialize the same data differently:

```
System A:  {"b": 2, "a": 1}
System B:  {"a": 1, "b": 2}
System C:  {"a":1,"b":2}
```

These all represent the same data but produce different bytes — and therefore different hashes.

## RFC 8785 JSON Canonicalization Scheme (JCS)

Froglet uses **JCS** to guarantee that any two implementations, in any language, given the same data, produce the exact same bytes:

- Object keys are sorted lexicographically
- No unnecessary whitespace
- Numbers use minimal representation
- Unicode escaping is standardized

```
Input:   {"b": false, "c": 12e1, "a": "Hello!"}
JCS:     {"a":"Hello!","b":false,"c":120}
```

:::note
Without canonical serialization, the protocol would be ambiguous. Two honest implementations could disagree on whether a signature is valid simply because they serialize the same data differently.
:::

## Why this matters for froglet

1. **Hashing**: `payload_hash = SHA-256(JCS(payload))` — both parties compute the same hash
2. **Signing**: the signer canonicalizes before signing, the verifier canonicalizes before verifying
3. **Interoperability**: a Rust node, a Python node, and a JavaScript node all produce identical bytes for the same artifact
