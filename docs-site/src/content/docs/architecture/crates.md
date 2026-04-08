---
title: Crate Structure
description: Three crates — protocol, node, marketplace.
---

## Workspace

```
froglet-protocol/     open source — the kernel
froglet/              open source — the node framework
froglet-marketplace/  closed source — first service
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

## froglet-marketplace (1,398 lines)

The first service built on froglet.

| Module | Purpose |
|--------|---------|
| `handlers/register` | Accept signed descriptor + offers |
| `handlers/search` | Filter providers by kind/runtime/price |
| `handlers/provider` | Provider details + stake info |
| `handlers/receipts` | Receipt history |
| `handlers/stake` | Identity stake deposit |
| `handlers/topup` | Stake topup |
| `indexer` | Feed polling, signature verification, projection |
| `db` | Postgres connection pool |
| `verify` | Shared artifact signature verification |
