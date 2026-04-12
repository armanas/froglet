pub mod api;

/// Initialize tracing with an env-filter default of `info`.
/// Safe to call multiple times; subsequent calls are silently ignored.
pub fn init_logging() {
    use tracing_subscriber::{EnvFilter, FmtSubscriber};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

// Protocol core — re-exported from froglet-protocol (single source of truth)
pub use froglet_protocol::canonical_json;
pub use froglet_protocol::crypto;

pub mod confidential;
pub mod config;
pub mod db;
pub mod execution;
pub mod identity;
pub mod lnd;
pub mod pricing;
pub mod protocol;
pub mod requester_deals;
pub mod runtime_auth;
pub mod sandbox;
pub mod server;
pub mod settlement;
pub mod state;
pub mod tls;
pub mod wasm;

pub mod deals;

// Internal modules — not part of the public library API
pub(crate) mod jobs;
pub(crate) mod nostr;
#[allow(dead_code)]
pub(crate) mod provider_catalog;
#[allow(dead_code)]
pub(crate) mod provider_resolution;
pub(crate) mod tor;
pub(crate) mod wasm_db;
pub(crate) mod wasm_host;
pub(crate) mod wasm_http;
