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
pub mod deals;
pub mod execution;
pub mod identity;
pub mod jobs;
pub mod lnd;
pub mod nostr;
pub mod pricing;
pub mod protocol;
pub mod provider_catalog;
pub mod provider_resolution;
pub mod requester_deals;
pub mod runtime_auth;
pub mod sandbox;
pub mod server;
pub mod settlement;
pub mod state;
pub mod tls;
pub mod tor;
pub mod wasm;
pub mod wasm_db;
pub mod wasm_host;
pub mod wasm_http;
