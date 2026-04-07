---
title: File Reference
description: Every source file in the codebase.
---

## froglet-protocol/src/

| File | Lines | Purpose | Key Exports |
|------|-------|---------|-------------|
| `lib.rs` | 47 | Crate root, ExecutionRuntime enum | `ExecutionRuntime` |
| `canonical_json.rs` | 45 | RFC 8785 JCS | `to_vec()`, `to_string()` |
| `crypto.rs` | 202 | secp256k1 BIP340, SHA-256 | `NodeSigningKey`, `verify_signature()`, `sha256_hex()` |
| `protocol/kernel.rs` | 1,033 | Signed envelope, payloads, verify/sign | `SignedArtifact<T>`, all payload types |
| `protocol/publication.rs` | 22 | Curated list types | `CuratedListPayload` |

## froglet/src/ — Core

| File | Lines | Purpose |
|------|-------|---------|
| `lib.rs` | 44 | Crate root, re-exports from froglet-protocol |
| `protocol/workload.rs` | 181 | WorkloadSpec enum, dispatch types |
| `execution.rs` | 938 | BuiltinServiceHandler trait, workload types |
| `identity.rs` | 268 | Keypair generation, node_id, Nostr key |

## froglet/src/ — Settlement

| File | Lines | Purpose |
|------|-------|---------|
| `settlement.rs` | 1,851 | Lightning two-leg model, InvoiceBundle lifecycle |
| `lnd.rs` | 683 | LND REST API client |
| `pricing.rs` | 72 | Service pricing table |

## froglet/src/ — Execution Adapters

| File | Lines | Purpose |
|------|-------|---------|
| `sandbox.rs` | 1,108 | Wasmtime WASM sandbox |
| `wasm.rs` | 474 | WASM workload types, submissions |
| `wasm_host.rs` | 119 | WASM host environment (HTTP, DB) |
| `wasm_http.rs` | 591 | HTTP fetch capability for WASM |
| `wasm_db.rs` | 467 | SQLite query capability for WASM |
| `confidential.rs` | 1,226 | TEE/encrypted execution |

## froglet/src/ — Transport & Identity

| File | Lines | Purpose |
|------|-------|---------|
| `tls.rs` | 280 | TLS/HTTPS, proxy config, custom CA |
| `tor.rs` | 254 | Tor hidden service sidecar |
| `nostr.rs` | 331 | Nostr event publication, linked identity |

## froglet/src/ — Node Runtime

| File | Lines | Purpose |
|------|-------|---------|
| `config.rs` | 1,014 | NodeConfig from environment |
| `state.rs` | 410 | AppState: shared runtime state |
| `server.rs` | 787 | HTTP server, supervision, Tor/Lightning loops |
| `db.rs` | 1,618 | SQLite storage, migrations, pooling |
| `deals.rs` | 1,245 | Provider-side deal lifecycle |
| `requester_deals.rs` | 317 | Requester-side deal tracking |
| `jobs.rs` | 470 | Async job queue |
| `provider_catalog.rs` | 46 | Offer publication |
| `provider_resolution.rs` | 219 | Endpoint validation, SSRF prevention |
| `runtime_auth.rs` | 219 | Localhost bearer token auth |

## froglet/src/api/

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | 12,464 | Central API: deal orchestration, execution dispatch |
| `types.rs` | 626 | Request/response DTOs |
| `http_catalog.rs` | 14 | Descriptor, offers, feed routes |
| `http_deals.rs` | 18 | Quote/deal routes |
| `http_discovery.rs` | 41 | Discovery stubs (410 GONE) |
| `http_events.rs` | 37 | Event publish/query routes |
| `http_execution.rs` | 32 | WASM execution and jobs routes |
| `http_settlement.rs` | 42 | Settlement routes |
| `http_confidential.rs` | 17 | Confidential profile/session routes |

## froglet-marketplace/src/

| File | Lines | Purpose |
|------|-------|---------|
| `lib.rs` | 165 | Startup: Postgres, handlers, offers, indexer, server |
| `config.rs` | 46 | MarketplaceConfig from env |
| `db.rs` | 67 | Postgres pool, migrations |
| `verify.rs` | 21 | Shared artifact signature verification |
| `handlers/search.rs` | 180 | marketplace.search handler |
| `handlers/provider.rs` | 139 | marketplace.provider handler |
| `handlers/receipts.rs` | 121 | marketplace.receipts handler |
| `handlers/register.rs` | 134 | marketplace.register handler |
| `indexer/mod.rs` | 319 | Feed polling, dynamic discovery |
| `indexer/projector.rs` | 185 | Artifact projection to Postgres |

## tests/

| File | Tests | Purpose |
|------|-------|---------|
| `builtin_service_dispatch.rs` | 3 | Custom handler dispatch |
| `kernel_conformance_vectors.rs` | ~10 | Protocol conformance |
| `production_contract_properties.rs` | ~10 | Contract properties |
| `runtime_routes.rs` | 12 | Runtime API roundtrip |
| `payments_and_discovery.rs` | 6 | Payment integration |
| `lnd_rest_settlement.rs` | 6 | LND REST integration |
| `generate_free_service_vectors.rs` | ~4 | Vector generation |
