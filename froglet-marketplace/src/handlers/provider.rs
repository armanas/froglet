use crate::db::PgPool;
use froglet::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct MarketplaceProviderHandler {
    pub pg: Arc<PgPool>,
}

#[derive(Debug, Deserialize)]
struct ProviderInput {
    provider_id: String,
}

#[derive(Debug, Serialize)]
struct ProviderResult {
    provider: Option<ProviderDetail>,
}

#[derive(Debug, Serialize)]
struct ProviderDetail {
    provider_id: String,
    descriptor_hash: String,
    descriptor_seq: i64,
    protocol_version: String,
    transport_endpoints: Value,
    linked_identities: Value,
    capabilities: Value,
    first_seen_at: String,
    last_seen_at: String,
    offers: Vec<OfferDetail>,
    stake: super::StakeSummary,
}

#[derive(Debug, Serialize)]
struct OfferDetail {
    offer_hash: String,
    offer_id: String,
    offer_kind: String,
    runtime: String,
    settlement_method: String,
    base_fee_msat: i64,
    success_fee_msat: i64,
    execution_profile: Value,
}

impl BuiltinServiceHandler for MarketplaceProviderHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: ProviderInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid marketplace.provider input: {e}"))?;

            let client = self.pg.get().await.map_err(|e| format!("db: {e}"))?;

            let row = client
                .query_opt(
                    "SELECT provider_id, descriptor_hash, descriptor_seq, protocol_version,
                            transport_endpoints, linked_identities, capabilities,
                            first_seen_at::text, last_seen_at::text
                     FROM marketplace_providers WHERE provider_id = $1",
                    &[&req.provider_id],
                )
                .await
                .map_err(|e| format!("provider query: {e}"))?;

            let Some(row) = row else {
                return serde_json::to_value(ProviderResult { provider: None })
                    .map_err(|e| e.to_string());
            };

            let provider_id: String = row.get(0);

            let offer_rows = client
                .query(
                    "SELECT offer_hash, offer_id, offer_kind, runtime, settlement_method,
                            base_fee_msat, success_fee_msat, execution_profile
                     FROM marketplace_offers WHERE provider_id = $1
                     ORDER BY created_at DESC",
                    &[&provider_id],
                )
                .await
                .map_err(|e| format!("offers query: {e}"))?;

            let stake_row = client
                .query_opt(
                    "SELECT total_staked_msat, last_staked_at::text
                     FROM marketplace_stakes WHERE provider_id = $1",
                    &[&provider_id],
                )
                .await
                .map_err(|e| format!("stake query: {e}"))?;

            let detail = ProviderDetail {
                provider_id,
                descriptor_hash: row.get(1),
                descriptor_seq: row.get(2),
                protocol_version: row.get(3),
                transport_endpoints: row.get(4),
                linked_identities: row.get(5),
                capabilities: row.get(6),
                first_seen_at: row.get(7),
                last_seen_at: row.get(8),
                offers: offer_rows
                    .iter()
                    .map(|r| OfferDetail {
                        offer_hash: r.get(0),
                        offer_id: r.get(1),
                        offer_kind: r.get(2),
                        runtime: r.get(3),
                        settlement_method: r.get(4),
                        base_fee_msat: r.get(5),
                        success_fee_msat: r.get(6),
                        execution_profile: r.get(7),
                    })
                    .collect(),
                stake: stake_row
                    .as_ref()
                    .map(|r| super::StakeSummary {
                        total_staked_msat: r.get(0),
                        last_staked_at: r.get(1),
                    })
                    .unwrap_or_default(),
            };

            serde_json::to_value(ProviderResult {
                provider: Some(detail),
            })
            .map_err(|e| format!("serialize: {e}"))
        })
    }
}
