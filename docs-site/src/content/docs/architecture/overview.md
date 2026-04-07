---
title: Architecture Overview
description: System layers, three pillars, crate structure.
---

## System layers

```
SERVICES          marketplace, brokers, reputation, data services
                  (built ON the protocol using BuiltinServiceHandler)
──────────────────────────────────────────────────────────────
NODE RUNTIME      HTTP server, deal tracking, jobs API,
                  storage (SQLite), auth, config, pricing
──────────────────────────────────────────────────────────────
ADAPTERS          WASM sandbox   LND driver    Tor transport
                  Python/Container  TLS        Nostr identity
                  TEE/Confidential             Mock settlement
──────────────────────────────────────────────────────────────
ECONOMIC KERNEL   SignedArtifact<T>             secp256k1 identity
(froglet-protocol) canonical JSON (RFC 8785)   BIP340 Schnorr
                  SHA-256 hashing              Lightning settlement
                  6 artifact types             state machines
```

Each layer depends only on the layers below it. The kernel is the smallest irreversible surface.

## Execution dispatch

When a deal is accepted, the provider dispatches:

```
match (runtime, package_kind):
  Wasm/InlineModule     → WasmSandbox.execute()
  Python/InlineSource   → run_python_execution()
  Container/OciImage    → run_container_execution()
  TeeService/Builtin    → run_confidential_service()
  Builtin/Builtin       → dispatch_builtin_workload()
                            → builtin_services.get(name)
                            → handler.execute(input)
```

## Signed evidence chain

```
Descriptor ──> Offer ──> Quote ──> Deal ──> Receipt
   who          what      price    commit    proof
 (provider)  (provider) (provider) (requester) (provider)
```

For paid deals, InvoiceBundle sits between Quote and Deal with two Lightning HTLCs.
