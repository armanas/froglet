---
title: Indexer
description: How the marketplace discovers and indexes providers.
---

## How it works

The indexer runs as background tokio tasks, polling provider feeds and projecting artifacts into Postgres.

## Two modes

**Static sources** — feed URLs from `MARKETPLACE_FEED_SOURCES` env. Each gets a dedicated polling loop.

**Dynamic discovery** — if `MARKETPLACE_DISCOVERY_URL` is set, periodically queries it for new providers and spawns pollers. Capped at `MARKETPLACE_MAX_DYNAMIC_SOURCES` (default 200).

## Polling loop

```
1. Load last_cursor from Postgres
2. GET {source}/v1/feed?cursor={last_cursor}&limit=100
3. For each artifact:
   - Verify BIP340 signature
   - Store raw artifact (idempotent by hash)
   - Project: descriptor → providers, offer → offers, receipt → receipts
   - Advance cursor
4. If has_more → loop immediately (catch up)
5. Otherwise → sleep poll_interval
```

## Projection rules

**Descriptors:** Upsert by `provider_id`. Only replace if `descriptor_seq >= current`. This is the supersession rule — a new descriptor from the same provider only wins if its sequence is higher.

**Offers:** Insert with `ON CONFLICT (offer_hash) DO UPDATE`. FK constraint on `provider_id` — if the provider isn't indexed yet, the insert fails gracefully (catches Postgres error code 23503).

**Receipts:** Insert with `ON CONFLICT DO NOTHING`. Receipts are immutable.

## Error handling

- Per-source exponential backoff on failures (capped at 16x interval)
- One source being down doesn't block others
- Cursor only advances after successful processing
- Error count and last error tracked in `indexer_cursors` table
