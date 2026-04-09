---
title: Marketplace
description: The first service built on froglet — provider discovery, search, and staking.
---

The marketplace is a froglet provider node with six builtin service handlers and a background indexer. From the outside it looks like any other provider — discoverable, quotable, payable through the standard deal flow.

## Six services

| Service | Purpose | Input | Output |
|---------|---------|-------|--------|
| `marketplace.register` | Provider pushes descriptor + offers | signed artifacts | confirmation |
| `marketplace.search` | Search by filters | offer_kind, runtime, price | provider list |
| `marketplace.provider` | Provider details | provider_id | descriptor, offers, stake |
| `marketplace.receipts` | Receipt history | provider_id, status | paginated receipts |
| `marketplace.stake` | Stake into provider identity | provider_id, amount | stake confirmation |
| `marketplace.topup` | Top up existing stake | provider_id, amount | updated total |

## The 3-node scenario

```
Node B (provider)           Node C (marketplace)         Node A (requester)

  ──register deal──────────>  indexes into Postgres
                                                    <──search deal──
                              returns: [B]          ──results──────>
  <─────────────────direct deal─────────────────────
  ──────────────────receipt──────────────────────────>
```

Every arrow is the same protocol.

## Handler details

Each handler implements `BuiltinServiceHandler` — JSON in, JSON out, Postgres queries.

**marketplace.register** — Providers push their signed descriptor and offers to be indexed immediately. Verifies BIP340 signature on descriptor and each offer.

**marketplace.search** — Search providers by filters (`offer_kind`, `runtime`, `max_price_sats`). Returns paginated provider list using a fan-in query.

**marketplace.provider** — Get one provider's details: full descriptor, all offers, and stake summary.

**marketplace.receipts** — Provider receipt history with status filtering. Receipts are evidence served as data — trust is determined solely by stake.

**marketplace.stake** — Deposit non-refundable value into a provider's identity. The stake amount is the deal price.

**marketplace.topup** — Add more value to an existing stake. Requires a prior `marketplace.stake`.

## Indexer

The indexer runs as background tokio tasks, polling provider feeds and projecting artifacts into Postgres.

**Two modes:** Static sources from `MARKETPLACE_FEED_SOURCES` env (each gets a dedicated polling loop), and dynamic discovery via `MARKETPLACE_DISCOVERY_URL` (queries for new providers, capped at 200).

**Polling loop:** Load cursor → GET feed → verify BIP340 signatures → project (descriptors → providers table, offers → offers table, receipts → receipts table) → advance cursor. Per-source exponential backoff on failures.

## Configuration

```bash
# Standard froglet config
FROGLET_LISTEN_ADDR=0.0.0.0:8080
FROGLET_PAYMENT_BACKEND=none

# Marketplace-specific
MARKETPLACE_DATABASE_URL=postgres://user:pass@localhost/froglet_marketplace
MARKETPLACE_FEED_SOURCES=http://provider1:8080,http://provider2:8080
MARKETPLACE_DISCOVERY_URL=http://discovery:8080   # optional
MARKETPLACE_POLL_INTERVAL_SECS=30
MARKETPLACE_MAX_DYNAMIC_SOURCES=200
```
