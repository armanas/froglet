---
title: "Canonical Serialization"
description: Why everyone must agree on the same bytes.
---

To hash or sign a data structure, you must first convert it to bytes. But different systems might serialize the same data differently:

<div class="learn-compare">
  <div class="learn-code-box">
    <span class="learn-kicker">Different serializers</span>
    <pre><code>System A: {&quot;b&quot;: 2, &quot;a&quot;: 1}
System B: {&quot;a&quot;: 1, &quot;b&quot;: 2}
System C: {&quot;a&quot;:1,&quot;b&quot;:2}</code></pre>
  </div>
  <div class="learn-code-box">
    <span class="learn-kicker">Same data, different bytes</span>
    <pre><code>semantic meaning: equal
byte sequence:    different
hash result:      different
signature:        different</code></pre>
  </div>
</div>

These all represent the same data but produce different bytes — and therefore different hashes.

## RFC 8785 JSON Canonicalization Scheme (JCS)

Froglet uses **JCS** to guarantee that any two implementations, in any language, given the same data, produce the exact same bytes:

- Object keys are sorted lexicographically
- No unnecessary whitespace
- Numbers use minimal representation
- Unicode escaping is standardized

<div class="learn-code-box">
  <span class="learn-kicker">Canonical output</span>
  <pre><code>Input: {"b": false, "c": 12e1, "a": "Hello!"}
JCS:   {"a":"Hello!","b":false,"c":120}</code></pre>
</div>

:::note
Without canonical serialization, the protocol would be ambiguous. Two honest implementations could disagree on whether a signature is valid simply because they serialize the same data differently.
:::

## Why this matters for froglet

<div class="learn-sequence">
  <div class="learn-sequence-step">
    <strong>1. Canonicalize</strong>
    <small>Every implementation turns the payload into the same byte stream.</small>
  </div>
  <div class="learn-sequence-step">
    <strong>2. Hash</strong>
    <small><code>payload_hash = SHA-256(JCS(payload))</code> becomes reproducible across languages.</small>
  </div>
  <div class="learn-sequence-step">
    <strong>3. Sign and verify</strong>
    <small>The signer and verifier both commit to identical bytes, not just equivalent JSON objects.</small>
  </div>
</div>

A Rust node, a Python node, and a JavaScript node all produce identical bytes for the same artifact. Without that property, interoperability breaks before the network even starts.
