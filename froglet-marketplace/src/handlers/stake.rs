use crate::db::PgPool;
use froglet::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct MarketplaceStakeHandler {
    pub pg: Arc<PgPool>,
}

#[derive(Debug, Deserialize)]
struct StakeInput {
    provider_id: String,
    amount_msat: i64,
    /// Injected by the execution engine from the deal's requester_id.
    /// Used to verify the caller is the provider itself.
    #[serde(default)]
    _caller_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct StakeResult {
    provider_id: String,
    total_staked_msat: i64,
    amount_msat: i64,
    kind: String,
    status: String,
}

impl BuiltinServiceHandler for MarketplaceStakeHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: StakeInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid marketplace.stake input: {e}"))?;

            if req.amount_msat <= 0 {
                return Err("amount_msat must be positive".to_string());
            }
            if req.amount_msat > 1_000_000_000_000 {
                return Err("amount_msat exceeds maximum (1,000,000,000,000 msat / 1B sats)".to_string());
            }

            // Authorization: only the provider itself can stake on its identity.
            // _caller_id is injected by the execution engine from the deal's requester_id.
            match &req._caller_id {
                Some(caller) if caller == &req.provider_id => {}
                Some(_) => return Err("caller is not the provider; only a provider can stake on its own identity".to_string()),
                None => return Err("stake requires a deal context with verified caller identity".to_string()),
            }

            let mut client = self.pg.get().await.map_err(|e| format!("db: {e}"))?;
            let txn = client.transaction().await.map_err(|e| format!("db txn: {e}"))?;

            // Verify provider exists
            let exists = txn
                .query_opt(
                    "SELECT 1 FROM marketplace_providers WHERE provider_id = $1",
                    &[&req.provider_id],
                )
                .await
                .map_err(|e| format!("provider check: {e}"))?;

            if exists.is_none() {
                return Err(format!("provider {} not found", req.provider_id));
            }

            // Upsert stake total
            let row = txn
                .query_one(
                    "INSERT INTO marketplace_stakes (provider_id, total_staked_msat, last_staked_at)
                     VALUES ($1, $2, NOW())
                     ON CONFLICT (provider_id) DO UPDATE
                     SET total_staked_msat = marketplace_stakes.total_staked_msat + $2,
                         last_staked_at = NOW()
                     RETURNING total_staked_msat",
                    &[&req.provider_id, &req.amount_msat],
                )
                .await
                .map_err(|e| format!("stake upsert: {e}"))?;

            // Record ledger entry
            txn.execute(super::LEDGER_INSERT_SQL, &[&req.provider_id, &req.amount_msat, &"stake"])
                .await.map_err(|e| format!("ledger insert: {e}"))?;

            txn.commit().await.map_err(|e| format!("commit: {e}"))?;

            let total: i64 = row.get(0);

            serde_json::to_value(StakeResult {
                provider_id: req.provider_id,
                total_staked_msat: total,
                amount_msat: req.amount_msat,
                kind: "stake".to_string(),
                status: "staked".to_string(),
            })
            .map_err(|e| format!("serialize: {e}"))
        })
    }
}
