use crate::db::PgPool;
use froglet::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct MarketplaceReceiptsHandler {
    pub pg: Arc<PgPool>,
}

#[derive(Debug, Deserialize)]
struct ReceiptsInput {
    provider_id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default = "super::default_page_limit")]
    limit: i64,
}

#[derive(Debug, Serialize)]
struct ReceiptsResult {
    receipts: Vec<ReceiptSummary>,
    cursor: Option<String>,
    has_more: bool,
}

#[derive(Debug, Serialize)]
struct ReceiptSummary {
    receipt_hash: String,
    deal_hash: String,
    requester_id: String,
    status: String,
    created_at: String,
}


impl BuiltinServiceHandler for MarketplaceReceiptsHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: ReceiptsInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid marketplace.receipts input: {e}"))?;

            let limit = req.limit.clamp(1, 100);
            let offset: i64 = req
                .cursor
                .as_deref()
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            let fetch_limit = limit + 1;

            let client = self.pg.get().await.map_err(|e| format!("db: {e}"))?;

            let status_filter = req.status.as_deref().unwrap_or("");
            let receipt_rows = client
                .query(
                    "SELECT receipt_hash, deal_hash, requester_id, status, created_at::text
                     FROM marketplace_receipts
                     WHERE provider_id = $1
                       AND ($2 = '' OR status = $2)
                     ORDER BY created_at DESC
                     LIMIT $3 OFFSET $4",
                    &[&req.provider_id, &status_filter, &fetch_limit, &offset],
                )
                .await
                .map_err(|e| format!("receipts query: {e}"))?;

            let has_more = receipt_rows.len() as i64 > limit;
            let receipt_rows: Vec<_> = receipt_rows
                .into_iter()
                .take(limit as usize)
                .collect();

            let next_cursor = if has_more {
                Some((offset + limit).to_string())
            } else {
                None
            };

            serde_json::to_value(ReceiptsResult {
                receipts: receipt_rows
                    .iter()
                    .map(|r| ReceiptSummary {
                        receipt_hash: r.get(0),
                        deal_hash: r.get(1),
                        requester_id: r.get(2),
                        status: r.get(3),
                        created_at: r.get(4),
                    })
                    .collect(),
                cursor: next_cursor,
                has_more,
            })
            .map_err(|e| format!("serialize: {e}"))
        })
    }
}
