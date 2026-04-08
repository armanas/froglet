---
title: Storage & Databases
description: How froglet persists data.
---

## SQLite (node-local)

Every froglet node stores its data in **SQLite** — a self-contained database in a single file:

- **Artifact documents** — every signed artifact the node has produced or received
- **Feed entries** — the ordered log of artifacts (the public feed)
- **Deal records** — state of every deal (pending, accepted, running, succeeded, failed)
- **Events** — Nostr-compatible events published to the node

## PostgreSQL (marketplace)

The marketplace uses **PostgreSQL** for complex queries:

- **marketplace_providers** — latest descriptor per provider
- **marketplace_offers** — all active offers with pricing
- **marketplace_receipts** — receipt history for audit and quality monitoring
- **marketplace_stakes** — non-refundable identity stake balances (trust = total staked)
- **marketplace_stake_ledger** — stake deposit and topup transaction history
- **indexer_cursors** — polling state for each feed source

## The feed

Every froglet node exposes a **feed** — an ordered log of all artifacts it has produced:

```
GET /v1/feed?cursor=0&limit=100

{
  "artifacts": [
    {"cursor": 1, "kind": "descriptor", "document": {...}},
    {"cursor": 2, "kind": "offer", "document": {...}}
  ],
  "has_more": false,
  "next_cursor": 2
}
```

The marketplace indexer polls these feeds to discover providers.
