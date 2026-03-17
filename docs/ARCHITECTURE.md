# Froglet Architecture

Status: non-normative supporting document

[`../SPEC.md`](../SPEC.md) is the kernel contract.
This document describes how the rest of the system is layered around that kernel.

## 1. Layering

Froglet is intentionally split into four layers:

- economic kernel
- adapters
- bot-facing localhost runtime
- higher-layer marketplace services

The kernel is the smallest irreversible surface.
Everything above it may evolve without changing how hashes, signatures, deals, or receipts work.

## 2. Economic Kernel

The kernel is the part that must remain stable and independently reimplementable.

It contains:

- the signed artifact envelope
- canonical serialization, hashing, and signing
- the `Descriptor`, `Offer`, `Quote`, `Deal`, and `Receipt` payloads
- cross-artifact commitments
- canonical deal, execution, and settlement states
- Lightning settlement binding rules
- the canonical Wasm workload objects (`compute.wasm.v1` and `compute.wasm.oci.v1`) and ABIs

## 3. Adapters

Adapters make the kernel usable in real environments without becoming part of the trust boundary.

Examples:

- HTTPS and Tor transport
- Lightning node drivers such as mock mode or LND REST
- Nostr publication and relay behavior
- discovery bootstrap formats
- runtime submission helpers such as transport-level Wasm uploads
- OCI registry pulls for `compute.wasm.oci.v1` workloads

Adapters may change, and implementations may support more than one adapter, as long as they preserve kernel semantics.

## 4. Bot Runtime

The bot runtime is the primary product surface for agent developers.

Its purpose is to make the signed kernel usable through a simpler localhost workflow:

- search
- quote
- deal
- wait
- accept or reject
- receipt

The runtime may expose local handles, helper endpoints, polling views, wallet-facing payment intents, and compatibility routes.
Those are product decisions, not protocol commitments.

The planned evolution from this runtime toward fuller long-running agent workflows is described in `REMOTE_AGENT_LAYER.md`.

## 5. Marketplace Services

Froglet's long-term marketplace should be composed from ordinary Froglet-consuming services rather than privileged protocol actors.

Examples:

- indexers over artifact feeds
- catalogs built from indexed descriptors and offers
- brokers that aggregate or route quotes
- reputation services that interpret receipt history

These services consume signed artifacts.
They are not themselves the source of truth.
Detailed staged planning for this layer lives under `../higher_layers/` while it
is incubated beside the core repo.

## 6. What Stays Out of the Kernel

The kernel should not hardwire:

- a relay network as the source of truth
- a single transport stack
- a single storage engine
- runtime HTTP endpoint shapes
- Python helper ergonomics
- marketplace roles or ranking logic
- archive bundle layout
- long-running session semantics

That boundary is deliberate.
The best core implementation is the smallest irreversible surface.
