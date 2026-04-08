---
title: Handlers
description: The six marketplace service handlers.
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

Get one provider's details with stake information.

**Input:**
```json
{ "provider_id": "02abc..." }
```

**Output:** Full descriptor, all offers, stake summary (total_staked_msat, last_staked_at).

## marketplace.receipts

Provider receipt history with filtering.

**Input:**
```json
{ "provider_id": "02abc...", "status": "succeeded", "limit": 20 }
```

**Output:** Paginated receipts. Receipts are evidence — they are served as data but do not determine trust. Trust is determined solely by stake (T = total staked msat).

## marketplace.stake

Deposit non-refundable value into a provider's identity. T = total_staked_msat — this is the complete trust signal.

**Input:**
```json
{ "provider_id": "02abc...", "amount_msat": 10000 }
```

**Output:**
```json
{
  "provider_id": "02abc...",
  "total_staked_msat": 10000,
  "amount_msat": 10000,
  "kind": "stake",
  "status": "staked"
}
```

**Logic:** Verify provider exists. Upsert into `marketplace_stakes` (add to total). Record in `marketplace_stake_ledger`. This is a paid marketplace service — the stake amount is the deal price.

## marketplace.topup

Add more value to an existing stake. Requires a prior `marketplace.stake`.

**Input:**
```json
{ "provider_id": "02abc...", "amount_msat": 5000 }
```

**Output:**
```json
{
  "provider_id": "02abc...",
  "total_staked_msat": 15000,
  "topup_amount_msat": 5000,
  "kind": "topup",
  "status": "topped_up"
}
```

**Logic:** Update existing stake (fails if no prior stake). Record in ledger.
