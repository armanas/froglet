//! `demo.notarize` — bonded timestamp notarization.
//!
//! The agent supplies a SHA-256 hash of some content (computed locally — the
//! provider never sees the content). The provider returns the hash with a
//! Unix-millisecond timestamp and a stable contract version. The kernel
//! receipt's BIP340 signature over this output is the notarization: it
//! cryptographically binds *provider P notarised hash H at time T*.
//!
//! Cell on the matrix: **Out-of-trust × Credential** — the value comes from
//! the provider's persistent staked identity and the receipt being
//! third-party-verifiable later. The LLM cannot supply this because it does
//! not have a stable counterparty-trusted key tied to a slashable bond.
//!
//! Why content stays with the agent: notarization is about binding hash to
//! time, not about the provider seeing the content. Sending content over the
//! wire would also expose the provider to liability for whatever it is.
//!
//! Input shape:
//! ```json
//! {
//!   "content_sha256": "ea8f...",
//!   "context": "release-tarball-v1.2.3"
//! }
//! ```
//! `context` is an optional opaque string echoed into the output for the
//! buyer's bookkeeping; max 256 chars.
//!
//! Output shape:
//! ```json
//! {
//!   "content_sha256": "ea8f...",
//!   "context": "release-tarball-v1.2.3",
//!   "notarized_at_ms": 1714123456789,
//!   "contract_version": "froglet.builtin.demo.notarize.v1"
//! }
//! ```

use crate::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

const NOTARIZE_CONTRACT_VERSION: &str = "froglet.builtin.demo.notarize.v1";
const MAX_CONTEXT_LEN: usize = 256;

#[derive(Debug, Deserialize)]
struct NotarizeInput {
    content_sha256: String,
    #[serde(default)]
    context: Option<String>,
}

#[derive(Debug, Serialize)]
struct NotarizeOutput {
    content_sha256: String,
    context: Option<String>,
    notarized_at_ms: u64,
    contract_version: String,
}

pub struct NotarizeHandler;

fn parse_sha256_hex(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.len() != 64 {
        return Err(format!(
            "content_sha256 must be 64 hex characters; got {} chars",
            trimmed.len()
        ));
    }
    if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("content_sha256 must be hexadecimal".to_string());
    }
    Ok(trimmed.to_ascii_lowercase())
}

fn validate_context(raw: Option<String>) -> Result<Option<String>, String> {
    match raw {
        None => Ok(None),
        Some(s) if s.len() > MAX_CONTEXT_LEN => Err(format!(
            "context must be at most {MAX_CONTEXT_LEN} characters; got {}",
            s.len()
        )),
        Some(s) => Ok(Some(s)),
    }
}

impl BuiltinServiceHandler for NotarizeHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: NotarizeInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid demo.notarize input: {e}"))?;
            let content_sha256 = parse_sha256_hex(&req.content_sha256)?;
            let context = validate_context(req.context)?;

            let notarized_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .map_err(|_| "system clock before unix epoch".to_string())?;

            let output = NotarizeOutput {
                content_sha256,
                context,
                notarized_at_ms,
                contract_version: NOTARIZE_CONTRACT_VERSION.to_string(),
            };
            serde_json::to_value(output).map_err(|e| format!("demo.notarize serialize: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn notarize_happy_path() {
        let handler = NotarizeHandler;
        let h = "ea".repeat(32); // 64 hex chars
        let out = handler
            .execute(json!({ "content_sha256": h, "context": "build-1.2.3" }))
            .await
            .unwrap();
        assert_eq!(out["content_sha256"], h);
        assert_eq!(out["context"], "build-1.2.3");
        assert_eq!(out["contract_version"], NOTARIZE_CONTRACT_VERSION);
        assert!(out["notarized_at_ms"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn notarize_normalizes_uppercase() {
        let handler = NotarizeHandler;
        let h = "EA".repeat(32);
        let out = handler
            .execute(json!({ "content_sha256": h }))
            .await
            .unwrap();
        assert_eq!(out["content_sha256"], "ea".repeat(32));
    }

    #[tokio::test]
    async fn notarize_rejects_bad_hash_length() {
        let handler = NotarizeHandler;
        let err = handler
            .execute(json!({ "content_sha256": "abc" }))
            .await
            .expect_err("short hash must error");
        assert!(err.contains("64 hex"), "got: {err}");
    }

    #[tokio::test]
    async fn notarize_rejects_non_hex() {
        let handler = NotarizeHandler;
        let err = handler
            .execute(json!({ "content_sha256": "z".repeat(64) }))
            .await
            .expect_err("non-hex must error");
        assert!(err.contains("hexadecimal"), "got: {err}");
    }

    #[tokio::test]
    async fn notarize_rejects_oversized_context() {
        let handler = NotarizeHandler;
        let h = "0".repeat(64);
        let err = handler
            .execute(json!({
                "content_sha256": h,
                "context": "x".repeat(MAX_CONTEXT_LEN + 1),
            }))
            .await
            .expect_err("oversized context must error");
        assert!(err.contains("at most"), "got: {err}");
    }

    #[tokio::test]
    async fn notarize_accepts_no_context() {
        let handler = NotarizeHandler;
        let h = "f".repeat(64);
        let out = handler
            .execute(json!({ "content_sha256": h }))
            .await
            .unwrap();
        assert!(out["context"].is_null());
    }
}
