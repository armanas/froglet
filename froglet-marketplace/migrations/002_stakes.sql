-- Staked reputation: providers deposit non-refundable value into their identity.
-- T(provider) = total_staked_msat. Requesters check stake/deal_value > threshold.

CREATE TABLE marketplace_stakes (
    provider_id TEXT NOT NULL REFERENCES marketplace_providers(provider_id),
    total_staked_msat BIGINT NOT NULL DEFAULT 0 CHECK (total_staked_msat >= 0 AND total_staked_msat <= 9000000000000000),
    last_staked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (provider_id)
);

CREATE TABLE marketplace_stake_ledger (
    id BIGSERIAL PRIMARY KEY,
    provider_id TEXT NOT NULL,
    amount_msat BIGINT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('stake', 'topup')),
    deal_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_stake_ledger_provider ON marketplace_stake_ledger(provider_id, created_at DESC);
