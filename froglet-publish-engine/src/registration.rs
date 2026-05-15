//! Marketplace registration + verify-indexed polling.
//!
//! After the local daemon has signed + persisted the offer in its
//! `/v1/feed`, the engine POSTs `/v1/registrations` on the marketplace
//! with the provider's public URL. The marketplace fetches the feed,
//! verifies signatures, and writes a `feed_sources` row.
//!
//! Indexer projection (`/v1/providers/<id>` returning 200) is
//! eventually-consistent; the engine polls for up to 90 seconds.

use crate::error::PublishError;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

const POLL_TIMEOUT: Duration = Duration::from_secs(90);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Serialize)]
struct RegistrationRequest<'a> {
    provider_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct RegistrationResponse {
    pub status: String,
    pub provider_id: String,
    pub provider_url: String,
    pub transport: String,
    pub descriptor_hash: String,
    #[serde(default)]
    pub offers_seen: u64,
    #[serde(default)]
    pub already_registered: bool,
}

/// POST `/v1/registrations` and return the marketplace's verification.
pub async fn register_with_marketplace(
    marketplace_url: &Url,
    provider_url: &str,
    transport_hint: Option<&str>,
) -> Result<RegistrationResponse, PublishError> {
    let mut endpoint = marketplace_url.clone();
    endpoint.set_path("/v1/registrations");

    let client = reqwest::Client::builder()
        .timeout(REGISTRATION_TIMEOUT)
        .build()?;

    let response = client
        .post(endpoint.clone())
        .json(&RegistrationRequest {
            provider_url,
            transport: transport_hint,
        })
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<empty>".to_string());
        return Err(PublishError::Registration {
            url: endpoint.to_string(),
            status: status.as_u16(),
            body,
        });
    }
    let parsed =
        response
            .json::<RegistrationResponse>()
            .await
            .map_err(|e| PublishError::Registration {
                url: endpoint.to_string(),
                status: 200,
                body: format!("response parse failed: {e}"),
            })?;
    Ok(parsed)
}

/// Poll `/v1/providers/<id>` until it returns 200 (indexer caught up)
/// or the timeout fires. Returns the number of seconds we waited.
pub async fn wait_for_indexer(
    marketplace_url: &Url,
    provider_id: &str,
) -> Result<u32, PublishError> {
    let mut endpoint = marketplace_url.clone();
    endpoint.set_path(&format!("/v1/providers/{provider_id}"));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let deadline = std::time::Instant::now() + POLL_TIMEOUT;
    let start = std::time::Instant::now();
    let mut last_status = 0u16;

    while std::time::Instant::now() < deadline {
        match client.get(endpoint.clone()).send().await {
            Ok(resp) if resp.status().is_success() => {
                let waited = start.elapsed().as_secs() as u32;
                return Ok(waited);
            }
            Ok(resp) => last_status = resp.status().as_u16(),
            Err(_) => last_status = 0,
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    Err(PublishError::Verification {
        tries: (POLL_TIMEOUT.as_secs() / POLL_INTERVAL.as_secs()) as u32,
        url: endpoint.to_string(),
        reason: format!(
            "last HTTP status: {last_status}; indexer did not project the provider within {POLL_TIMEOUT:?}"
        ),
    })
}

/// Build the canonical marketplace URL for an offer.
pub fn marketplace_offer_url(marketplace_url: &Url, offer_hash: &str) -> String {
    let mut url = marketplace_url.clone();
    url.set_path(&format!("/v1/offers/{offer_hash}"));
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_url_builds_correctly() {
        let base = Url::parse("https://marketplace.froglet.dev").unwrap();
        let url = marketplace_offer_url(&base, "abc123");
        assert_eq!(url, "https://marketplace.froglet.dev/v1/offers/abc123");
    }
}
