//! Requester-side spend ledger.
//!
//! Node-local accounting that backs the requester spend policy
//! ([`crate::config::RequesterSpendConfig`]): every paid deal this node
//! commits to is recorded here, and the cumulative budget check happens
//! atomically against this table inside the single write connection — so
//! concurrent deal creations cannot oversubscribe the budget.
//!
//! Accounting policy: the full quoted total (base + success fee) is counted
//! at admission for every settlement rail. For bundle deals whose success fee
//! is never released this over-counts conservatively; receipt-based
//! reconciliation is deliberately out of scope.
//!
//! Row states: `reserved` (deal admission in flight), `committed` (deal
//! persisted; counts against the budget), `archived` (cleared by an explicit
//! budget reset; no longer counted). Reserved and committed rows both count
//! against the budget; archived rows are kept for audit.

use crate::config::RequesterSpendConfig;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// Outcome of a reservation attempt against the spend policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendDecision {
    /// Within policy; a `reserved` row now holds the amount.
    Reserved,
    /// No cumulative budget is configured — paid deals are refused
    /// (fail-closed).
    Unconfigured,
    /// The single deal exceeds the per-deal cap.
    CapExceeded { max_deal_msat: u64 },
    /// The cumulative budget cannot absorb this deal.
    BudgetExceeded {
        spend_budget_msat: u64,
        spent_msat: u64,
        remaining_msat: u64,
    },
}

/// Current ledger totals, as exposed by `GET /v1/runtime/spend`.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct SpendTotals {
    pub reserved_msat: u64,
    pub committed_msat: u64,
}

fn outstanding_msat(conn: &Connection) -> Result<u64, String> {
    let sum: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(amount_msat), 0) FROM requester_spend_ledger \
             WHERE state IN ('reserved', 'committed')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("spend ledger sum failed: {e}"))?;
    Ok(sum.max(0) as u64)
}

/// Atomically check the spend policy and reserve `amount_msat` for
/// `deal_hash`. Must run inside the DB's single write connection so the
/// read-sum + insert pair is serialized against concurrent deal creations.
///
/// Re-reserving the same `deal_hash` is idempotent and does not double-count.
/// Callers skip this entirely for free deals (`amount_msat == 0`).
pub fn try_reserve_spend(
    conn: &Connection,
    policy: &RequesterSpendConfig,
    deal_hash: &str,
    provider_id: &str,
    amount_msat: u64,
    settlement_method: &str,
    now: i64,
) -> Result<SpendDecision, String> {
    let Some(budget) = policy.spend_budget_msat else {
        return Ok(SpendDecision::Unconfigured);
    };
    if let Some(cap) = policy.max_deal_msat
        && amount_msat > cap
    {
        return Ok(SpendDecision::CapExceeded { max_deal_msat: cap });
    }

    // Idempotent replay of the same deal artifact: the amount is already held.
    let existing: Option<String> = conn
        .query_row(
            "SELECT state FROM requester_spend_ledger WHERE deal_hash = ?1",
            params![deal_hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("spend ledger lookup failed: {e}"))?;
    if let Some(state) = existing {
        if state == "archived" {
            // A reset archived this hash; treat the replay as a fresh spend.
            conn.execute(
                "UPDATE requester_spend_ledger \
                 SET state = 'reserved', updated_at = ?2 WHERE deal_hash = ?1",
                params![deal_hash, now],
            )
            .map_err(|e| format!("spend ledger re-reserve failed: {e}"))?;
        }
        return Ok(SpendDecision::Reserved);
    }

    let spent = outstanding_msat(conn)?;
    let remaining = budget.saturating_sub(spent);
    if amount_msat > remaining {
        return Ok(SpendDecision::BudgetExceeded {
            spend_budget_msat: budget,
            spent_msat: spent,
            remaining_msat: remaining,
        });
    }

    conn.execute(
        "INSERT INTO requester_spend_ledger \
         (deal_hash, deal_id, provider_id, amount_msat, settlement_method, state, created_at, updated_at) \
         VALUES (?1, NULL, ?2, ?3, ?4, 'reserved', ?5, ?5)",
        params![deal_hash, provider_id, amount_msat as i64, settlement_method, now],
    )
    .map_err(|e| format!("spend ledger reserve failed: {e}"))?;
    Ok(SpendDecision::Reserved)
}

/// Flip a reservation to `committed` once the deal is persisted, recording
/// the provider-assigned deal id.
pub fn commit_spend(
    conn: &Connection,
    deal_hash: &str,
    deal_id: &str,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE requester_spend_ledger \
         SET state = 'committed', deal_id = ?2, updated_at = ?3 \
         WHERE deal_hash = ?1 AND state = 'reserved'",
        params![deal_hash, deal_id, now],
    )
    .map_err(|e| format!("spend ledger commit failed: {e}"))?;
    Ok(())
}

/// Drop a reservation after a failed deal-creation attempt, freeing the
/// held budget. Committed rows are never released here.
pub fn release_spend(conn: &Connection, deal_hash: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM requester_spend_ledger \
         WHERE deal_hash = ?1 AND state = 'reserved'",
        params![deal_hash],
    )
    .map_err(|e| format!("spend ledger release failed: {e}"))?;
    Ok(())
}

/// Archive all committed spend, restoring budget headroom. In-flight
/// (`reserved`) rows are left alone. Returns the number of archived rows.
pub fn reset_spend(conn: &Connection, now: i64) -> Result<usize, String> {
    conn.execute(
        "UPDATE requester_spend_ledger \
         SET state = 'archived', updated_at = ?1 WHERE state = 'committed'",
        params![now],
    )
    .map_err(|e| format!("spend ledger reset failed: {e}"))
}

/// Current reserved / committed totals.
pub fn spend_totals(conn: &Connection) -> Result<SpendTotals, String> {
    let mut totals = SpendTotals::default();
    let mut stmt = conn
        .prepare(
            "SELECT state, COALESCE(SUM(amount_msat), 0) FROM requester_spend_ledger \
             WHERE state IN ('reserved', 'committed') GROUP BY state",
        )
        .map_err(|e| format!("spend ledger totals failed: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| format!("spend ledger totals failed: {e}"))?;
    for row in rows {
        let (state, sum) = row.map_err(|e| format!("spend ledger totals failed: {e}"))?;
        let sum = sum.max(0) as u64;
        match state.as_str() {
            "reserved" => totals.reserved_msat = sum,
            "committed" => totals.committed_msat = sum,
            _ => {}
        }
    }
    Ok(totals)
}

/// Release reservations older than `max_age_secs`. Reservations only outlive
/// a deal-creation attempt when the process died mid-flight; leaked rows are
/// fail-closed (they consume budget) until this sweep or a manual release.
pub fn sweep_stale_reservations(
    conn: &Connection,
    max_age_secs: i64,
    now: i64,
) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM requester_spend_ledger \
         WHERE state = 'reserved' AND updated_at < ?1",
        params![now - max_age_secs],
    )
    .map_err(|e| format!("spend ledger sweep failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        crate::db::initialize_db_for_connection(&conn).expect("init schema");
        conn
    }

    fn policy(max_deal: Option<u64>, budget: Option<u64>) -> RequesterSpendConfig {
        RequesterSpendConfig {
            max_deal_msat: max_deal,
            spend_budget_msat: budget,
        }
    }

    #[test]
    fn unconfigured_budget_refuses_paid_spend() {
        let conn = test_conn();
        let decision = try_reserve_spend(&conn, &policy(None, None), "h1", "p1", 1, "lightning", 0)
            .expect("reserve");
        assert_eq!(decision, SpendDecision::Unconfigured);
        assert_eq!(spend_totals(&conn).unwrap().reserved_msat, 0);
    }

    #[test]
    fn per_deal_cap_boundary() {
        let conn = test_conn();
        let p = policy(Some(100), Some(1_000));
        // == cap allowed
        assert_eq!(
            try_reserve_spend(&conn, &p, "h-at-cap", "p1", 100, "lightning", 0).unwrap(),
            SpendDecision::Reserved
        );
        // cap + 1 refused, no ledger row
        assert_eq!(
            try_reserve_spend(&conn, &p, "h-over-cap", "p1", 101, "lightning", 0).unwrap(),
            SpendDecision::CapExceeded { max_deal_msat: 100 }
        );
        assert_eq!(spend_totals(&conn).unwrap().reserved_msat, 100);
    }

    #[test]
    fn budget_exhaustion_reports_remaining() {
        let conn = test_conn();
        let p = policy(None, Some(1_000));
        assert_eq!(
            try_reserve_spend(&conn, &p, "h1", "p1", 950, "lightning", 0).unwrap(),
            SpendDecision::Reserved
        );
        let decision = try_reserve_spend(&conn, &p, "h2", "p1", 100, "lightning", 0).unwrap();
        assert_eq!(
            decision,
            SpendDecision::BudgetExceeded {
                spend_budget_msat: 1_000,
                spent_msat: 950,
                remaining_msat: 50,
            }
        );
    }

    #[test]
    fn reserved_and_committed_both_count_archived_does_not() {
        let conn = test_conn();
        let p = policy(None, Some(1_000));
        try_reserve_spend(&conn, &p, "h1", "p1", 400, "lightning", 0).unwrap();
        commit_spend(&conn, "h1", "deal-1", 1).unwrap();
        try_reserve_spend(&conn, &p, "h2", "p1", 300, "lightning", 2).unwrap();
        let totals = spend_totals(&conn).unwrap();
        assert_eq!(totals.committed_msat, 400);
        assert_eq!(totals.reserved_msat, 300);

        // Reset archives committed but leaves the in-flight reservation.
        assert_eq!(reset_spend(&conn, 3).unwrap(), 1);
        let totals = spend_totals(&conn).unwrap();
        assert_eq!(totals.committed_msat, 0);
        assert_eq!(totals.reserved_msat, 300);

        // Freed headroom is usable again.
        assert_eq!(
            try_reserve_spend(&conn, &p, "h3", "p1", 700, "lightning", 4).unwrap(),
            SpendDecision::Reserved
        );
    }

    #[test]
    fn release_frees_headroom() {
        let conn = test_conn();
        let p = policy(None, Some(500));
        try_reserve_spend(&conn, &p, "h1", "p1", 500, "lightning", 0).unwrap();
        release_spend(&conn, "h1").unwrap();
        assert_eq!(
            try_reserve_spend(&conn, &p, "h2", "p1", 500, "lightning", 1).unwrap(),
            SpendDecision::Reserved
        );
        // Release never drops committed rows.
        commit_spend(&conn, "h2", "deal-2", 2).unwrap();
        release_spend(&conn, "h2").unwrap();
        assert_eq!(spend_totals(&conn).unwrap().committed_msat, 500);
    }

    #[test]
    fn same_deal_hash_does_not_double_count() {
        let conn = test_conn();
        let p = policy(None, Some(1_000));
        try_reserve_spend(&conn, &p, "h1", "p1", 600, "lightning", 0).unwrap();
        // Replay of the same deal artifact is idempotent…
        assert_eq!(
            try_reserve_spend(&conn, &p, "h1", "p1", 600, "lightning", 1).unwrap(),
            SpendDecision::Reserved
        );
        // …and held only once.
        assert_eq!(spend_totals(&conn).unwrap().reserved_msat, 600);
    }

    #[test]
    fn sweep_releases_only_stale_reservations() {
        let conn = test_conn();
        let p = policy(None, Some(1_000));
        try_reserve_spend(&conn, &p, "h-old", "p1", 100, "lightning", 0).unwrap();
        try_reserve_spend(&conn, &p, "h-new", "p1", 100, "lightning", 5_000).unwrap();
        try_reserve_spend(&conn, &p, "h-done", "p1", 100, "lightning", 0).unwrap();
        commit_spend(&conn, "h-done", "deal-3", 0).unwrap();

        // now = 5_100, max age 3_600: only h-old (updated_at 0) is stale.
        assert_eq!(sweep_stale_reservations(&conn, 3_600, 5_100).unwrap(), 1);
        let totals = spend_totals(&conn).unwrap();
        assert_eq!(totals.reserved_msat, 100);
        assert_eq!(totals.committed_msat, 100);
    }
}
