use crate::db::PgPool;
use crate::indexer::projector;
use froglet::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{info, warn};

pub struct MarketplaceRegisterHandler {
    pub pg: Arc<PgPool>,
}

#[derive(Debug, Deserialize)]
struct RegisterInput {
    /// The signed descriptor artifact (full document)
    descriptor: Value,
    /// Signed offer artifacts (full documents)
    #[serde(default)]
    offers: Vec<Value>,
    /// The provider's feed URL for ongoing polling (optional)
    #[serde(default)]
    feed_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct RegisterResult {
    registered: bool,
    provider_id: String,
    descriptor_hash: String,
    offers_indexed: usize,
}

impl BuiltinServiceHandler for MarketplaceRegisterHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: RegisterInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid marketplace.register input: {e}"))?;

            // Verify the descriptor signature
            if !verify_artifact_document(&req.descriptor) {
                return Err("descriptor has invalid signature".to_string());
            }

            let provider_id = req
                .descriptor
                .get("payload")
                .and_then(|p| p.get("provider_id"))
                .and_then(|v| v.as_str())
                .ok_or("descriptor missing provider_id")?
                .to_string();
            let descriptor_hash = req
                .descriptor
                .get("hash")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Determine source_url: use provided feed_url or derive from descriptor
            let source_url = req
                .feed_url
                .clone()
                .or_else(|| {
                    req.descriptor
                        .get("payload")
                        .and_then(|p| p.get("transport_endpoints"))
                        .and_then(|t| t.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|e| e.get("uri"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .unwrap_or_default();

            // Project the descriptor
            projector::project_artifact(&self.pg, &source_url, "descriptor", &req.descriptor)
                .await
                .map_err(|e| format!("descriptor projection: {e}"))?;

            // Verify and project each offer
            let mut offers_indexed = 0_usize;
            for offer in &req.offers {
                if !verify_artifact_document(offer) {
                    warn!(
                        provider_id = provider_id,
                        "skipping offer with invalid signature"
                    );
                    continue;
                }

                // Verify offer belongs to this provider
                let offer_provider = offer
                    .get("payload")
                    .and_then(|p| p.get("provider_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if offer_provider != provider_id {
                    warn!(
                        expected = provider_id,
                        got = offer_provider,
                        "skipping offer from different provider"
                    );
                    continue;
                }

                match projector::project_artifact(&self.pg, &source_url, "offer", offer).await {
                    Ok(_) => offers_indexed += 1,
                    Err(e) => {
                        warn!(provider_id = provider_id, error = %e, "offer projection failed");
                    }
                }
            }

            info!(
                provider_id = provider_id,
                offers = offers_indexed,
                "provider registered"
            );

            serde_json::to_value(RegisterResult {
                registered: true,
                provider_id,
                descriptor_hash,
                offers_indexed,
            })
            .map_err(|e| format!("serialize: {e}"))
        })
    }
}

fn verify_artifact_document(document: &Value) -> bool {
    crate::verify::verify_artifact_document(document)
}
