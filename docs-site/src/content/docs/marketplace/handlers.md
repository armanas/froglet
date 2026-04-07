---
title: Handlers
description: The four marketplace service handlers.
---

Each handler implements `BuiltinServiceHandler` — JSON in, JSON out, Postgres queries.

## marketplace.register

Providers push their signed descriptor and offers to be indexed immediately.

**Input:**
```json
{
  "descriptor": { "artifact_type": "descriptor", "signer": "...", ... },
  "offers": [{ "artifact_type": "offer", ... }, ...],
  "feed_url": "http://provider:8080"
}
```

**Logic:** Verify BIP340 signature on descriptor and each offer. Check offer provider_id matches descriptor. Project into Postgres.

## marketplace.search

Search providers by filters.

**Input:**
```json
{
  "offer_kind": "compute.execution.v1",
  "runtime": "wasm",
  "max_price_sats": 100,
  "limit": 20,
  "cursor": null
}
```

**Output:** Provider list with offers, paginated. Uses a fan-in query (no N+1).

## marketplace.provider

Get one provider's details with trust scores.

**Input:**
```json
{ "provider_id": "02abc..." }
```

**Output:** Full descriptor, all offers, trust summary (total/succeeded/failed receipts).

## marketplace.receipts

Provider receipt history with filtering.

**Input:**
```json
{ "provider_id": "02abc...", "status": "succeeded", "limit": 20 }
```

**Output:** Paginated receipts + trust summary.
