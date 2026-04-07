-- Froglet Marketplace schema v1
-- Cursor-based indexer state, provider/offer/receipt projections, trust aggregates.

CREATE TABLE indexer_cursors (
    source_url    TEXT PRIMARY KEY,
    last_cursor   BIGINT NOT NULL DEFAULT 0,
    last_polled_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    error_count   INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT
);

CREATE TABLE marketplace_providers (
    provider_id        TEXT PRIMARY KEY,
    descriptor_hash    TEXT NOT NULL,
    descriptor_seq     BIGINT NOT NULL DEFAULT 0,
    protocol_version   TEXT NOT NULL DEFAULT '',
    transport_endpoints JSONB NOT NULL DEFAULT '[]',
    linked_identities  JSONB NOT NULL DEFAULT '[]',
    capabilities       JSONB NOT NULL DEFAULT '{}',
    source_url         TEXT NOT NULL,
    first_seen_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    descriptor_json    JSONB NOT NULL
);

CREATE INDEX idx_providers_last_seen ON marketplace_providers (last_seen_at DESC);

CREATE TABLE marketplace_offers (
    offer_hash        TEXT PRIMARY KEY,
    provider_id       TEXT NOT NULL REFERENCES marketplace_providers(provider_id) ON DELETE CASCADE,
    offer_id          TEXT NOT NULL,
    descriptor_hash   TEXT NOT NULL,
    offer_kind        TEXT NOT NULL,
    runtime           TEXT NOT NULL,
    package_kind      TEXT NOT NULL DEFAULT '',
    contract_version  TEXT NOT NULL DEFAULT '',
    settlement_method TEXT NOT NULL DEFAULT '',
    base_fee_msat     BIGINT NOT NULL DEFAULT 0,
    success_fee_msat  BIGINT NOT NULL DEFAULT 0,
    execution_profile JSONB NOT NULL DEFAULT '{}',
    expires_at        TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    offer_json        JSONB NOT NULL
);

CREATE INDEX idx_offers_provider ON marketplace_offers (provider_id);
CREATE INDEX idx_offers_kind ON marketplace_offers (offer_kind);
CREATE INDEX idx_offers_runtime ON marketplace_offers (runtime);
CREATE INDEX idx_offers_price ON marketplace_offers ((base_fee_msat + success_fee_msat));

CREATE TABLE marketplace_receipts (
    receipt_hash   TEXT PRIMARY KEY,
    provider_id    TEXT NOT NULL,
    deal_hash      TEXT NOT NULL,
    quote_hash     TEXT NOT NULL DEFAULT '',
    requester_id   TEXT NOT NULL DEFAULT '',
    status         TEXT NOT NULL,
    workload_kind  TEXT NOT NULL DEFAULT '',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    receipt_json   JSONB NOT NULL
);

CREATE INDEX idx_receipts_provider ON marketplace_receipts (provider_id, created_at DESC);
CREATE INDEX idx_receipts_status ON marketplace_receipts (provider_id, status);

-- Raw artifact store for deduplication and replay
CREATE TABLE raw_artifacts (
    artifact_hash  TEXT PRIMARY KEY,
    artifact_kind  TEXT NOT NULL,
    actor_id       TEXT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    document_json  JSONB NOT NULL
);
