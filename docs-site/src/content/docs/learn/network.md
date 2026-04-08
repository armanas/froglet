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

<div class="learn-paths">
  <div class="learn-path">
    <span class="learn-kicker">Path 1</span>
    <strong>Via marketplace</strong>
    <ol>
      <li>Provider registers signed artifacts with a marketplace.</li>
      <li>Requester buys a search result from that marketplace service.</li>
      <li>Requester then deals directly with the chosen provider.</li>
    </ol>
  </div>
  <div class="learn-path">
    <span class="learn-kicker">Path 2</span>
    <strong>Direct</strong>
    <ol>
      <li>The requester already knows the provider URL.</li>
      <li>Discovery is skipped entirely.</li>
      <li>The same quote, deal, execution, and receipt flow still applies.</li>
    </ol>
  </div>
</div>

The marketplace is optional — a convenience for public discovery.

## Multi-marketplace

A provider can register with multiple marketplaces. Marketplaces compete on coverage, quality, and price. They cannot forge artifacts — they only index what providers signed. The provider's signature is the source of truth.
