---
title: Crate Structure
description: Two public crates — protocol and node.
---

## Workspace

```
froglet-protocol/     open source — the kernel
froglet/              open source — the node framework
```

## froglet-protocol (1,354 lines)

The single source of truth for kernel types. Independently reimplementable.

| Module | Purpose |
|--------|---------|
| `canonical_json` | RFC 8785 JCS canonicalization |
| `crypto` | secp256k1 BIP340, SHA-256, HMAC |
| `protocol/kernel` | SignedArtifact, all 6 payload types, verify/sign |
| `protocol/publication` | CuratedList types |
| `ExecutionRuntime` | Enum shared across crates |

Dependencies: `serde`, `serde_json`, `serde_json_canonicalizer`, `k256`, `rand`, `hex`, `sha2`.

## froglet (28,584 lines)

The node framework. Re-exports kernel types from `froglet-protocol`.

| Layer | Modules |
|-------|---------|
| **Core** | `execution` (BuiltinServiceHandler), `identity`, `protocol/workload` |
| **Settlement** | `settlement`, `lnd`, `pricing` |
| **Execution** | `sandbox`, `wasm*`, `confidential` |
| **Transport** | `tls`, `tor`, `nostr` |
| **Runtime** | `api/*`, `server`, `config`, `state`, `db`, `deals`, `jobs` |

The default marketplace implementation now lives outside this public repo while
continuing to use Froglet's public marketplace integration contract.
