use crate::db::PgPool;
use froglet::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct MarketplaceTopupHandler {
    pub pg: Arc<PgPool>,
}

#[derive(Debug, Deserialize)]
struct TopupInput {
    provider_id: String,
    amount_msat: i64,
    /// Injected by the execution engine from the deal's requester_id.
    #[serde(default)]
    _caller_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct TopupResult {
    provider_id: String,
    total_staked_msat: i64,
    topup_amount_msat: i64,
    kind: String,
    status: String,
}

impl BuiltinServiceHandler for MarketplaceTopupHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: TopupInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid marketplace.topup input: {e}"))?;

            if req.amount_msat <= 0 {
                return Err("amount_msat must be positive".to_string());
            }
            if req.amount_msat > 1_000_000_000_000 {
                return Err(
                    "amount_msat exceeds maximum (1,000,000,000,000 msat / 1B sats)".to_string(),
                );
            }

            // Authorization: only the provider itself can top up its stake.
            match &req._caller_id {
                Some(caller) if caller == &req.provider_id => {}
                Some(_) => {
                    return Err(
                        "caller is not the provider; only a provider can top up its own stake"
                            .to_string(),
                    );
                }
                None => {
                    return Err(
                        "topup requires a deal context with verified caller identity".to_string(),
                    );
                }
            }

            let mut client = self.pg.get().await.map_err(|e| format!("db: {e}"))?;
            let txn = client
                .transaction()
                .await
                .map_err(|e| format!("db txn: {e}"))?;

            // Provider must have an existing stake
            let row = txn
                .query_opt(
                    "UPDATE marketplace_stakes
                     SET total_staked_msat = total_staked_msat + $2,
                         last_staked_at = NOW()
                     WHERE provider_id = $1
                     RETURNING total_staked_msat",
                    &[&req.provider_id, &req.amount_msat],
                )
                .await
                .map_err(|e| format!("topup update: {e}"))?;

            let Some(row) = row else {
                return Err(format!(
                    "no existing stake for provider {}; use marketplace.stake first",
                    req.provider_id
                ));
            };

            // Record ledger entry
            txn.execute(
                super::LEDGER_INSERT_SQL,
                &[&req.provider_id, &req.amount_msat, &"topup"],
            )
            .await
            .map_err(|e| format!("ledger insert: {e}"))?;

            txn.commit().await.map_err(|e| format!("commit: {e}"))?;

            let total: i64 = row.get(0);

            serde_json::to_value(TopupResult {
                provider_id: req.provider_id,
                total_staked_msat: total,
                topup_amount_msat: req.amount_msat,
                kind: "topup".to_string(),
                status: "topped_up".to_string(),
            })
            .map_err(|e| format!("serialize: {e}"))
        })
    }
}
