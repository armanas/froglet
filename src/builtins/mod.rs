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
use serde_json::{Value, json};
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

struct DemoServiceSpec {
    service_id: &'static str,
    summary: &'static str,
    starter: &'static str,
    input_schema: Value,
    output_schema: Value,
}

fn demo_service_specs() -> Vec<DemoServiceSpec> {
    let sha256_pattern = "^[A-Fa-f0-9]{64}$";
    vec![
        DemoServiceSpec {
            service_id: "demo.echo",
            summary: "Echo — returns your input unchanged. Proves the discover → deal → execute round-trip works.",
            starter: r#"{"message":"hello froglet"}"#,
            input_schema: json!({
                "description": "Any JSON value is accepted and returned unchanged."
            }),
            output_schema: json!({
                "description": "The exact JSON value supplied as input."
            }),
        },
        DemoServiceSpec {
            service_id: "demo.add",
            summary: "Add — returns {sum: a + b} for signed 64-bit integer operands.",
            starter: r#"{"a":7,"b":5}"#,
            input_schema: json!({
                "type": "object",
                "required": ["a", "b"],
                "additionalProperties": false,
                "properties": {
                    "a": { "type": "integer", "description": "Signed 64-bit integer addend." },
                    "b": { "type": "integer", "description": "Signed 64-bit integer addend." }
                }
            }),
            output_schema: json!({
                "type": "object",
                "required": ["sum"],
                "additionalProperties": false,
                "properties": {
                    "sum": { "type": "integer", "description": "Signed 64-bit sum of a + b." }
                }
            }),
        },
        DemoServiceSpec {
            service_id: "demo.fetch-witness",
            summary: "Fetch witness — provider fetches a URL and returns the body's SHA-256, status, content-type, length, and timestamp. The signed receipt is a third-party-verifiable attestation of what the URL served.",
            starter: r#"{"url":"https://example.com/","max_bytes":1048576}"#,
            input_schema: json!({
                "type": "object",
                "required": ["url"],
                "additionalProperties": false,
                "properties": {
                    "url": { "type": "string", "format": "uri", "description": "HTTP(S) URL for the provider to fetch." },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "description": "Optional response byte ceiling." }
                }
            }),
            output_schema: json!({
                "type": "object",
                "required": ["url", "final_url", "status_code", "content_length", "content_sha256", "fetched_at_ms"],
                "additionalProperties": false,
                "properties": {
                    "url": { "type": "string", "format": "uri" },
                    "final_url": { "type": "string", "format": "uri" },
                    "status_code": { "type": "integer" },
                    "content_type": { "type": ["string", "null"] },
                    "content_length": { "type": "integer", "minimum": 0 },
                    "content_sha256": { "type": "string", "pattern": sha256_pattern },
                    "fetched_at_ms": { "type": "integer" }
                }
            }),
        },
        DemoServiceSpec {
            service_id: "demo.hash-verify",
            summary: "Hash verify — provider fetches a URL and reports whether the live SHA-256 matches the buyer's expected hash. Bonded reproducibility check: a wrong answer is detectable by anyone with the URL.",
            starter: r#"{"url":"https://example.com/","expected_sha256":"fb91d75a6bb430787a61b0aec5e374f580030f2878e1613eab5ca6310f7bbb9a","max_bytes":1048576}"#,
            input_schema: json!({
                "type": "object",
                "required": ["url", "expected_sha256"],
                "additionalProperties": false,
                "properties": {
                    "url": { "type": "string", "format": "uri", "description": "HTTP(S) URL for the provider to fetch." },
                    "expected_sha256": { "type": "string", "pattern": sha256_pattern },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "description": "Optional response byte ceiling." }
                }
            }),
            output_schema: json!({
                "type": "object",
                "required": ["url", "actual_sha256", "expected_sha256", "matches", "status_code", "content_length", "verified_at_ms"],
                "additionalProperties": false,
                "properties": {
                    "url": { "type": "string", "format": "uri" },
                    "actual_sha256": { "type": "string", "pattern": sha256_pattern },
                    "expected_sha256": { "type": "string", "pattern": sha256_pattern },
                    "matches": { "type": "boolean" },
                    "status_code": { "type": "integer" },
                    "content_length": { "type": "integer", "minimum": 0 },
                    "verified_at_ms": { "type": "integer" }
                }
            }),
        },
        DemoServiceSpec {
            service_id: "demo.notarize",
            summary: "Notarize — provider binds a caller-supplied content hash to a Unix-millisecond timestamp. The kernel receipt's BIP340 signature on the output is the notarization itself.",
            starter: r#"{"content_sha256":"fb91d75a6bb430787a61b0aec5e374f580030f2878e1613eab5ca6310f7bbb9a","context":"froglet-demo-example-com"}"#,
            input_schema: json!({
                "type": "object",
                "required": ["content_sha256"],
                "additionalProperties": false,
                "properties": {
                    "content_sha256": { "type": "string", "pattern": sha256_pattern },
                    "context": { "type": "string", "maxLength": 256, "description": "Optional opaque caller context echoed into the output." }
                }
            }),
            output_schema: json!({
                "type": "object",
                "required": ["content_sha256", "notarized_at_ms", "contract_version"],
                "additionalProperties": false,
                "properties": {
                    "content_sha256": { "type": "string", "pattern": sha256_pattern },
                    "context": { "type": ["string", "null"] },
                    "notarized_at_ms": { "type": "integer" },
                    "contract_version": { "type": "string", "const": "froglet.builtin.demo.notarize.v1" }
                }
            }),
        },
    ]
}

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
    for spec in demo_service_specs() {
        let definition = ProviderManagedOfferDefinition {
            offer_id: spec.service_id.to_string(),
            service_id: Some(spec.service_id.to_string()),
            project_id: None,
            offer_kind: spec.service_id.to_string(),
            runtime: "builtin".to_string(),
            package_kind: "builtin".to_string(),
            entrypoint_kind: "builtin".to_string(),
            entrypoint: spec.service_id.to_string(),
            contract_version: format!("froglet.builtin.{}.v1", spec.service_id),
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
            starter: Some(spec.starter.to_string()),
            module_hash: None,
            module_bytes_hex: None,
            inline_source: None,
            oci_reference: None,
            oci_digest: None,
            source_path: None,
            source_kind: "builtin".to_string(),
            summary: Some(spec.summary.to_string()),
            input_schema: Some(spec.input_schema),
            output_schema: Some(spec.output_schema),
            terms_hash: None,
            confidential_profile_hash: None,
        };

        let _response = persist_provider_offer_mutation(
            state,
            definition,
            StatusCode::CREATED,
            format!("registered demo service {}", spec.service_id),
        )
        .await
        .map_err(|(status, body)| {
            format!(
                "persist demo offer for {}: {status} {body:?}",
                spec.service_id
            )
        })?;

        tracing::info!(service_id = %spec.service_id, "registered demo service");
    }
    Ok(())
}
