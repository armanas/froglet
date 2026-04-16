# Froglet Architecture

Status: non-normative supporting document

[`KERNEL.md`](KERNEL.md) is the kernel contract.
This document describes how the system is layered around that kernel.

## 1. What Froglet Is

Froglet is three things:

1. **Identity** — a secp256k1 keypair generated locally. Your public key is your
   identity. Every artifact you produce is signed. Nobody grants or revokes this.
   Linked identities (Nostr) provide immutable, publicly verifiable identity
   publication.

2. **Execution** — a signed evidence chain. Descriptor, Offer, Quote, Deal,
   Receipt. A provider describes what it offers, a requester deals for it, the
   provider executes and signs a receipt. The entire chain is independently
   verifiable.

3. **Settlement** — Lightning Network. Base fee locks on admission, success fee
   settles on completion. Cryptographically bound to the deal. Or free.

Everything else is either an adapter that makes the core usable in a specific
environment, or a service built on top of the protocol.

## 2. Layering

```
┌─────────────────────────────────────────────────────┐
│                    Services                         │
│   marketplace, brokers, reputation, curated lists   │
│   (built ON the protocol, not part of it)           │
├─────────────────────────────────────────────────────┤
│                  Node Runtime                       │
│   HTTP server, deal tracking, jobs API,             │
│   storage, auth, config                             │
├─────────────────────────────────────────────────────┤
│                    Adapters                          │
│   WASM sandbox, Python, Container, LND,             │
│   Tor, TLS, Nostr, TEE/confidential                 │
├─────────────────────────────────────────────────────┤
│              Economic Kernel                         │
│   signed envelope, canonical JSON, hashing,         │
│   signing, verification, artifact types,            │
│   settlement state machine                          │
└─────────────────────────────────────────────────────┘
```

Each layer depends only on the layers below it. The kernel is the smallest
irreversible surface. Everything above it may evolve without changing how
hashes, signatures, deals, or receipts work.

## 3. Economic Kernel

The kernel must remain stable and independently reimplementable.

It lives in the `froglet-protocol` crate (open source) and contains:

- the signed artifact envelope (`SignedArtifact<T>`)
- canonical JSON serialization (RFC 8785 JCS)
- SHA-256 hashing and BIP340 Schnorr signing
- the six artifact types: Descriptor, Offer, Quote, Deal, InvoiceBundle, Receipt
- cross-artifact hash commitments
- verification rules
- the `ExecutionRuntime` enum

The kernel does not know about HTTP, databases, sandboxes, or any specific
execution environment. It is pure types, signing, and verification.

## 4. Adapters

Adapters make the kernel usable in real environments without becoming part of
the trust boundary.

| Adapter | Purpose |
|---------|---------|
| WASM sandbox | Sandboxed execution with fuel, memory, and I/O limits |
| Python / Container | Alternative execution runtimes |
| LND REST | Lightning invoice creation and settlement |
| Tor | Anonymous transport via hidden services |
| TLS | HTTPS transport with certificate management |
| Nostr | Linked identity publication to Nostr relays |
| TEE / Confidential | Trusted execution environment attestation and encryption |

Adapters may change, and implementations may support more than one, as long as
they preserve kernel semantics.

## 5. Node Runtime

The node runtime is the product surface. It takes the kernel and adapters and
makes them usable through HTTP endpoints:

- **Provider API** (`/v1/provider/*`) — quotes, deals, offers, feed, artifacts
- **Runtime API** (`/v1/runtime/*`) — deal creation, status polling, acceptance
- **Jobs API** (`/v1/node/jobs`) — async execution with polling

The runtime handles deal state tracking, storage (SQLite), pricing, auth,
and the execution dispatch that routes workloads to the correct adapter.

### BuiltinServiceHandler

The `BuiltinServiceHandler` trait is how services plug into the node:

```rust
pub trait BuiltinServiceHandler: Send + Sync + 'static {
    fn execute(&self, input: Value) -> Future<Result<Value, String>>;
}
```

A node registers handlers in `AppState.builtin_services` at startup. When a
deal targets a builtin offer, the dispatch calls the handler. JSON in, JSON out.
The handler owns its state (database pools, caches, HTTP clients).

This is the same mechanism for all services — marketplace, reputation engines,
data services, anything built on Froglet.

## 6. Services

Services are Froglet providers that serve domain-specific queries or
computations through the standard deal flow.

Marketplaces are one example of that service layer. From the public Froglet
boundary they stay external integration points for:

- provider registration
- runtime discovery and provider lookup
- marketplace-specific ranking or trust policy layered above the signed protocol

Services are not privileged protocol actors. They consume signed artifacts.
They are not the source of truth — the signed artifacts are.

### How a Service Works

```
1. Service starts a Froglet node
2. Registers BuiltinServiceHandler(s) in AppState
3. Publishes offers for its service kinds
4. Serves deals: requester → quote → deal → execute handler → receipt
```

The deal flow is identical to WASM compute or any other execution. The only
difference is the handler runs in-process instead of in a sandbox.

## 7. Network Model

```
Node B (provider)                 Node C (external marketplace)

private by default                a Froglet provider
generates identity locally        serves discovery through the same deal flow
publishes to marketplace          serves search as deals
  (optional, explicit)
      │                                  │
      │   marketplace.register deal      │
      ├─────────────────────────────────►│
      │                                  │
                                         │
Node A (requester)                       │
                                         │
      │   marketplace.search deal        │
      ├─────────────────────────────────►│
      │   results: [Node B, ...]         │
      │◄─────────────────────────────────┤
      │                                  │
      │          direct deal             │
      ├─────────────────────────────────►│ Node B
      │                                  │
```

Two paths, same protocol:

- **Via marketplace** — provider registers, requester searches, then deals
  directly with the found provider
- **Direct** — requester knows the provider URL, deals without marketplace

Nodes are private by default. They are not known publicly unless they
explicitly register with a marketplace.

## 8. Crate Structure

```
froglet-protocol/        open source kernel
  canonical_json          RFC 8785 canonicalization
  crypto                  secp256k1, SHA-256
  protocol/kernel         SignedArtifact, all payload types, verification
  protocol/publication    CuratedList types
  ExecutionRuntime        execution runtime enum

froglet/                 open source node framework
  protocol/workload       WorkloadSpec (re-exports kernel from froglet-protocol)
  execution               BuiltinServiceHandler trait, workload types
  identity                keypair generation, node_id
  settlement              Lightning two-leg model
  api/                    HTTP routes for deal flow
  server                  HTTP server, supervision
  db                      SQLite storage
  deals, jobs             deal tracking, async execution
  sandbox, wasm*          WASM execution adapter
  confidential            TEE/encrypted execution
  nostr                   linked identity publication
  lnd                     Lightning driver
  tls, tor                transport adapters

default marketplace      first-party implementation lives outside this repo
  public contract        registration + runtime discovery stay in froglet
  implementation         see ../froglet-services and MARKETPLACE_SPLIT.md
```

`froglet-protocol` is the single source of truth for kernel types. `froglet`
re-exports from it. No duplication.

## 9. Deal Flow

Every interaction follows the same signed evidence chain:

```
Descriptor ──► Offer ──► Quote ──► Deal ──► Receipt
   (who)        (what)    (price)   (commit)  (proof)
```

1. **Descriptor** — provider declares identity, capabilities, transport endpoints
2. **Offer** — provider declares a specific service with pricing and execution profile
3. **Quote** — provider prices a specific workload for a specific requester
4. **Deal** — requester commits to the quote (signed by requester)
5. **Receipt** — provider signs proof of execution outcome and settlement state

Each artifact references the previous one by hash. The chain is independently
verifiable by any party holding the artifacts.

### Settlement

For paid deals, an InvoiceBundle is created between Quote and Deal:

```
Quote ──► InvoiceBundle ──► Deal ──► Receipt
            base_fee          │
            success_fee       │
                              ▼
                        execution
```

- **Base fee** locks on deal admission (prevents free-riding)
- **Success fee** settles only on successful execution (fair to requester)
- Both are Lightning HTLCs with cryptographic preimage binding

Free deals (`settlement_method: "none"`) skip the InvoiceBundle entirely.

## 10. What Stays Out of the Kernel

The kernel does not hardwire:

- a specific marketplace or discovery mechanism
- a single transport stack
- a single storage engine
- runtime HTTP endpoint shapes
- a single execution runtime or packaging format
- ranking, broker, or reputation logic
- archive or deployment layouts
- long-running session semantics

That boundary is deliberate.
The best core implementation is the smallest irreversible surface.

## 11. Code Layout

- `src/protocol/` — re-exports kernel types from `froglet-protocol`, plus `workload.rs`
- `src/api/` — HTTP routes organized by domain (catalog, deals, settlement, execution, events, confidential)
- `src/execution.rs` — `BuiltinServiceHandler` trait, `ExecutionWorkload`, runtime/package enums
- `src/settlement.rs` — `SettlementDriver` trait, Lightning invoice bundle lifecycle
- `src/identity.rs` — secp256k1 key loading and generation
- `src/server.rs` — HTTP server bootstrap, `run_provider()` and `run_provider_with_state()`
- `src/provider_catalog.rs` — offer publication and service catalog
