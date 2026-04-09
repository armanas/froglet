pub(crate) mod projector;

use crate::config::MarketplaceConfig;
use crate::db::PgPool;
use projector::ProjectionResult;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

#[derive(Debug, Deserialize)]
struct FeedResponse {
    artifacts: Vec<FeedArtifact>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    next_cursor: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct FeedArtifact {
    cursor: i64,
    hash: String,
    kind: String,
    #[allow(dead_code)]
    actor_id: String,
    document: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ProviderOffersResponse {
    #[serde(default)]
    offers: Vec<serde_json::Value>,
}

pub async fn run(pg: Arc<PgPool>, config: MarketplaceConfig, http: reqwest::Client) {
    let mut handles = Vec::new();

    for source_url in &config.feed_sources {
        let pg = pg.clone();
        let http = http.clone();
        let interval = config.poll_interval;
        let url = source_url.clone();
        handles.push(tokio::spawn(poll_source_loop(pg, http, url, interval)));
    }

    if handles.is_empty() {
        warn!("No feed sources configured; indexer idle");
    }

    futures::future::join_all(handles).await;
}

async fn poll_source_loop(
    pg: Arc<PgPool>,
    http: reqwest::Client,
    source_url: String,
    interval: Duration,
) {
    let mut consecutive_errors: u32 = 0;

    loop {
        match poll_source_once(&pg, &http, &source_url).await {
            Ok(had_work) => {
                consecutive_errors = 0;
                if had_work {
                    continue;
                }
            }
            Err(error) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                let backoff = interval.saturating_mul(2_u32.pow(consecutive_errors.min(4)));
                let capped = backoff.min(interval * 16);
                warn!(
                    source = source_url,
                    error = %error,
                    backoff_secs = capped.as_secs(),
                    "feed poll failed"
                );
                record_poll_error(&pg, &source_url, &error).await;
                tokio::time::sleep(capped).await;
                continue;
            }
        }

        tokio::time::sleep(interval).await;
    }
}

async fn poll_source_once(
    pg: &PgPool,
    http: &reqwest::Client,
    source_url: &str,
) -> Result<bool, String> {
    let last_cursor = load_cursor(pg, source_url).await?;

    let feed_url = format!("{source_url}/v1/feed?cursor={last_cursor}&limit=100");
    let response: FeedResponse = http
        .get(&feed_url)
        .send()
        .await
        .map_err(|e| format!("feed fetch: {e}"))?
        .error_for_status()
        .map_err(|e| format!("feed status: {e}"))?
        .json()
        .await
        .map_err(|e| format!("feed parse: {e}"))?;

    let mut new_cursor = last_cursor;
    let mut projected = 0_u32;

    for artifact in &response.artifacts {
        if !verify_artifact_document(&artifact.document) {
            warn!(
                hash = artifact.hash,
                kind = artifact.kind,
                "invalid signature, skipping"
            );
            new_cursor = artifact.cursor;
            continue;
        }

        store_raw_artifact(pg, &artifact.hash, &artifact.kind, &artifact.document).await?;

        match projector::project_artifact(pg, source_url, &artifact.kind, &artifact.document).await
        {
            Ok(ProjectionResult::Projected) => projected += 1,
            Ok(ProjectionResult::Skipped) => {}
            Err(error) => {
                warn!(hash = artifact.hash, kind = artifact.kind, error = %error, "projection failed");
            }
        }
        new_cursor = artifact.cursor;
    }

    if let Some(next_cursor) = response.next_cursor && next_cursor > new_cursor {
        new_cursor = next_cursor;
    }

    let refreshed_offers = reconcile_provider_offers(pg, http, source_url).await?;

    if new_cursor > last_cursor {
        save_cursor(pg, source_url, new_cursor).await?;
    }

    if projected > 0 || refreshed_offers > 0 {
        info!(
            source = source_url,
            projected,
            refreshed_offers,
            cursor = new_cursor,
            "indexed artifacts"
        );
    }

    Ok(response.has_more)
}

fn verify_artifact_document(document: &serde_json::Value) -> bool {
    crate::verify::verify_artifact_document(document)
}

async fn reconcile_provider_offers(
    pg: &PgPool,
    http: &reqwest::Client,
    source_url: &str,
) -> Result<u32, String> {
    let offers_url = format!("{source_url}/v1/provider/offers");
    let response: ProviderOffersResponse = http
        .get(&offers_url)
        .send()
        .await
        .map_err(|e| format!("offers fetch: {e}"))?
        .error_for_status()
        .map_err(|e| format!("offers status: {e}"))?
        .json()
        .await
        .map_err(|e| format!("offers parse: {e}"))?;

    for offer in &response.offers {
        if !verify_artifact_document(offer) {
            return Err("offers snapshot contained an invalid signature".to_string());
        }
    }

    let provider_id = match projector::provider_id_for_source_url(pg, source_url).await? {
        Some(provider_id) => provider_id,
        None if response.offers.is_empty() => return Ok(0),
        None => return Err(format!("provider not indexed for source {source_url}")),
    };

    projector::replace_provider_offers(pg, &provider_id, &response.offers)
        .await
        .map(|count| count as u32)
}

async fn store_raw_artifact(
    pg: &PgPool,
    hash: &str,
    kind: &str,
    document: &serde_json::Value,
) -> Result<(), String> {
    let actor_id = document
        .get("signer")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let doc_json = document.clone();

    let client = pg.get().await.map_err(|e| format!("db: {e}"))?;
    client
        .execute(
            "INSERT INTO raw_artifacts (artifact_hash, artifact_kind, actor_id, document_json)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (artifact_hash) DO NOTHING",
            &[&hash, &kind, &actor_id, &doc_json],
        )
        .await
        .map_err(|e| format!("raw artifact insert: {e}"))?;
    Ok(())
}

async fn load_cursor(pg: &PgPool, source_url: &str) -> Result<i64, String> {
    let client = pg.get().await.map_err(|e| format!("db: {e}"))?;
    let row = client
        .query_opt(
            "SELECT last_cursor FROM indexer_cursors WHERE source_url = $1",
            &[&source_url],
        )
        .await
        .map_err(|e| format!("cursor load: {e}"))?;
    Ok(row.map(|r| r.get::<_, i64>(0)).unwrap_or(0))
}

async fn save_cursor(pg: &PgPool, source_url: &str, cursor: i64) -> Result<(), String> {
    let client = pg.get().await.map_err(|e| format!("db: {e}"))?;
    client
        .execute(
            "INSERT INTO indexer_cursors (source_url, last_cursor, last_polled_at, error_count)
             VALUES ($1, $2, NOW(), 0)
             ON CONFLICT (source_url) DO UPDATE
             SET last_cursor = $2, last_polled_at = NOW(), error_count = 0, last_error = NULL",
            &[&source_url, &cursor],
        )
        .await
        .map_err(|e| format!("cursor save: {e}"))?;
    Ok(())
}

async fn record_poll_error(pg: &PgPool, source_url: &str, error: &str) {
    let Ok(client) = pg.get().await else { return };
    let _ = client
        .execute(
            "INSERT INTO indexer_cursors (source_url, last_polled_at, error_count, last_error)
             VALUES ($1, NOW(), 1, $2)
             ON CONFLICT (source_url) DO UPDATE
             SET last_polled_at = NOW(),
                 error_count = indexer_cursors.error_count + 1,
                 last_error = $2",
            &[&source_url, &error],
        )
        .await;
}
