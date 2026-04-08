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
    #[allow(dead_code)]
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

pub async fn run(pg: Arc<PgPool>, config: MarketplaceConfig, http: reqwest::Client) {
    let sources = config.feed_sources.clone();

    // Track active source URLs so discovery doesn't duplicate static ones
    let active_sources: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>> =
        Arc::new(tokio::sync::Mutex::new(sources.iter().cloned().collect()));

    let mut handles = Vec::new();

    // Spawn pollers for statically configured sources
    for source_url in sources {
        let pg = pg.clone();
        let http = http.clone();
        let interval = config.poll_interval;
        handles.push(tokio::spawn(
            poll_source_loop(pg, http, source_url, interval),
        ));
    }

    // Spawn dynamic discovery loop if discovery URL is configured
    if let Some(discovery_url) = config.discovery_url.clone() {
        let pg = pg.clone();
        let http = http.clone();
        let interval = config.poll_interval;
        let active = active_sources.clone();
        let max_sources = config.max_dynamic_sources;
        handles.push(tokio::spawn(async move {
            discover_and_poll_loop(pg, http, discovery_url, interval, active, max_sources).await;
        }));
        info!("Dynamic source discovery enabled");
    }

    if handles.is_empty() {
        warn!("No feed sources and no discovery URL configured; indexer idle");
    }

    futures::future::join_all(handles).await;
}

/// Periodically query the reference discovery server, discover new provider
/// URLs, and spawn pollers for them.
async fn discover_and_poll_loop(
    pg: Arc<PgPool>,
    http: reqwest::Client,
    discovery_url: String,
    poll_interval: Duration,
    active_sources: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    max_dynamic_sources: usize,
) {
    let discovery_interval = poll_interval * 4;

    loop {
        match discover_providers(&http, &discovery_url).await {
            Ok(urls) => {
                let mut active = active_sources.lock().await;
                for url in urls {
                    if active.len() >= max_dynamic_sources {
                        warn!(
                            max = max_dynamic_sources,
                            "dynamic source cap reached, skipping new providers"
                        );
                        break;
                    }
                    if active.insert(url.clone()) {
                        info!(source = url, "discovered new provider, starting poller");
                        let pg = pg.clone();
                        let http = http.clone();
                        let interval = poll_interval;
                        tokio::spawn(poll_source_loop(pg, http, url, interval));
                    }
                }
            }
            Err(error) => {
                warn!(error = %error, "discovery query failed");
            }
        }

        tokio::time::sleep(discovery_interval).await;
    }
}

/// Query the reference discovery server for active providers and extract
/// their clearnet/onion URLs.
async fn discover_providers(
    http: &reqwest::Client,
    discovery_url: &str,
) -> Result<Vec<String>, String> {
    let search_url = format!("{discovery_url}/v1/discovery/search");
    let response: serde_json::Value = http
        .post(&search_url)
        .json(&serde_json::json!({ "limit": 200 }))
        .send()
        .await
        .map_err(|e| format!("discovery search: {e}"))?
        .error_for_status()
        .map_err(|e| format!("discovery status: {e}"))?
        .json()
        .await
        .map_err(|e| format!("discovery parse: {e}"))?;

    let nodes = response
        .get("nodes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut urls = Vec::new();
    for node in &nodes {
        let transports = node.get("descriptor").and_then(|d| d.get("transports"));
        // Prefer clearnet, fall back to onion
        if let Some(url) = transports
            .and_then(|t| t.get("clearnet_url"))
            .and_then(|v| v.as_str())
        {
            if !url.is_empty() {
                urls.push(url.to_string());
            }
        } else if let Some(url) = transports
            .and_then(|t| t.get("onion_url"))
            .and_then(|v| v.as_str())
        {
            if !url.is_empty() {
                urls.push(url.to_string());
            }
        }
    }

    Ok(urls)
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
            warn!(hash = artifact.hash, kind = artifact.kind, "invalid signature, skipping");
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

    if new_cursor > last_cursor {
        save_cursor(pg, source_url, new_cursor).await?;
    }

    if projected > 0 {
        info!(source = source_url, projected, cursor = new_cursor, "indexed artifacts");
    }

    Ok(response.has_more)
}

fn verify_artifact_document(document: &serde_json::Value) -> bool {
    crate::verify::verify_artifact_document(document)
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
