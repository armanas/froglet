//! Demo seed builtins shipped with the main `froglet-node` binary.
//!
//! These are published by the hosted `ai.froglet.dev` reference node when the
//! operator sets `FROGLET_PUBLISH_DEMO_SERVICES=1`. A regular self-host install
//! does NOT publish them, so a plain `froglet-node` doesn't fingerprint as a
//! public demo.
//!
//! Five services are exposed:
//!
//! - `demo.echo` — returns input unchanged. Proves the discovery → deal →
//!   execute round-trip works against a live provider.
//! - `demo.add`  — `{a, b} → {sum: a + b}` over `i64`. Proves typed JSON I/O.
//! - `demo.fetch-witness` — fetch a URL and return signed metadata + body
//!   hash. **Out-of-reach × Trust** cell: agent gets a signed attestation of
//!   what the URL served without making the request itself.
//! - `demo.hash-verify` — bonded reproducibility check: agent supplies a URL
//!   and expected SHA-256, provider returns whether the live content
//!   matches. **Out-of-trust × Math** cell: deterministic, slashable.
//! - `demo.notarize` — bind a content hash to a Unix-millisecond timestamp;
//!   the kernel receipt's signature is the notarization. **Out-of-trust ×
//!   Credential** cell: provider's persistent staked identity is the proof.
//!
//! A sixth demo (`demo.wasm-compute`) is a published WASM artifact, not a
//! builtin — it ships separately under `examples/initial-services/wasm-compute/`.
//!
//! Registration is a two-step process mirroring the marketplace-node pattern:
//!
//! 1. [`demo_handlers`] returns handler instances that callers inject into
//!    `AppState.builtin_services` via `Arc::get_mut` before any state clones
//!    exist.
//! 2. [`register_demo_offers`] writes `ProviderManagedOfferDefinition` rows
//!    for each demo so the services appear in `/v1/feed` and are
//!    discoverable through the marketplace.

use crate::{
    api::{ProviderManagedOfferDefinition, persist_provider_offer_mutation},
    execution::BuiltinServiceHandler,
    state::AppState,
};
use axum::http::StatusCode;
use std::collections::HashMap;
use std::sync::Arc;

pub mod demo_add;
pub mod demo_echo;
pub mod demo_fetch_witness;
pub mod demo_hash_verify;
pub mod demo_notarize;
pub(crate) mod safe_fetch;

pub use demo_add::AddHandler;
pub use demo_echo::EchoHandler;
pub use demo_fetch_witness::FetchWitnessHandler;
pub use demo_hash_verify::HashVerifyHandler;
pub use demo_notarize::NotarizeHandler;

/// Env var that enables demo-service publication on startup.
pub const DEMO_SERVICES_ENV: &str = "FROGLET_PUBLISH_DEMO_SERVICES";

/// Demo services published when `FROGLET_PUBLISH_DEMO_SERVICES=1`. Tuple is
/// `(service_id, human-readable summary)`.
const DEMO_SERVICES: &[(&str, &str)] = &[
    (
        "demo.echo",
        "Echo — returns your input unchanged. Proves the discover → deal → execute round-trip works.",
    ),
    (
        "demo.add",
        "Add — returns {sum: a + b} for signed 64-bit integer operands.",
    ),
    (
        "demo.fetch-witness",
        "Fetch witness — provider fetches a URL and returns the body's SHA-256, status, content-type, length, and timestamp. The signed receipt is a third-party-verifiable attestation of what the URL served.",
    ),
    (
        "demo.hash-verify",
        "Hash verify — provider fetches a URL and reports whether the live SHA-256 matches the buyer's expected hash. Bonded reproducibility check: a wrong answer is detectable by anyone with the URL.",
    ),
    (
        "demo.notarize",
        "Notarize — provider binds a caller-supplied content hash to a Unix-millisecond timestamp. The kernel receipt's BIP340 signature on the output is the notarization itself.",
    ),
];

/// Returns true if the operator asked for demo services to be published via
/// env var. A value of exactly "1" enables. Any other value or absence means
/// disabled.
pub fn demo_enabled() -> bool {
    std::env::var(DEMO_SERVICES_ENV).ok().as_deref() == Some("1")
}

/// Build fresh handler instances for every demo service. Intended to be
/// extended into `AppState.builtin_services` at startup.
pub fn demo_handlers() -> HashMap<String, Arc<dyn BuiltinServiceHandler>> {
    let mut map: HashMap<String, Arc<dyn BuiltinServiceHandler>> = HashMap::new();
    map.insert("demo.echo".to_string(), Arc::new(EchoHandler));
    map.insert("demo.add".to_string(), Arc::new(AddHandler));
    map.insert(
        "demo.fetch-witness".to_string(),
        Arc::new(FetchWitnessHandler),
    );
    map.insert("demo.hash-verify".to_string(), Arc::new(HashVerifyHandler));
    map.insert("demo.notarize".to_string(), Arc::new(NotarizeHandler));
    map
}

/// Persist a `ProviderManagedOfferDefinition` for every demo service so it
/// appears in `/v1/feed` and in downstream marketplace indexes.
pub async fn register_demo_offers(state: &AppState) -> Result<(), String> {
    for (service_id, summary) in DEMO_SERVICES {
        let definition = ProviderManagedOfferDefinition {
            offer_id: (*service_id).to_string(),
            service_id: Some((*service_id).to_string()),
            project_id: None,
            offer_kind: (*service_id).to_string(),
            runtime: "builtin".to_string(),
            package_kind: "builtin".to_string(),
            entrypoint_kind: "builtin".to_string(),
            entrypoint: (*service_id).to_string(),
            contract_version: format!("froglet.builtin.{service_id}.v1"),
            mounts: Vec::new(),
            mode: "sync".to_string(),
            capabilities: Vec::new(),
            max_input_bytes: 1_048_576,
            max_runtime_ms: state.config.execution_timeout_secs.saturating_mul(1_000),
            max_memory_bytes: 0,
            max_output_bytes: 1_048_576,
            fuel_limit: 0,
            price_sats: 0,
            publication_state: "active".to_string(),
            starter: None,
            module_hash: None,
            module_bytes_hex: None,
            inline_source: None,
            oci_reference: None,
            oci_digest: None,
            source_path: None,
            source_kind: "builtin".to_string(),
            summary: Some((*summary).to_string()),
            input_schema: None,
            output_schema: None,
            terms_hash: None,
            confidential_profile_hash: None,
        };

        let _response = persist_provider_offer_mutation(
            state,
            definition,
            StatusCode::CREATED,
            format!("registered demo service {service_id}"),
        )
        .await
        .map_err(|(status, body)| {
            format!("persist demo offer for {service_id}: {status} {body:?}")
        })?;

        tracing::info!(service_id = %service_id, "registered demo service");
    }
    Ok(())
}
