//! `demo.fetch-witness` — single-provider URL witness.
//!
//! The agent supplies a URL. The provider fetches it, hashes the body, and
//! returns the hash plus metadata. The kernel then signs the receipt over
//! that output, so the buyer (or a downstream auditor) can later prove
//! "provider P claimed URL U served content with SHA-256 H at time T."
//!
//! Cell on the matrix: **Out-of-reach × Trust** — the agent doesn't have to
//! make the request itself, and gets a signed attestation of what the
//! provider saw.
//!
//! Input shape:
//! ```json
//! { "url": "https://example.com/", "max_bytes": 1048576 }
//! ```
//! `max_bytes` is optional and clamped to the global ceiling.
//!
//! Output shape:
//! ```json
//! {
//!   "url": "https://example.com/",
//!   "final_url": "https://example.com/",
//!   "status_code": 200,
//!   "content_type": "text/html; charset=UTF-8",
//!   "content_length": 1256,
//!   "content_sha256": "ea8f...",
//!   "fetched_at_ms": 1714123456789
//! }
//! ```
//!
//! Non-2xx responses still succeed at the protocol level — the buyer gets the
//! status code and decides whether to release the success-fee leg. The
//! provider's slashable claim is "this is what the URL served", not "the URL
//! returned 200".

use crate::builtins::safe_fetch::{DEFAULT_MAX_BYTES, FetchPolicy, MAX_ALLOWED_BYTES, safe_fetch};
use crate::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize)]
struct FetchWitnessInput {
    url: String,
    #[serde(default)]
    max_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
struct FetchWitnessOutput {
    url: String,
    final_url: String,
    status_code: u16,
    content_type: Option<String>,
    content_length: u64,
    content_sha256: String,
    fetched_at_ms: u64,
}

pub struct FetchWitnessHandler;

impl BuiltinServiceHandler for FetchWitnessHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: FetchWitnessInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid demo.fetch-witness input: {e}"))?;

            let policy = FetchPolicy {
                max_bytes: req
                    .max_bytes
                    .unwrap_or(DEFAULT_MAX_BYTES)
                    .min(MAX_ALLOWED_BYTES),
                ..FetchPolicy::default()
            };

            let outcome = safe_fetch(&req.url, policy).await?;

            let mut hasher = Sha256::new();
            hasher.update(&outcome.body);
            let digest = hasher.finalize();
            let content_sha256 = hex::encode(digest);

            let fetched_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let output = FetchWitnessOutput {
                url: req.url,
                final_url: outcome.final_url,
                status_code: outcome.status_code,
                content_type: outcome.content_type,
                content_length: outcome.body.len() as u64,
                content_sha256,
                fetched_at_ms,
            };
            serde_json::to_value(output).map_err(|e| format!("demo.fetch-witness serialize: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn rejects_missing_url() {
        let handler = FetchWitnessHandler;
        let err = handler
            .execute(json!({}))
            .await
            .expect_err("missing url must error");
        assert!(
            err.contains("invalid demo.fetch-witness input"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let handler = FetchWitnessHandler;
        let err = handler
            .execute(json!({ "url": "file:///etc/passwd" }))
            .await
            .expect_err("file:// must be rejected");
        assert!(err.contains("unsupported url scheme"), "got: {err}");
    }

    #[tokio::test]
    async fn rejects_loopback() {
        let handler = FetchWitnessHandler;
        let err = handler
            .execute(json!({ "url": "http://127.0.0.1/" }))
            .await
            .expect_err("loopback must be rejected");
        assert!(err.contains("private/loopback"), "got: {err}");
    }

    #[tokio::test]
    async fn rejects_metadata_endpoint() {
        let handler = FetchWitnessHandler;
        let err = handler
            .execute(json!({ "url": "http://169.254.169.254/latest/meta-data/" }))
            .await
            .expect_err("cloud metadata IP must be rejected");
        assert!(err.contains("private/loopback"), "got: {err}");
    }
}
