pub mod provider;
pub mod receipts;
pub mod register;
pub mod search;
pub mod stake;
pub mod topup;

use serde::Serialize;

pub(crate) fn default_page_limit() -> i64 {
    20
}

#[derive(Debug, Serialize, Default)]
pub(crate) struct StakeSummary {
    pub total_staked_msat: i64,
    pub last_staked_at: Option<String>,
}

/// SQL for the stake ledger insert — shared constant to keep both handlers in sync.
pub(crate) const LEDGER_INSERT_SQL: &str =
    "INSERT INTO marketplace_stake_ledger (provider_id, amount_msat, kind) VALUES ($1, $2, $3)";
