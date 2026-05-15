//! Error + warning types for the publish engine.

use thiserror::Error;

/// Hard failure during the publish pipeline. Each variant has enough
/// structure for the caller (CLI or MCP) to render an actionable
/// message without a stack trace.
#[derive(Debug, Error)]
pub enum PublishError {
    #[error("invalid input: {field}: {reason}")]
    InvalidInput { field: &'static str, reason: String },
    #[error("manifest validation failed: {0}")]
    Manifest(#[from] froglet_protocol::manifest::ManifestError),
    #[error("artifact build failed: {0}")]
    Build(String),
    #[error("hosting backend {backend:?} failed: {reason}")]
    Hosting {
        backend: &'static str,
        reason: String,
    },
    #[error("signing failed: {0}")]
    Signing(String),
    #[error("marketplace registration failed: {url}: HTTP {status}: {body}")]
    Registration {
        url: String,
        status: u16,
        body: String,
    },
    #[error("verification failed after {tries} polls: {url}: {reason}")]
    Verification {
        tries: u32,
        url: String,
        reason: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(String),
    #[error("not implemented yet: {what}")]
    NotImplemented { what: String },
}

impl From<reqwest::Error> for PublishError {
    fn from(value: reqwest::Error) -> Self {
        PublishError::Http(value.to_string())
    }
}

/// Soft warning. Returned in [`crate::PublishOutput`]. Not a failure.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum PublishWarning {
    /// The provider was not registered with a marketplace (Local backend).
    NotRegistered { reason: String },
    /// Manifest had legacy v2 sections; engine substituted defaults.
    LegacyV2 { missing_section: String },
    /// Indexer has not yet projected the offer; the offer is signed
    /// and persisted but may take up to ~60s to appear in /v1/providers.
    IndexerLag { seconds_waited: u32 },
}
