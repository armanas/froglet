---
title: "Marketplace: How It Works"
description: The first service built on froglet.
---

The marketplace is a froglet provider node with four builtin service handlers and a background indexer. From the outside it looks like any other provider — discoverable, quotable, payable through the standard deal flow.

## Four services

| Service | Purpose | Input | Output |
|---------|---------|-------|--------|
| `marketplace.register` | Provider pushes descriptor + offers | signed artifacts | confirmation |
| `marketplace.search` | Search by filters | offer_kind, runtime, price | provider list |
| `marketplace.provider` | Provider details | provider_id | descriptor, offers, trust |
| `marketplace.receipts` | Receipt history | provider_id, status | receipts, trust summary |

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
