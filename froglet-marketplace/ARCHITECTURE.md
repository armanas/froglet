# Froglet Marketplace Architecture

The marketplace is the first service built on Froglet. It proves the protocol
can sustain its own infrastructure: a service that indexes Froglet artifacts
and serves queries through Froglet deals.

## What It Is

A Froglet provider node with six builtin service handlers and a background
indexer. From the outside it looks like any other provider — discoverable,
quotable, payable through the standard deal flow.

## Components

```
┌──────────────────────────────────────────────────────────┐
│                  froglet-marketplace binary               │
│                                                          │
│  ┌─────────────────┐     ┌────────────────────────────┐  │
│  │   Froglet Node   │     │   Marketplace Logic        │  │
│  │                  │     │                            │  │
│  │  deal flow       │     │  handlers/                 │  │
│  │  settlement      │     │    search.rs               │  │
│  │  feed/artifacts  │     │    provider.rs             │  │
│  │  HTTP server     │     │    receipts.rs             │  │
│  │                  │     │    register.rs             │  │
│  │                  │     │    stake.rs                │  │
│  │                  │     │    topup.rs                │  │
│  │  BuiltinService  │────►│                            │  │
│  │  Handler dispatch│     │  indexer/                  │  │
│  │                  │     │    mod.rs (feed polling)   │  │
│  └─────────────────┘     │    projector.rs            │  │
│                          │                            │  │
│                          │  db.rs (Postgres pool)     │  │
│                          │  verify.rs                 │  │
│                          └──────────────┬─────────────┘  │
│                                         │                │
│                                         ▼                │
│                               ┌──────────────────┐       │
│                               │    PostgreSQL     │       │
│                               │                  │       │
│                               │  providers       │       │
│                               │  offers          │       │
│                               │  receipts        │       │
│                               │  raw_artifacts   │       │
│                               │  cursors         │       │
│                               └──────────────────┘       │
└──────────────────────────────────────────────────────────┘
```

## Services

Six builtin service offers:

| Service | Purpose | Input | Output |
|---------|---------|-------|--------|
| `marketplace.register` | Provider pushes descriptor + offers | signed artifacts | confirmation |
| `marketplace.search` | Search providers by filters | offer_kind, runtime, price | provider list with offers |
| `marketplace.provider` | Get one provider's details + stake | provider_id | descriptor, offers, stake summary |
| `marketplace.receipts` | Get provider receipt history | provider_id, status filter | paginated receipts |
| `marketplace.stake` | Stake into provider identity | provider_id, amount_msat | stake confirmation + total |
| `marketplace.topup` | Top up existing stake | provider_id, amount_msat | updated total |

Each service is a `BuiltinServiceHandler` implementation that queries Postgres
and returns JSON. The deal flow wraps it: quote, deal, execute handler, receipt.

## Indexer

The indexer runs as background tokio tasks. Two modes:

**Static sources** — feed URLs configured via `MARKETPLACE_FEED_SOURCES` env var.
Each source gets a dedicated polling loop.

**Dynamic discovery** — if `MARKETPLACE_DISCOVERY_URL` is set, the indexer
periodically queries it for new providers and spawns pollers for each.
Capped at `MARKETPLACE_MAX_DYNAMIC_SOURCES` (default 200).

Per-source loop:

```
1. Load last_cursor from Postgres
2. GET {source}/v1/feed?cursor={last_cursor}&limit=100
3. For each artifact:
   - Verify BIP340 signature
   - Store raw artifact (idempotent by hash)
   - Project: descriptor → providers, offer → offers, receipt → receipts
   - Advance cursor
4. If has_more, loop immediately (catch up)
5. Otherwise sleep poll_interval
```

Signature verification uses `froglet_protocol::crypto::verify_signature`.
Projection is idempotent — descriptors upsert by provider_id with sequence
ordering, offers upsert by hash, receipts insert-or-ignore by hash.

## Postgres Schema

```
indexer_cursors           per-source polling state
marketplace_providers     projected descriptors (latest per provider_id)
marketplace_offers        projected offers (FK to providers)
marketplace_receipts      projected receipts
raw_artifacts             full artifact documents by hash
marketplace_stakes        non-refundable identity stake balances (trust = total staked)
marketplace_stake_ledger  stake deposit and topup transaction history
```

Descriptor supersession: a new descriptor from the same provider_id only
replaces the current one if `descriptor_seq` is higher.

Offer FK violations (provider not yet indexed) are caught and skipped, not
pre-checked — the indexer attempts the insert and handles the constraint error.

## Startup Sequence

```
1. Load NodeConfig from env (standard Froglet config)
2. Load MarketplaceConfig from env (Postgres URL, feed sources)
3. Connect to Postgres, run migrations
4. Build 6 service handlers with shared PgPool
5. Build Froglet AppState
6. Inject handlers into AppState.builtin_services
7. Auto-register 6 marketplace offer definitions
8. Spawn indexer background tasks
9. Start server via froglet::server::run_provider_with_state()
```

The marketplace is a standard Froglet provider after startup. The server
infrastructure (HTTP, Tor, Lightning, settlement loops) is all inherited.

## Configuration

```
# Standard Froglet node config
FROGLET_LISTEN_ADDR=0.0.0.0:8080
FROGLET_PAYMENT_BACKEND=none

# Marketplace-specific
MARKETPLACE_DATABASE_URL=postgres://user:pass@localhost/froglet_marketplace
MARKETPLACE_FEED_SOURCES=http://provider1:8080,http://provider2:8080
MARKETPLACE_DISCOVERY_URL=http://discovery:8080   (optional)
MARKETPLACE_POLL_INTERVAL_SECS=30                 (default)
MARKETPLACE_MAX_DYNAMIC_SOURCES=200               (default)
```

## The 3-Node Scenario

```
Node B (provider)           Node C (marketplace)         Node A (requester)

starts, generates           starts, connects to          starts, knows C's URL
secp256k1 identity          Postgres, registers offers

                  deal: marketplace.register
B ──────────────────────────► C
  descriptor + offers          indexes into Postgres

                                                   deal: marketplace.search
                                              A ◄──────────────────── C
                                                   { offer_kind: "compute.wasm" }
                                                   returns: [B]

                            direct deal
A ──────────────────────────────────────────── B
  quote → deal → execute → receipt
```

B registers with C through the protocol. A searches C through the protocol.
A then deals with B through the protocol. Every arrow is the same deal flow.
