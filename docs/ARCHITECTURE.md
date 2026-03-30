# Froglet Architecture

Status: non-normative supporting document

[`KERNEL.md`](KERNEL.md) is the kernel contract.
This document describes how the rest of the system is layered around that kernel.

## 1. Layering

Froglet is intentionally split into four layers:

- economic kernel
- adapters
- bot-facing localhost runtime
- higher-layer discovery, indexers, brokers, operators, and bot integrations

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
- the signed execution/request commitments that every execution profile must use

The current codebase still ships Wasm reference profiles, but the product
model is broader:

- a predefined service
- a predefined data service
- open-ended compute

are all the same Froglet primitive with different product-layer bindings.

## 3. Adapters

Adapters make the kernel usable in real environments without becoming part of the trust boundary.

Examples:

- HTTPS and Tor transport
- Lightning node drivers such as mock mode or LND REST
- Nostr publication and relay behavior
- discovery bootstrap formats
- execution-material delivery such as module uploads, interpreted source
  bundles, archive bundles such as zip files, or container/image references
- registry pulls for runtime-specific packaged workloads

Adapters may change, and implementations may support more than one adapter, as long as they preserve kernel semantics.

## 4. Bot Runtime

The bot runtime is the primary product surface for agent developers.

Its purpose is to make the signed kernel usable through a simpler localhost
workflow:

- search
- quote
- deal
- wait
- accept or reject
- receipt

At the product surface, this should feel like one thing:

- discover a resource
- invoke a named service
- invoke a data service
- send open-ended compute

Those are UX distinctions over the same underlying deal flow.

The runtime may expose local handles, helper endpoints, polling views,
wallet-facing payment intents, and compatibility routes.
Those are product decisions, not protocol commitments.
Longer-running agent workflows should stay above the runtime and reuse ordinary
Froglet deals rather than widening the kernel.

## 5. Reference Discovery and Higher Layers

Froglet's long-term discovery and commercial product layers should be composed
from ordinary Froglet services rather than privileged protocol actors.

Examples:

- indexers over artifact feeds
- catalogs built from indexed descriptors and offers
- brokers that aggregate or route quotes
- reputation services that interpret receipt history
- marketplaces that publish search, listing, or routing services
- resource providers that publish named services, data services, or open-ended
  compute through the same deal primitive

These services consume signed artifacts.
They are not themselves the source of truth.
The core repo defines the boundary and shared contract; product-specific
planning should live with the owning service or deployment. Local ignored
incubation may exist under `private_work/`, but that workspace is not part of
the public Froglet release surface.

## 6. What Stays Out of the Kernel

The kernel should not hardwire:

- a relay network as the source of truth
- a single transport stack
- a single storage engine
- runtime HTTP endpoint shapes
- a single execution runtime or packaging format
- reference-discovery, ranking, or broker logic
- archive bundle layout
- long-running session semantics
- cloud-provider-specific deployment behavior

That boundary is deliberate.
The best core implementation is the smallest irreversible surface.
