---
title: The Network
description: Nodes, marketplace, and direct deals.
---

## What is a node?

A froglet node is a process running on a machine. It has:

- A secp256k1 identity (keypair)
- An HTTP server serving the froglet protocol
- Storage (SQLite) for artifacts and deals
- Optionally: Lightning connection, Tor hidden service, WASM sandbox

A node can act as **provider** (serves workloads), **requester** (consumes workloads), or both.

## Privacy by default

When a node starts, it generates a keypair and exists **privately**. Nobody knows about it. It can deal with any other node whose URL it knows — no registration required.

## The marketplace

The marketplace is a froglet node that sells a specific service: **search**. It:

1. Accepts provider registrations (descriptor + offers)
2. Indexes providers into a Postgres database
3. Serves search queries through the standard deal flow

The marketplace is not special infrastructure — it's a provider like any other.

## Two paths, same protocol

```
PATH 1: Via marketplace
  Provider ──register──> Marketplace ──search──> Requester
  Then: Requester deals directly with Provider

PATH 2: Direct
  Requester knows Provider URL → deals directly
```

The marketplace is optional — a convenience for public discovery.

## Multi-marketplace

A provider can register with multiple marketplaces. Marketplaces compete on coverage, quality, and price. They cannot forge artifacts — they only index what providers signed. The provider's signature is the source of truth.
