use crate::db::PgPool;
use froglet::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct MarketplaceSearchHandler {
    pub pg: Arc<PgPool>,
}

#[derive(Debug, Deserialize)]
struct SearchInput {
    #[serde(default)]
    offer_kind: Option<String>,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    max_price_sats: Option<i64>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default = "super::default_page_limit")]
    limit: i64,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    providers: Vec<ProviderWithOffers>,
    cursor: Option<String>,
    has_more: bool,
}

#[derive(Debug, Serialize)]
struct ProviderWithOffers {
    provider_id: String,
    descriptor_hash: String,
    transport_endpoints: Value,
    offers: Vec<OfferSummary>,
    last_seen_at: String,
}

#[derive(Debug, Serialize)]
struct OfferSummary {
    offer_hash: String,
    offer_id: String,
    offer_kind: String,
    runtime: String,
    base_fee_msat: i64,
    success_fee_msat: i64,
    execution_profile: Value,
}

impl BuiltinServiceHandler for MarketplaceSearchHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: SearchInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid marketplace.search input: {e}"))?;

            let limit = req.limit.clamp(1, 100);
            let offset: i64 = req
                .cursor
                .as_deref()
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            let fetch_limit = limit + 1;

            let client = self.pg.get().await.map_err(|e| format!("db: {e}"))?;

            // Fetch providers (with optional offer filters via subquery)
            let provider_rows = if req.offer_kind.is_some()
                || req.runtime.is_some()
                || req.max_price_sats.is_some()
            {
                let offer_kind = req.offer_kind.as_deref().unwrap_or("");
                let runtime = req.runtime.as_deref().unwrap_or("");
                let max_msat = match req.max_price_sats {
                    Some(p) => p.checked_mul(1000).ok_or("max_price_sats overflow")?,
                    None => i64::MAX,
                };

                client
                    .query(
                        "SELECT DISTINCT p.provider_id, p.descriptor_hash,
                                p.transport_endpoints, p.last_seen_at::text
                         FROM marketplace_providers p
                         JOIN marketplace_offers o ON o.provider_id = p.provider_id
                         WHERE ($1 = '' OR o.offer_kind = $1)
                           AND ($2 = '' OR o.runtime = $2)
                           AND (o.base_fee_msat + o.success_fee_msat) <= $3
                         ORDER BY p.last_seen_at DESC
                         LIMIT $4 OFFSET $5",
                        &[&offer_kind, &runtime, &max_msat, &fetch_limit, &offset],
                    )
                    .await
                    .map_err(|e| format!("search query: {e}"))?
            } else {
                client
                    .query(
                        "SELECT provider_id, descriptor_hash, transport_endpoints, last_seen_at::text
                         FROM marketplace_providers
                         ORDER BY last_seen_at DESC
                         LIMIT $1 OFFSET $2",
                        &[&fetch_limit, &offset],
                    )
                    .await
                    .map_err(|e| format!("search query: {e}"))?
            };

            let has_more = provider_rows.len() as i64 > limit;
            let provider_rows: Vec<_> = provider_rows.into_iter().take(limit as usize).collect();

            // Fan-in: fetch all offers for the result set in a single query
            let provider_ids: Vec<&str> =
                provider_rows.iter().map(|r| r.get::<_, &str>(0)).collect();

            let offer_rows = if provider_ids.is_empty() {
                Vec::new()
            } else {
                client
                    .query(
                        "SELECT provider_id, offer_hash, offer_id, offer_kind, runtime,
                                base_fee_msat, success_fee_msat, execution_profile
                         FROM marketplace_offers
                         WHERE provider_id = ANY($1)
                         ORDER BY provider_id, created_at DESC",
                        &[&provider_ids],
                    )
                    .await
                    .map_err(|e| format!("offers query: {e}"))?
            };

            // Group offers by provider_id
            let mut offers_by_provider: std::collections::HashMap<String, Vec<OfferSummary>> =
                std::collections::HashMap::new();
            for row in &offer_rows {
                let pid: String = row.get(0);
                offers_by_provider
                    .entry(pid)
                    .or_default()
                    .push(OfferSummary {
                        offer_hash: row.get(1),
                        offer_id: row.get(2),
                        offer_kind: row.get(3),
                        runtime: row.get(4),
                        base_fee_msat: row.get(5),
                        success_fee_msat: row.get(6),
                        execution_profile: row.get(7),
                    });
            }

            let mut results = Vec::with_capacity(provider_rows.len());
            for row in &provider_rows {
                let provider_id: String = row.get(0);
                results.push(ProviderWithOffers {
                    descriptor_hash: row.get(1),
                    transport_endpoints: row.get(2),
                    last_seen_at: row.get(3),
                    offers: offers_by_provider.remove(&provider_id).unwrap_or_default(),
                    provider_id,
                });
            }

            let next_cursor = if has_more {
                Some((offset + limit).to_string())
            } else {
                None
            };

            serde_json::to_value(SearchResult {
                providers: results,
                cursor: next_cursor,
                has_more,
            })
            .map_err(|e| format!("serialize: {e}"))
        })
    }
}
