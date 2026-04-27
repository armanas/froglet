//! `demo.hash-verify` — bonded reproducibility check for a URL.
//!
//! The agent supplies a URL and the SHA-256 they expect that URL to serve.
//! The provider fetches and computes the actual hash, then returns both with
//! a `matches` boolean. The kernel-signed receipt is the proof.
//!
//! Cell on the matrix: **Out-of-trust × Math** — deterministic, slashable
//! claim. If a provider returns `matches: true` while serving content that
//! does not in fact hash to the expected value, an auditor with the same URL
//! can trivially detect the lie and trigger a slash.
//!
//! Input shape:
//! ```json
//! {
//!   "url": "https://example.com/release.tar.gz",
//!   "expected_sha256": "ea8f...",
//!   "max_bytes": 8388608
//! }
//! ```
//!
//! Output shape:
//! ```json
//! {
//!   "url": "...",
//!   "actual_sha256": "ea8f...",
//!   "expected_sha256": "ea8f...",
//!   "matches": true,
//!   "status_code": 200,
//!   "content_length": 12345,
//!   "verified_at_ms": 1714123456789
//! }
//! ```
//!
//! Why bonded: the natural buyer use case is "before I install or trust this
//! artefact, has it been tampered with at the source?". The provider is paid
//! to give a yes/no, with stake on the line if the answer is wrong.

use crate::builtins::safe_fetch::{DEFAULT_MAX_BYTES, FetchPolicy, MAX_ALLOWED_BYTES, safe_fetch};
use crate::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize)]
struct HashVerifyInput {
    url: String,
    expected_sha256: String,
    #[serde(default)]
    max_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
struct HashVerifyOutput {
    url: String,
    actual_sha256: String,
    expected_sha256: String,
    matches: bool,
    status_code: u16,
    content_length: u64,
    verified_at_ms: u64,
}

pub struct HashVerifyHandler;

/// Validate that a string is a 64-character lowercase hexadecimal SHA-256
/// digest. We canonicalize to lowercase before comparison so callers can pass
/// either case.
fn parse_sha256_hex(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.len() != 64 {
        return Err(format!(
            "expected_sha256 must be 64 hex characters; got {} ({} chars)",
            trimmed,
            trimmed.len()
        ));
    }
    if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("expected_sha256 must be hexadecimal".to_string());
    }
    Ok(trimmed.to_ascii_lowercase())
}

impl BuiltinServiceHandler for HashVerifyHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: HashVerifyInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid demo.hash-verify input: {e}"))?;

            let expected = parse_sha256_hex(&req.expected_sha256)?;

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
            let actual = hex::encode(hasher.finalize());

            let verified_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let output = HashVerifyOutput {
                url: req.url,
                matches: actual == expected,
                actual_sha256: actual,
                expected_sha256: expected,
                status_code: outcome.status_code,
                content_length: outcome.body.len() as u64,
                verified_at_ms,
            };
            serde_json::to_value(output).map_err(|e| format!("demo.hash-verify serialize: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_sha256_accepts_lowercase() {
        let h = "0".repeat(64);
        assert_eq!(parse_sha256_hex(&h).unwrap(), h);
    }

    #[test]
    fn parse_sha256_normalizes_uppercase() {
        let h = "A".repeat(64);
        assert_eq!(parse_sha256_hex(&h).unwrap(), "a".repeat(64));
    }

    #[test]
    fn parse_sha256_rejects_short() {
        let err = parse_sha256_hex("abc").unwrap_err();
        assert!(err.contains("64 hex"), "got: {err}");
    }

    #[test]
    fn parse_sha256_rejects_non_hex() {
        let bad = format!("{}{}", "g".repeat(63), "0");
        let err = parse_sha256_hex(&bad).unwrap_err();
        assert!(err.contains("hexadecimal"), "got: {err}");
    }

    #[tokio::test]
    async fn rejects_missing_expected() {
        let handler = HashVerifyHandler;
        let err = handler
            .execute(json!({ "url": "https://example.com/" }))
            .await
            .expect_err("missing expected_sha256 must error");
        assert!(err.contains("invalid demo.hash-verify input"), "got: {err}");
    }

    #[tokio::test]
    async fn rejects_bad_hex() {
        let handler = HashVerifyHandler;
        let err = handler
            .execute(json!({
                "url": "https://example.com/",
                "expected_sha256": "not-hex"
            }))
            .await
            .expect_err("non-hex must error");
        assert!(
            err.contains("64 hex") || err.contains("hexadecimal"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn rejects_loopback_url() {
        let handler = HashVerifyHandler;
        let err = handler
            .execute(json!({
                "url": "http://127.0.0.1/",
                "expected_sha256": "0".repeat(64),
            }))
            .await
            .expect_err("loopback url must be rejected");
        assert!(err.contains("private/loopback"), "got: {err}");
    }
}
