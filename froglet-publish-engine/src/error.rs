//! Error + warning types for the publish engine.

use thiserror::Error;

/// Structured evidence for an exact registration whose result remains
/// ambiguous after an idempotent replay. Boxed inside [`PublishError`] so this
/// evidence-rich edge case does not inflate every publish result.
#[derive(Debug, Error)]
#[error(
    "marketplace state is unknown for service {service_id} offer {offer_hash} revision {revision_hash} at provider {provider_url}: {reason}; listing visibility may persist for at most {visibility_bound_secs}s; local_pause={local_pause_result}; relay_withdrawal={relay_withdrawal_result}"
)]
pub struct MarketplaceStateUnknownContext {
    pub service_id: String,
    pub offer_hash: String,
    pub revision_hash: String,
    pub provider_url: String,
    pub visibility_bound_secs: u64,
    pub reason: String,
    pub local_pause_result: String,
    pub relay_withdrawal_result: String,
}

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
    #[error(transparent)]
    MarketplaceStateUnknown(Box<MarketplaceStateUnknownContext>),
    #[error("not implemented yet: {what}")]
    NotImplemented { what: String },
    #[error(
        "public publication entered a partial state for service {service_id} revision {revision_hash}: completion failed: {completion_error}; exact pause compensation failed: {compensation_error}"
    )]
    PartialState {
        service_id: String,
        revision_hash: String,
        completion_error: String,
        compensation_error: String,
    },
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
    /// The marketplace already returned exact `active`, but the follow-up
    /// projection read could not be confirmed. Publication remains successful
    /// and the exact status URL is returned for later observation.
    MarketplaceProjectionUnconfirmed { status_url: String, reason: String },
    /// The publication had no private verification fixture, so the client
    /// deliberately used the legacy operator-review registration path.
    PendingMarketplaceReview { status: String },
}
