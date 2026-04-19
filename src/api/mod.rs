use axum::{
    Json, Router,
    error_handling::HandleErrorLayer,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use futures::{StreamExt, stream};
use rand::RngCore;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    error::Error as StdError,
    fs,
    io::Write,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tower::{BoxError, ServiceBuilder, limit::ConcurrencyLimitLayer, timeout::TimeoutLayer};

use crate::{
    canonical_json,
    confidential::{
        self, AttestationBundle, AttestationProvider, ConfidentialExecutionContext,
        ConfidentialExecutor, ConfidentialProfileConfig, ConfidentialProfilePayload,
        ConfidentialSessionOpenRequest, ConfidentialSessionPayload, EncryptedEnvelope,
        KeyReleaseProvider, MockExternalKeyReleaseProvider, NvidiaMockAttestationProvider,
        PolicyConfidentialExecutor, SessionPrivateMaterial,
    },
    config::{LightningMode, PaymentBackend},
    crypto, db,
    deals::{self, NewDeal},
    execution::{
        CONTRACT_BUILTIN_EVENTS_QUERY_V1, CONTRACT_CONTAINER_JSON_V1,
        CONTRACT_PYTHON_HANDLER_JSON_V1, CONTRACT_PYTHON_SCRIPT_JSON_V1, ExecutionEntrypointKind,
        ExecutionMount, ExecutionPackageKind, ExecutionRuntime, ExecutionSecurityMode,
        ExecutionWorkload, default_contract_version_for, default_entrypoint_for,
        default_entrypoint_kind_for,
    },
    jobs::{self, JobSpec, NewJob},
    nostr,
    pricing::{PricingInfo, ServiceId},
    protocol::{
        self, ARTIFACT_KIND_CONFIDENTIAL_PROFILE, ARTIFACT_KIND_CONFIDENTIAL_SESSION,
        ARTIFACT_KIND_DEAL, ARTIFACT_KIND_DESCRIPTOR, ARTIFACT_KIND_OFFER, ARTIFACT_KIND_QUOTE,
        ARTIFACT_KIND_RECEIPT, CuratedListPayload, DealPayload, DescriptorPayload, ExecutionLimits,
        InvoiceBundleLegState, InvoiceBundlePayload, LinkedIdentity, OfferPayload, QuotePayload,
        QuoteSettlementTerms, ReceiptExecutor, ReceiptFailure, ReceiptLegState,
        ReceiptLimitsApplied, ReceiptPayload, ReceiptSettlement, ReceiptSettlementLeg,
        SignedArtifact, WorkloadSpec,
    },
    provider_resolution,
    requester_deals::{self, NewRequesterDeal},
    runtime_auth, sandbox,
    settlement::{self, PaymentReceipt, PaymentReservation, ProvidedPayment},
    state::AppState,
    wasm::{self, WasmSubmission},
};

mod http_catalog;
mod http_confidential;
mod http_deals;
mod http_discovery;
mod http_events;
mod http_execution;
mod http_settlement;
pub(crate) mod types;
pub use types::*;
#[cfg(test)]
mod test_support;

#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    #[serde(default)]
    pub cursor: Option<i64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct FeedResponse {
    pub artifacts: Vec<db::LedgerArtifact>,
    pub cursor_type: String,
    pub cursor_semantics: String,
    pub applied_cursor: i64,
    pub page_size: usize,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<i64>,
}

const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_EVENT_CONTENT_BYTES: usize = 64 * 1024;
const MAX_WASM_HEX_BYTES: usize = 512 * 1024;
const MAX_WASM_INPUT_BYTES: usize = 128 * 1024;
const MAX_OCI_WASM_MODULE_BYTES: usize = 50 * 1024 * 1024;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
const BLOCKING_EXECUTION_TIMEOUT_GRACE_SECS: u64 = 1;
const DEFAULT_ROUTE_TIMEOUT_SECS: u64 = 10;
const RUNTIME_WAIT_ROUTE_TIMEOUT_SECS: u64 = 65;
const DEFAULT_EVENTS_QUERY_ROUTE_CONCURRENCY_LIMIT: usize = 16;

/// Offer ID for the generic compute offer that accepts any supported runtime
/// (Python, Container, Wasm) with `offer_kind = "compute.execution.v1"`.
pub(crate) const EXECUTE_COMPUTE_GENERIC_OFFER_ID: &str = "execute.compute.generic";
pub(crate) type ApiFailure = (StatusCode, serde_json::Value);

fn private_runtime_tempdir(prefix: &str) -> Result<std::path::PathBuf, String> {
    let mut rng_bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut rng_bytes);
    let tempdir = std::env::temp_dir().join(format!("{prefix}-{}", hex::encode(rng_bytes)));
    fs::create_dir_all(&tempdir)
        .map_err(|error| format!("failed to create tempdir {}: {error}", tempdir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&tempdir, fs::Permissions::from_mode(0o700)).map_err(|error| {
            format!(
                "failed to secure tempdir permissions {}: {error}",
                tempdir.display()
            )
        })?;
    }
    Ok(tempdir)
}

async fn runtime_accessible_provider_url(
    state: &AppState,
    raw_url: &str,
    provider_id: Option<&str>,
) -> Result<String, ApiFailure> {
    provider_resolution::runtime_accessible_provider_url(state, raw_url, provider_id).await
}

fn default_offer_publication_state() -> String {
    "active".to_string()
}

fn default_service_mode() -> String {
    "sync".to_string()
}

fn default_service_resource_kind() -> String {
    "service".to_string()
}

pub(crate) fn normalize_offer_publication_state(value: Option<&str>) -> Result<String, String> {
    let normalized = value.unwrap_or("active").trim();
    match normalized {
        "active" | "hidden" => Ok(normalized.to_string()),
        _ => Err(format!(
            "publication_state must be one of active or hidden, got {normalized}"
        )),
    }
}

pub(crate) fn normalize_short_id(offer_id: &str) -> Result<String, String> {
    let trimmed = offer_id.trim();
    if trimmed.is_empty() {
        return Err("offer_id must be a non-empty string".to_string());
    }
    if trimmed.len() > 128 {
        return Err("offer_id must be 128 bytes or fewer".to_string());
    }
    Ok(trimmed.to_string())
}

fn publish_routes() -> Router<Arc<AppState>> {
    http_events::publish_routes()
}

fn execute_wasm_routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    http_execution::execute_wasm_routes(state)
}

fn jobs_routes() -> Router<Arc<AppState>> {
    http_execution::jobs_routes()
}

fn events_query_routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    http_events::query_routes(state)
}

fn provider_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(http_catalog::routes())
        .merge(http_confidential::routes())
        .merge(http_deals::provider_routes())
        .merge(http_events::provider_routes())
        .merge(http_settlement::provider_routes())
        .route_layer(ConcurrencyLimitLayer::new(16))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(
                    DEFAULT_ROUTE_TIMEOUT_SECS,
                ))),
        )
}

fn runtime_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(http_discovery::runtime_routes())
        .merge(http_deals::runtime_routes())
        .merge(http_settlement::runtime_routes())
        .route_layer(ConcurrencyLimitLayer::new(16))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(
                    RUNTIME_WAIT_ROUTE_TIMEOUT_SECS,
                ))),
        )
}

fn common_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health_check))
        .route("/v1/node/capabilities", get(node_capabilities))
        .route("/v1/node/identity", get(node_identity))
        .route("/v1/openapi.yaml", get(openapi_spec))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::SERVER,
            HeaderValue::from_static("froglet"),
        ))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

pub fn public_router(state: Arc<AppState>) -> Router {
    // NOTE: Per-IP rate limiting requires axum 0.8+ and tower_governor.
    // The provider_routes already enforce ConcurrencyLimitLayer(16) which
    // limits total in-flight requests. When upgrading to axum 0.8, add
    // tower_governor::GovernorLayer for per-caller throttling.
    common_routes()
        .merge(events_query_routes(&state))
        .merge(provider_routes())
        .merge(publish_routes())
        .with_state(state)
}

pub fn runtime_router(state: Arc<AppState>) -> Router {
    common_routes()
        .merge(runtime_routes())
        .merge(execute_wasm_routes(&state))
        .merge(jobs_routes())
        .with_state(state)
}

/// Combined router with all routes (public + runtime). Used only in dual-mode
/// and in tests. **Do not use for internet-facing deployments** — execute_wasm
/// and jobs endpoints are included without authentication. Use `public_router`
/// and `runtime_router` separately for production.
#[cfg_attr(
    not(test),
    deprecated(note = "use public_router + runtime_router for production")
)]
pub fn router(state: Arc<AppState>) -> Router {
    common_routes()
        .merge(events_query_routes(&state))
        .merge(runtime_routes())
        .merge(provider_routes())
        .merge(publish_routes())
        .merge(execute_wasm_routes(&state))
        .merge(jobs_routes())
        .with_state(state)
}

pub async fn health_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({"status": "ok", "service": "froglet"})),
    )
}

pub async fn openapi_spec() -> impl IntoResponse {
    static SPEC: &str = include_str!("../../docs/openapi.yaml");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-yaml")],
        SPEC,
    )
}

pub async fn node_capabilities(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let transport_status = state.transport_status.lock().await.clone();
    let settlement_descriptor = settlement::driver_descriptor(state.as_ref());
    let faas_descriptor = jobs::FaaSDescriptor::standard();

    let capabilities = NodeCapabilities {
        api_version: "v1".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        identity: IdentityInfo {
            node_id: state.identity.node_id().to_string(),
            public_key: state.identity.public_key_hex().to_string(),
        },
        discovery: DiscoveryInfo {
            mode: "none".to_string(),
        },
        transports: TransportsInfo {
            clearnet: ClearnetInfo {
                enabled: transport_status.clearnet_enabled,
                url: transport_status.clearnet_url,
            },
            tor: TorInfo {
                enabled: transport_status.tor_enabled,
                onion_url: transport_status.tor_onion_url,
                status: transport_status.tor_status,
            },
        },
        execution: ExecutionInfo {
            wasm: WasmInfo {
                enabled: true,
                fuel_limit: 50_000_000,
                entrypoints: vec!["alloc".to_string(), "run".to_string()],
            },
        },
        limits: LimitsInfo {
            events_query_limit_default: 100,
            events_query_limit_max: 500,
            body_limit_bytes: MAX_BODY_BYTES,
            wasm_hex_limit_bytes: MAX_WASM_HEX_BYTES,
            wasm_input_limit_bytes: MAX_WASM_INPUT_BYTES,
        },
        pricing: state.pricing.info().clone(),
        payments: PaymentsInfo {
            backend: settlement_descriptor.backend,
            verifier_mode: (settlement_descriptor.mode != "disabled")
                .then(|| settlement_descriptor.mode.clone()),
            accepted_payment_methods: settlement_descriptor.accepted_payment_methods,
            reservations: settlement_descriptor.reservations,
            receipts: settlement_descriptor.receipts,
        },
        faas: FaaSInfo {
            jobs_api: faas_descriptor.jobs_api,
            async_jobs: true,
            idempotency_keys: faas_descriptor.idempotency_keys,
            runtimes: faas_descriptor.runtimes,
        },
    };

    (StatusCode::OK, Json(capabilities))
}

pub async fn node_identity(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(IdentityInfo {
            node_id: state.identity.node_id().to_string(),
            public_key: state.identity.public_key_hex().to_string(),
        }),
    )
}

pub async fn runtime_wallet_balance(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    match settlement::wallet_balance_snapshot(state.as_ref()).await {
        Ok(snapshot) => (
            StatusCode::OK,
            Json(json!(RuntimeWalletBalanceResponse {
                backend: snapshot.backend,
                mode: snapshot.mode,
                balance_known: snapshot.balance_known,
                balance_sats: snapshot.balance_sats,
                accepted_payment_methods: snapshot.accepted_payment_methods,
                reservations: snapshot.reservations,
                receipts: snapshot.receipts,
            })),
        ),
        Err(error) => error_json(error.status_code(), error.details()),
    }
}

/// Return a compact summary of recent requester-side deals and their
/// settlement state. Complements the per-deal `/v1/runtime/deals/:id`
/// endpoint with a list view; used by the MCP `list_settlement_activity`
/// action so an LLM can reason about pending/settled deals without knowing
/// specific deal ids.
pub async fn runtime_settlement_activity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<SettlementActivityQuery>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    let limit = params
        .limit
        .unwrap_or(SETTLEMENT_ACTIVITY_DEFAULT_LIMIT)
        .clamp(1, SETTLEMENT_ACTIVITY_MAX_LIMIT);

    let records = match state
        .db
        .with_read_conn(move |conn| {
            crate::requester_deals::list_recent_requester_deals(conn, limit)
        })
        .await
    {
        Ok(records) => records,
        Err(error) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("list requester deals: {error}") }),
            );
        }
    };

    let items: Vec<SettlementActivityItem> = records
        .iter()
        .map(|record| {
            let terms = &record.quote.payload.settlement_terms;
            SettlementActivityItem {
                deal_id: record.deal_id.clone(),
                provider_id: record.provider_id.clone(),
                status: record.status.clone(),
                workload_kind: record.spec.workload_kind().to_string(),
                settlement_method: terms.method.clone(),
                base_fee_msat: terms.base_fee_msat,
                success_fee_msat: terms.success_fee_msat,
                has_receipt: record.receipt.is_some(),
                has_result: record.result.is_some(),
                error: record.error.clone(),
                created_at: record.created_at,
                updated_at: record.updated_at,
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!(RuntimeSettlementActivityResponse { items, limit })),
    )
}

const SETTLEMENT_ACTIVITY_DEFAULT_LIMIT: usize = 25;
const SETTLEMENT_ACTIVITY_MAX_LIMIT: usize = 200;

#[derive(Debug, serde::Deserialize)]
pub struct SettlementActivityQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

fn format_reqwest_error(error: &reqwest::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source: Option<&(dyn StdError + 'static)> = error.source();
    while let Some(next) = source {
        parts.push(next.to_string());
        source = next.source();
    }
    parts.join(": ")
}

async fn remote_json_request<T, B>(
    state: &AppState,
    method: reqwest::Method,
    url: String,
    body: Option<&B>,
) -> Result<T, ApiFailure>
where
    T: DeserializeOwned,
    B: Serialize + ?Sized,
{
    remote_json_request_with_client_error_passthrough(state, method, url, body, false).await
}

async fn remote_json_request_with_client_error_passthrough<T, B>(
    state: &AppState,
    method: reqwest::Method,
    url: String,
    body: Option<&B>,
    preserve_client_errors: bool,
) -> Result<T, ApiFailure>
where
    T: DeserializeOwned,
    B: Serialize + ?Sized,
{
    let mut request = state.http_client.request(method, &url);
    if let Some(body) = body {
        request = request.json(body);
    }

    let response = request.send().await.map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            json!({ "error": "upstream request failed", "details": format_reqwest_error(&error), "url": url }),
        )
    })?;
    let status = response.status();
    let body_text = response.text().await.map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            json!({ "error": "failed to read upstream response", "details": error.to_string(), "url": url }),
        )
    })?;
    if !status.is_success() {
        if preserve_client_errors && status.is_client_error() {
            if let Ok(payload) = serde_json::from_str::<Value>(&body_text) {
                return Err((status, payload));
            }
            return Err((
                status,
                json!({
                    "error": "upstream client error",
                    "upstream_body": body_text,
                    "url": url,
                }),
            ));
        }
        return Err((
            StatusCode::BAD_GATEWAY,
            json!({
                "error": "upstream request failed",
                "upstream_status": status.as_u16(),
                "upstream_body": body_text,
                "url": url,
            }),
        ));
    }
    serde_json::from_str(&body_text).map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            json!({ "error": "invalid upstream json", "details": error.to_string(), "url": url }),
        )
    })
}

async fn fetch_provider_descriptor(
    state: &AppState,
    provider_url: &str,
) -> Result<SignedArtifact<DescriptorPayload>, ApiFailure> {
    remote_json_request(
        state,
        reqwest::Method::GET,
        format!("{provider_url}/v1/provider/descriptor"),
        Option::<&()>::None,
    )
    .await
}

fn provider_bad_gateway(message: &str) -> ApiFailure {
    (StatusCode::BAD_GATEWAY, json!({ "error": message }))
}

fn verify_provider_descriptor_artifact(
    descriptor: &SignedArtifact<DescriptorPayload>,
) -> Result<(), ApiFailure> {
    if !protocol::verify_artifact(descriptor) {
        return Err(provider_bad_gateway(
            "provider descriptor signature verification failed",
        ));
    }
    if let Err(error) = protocol::validate_descriptor_artifact(descriptor) {
        return Err(provider_bad_gateway(&format!(
            "provider descriptor semantic validation failed: {error}"
        )));
    }
    Ok(())
}

fn verify_provider_receipt_artifact(
    receipt: &SignedArtifact<ReceiptPayload>,
    quote: &SignedArtifact<QuotePayload>,
    deal: &SignedArtifact<DealPayload>,
    expected_provider_id: &str,
    expected_requester_id: &str,
    result: Option<&Value>,
    result_hash: Option<&str>,
) -> Result<(), ApiFailure> {
    if !protocol::verify_artifact(receipt) {
        return Err(provider_bad_gateway(
            "provider receipt signature verification failed",
        ));
    }
    if let Err(error) = protocol::validate_receipt_artifact(receipt) {
        return Err(provider_bad_gateway(&format!(
            "provider receipt semantic validation failed: {error}"
        )));
    }
    if receipt.payload.provider_id != expected_provider_id {
        return Err(provider_bad_gateway(
            "provider receipt provider_id does not match selected provider",
        ));
    }
    if receipt.payload.requester_id != expected_requester_id {
        return Err(provider_bad_gateway(
            "provider receipt requester_id does not match local runtime identity",
        ));
    }
    if receipt.payload.quote_hash != quote.hash {
        return Err(provider_bad_gateway(
            "provider receipt quote_hash does not match local requester quote",
        ));
    }
    if receipt.payload.deal_hash != deal.hash {
        return Err(provider_bad_gateway(
            "provider receipt deal_hash does not match local requester deal",
        ));
    }
    if receipt.payload.settlement_refs.method != quote.payload.settlement_terms.method {
        return Err(provider_bad_gateway(
            "provider receipt settlement method does not match local requester quote",
        ));
    }
    if let Some(result_hash) = result_hash
        && receipt.payload.result_hash.as_deref() != Some(result_hash)
    {
        return Err(provider_bad_gateway(
            "provider receipt result_hash does not match provider result_hash",
        ));
    }
    if let Some(result) = result {
        let canonical_hash = canonical_result_hash(result);
        if receipt.payload.result_hash.as_deref() != Some(canonical_hash.as_str()) {
            return Err(provider_bad_gateway(
                "provider receipt result_hash does not match provider result",
            ));
        }
    }
    Ok(())
}

struct ResolvedProvider {
    provider_id: String,
    provider_url: String,
}

async fn resolve_runtime_provider(
    state: &AppState,
    provider: &RuntimeProviderRef,
) -> Result<ResolvedProvider, ApiFailure> {
    let explicit_provider_id = provider.provider_id.clone();
    if let Some(provider_url) = provider.provider_url.clone() {
        let provider_url =
            runtime_accessible_provider_url(state, &provider_url, explicit_provider_id.as_deref())
                .await?;
        let descriptor = fetch_provider_descriptor(state, &provider_url).await?;
        verify_provider_descriptor_artifact(&descriptor)?;
        if let Some(expected_provider_id) = explicit_provider_id.as_deref()
            && descriptor.payload.provider_id != expected_provider_id
        {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "provider_id does not match provider_url descriptor",
                    "provider_id": expected_provider_id,
                    "descriptor_provider_id": descriptor.payload.provider_id,
                }),
            ));
        }
        return Ok(ResolvedProvider {
            provider_id: descriptor.payload.provider_id.clone(),
            provider_url,
        });
    }

    let Some(provider_id) = explicit_provider_id else {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "provider.provider_id or provider.provider_url is required" }),
        ));
    };
    // Discovery server has been removed.  Resolving a provider by ID alone
    // is no longer possible -- callers must supply a provider_url.
    Err((
        StatusCode::BAD_REQUEST,
        json!({
            "error": "provider.provider_url is required — discovery server removed; use marketplace or direct URL",
            "provider_id": provider_id,
        }),
    ))
}

fn generate_success_preimage_hex() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn lightning_quote_max_admission_deadline(quote: &SignedArtifact<QuotePayload>) -> i64 {
    quote
        .payload
        .expires_at
        .saturating_sub(deal_execution_window_secs(&quote.payload.execution_limits) as i64)
        .saturating_sub(quote.payload.settlement_terms.max_success_hold_expiry_secs as i64)
}

fn build_runtime_requester_deal_artifact(
    state: &AppState,
    quote: &SignedArtifact<QuotePayload>,
    success_payment_hash: &str,
    created_at: i64,
    uses_lightning_bundle: bool,
) -> Result<SignedArtifact<DealPayload>, String> {
    let requester_id = state.identity.node_id().to_string();
    let execution_window_secs = deal_execution_window_secs(&quote.payload.execution_limits);
    let admission_deadline = if uses_lightning_bundle {
        lightning_quote_max_admission_deadline(quote).min(created_at.saturating_add(
            lightning_admission_window_secs(&quote.payload.settlement_terms) as i64,
        ))
    } else {
        quote.payload.expires_at
    };
    if uses_lightning_bundle && admission_deadline < created_at {
        return Err(
            "quote expiry already leaves no remaining Lightning admission window".to_string(),
        );
    }
    let completion_deadline = admission_deadline.saturating_add(execution_window_secs as i64);
    let acceptance_deadline = if uses_lightning_bundle {
        completion_deadline
            .saturating_add(quote.payload.settlement_terms.max_success_hold_expiry_secs as i64)
    } else {
        completion_deadline
    };

    if uses_lightning_bundle && acceptance_deadline > quote.payload.expires_at {
        return Err(
            "quote expiry is too short for the Lightning admission, execution, and acceptance windows"
                .to_string(),
        );
    }

    protocol::sign_artifact(
        &requester_id,
        |message| state.identity.sign_message_hex(message),
        ARTIFACT_KIND_DEAL,
        created_at,
        DealPayload {
            requester_id: requester_id.clone(),
            provider_id: quote.payload.provider_id.clone(),
            quote_hash: quote.hash.clone(),
            workload_hash: quote.payload.workload_hash.clone(),
            confidential_session_hash: quote.payload.confidential_session_hash.clone(),
            extension_refs: Vec::new(),
            authority_ref: None,
            supersedes_deal_hash: None,
            client_nonce: None,
            success_payment_hash: success_payment_hash.to_string(),
            admission_deadline,
            completion_deadline,
            acceptance_deadline,
        },
    )
}

fn persist_runtime_artifact(
    conn: &rusqlite::Connection,
    artifact_hash: &str,
    payload_hash: &str,
    artifact_kind: &str,
    actor_id: &str,
    created_at: i64,
    document_json: &str,
) -> Result<(), String> {
    db::insert_artifact_document(
        conn,
        artifact_hash,
        payload_hash,
        artifact_kind,
        actor_id,
        created_at,
        document_json,
    )
}

async fn persist_requester_artifacts(
    state: Arc<AppState>,
    quote: &SignedArtifact<QuotePayload>,
    deal: &SignedArtifact<DealPayload>,
    receipt: Option<&SignedArtifact<ReceiptPayload>>,
) -> Result<(), String> {
    let quote_json = serde_json::to_string(quote).map_err(|error| error.to_string())?;
    let deal_json = serde_json::to_string(deal).map_err(|error| error.to_string())?;
    let receipt_json = receipt
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| error.to_string())?;
    let quote = quote.clone();
    let deal = deal.clone();
    let receipt = receipt.cloned();
    state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|error| error.to_string())?;
            let operation = (|| -> Result<(), String> {
                persist_runtime_artifact(
                    conn,
                    &quote.hash,
                    &quote.payload_hash,
                    &quote.artifact_type,
                    &quote.signer,
                    quote.created_at,
                    &quote_json,
                )?;
                persist_runtime_artifact(
                    conn,
                    &deal.hash,
                    &deal.payload_hash,
                    &deal.artifact_type,
                    &deal.signer,
                    deal.created_at,
                    &deal_json,
                )?;
                if let (Some(receipt), Some(receipt_json)) =
                    (receipt.as_ref(), receipt_json.as_ref())
                {
                    persist_runtime_artifact(
                        conn,
                        &receipt.hash,
                        &receipt.payload_hash,
                        &receipt.artifact_type,
                        &receipt.signer,
                        receipt.created_at,
                        receipt_json,
                    )?;
                }
                Ok(())
            })();
            if let Err(error) = operation {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error);
            }
            conn.execute_batch("COMMIT")
                .map_err(|error| error.to_string())
        })
        .await
}

async fn sync_requester_deal_from_provider(
    state: Arc<AppState>,
    deal_id: &str,
) -> Result<requester_deals::StoredRequesterDeal, ApiFailure> {
    let lookup_deal_id = deal_id.to_string();
    let stored = state
        .db
        .with_read_conn(move |conn| requester_deals::get_requester_deal(conn, &lookup_deal_id))
        .await
        .map_err(|error| {
            tracing::error!("database error fetching deal {deal_id}: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "internal error" }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                json!({ "error": "deal not found", "deal_id": deal_id }),
            )
        })?;

    let remote: deals::DealRecord = remote_json_request(
        state.as_ref(),
        reqwest::Method::GET,
        format!(
            "{}/v1/provider/deals/{}",
            stored.provider_url,
            urlencoding::encode(deal_id)
        ),
        Option::<&()>::None,
    )
    .await?;

    if remote.quote.hash != stored.quote.hash || remote.deal.hash != stored.deal.hash {
        return Err((
            StatusCode::BAD_GATEWAY,
            json!({ "error": "provider deal does not match local requester deal" }),
        ));
    }
    if let Some(receipt) = remote.receipt.as_ref() {
        verify_provider_receipt_artifact(
            receipt,
            &stored.quote,
            &stored.deal,
            &stored.provider_id,
            &stored.deal.payload.requester_id,
            remote.result.as_ref(),
            remote.result_hash.as_deref(),
        )?;
    }

    persist_requester_artifacts(
        state.clone(),
        &remote.quote,
        &remote.deal,
        remote.receipt.as_ref(),
    )
    .await
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to persist requester artifacts", "details": error }),
        )
    })?;

    let update_id = deal_id.to_string();
    let status = remote.status.clone();
    let result = remote.result.clone();
    let result_hash = remote.result_hash.clone();
    let error = remote.error.clone();
    let receipt = remote.receipt.clone();
    let updated_at = settlement::current_unix_timestamp();
    state
        .db
        .with_write_conn(move |conn| {
            requester_deals::update_requester_deal_state(
                conn,
                &update_id,
                &status,
                result.as_ref(),
                result_hash.as_deref(),
                error.as_deref(),
                receipt.as_ref(),
                updated_at,
            )
        })
        .await
        .map_err(|error| {
            tracing::error!("database error syncing deal {deal_id}: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "internal error" }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                json!({ "error": "deal not found after sync", "deal_id": deal_id }),
            )
        })
}

pub async fn runtime_create_deal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<RuntimeCreateDealRequest>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }
    if let Err(response) = validate_workload_spec(&payload.spec) {
        return response;
    }

    let provider = match resolve_runtime_provider(state.as_ref(), &payload.provider).await {
        Ok(provider) => provider,
        Err(error) => return error_json(error.0, error.1),
    };
    let expected_workload_kind = payload.spec.workload_kind().to_string();
    let expected_workload_hash = match payload.spec.request_hash() {
        Ok(hash) => hash,
        Err(error) => {
            return error_json(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("failed to hash requested workload: {error}") }),
            );
        }
    };
    let expected_confidential_session_hash =
        payload.spec.confidential_session_hash().map(str::to_string);

    let quote = match remote_json_request::<SignedArtifact<QuotePayload>, _>(
        state.as_ref(),
        reqwest::Method::POST,
        format!("{}/v1/provider/quotes", provider.provider_url),
        Some(&CreateQuoteRequest {
            offer_id: payload.offer_id.clone(),
            requester_id: state.identity.node_id().to_string(),
            spec: payload.spec.clone(),
            max_price_sats: payload.max_price_sats,
        }),
    )
    .await
    {
        Ok(quote) => quote,
        Err(error) => return error_json(error.0, error.1),
    };

    if !protocol::verify_artifact(&quote) {
        return error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "provider quote signature verification failed" }),
        );
    }
    if quote.payload.provider_id != provider.provider_id {
        return error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "provider quote provider_id does not match selected provider" }),
        );
    }
    if quote.payload.requester_id != state.identity.node_id() {
        return error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "provider quote requester_id does not match local runtime identity" }),
        );
    }
    if quote.payload.workload_kind != expected_workload_kind {
        return error_json(
            StatusCode::BAD_GATEWAY,
            json!({
                "error": "provider quote workload_kind does not match requested workload",
                "quote_workload_kind": quote.payload.workload_kind,
                "requested_workload_kind": expected_workload_kind,
            }),
        );
    }
    if quote.payload.workload_hash != expected_workload_hash {
        return error_json(
            StatusCode::BAD_GATEWAY,
            json!({
                "error": "provider quote workload_hash does not match requested workload",
                "quote_workload_hash": quote.payload.workload_hash,
                "requested_workload_hash": expected_workload_hash,
            }),
        );
    }
    if quote.payload.confidential_session_hash != expected_confidential_session_hash {
        return error_json(
            StatusCode::BAD_GATEWAY,
            json!({
                "error": "provider quote confidential_session_hash does not match requested workload",
                "quote_confidential_session_hash": quote.payload.confidential_session_hash,
                "requested_confidential_session_hash": expected_confidential_session_hash,
            }),
        );
    }

    let success_preimage = generate_success_preimage_hex();
    let success_payment_hash = crypto::sha256_hex(
        hex::decode(&success_preimage)
            .expect("generated success preimage should always be valid hex"),
    );
    let deal_artifact = match build_runtime_requester_deal_artifact(
        state.as_ref(),
        &quote,
        &success_payment_hash,
        settlement::current_unix_timestamp(),
        quote_uses_lightning_bundle(state.as_ref(), &quote),
    ) {
        Ok(deal) => deal,
        Err(error) => {
            return error_json(StatusCode::BAD_REQUEST, json!({ "error": error }));
        }
    };

    let remote_deal = match remote_json_request::<deals::DealRecord, _>(
        state.as_ref(),
        reqwest::Method::POST,
        format!("{}/v1/provider/deals", provider.provider_url),
        Some(&CreateDealRequest {
            quote: quote.clone(),
            deal: deal_artifact.clone(),
            spec: payload.spec.clone(),
            idempotency_key: payload.idempotency_key.clone(),
            payment: payload.payment.clone(),
        }),
    )
    .await
    {
        Ok(deal) => deal,
        Err(error) => return error_json(error.0, error.1),
    };

    if remote_deal.quote.hash != quote.hash || remote_deal.deal.hash != deal_artifact.hash {
        return error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "provider deal response does not match submitted artifacts" }),
        );
    }
    if let Some(receipt) = remote_deal.receipt.as_ref()
        && let Err(error) = verify_provider_receipt_artifact(
            receipt,
            &quote,
            &deal_artifact,
            &provider.provider_id,
            &deal_artifact.payload.requester_id,
            remote_deal.result.as_ref(),
            remote_deal.result_hash.as_deref(),
        )
    {
        return error_json(error.0, error.1);
    }

    if let Err(error) = persist_requester_artifacts(
        state.clone(),
        &quote,
        &deal_artifact,
        remote_deal.receipt.as_ref(),
    )
    .await
    {
        return error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to persist requester artifacts", "details": error }),
        );
    }

    let created_at = settlement::current_unix_timestamp();
    let insert_deal_id = remote_deal.deal_id.clone();
    let provider_id = provider.provider_id.clone();
    let provider_url = provider.provider_url.clone();
    let stored = match state
        .db
        .with_write_conn(move |conn| {
            requester_deals::insert_or_get_requester_deal(
                conn,
                NewRequesterDeal {
                    deal_id: insert_deal_id,
                    idempotency_key: payload.idempotency_key.clone(),
                    provider_id,
                    provider_url,
                    spec: payload.spec.clone(),
                    quote: quote.clone(),
                    deal: deal_artifact.clone(),
                    status: remote_deal.status.clone(),
                    success_preimage,
                    created_at,
                },
            )
        })
        .await
    {
        Ok(stored) => stored,
        Err(error) => {
            tracing::error!("database error persisting deal: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "internal error" }),
            );
        }
    };

    let stored = match sync_requester_deal_from_provider(state.clone(), &stored.deal_id).await {
        Ok(stored) => stored,
        Err(_) => stored,
    };
    let mut payment_intent = None;
    if quote_uses_lightning_bundle(state.as_ref(), &stored.quote) {
        match load_runtime_requester_deal_and_payment_intent(state.clone(), &stored.deal_id).await {
            Ok((_deal, intent)) => payment_intent = intent,
            Err(error) => return error_json(error.0, error.1),
        }
    }

    (
        StatusCode::OK,
        Json(json!(RuntimeCreateDealResponse {
            provider_id: stored.provider_id.clone(),
            provider_url: stored.provider_url.clone(),
            quote: stored.quote.clone(),
            deal: stored.public_record(),
            payment_intent_path: payment_intent
                .as_ref()
                .map(|intent| runtime_payment_intent_path(&intent.deal_id)),
            payment_intent,
        })),
    )
}

pub async fn runtime_get_deal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(deal_id): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    match sync_requester_deal_from_provider(state, &deal_id).await {
        Ok(deal) => (
            StatusCode::OK,
            Json(json!(RuntimeDealResponse {
                deal: deal.public_record()
            })),
        ),
        Err(error) => error_json(error.0, error.1),
    }
}

pub async fn runtime_mock_pay_deal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(deal_id): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    if !state
        .config
        .payment_backends
        .contains(&PaymentBackend::Lightning)
        || state.config.lightning.mode != LightningMode::Mock
    {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "runtime mock-pay is only available for lightning mock mode",
                "deal_id": deal_id,
            }),
        );
    }

    let stored = match sync_requester_deal_from_provider(state.clone(), &deal_id).await {
        Ok(deal) => deal,
        Err(error) => return error_json(error.0, error.1),
    };

    if !quote_uses_lightning_bundle(state.as_ref(), &stored.quote) {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "deal does not expose a lightning mock payment flow",
                "deal_id": deal_id,
            }),
        );
    }

    let remote = match remote_json_request::<deals::DealRecord, _>(
        state.as_ref(),
        reqwest::Method::POST,
        format!(
            "{}/v1/provider/deals/{}/mock-pay",
            stored.provider_url,
            urlencoding::encode(&deal_id)
        ),
        Some(&MockPayDealRequest {
            success_preimage: stored.success_preimage.clone(),
        }),
    )
    .await
    {
        Ok(remote) => remote,
        Err(error) => return error_json(error.0, error.1),
    };

    if remote.quote.hash != stored.quote.hash || remote.deal.hash != stored.deal.hash {
        return error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "provider mock-pay response does not match local requester deal" }),
        );
    }
    if let Some(receipt) = remote.receipt.as_ref()
        && let Err(error) = verify_provider_receipt_artifact(
            receipt,
            &stored.quote,
            &stored.deal,
            &stored.provider_id,
            &stored.deal.payload.requester_id,
            remote.result.as_ref(),
            remote.result_hash.as_deref(),
        )
    {
        return error_json(error.0, error.1);
    }

    match load_runtime_requester_deal_and_payment_intent(state, &deal_id).await {
        Ok((deal, payment_intent)) => (
            StatusCode::OK,
            Json(json!(RuntimeMockPayDealResponse {
                deal,
                payment_intent_path: payment_intent
                    .as_ref()
                    .map(|intent| runtime_payment_intent_path(&intent.deal_id)),
                payment_intent,
            })),
        ),
        Err(error) => error_json(error.0, error.1),
    }
}

pub async fn runtime_accept_deal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(deal_id): Path<String>,
    Json(payload): Json<RuntimeAcceptDealRequest>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    let stored = match sync_requester_deal_from_provider(state.clone(), &deal_id).await {
        Ok(deal) => deal,
        Err(error) => return error_json(error.0, error.1),
    };
    let expected_result_hash = payload
        .expected_result_hash
        .or_else(|| stored.result_hash.clone());

    let terminal = match remote_json_request_with_client_error_passthrough::<deals::DealRecord, _>(
        state.as_ref(),
        reqwest::Method::POST,
        format!(
            "{}/v1/provider/deals/{}/accept",
            stored.provider_url,
            urlencoding::encode(&deal_id)
        ),
        Some(&ReleaseDealPreimageRequest {
            success_preimage: stored.success_preimage.clone(),
            expected_result_hash,
        }),
        true,
    )
    .await
    {
        Ok(terminal) => terminal,
        Err(error) => return error_json(error.0, error.1),
    };
    if terminal.quote.hash != stored.quote.hash || terminal.deal.hash != stored.deal.hash {
        return error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "provider accept response does not match local requester deal" }),
        );
    }
    if let Some(receipt) = terminal.receipt.as_ref()
        && let Err(error) = verify_provider_receipt_artifact(
            receipt,
            &stored.quote,
            &stored.deal,
            &stored.provider_id,
            &stored.deal.payload.requester_id,
            terminal.result.as_ref(),
            terminal.result_hash.as_deref(),
        )
    {
        return error_json(error.0, error.1);
    }

    if let Err(error) = persist_requester_artifacts(
        state.clone(),
        &terminal.quote,
        &terminal.deal,
        terminal.receipt.as_ref(),
    )
    .await
    {
        return error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to persist requester artifacts", "details": error }),
        );
    }

    let updated_at = settlement::current_unix_timestamp();
    let update_deal_id = deal_id.clone();
    let terminal_status = terminal.status.clone();
    let terminal_result = terminal.result.clone();
    let terminal_result_hash = terminal.result_hash.clone();
    let terminal_error = terminal.error.clone();
    let terminal_receipt = terminal.receipt.clone();
    let updated = match state
        .db
        .with_write_conn(move |conn| {
            requester_deals::update_requester_deal_state(
                conn,
                &update_deal_id,
                &terminal_status,
                terminal_result.as_ref(),
                terminal_result_hash.as_deref(),
                terminal_error.as_deref(),
                terminal_receipt.as_ref(),
                updated_at,
            )
        })
        .await
    {
        Ok(Some(updated)) => updated,
        Ok(None) => {
            return error_json(
                StatusCode::NOT_FOUND,
                json!({ "error": "deal not found", "deal_id": deal_id }),
            );
        }
        Err(error) => {
            tracing::error!("database error fetching deal {deal_id}: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "internal error" }),
            );
        }
    };

    (
        StatusCode::OK,
        Json(json!(RuntimeAcceptDealResponse {
            deal: updated.public_record(),
        })),
    )
}

fn deal_execution_window_secs(execution_limits: &ExecutionLimits) -> u64 {
    execution_limits.max_runtime_ms.div_ceil(1_000).max(1)
}

fn lightning_admission_window_secs(terms: &QuoteSettlementTerms) -> u64 {
    terms
        .max_base_invoice_expiry_secs
        .max(terms.max_success_hold_expiry_secs)
}

fn validate_deal_deadlines(
    quote: &SignedArtifact<QuotePayload>,
    deal: &SignedArtifact<DealPayload>,
    now: i64,
    uses_lightning_bundle: bool,
) -> Result<(), (StatusCode, &'static str)> {
    if deal.payload.admission_deadline < now {
        return Err((StatusCode::GONE, "deal admission_deadline already passed"));
    }
    if deal.payload.admission_deadline > quote.payload.expires_at {
        return Err((
            StatusCode::BAD_REQUEST,
            "deal admission_deadline exceeds the quote expiry",
        ));
    }
    if deal.payload.completion_deadline <= deal.payload.admission_deadline {
        return Err((
            StatusCode::BAD_REQUEST,
            "deal completion_deadline must be greater than admission_deadline",
        ));
    }
    if deal.payload.acceptance_deadline < deal.payload.completion_deadline {
        return Err((
            StatusCode::BAD_REQUEST,
            "deal acceptance_deadline must be greater than or equal to completion_deadline",
        ));
    }

    if uses_lightning_bundle {
        if deal.payload.acceptance_deadline > quote.payload.expires_at {
            return Err((
                StatusCode::BAD_REQUEST,
                "lightning deal acceptance_deadline exceeds the quoted settlement window",
            ));
        }

        let max_admission_deadline = now.saturating_add(lightning_admission_window_secs(
            &quote.payload.settlement_terms,
        ) as i64);
        if deal.payload.admission_deadline > max_admission_deadline {
            return Err((
                StatusCode::BAD_REQUEST,
                "lightning deal admission_deadline exceeds the quoted invoice expiry bounds",
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
fn build_requester_signed_deal_artifact(
    quote: &SignedArtifact<QuotePayload>,
    requester_signing_key: &crypto::NodeSigningKey,
    success_payment_hash: &str,
    created_at: i64,
    uses_lightning_bundle: bool,
) -> Result<SignedArtifact<DealPayload>, String> {
    let requester_id = crypto::public_key_hex(requester_signing_key);
    let execution_window_secs = deal_execution_window_secs(&quote.payload.execution_limits);
    let admission_deadline = if uses_lightning_bundle {
        lightning_quote_max_admission_deadline(quote).min(created_at.saturating_add(
            lightning_admission_window_secs(&quote.payload.settlement_terms) as i64,
        ))
    } else {
        quote.payload.expires_at
    };
    if uses_lightning_bundle && admission_deadline < created_at {
        return Err(
            "quote expiry already leaves no remaining Lightning admission window".to_string(),
        );
    }
    let completion_deadline = admission_deadline.saturating_add(execution_window_secs as i64);
    let acceptance_deadline = if uses_lightning_bundle {
        completion_deadline
            .saturating_add(quote.payload.settlement_terms.max_success_hold_expiry_secs as i64)
    } else {
        completion_deadline
    };

    if uses_lightning_bundle && acceptance_deadline > quote.payload.expires_at {
        return Err(
            "quote expiry is too short for the Lightning admission, execution, and acceptance windows"
                .to_string(),
        );
    }

    protocol::sign_artifact(
        &requester_id,
        |message| crypto::sign_message_hex(requester_signing_key, message),
        ARTIFACT_KIND_DEAL,
        created_at,
        DealPayload {
            requester_id: requester_id.clone(),
            provider_id: quote.payload.provider_id.clone(),
            quote_hash: quote.hash.clone(),
            workload_hash: quote.payload.workload_hash.clone(),
            confidential_session_hash: quote.payload.confidential_session_hash.clone(),
            extension_refs: Vec::new(),
            authority_ref: None,
            supersedes_deal_hash: None,
            client_nonce: None,
            success_payment_hash: success_payment_hash.to_string(),
            admission_deadline,
            completion_deadline,
            acceptance_deadline,
        },
    )
}

fn quote_uses_lightning_bundle(state: &AppState, quote: &SignedArtifact<QuotePayload>) -> bool {
    let total_msat = quote.payload.settlement_terms.base_fee_msat
        + quote.payload.settlement_terms.success_fee_msat;
    state
        .config
        .payment_backends
        .contains(&PaymentBackend::Lightning)
        && total_msat > 0
        && quote.payload.settlement_terms.method == "lightning.base_fee_plus_success_fee.v1"
}

pub async fn runtime_nostr_provider_publications(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    let descriptor = match current_descriptor_artifact(state.as_ref()).await {
        Ok(descriptor) => descriptor,
        Err(error) => {
            tracing::error!("Failed to build descriptor for Nostr summaries: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to build descriptor summary" }),
            );
        }
    };
    let offers = match current_offer_artifacts(state.as_ref()).await {
        Ok(offers) => offers,
        Err(error) => {
            tracing::error!("Failed to build offers for Nostr summaries: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to build offer summaries" }),
            );
        }
    };

    let publication_pubkey = state.identity.nostr_publication_key_hex().to_string();
    let descriptor_summary =
        match nostr::build_descriptor_summary_event(&descriptor, &publication_pubkey, |message| {
            state.identity.sign_nostr_publication_message_hex(message)
        }) {
            Ok(event) => event,
            Err(error) => {
                return error_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to build descriptor summary: {error}") }),
                );
            }
        };
    let mut offer_summaries = Vec::new();
    for offer in offers {
        match nostr::build_offer_summary_event(
            &descriptor,
            &offer,
            &publication_pubkey,
            |message| state.identity.sign_nostr_publication_message_hex(message),
        ) {
            Ok(event) => offer_summaries.push(event),
            Err(error) => {
                return error_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to build offer summary: {error}") }),
                );
            }
        }
    }

    (
        StatusCode::OK,
        Json(json!(RuntimeNostrProviderPublicationsResponse {
            descriptor_summary,
            offer_summaries,
        })),
    )
}

pub async fn runtime_nostr_receipt_publication(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(deal_id): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    let deal = match state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &deal_id))
        .await
    {
        Ok(Some(deal)) => deal,
        Ok(None) => return error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" })),
        Err(error) => {
            tracing::error!("Failed to load deal for Nostr receipt summary: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            );
        }
    };

    let Some(receipt) = deal.receipt.as_ref() else {
        return error_json(
            StatusCode::CONFLICT,
            json!({ "error": "deal does not have a terminal receipt yet" }),
        );
    };

    match nostr::build_receipt_summary_event(
        receipt,
        state.identity.nostr_publication_key_hex(),
        |message| state.identity.sign_nostr_publication_message_hex(message),
    ) {
        Ok(receipt_summary) => (
            StatusCode::OK,
            Json(json!(RuntimeNostrReceiptPublicationResponse {
                receipt_summary
            })),
        ),
        Err(error) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to build receipt summary: {error}") }),
        ),
    }
}

pub async fn runtime_deal_payment_intent(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(deal_id): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    match load_runtime_requester_deal_and_payment_intent(state, &deal_id).await {
        Ok((_deal, Some(payment_intent))) => (
            StatusCode::OK,
            Json(json!(RuntimeDealPaymentIntentResponse { payment_intent })),
        ),
        Ok((_deal, None)) => error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "deal does not expose a lightning payment intent",
                "deal_id": deal_id,
            }),
        ),
        Err(error) => error_json(error.0, error.1),
    }
}

pub async fn runtime_archive_subject(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((subject_kind, subject_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    match build_runtime_archive_export(state.as_ref(), &subject_kind, &subject_id).await {
        Ok(Some(export)) => (StatusCode::OK, Json(json!(export))),
        Ok(None) => error_json(
            StatusCode::NOT_FOUND,
            json!({
                "error": "archive subject not found",
                "subject_kind": subject_kind,
                "subject_id": subject_id,
            }),
        ),
        Err(error) => {
            let (status, client_error) = if error.contains("unsupported archive subject kind") {
                (
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "unsupported archive subject kind" }),
                )
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "failed to build archive export" }),
                )
            };
            tracing::error!(
                "Failed to build archive export for {} {}: {error}",
                subject_kind,
                subject_id
            );
            error_json(status, client_error)
        }
    }
}

pub async fn protocol_descriptor(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match current_descriptor_artifact(state.as_ref()).await {
        Ok(descriptor) => (StatusCode::OK, Json(json!(descriptor))),
        Err(error) => {
            tracing::error!("Failed to build protocol descriptor: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to build descriptor" }),
            )
        }
    }
}

pub async fn list_offers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match current_offer_artifacts(state.as_ref()).await {
        Ok(offers) => (StatusCode::OK, Json(json!({ "offers": offers }))),
        Err(error) => {
            tracing::error!("Failed to build offers: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to build offers" }),
            )
        }
    }
}

pub async fn list_provider_services(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match current_service_records(state.as_ref(), false, false).await {
        Ok(services) => {
            (StatusCode::OK, Json(ProviderServicesResponse { services })).into_response()
        }
        Err(error) => {
            tracing::error!("Failed to build provider services: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to build provider services" }),
            )
            .into_response()
        }
    }
}

pub async fn get_provider_service(
    State(state): State<Arc<AppState>>,
    Path(service_id): Path<String>,
) -> impl IntoResponse {
    match provider_service_record(state.as_ref(), &service_id, false, false).await {
        Ok(Some(service)) => {
            (StatusCode::OK, Json(ProviderServiceResponse { service })).into_response()
        }
        Ok(None) => error_json(
            StatusCode::NOT_FOUND,
            json!({ "error": "service not found", "service_id": service_id }),
        )
        .into_response(),
        Err(error) => {
            tracing::error!("Failed to build provider service {service_id}: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to load provider service" }),
            )
            .into_response()
        }
    }
}

pub async fn get_feed(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FeedQuery>,
) -> impl IntoResponse {
    if let Err(error) = ensure_protocol_root_artifacts(state.as_ref()).await {
        tracing::error!("Failed to publish root protocol artifacts: {error}");
        return error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to build protocol feed" }),
        );
    }

    let applied_cursor = match query.cursor {
        Some(cursor) if cursor < 0 => {
            return error_json(
                StatusCode::BAD_REQUEST,
                json!({ "error": "cursor must be greater than or equal to zero" }),
            );
        }
        Some(cursor) => cursor,
        None => 0,
    };

    let limit = match query.limit {
        Some(0) => {
            return error_json(
                StatusCode::BAD_REQUEST,
                json!({ "error": "limit must be greater than zero" }),
            );
        }
        Some(limit) => limit.min(100),
        None => 50,
    };

    let current_public_feed = match current_public_feed_artifacts(state.as_ref()).await {
        Ok(current_public_feed) => current_public_feed,
        Err(error) => {
            tracing::error!("Failed to build public feed snapshot: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to build protocol feed" }),
            );
        }
    };

    match state
        .db
        .with_read_conn(move |conn| {
            list_public_feed_artifacts(conn, applied_cursor, limit, &current_public_feed)
        })
        .await
    {
        Ok((artifacts, has_more, next_cursor)) => (
            StatusCode::OK,
            Json(json!(FeedResponse {
                artifacts,
                cursor_type: "artifact_sequence".to_string(),
                cursor_semantics: "exclusive_after".to_string(),
                applied_cursor,
                page_size: limit,
                has_more,
                next_cursor,
            })),
        ),
        Err(error) => {
            tracing::error!("Failed to read feed: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        }
    }
}

#[derive(Clone)]
struct CurrentPublicFeedArtifacts {
    descriptor_hash: String,
    offer_hashes: HashSet<String>,
}

impl CurrentPublicFeedArtifacts {
    fn contains(&self, artifact: &db::LedgerArtifact) -> bool {
        match artifact.kind.as_str() {
            ARTIFACT_KIND_DESCRIPTOR => artifact.hash == self.descriptor_hash,
            ARTIFACT_KIND_OFFER => self.offer_hashes.contains(artifact.hash.as_str()),
            ARTIFACT_KIND_RECEIPT => true,
            _ => false,
        }
    }
}

async fn current_public_feed_artifacts(
    state: &AppState,
) -> Result<CurrentPublicFeedArtifacts, String> {
    let descriptor_hash = current_descriptor_artifact(state).await?.hash;
    let offer_hashes = current_offer_artifacts(state)
        .await?
        .into_iter()
        .map(|offer| offer.hash)
        .collect::<HashSet<_>>();
    Ok(CurrentPublicFeedArtifacts {
        descriptor_hash,
        offer_hashes,
    })
}

fn list_public_feed_artifacts(
    conn: &rusqlite::Connection,
    applied_cursor: i64,
    limit: usize,
    current_public_feed: &CurrentPublicFeedArtifacts,
) -> Result<(Vec<db::LedgerArtifact>, bool, Option<i64>), String> {
    let limit = limit.clamp(1, 100);
    let mut artifacts = Vec::with_capacity(limit);
    let mut scan_cursor = applied_cursor;
    let mut next_cursor = applied_cursor;
    let mut advanced = false;
    let mut page_full = false;
    let mut has_more = false;

    loop {
        let (batch, raw_has_more) = db::list_artifacts(conn, Some(scan_cursor), 100)?;
        if batch.is_empty() {
            break;
        }

        for artifact in batch {
            scan_cursor = artifact.cursor;
            advanced = true;
            let is_public = current_public_feed.contains(&artifact);

            if page_full {
                if is_public {
                    has_more = true;
                    break;
                }
                next_cursor = scan_cursor;
                continue;
            }

            next_cursor = scan_cursor;
            if is_public {
                artifacts.push(artifact);
                if artifacts.len() == limit {
                    page_full = true;
                }
            }
        }

        if has_more || !raw_has_more {
            break;
        }
    }

    Ok((artifacts, has_more, advanced.then_some(next_cursor)))
}

pub async fn get_artifact(
    State(state): State<Arc<AppState>>,
    Path(artifact_hash): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = ensure_protocol_root_artifacts(state.as_ref()).await {
        tracing::error!("Failed to publish root protocol artifacts: {error}");
        return error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to build protocol artifacts" }),
        );
    }

    let lookup_hash = artifact_hash.clone();
    match state
        .db
        .with_read_conn(move |conn| db::get_artifact_by_hash(conn, &lookup_hash))
        .await
    {
        Ok(Some(artifact)) => (StatusCode::OK, Json(json!(artifact))),
        Ok(None) => error_json(
            StatusCode::NOT_FOUND,
            json!({ "error": "artifact not found", "artifact_hash": artifact_hash }),
        ),
        Err(error) => {
            tracing::error!("Failed to fetch artifact {artifact_hash}: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        }
    }
}

pub async fn create_quote(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateQuoteRequest>,
) -> impl IntoResponse {
    match create_quote_record(state.clone(), payload).await {
        Ok(quote) => (StatusCode::CREATED, Json(json!(quote))),
        Err(error) => error_json(error.0, error.1),
    }
}

pub async fn create_deal(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateDealRequest>,
) -> impl IntoResponse {
    match create_deal_record(state.clone(), payload).await {
        Ok((deal, status)) => (status, Json(json!(deal))),
        Err(error) => error_json(error.0, error.1),
    }
}

pub async fn get_deal_status(
    State(state): State<Arc<AppState>>,
    Path(deal_id): Path<String>,
) -> impl IntoResponse {
    let lookup_deal_id = deal_id.clone();
    let mut deal = match state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
        .await
    {
        Ok(Some(deal)) => deal,
        Ok(None) => {
            return error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" }));
        }
        Err(error) => {
            tracing::error!("Failed to fetch deal {deal_id}: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            );
        }
    };

    if deal.payment_method.as_deref() == Some("lightning") {
        if let Err(error) = sync_and_maybe_promote_lightning_deal(state.clone(), &deal).await {
            tracing::error!("Failed to sync Lightning deal {deal_id}: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to sync lightning settlement state" }),
            );
        }

        let reload_deal_id = deal_id.clone();
        deal = match state
            .db
            .with_read_conn(move |conn| deals::get_deal(conn, &reload_deal_id))
            .await
        {
            Ok(Some(deal)) => deal,
            Ok(None) => {
                return error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" }));
            }
            Err(error) => {
                tracing::error!("Failed to refetch deal {deal_id}: {error}");
                return error_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "database error" }),
                );
            }
        };
    }

    (StatusCode::OK, Json(json!(deal.public_record())))
}

pub async fn get_deal_invoice_bundle(
    State(state): State<Arc<AppState>>,
    Path(deal_id): Path<String>,
) -> impl IntoResponse {
    let lookup_deal_id = deal_id.clone();
    let deal = match state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
        .await
    {
        Ok(Some(deal)) => deal,
        Ok(None) => {
            return error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" }));
        }
        Err(error) => {
            tracing::error!("Failed to fetch deal {deal_id}: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            );
        }
    };

    match sync_and_maybe_promote_lightning_deal(state.clone(), &deal).await {
        Ok(Some(bundle)) => {
            let report = settlement::validate_lightning_invoice_bundle(
                &bundle.bundle,
                &deal.quote,
                &deal.artifact,
                None,
            );
            if !report.valid {
                return error_json(
                    StatusCode::CONFLICT,
                    json!({
                        "error": "stored lightning invoice bundle failed commitment validation",
                        "deal_id": deal_id,
                        "validation": report,
                    }),
                );
            }

            (StatusCode::OK, Json(json!(bundle)))
        }
        Ok(None) => error_json(
            StatusCode::NOT_FOUND,
            json!({ "error": "lightning invoice bundle not found", "deal_id": deal_id }),
        ),
        Err(error) => {
            tracing::error!("Failed to fetch invoice bundle for deal {deal_id}: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        }
    }
}

pub async fn mock_pay_deal(
    State(state): State<Arc<AppState>>,
    Path(deal_id): Path<String>,
    Json(payload): Json<MockPayDealRequest>,
) -> impl IntoResponse {
    if !state
        .config
        .payment_backends
        .contains(&PaymentBackend::Lightning)
        || state.config.lightning.mode != LightningMode::Mock
    {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "provider mock-pay is only available for lightning mock mode",
                "deal_id": deal_id,
            }),
        );
    }

    let lookup_deal_id = deal_id.clone();
    let deal = match state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
        .await
    {
        Ok(Some(deal)) => deal,
        Ok(None) => {
            return error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" }));
        }
        Err(error) => {
            tracing::error!("Failed to load deal {deal_id} for mock pay: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            );
        }
    };

    if deal.payment_method.as_deref() != Some("lightning") {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "deal does not use lightning settlement", "deal_id": deal_id }),
        );
    }

    let (_success_preimage, _payment_lock) =
        match validate_success_preimage_for_deal(&deal_id, &deal, payload.success_preimage) {
            Ok(validated) => validated,
            Err(error) => return error_json(error.0, error.1),
        };

    let bundle =
        match load_validated_lightning_bundle_for_deal(state.clone(), &deal_id, &deal).await {
            Ok(bundle) => bundle,
            Err(error) => return error_json(error.0, error.1),
        };

    if deal.status != deals::DEAL_STATUS_PAYMENT_PENDING {
        if settlement::lightning_bundle_is_funded(&bundle) {
            let reload_deal_id = deal_id.clone();
            return match state
                .db
                .with_read_conn(move |conn| deals::get_deal(conn, &reload_deal_id))
                .await
            {
                Ok(Some(updated)) => (StatusCode::OK, Json(json!(updated.public_record()))),
                Ok(None) => error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" })),
                Err(error) => {
                    tracing::error!("Failed to reload deal {deal_id} after mock pay: {error}");
                    error_json(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({ "error": "database error" }),
                    )
                }
            };
        }

        return error_json(
            StatusCode::CONFLICT,
            json!({
                "error": "deal is not waiting for mock lightning admission",
                "deal_id": deal_id,
                "status": deal.status,
            }),
        );
    }

    let funded_bundle = if bundle.base_state == InvoiceBundleLegState::Settled
        && matches!(
            bundle.success_state,
            InvoiceBundleLegState::Accepted | InvoiceBundleLegState::Settled
        ) {
        bundle
    } else {
        match settlement::update_lightning_invoice_bundle_states(
            state.as_ref(),
            &bundle.session_id,
            InvoiceBundleLegState::Settled,
            InvoiceBundleLegState::Accepted,
        )
        .await
        {
            Ok(Some(updated)) => updated,
            Ok(None) => {
                return error_json(
                    StatusCode::NOT_FOUND,
                    json!({
                        "error": "lightning invoice bundle disappeared during mock payment",
                        "deal_id": deal_id,
                    }),
                );
            }
            Err(error) => {
                tracing::error!(
                    "Failed to update Lightning bundle during mock pay for deal {deal_id}: {error}"
                );
                return error_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "failed to mark lightning bundle as funded" }),
                );
            }
        }
    };

    if let Err(error) = promote_lightning_deal_if_funded(state.clone(), &deal, &funded_bundle).await
    {
        tracing::error!("Failed to promote mock-funded deal {deal_id}: {error}");
        return error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to promote lightning deal after mock payment" }),
        );
    }

    let reload_deal_id = deal_id.clone();
    match state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &reload_deal_id))
        .await
    {
        Ok(Some(updated)) => (StatusCode::OK, Json(json!(updated.public_record()))),
        Ok(None) => error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" })),
        Err(error) => {
            tracing::error!("Failed to reload deal {deal_id} after mock pay: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        }
    }
}

pub async fn release_deal_preimage(
    State(state): State<Arc<AppState>>,
    Path(deal_id): Path<String>,
    Json(payload): Json<ReleaseDealPreimageRequest>,
) -> impl IntoResponse {
    let success_preimage = payload.success_preimage;
    let expected_result_hash = match payload.expected_result_hash {
        Some(expected_result_hash) => {
            match normalize_hex_value("expected_result_hash", expected_result_hash, 64) {
                Ok(expected_result_hash) => Some(expected_result_hash),
                Err(error) => return error_json(StatusCode::BAD_REQUEST, error),
            }
        }
        None => None,
    };

    let lookup_deal_id = deal_id.clone();
    let deal = match state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
        .await
    {
        Ok(Some(deal)) => deal,
        Ok(None) => {
            return error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" }));
        }
        Err(error) => {
            tracing::error!("Failed to load deal {deal_id} for preimage release: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            );
        }
    };

    if deal.payment_method.as_deref() != Some("lightning") {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "deal does not use lightning settlement", "deal_id": deal_id }),
        );
    }

    if deal.status == deals::DEAL_STATUS_SUCCEEDED {
        return (StatusCode::OK, Json(json!(deal.public_record())));
    }

    if deal.status != deals::DEAL_STATUS_RESULT_READY {
        return error_json(
            StatusCode::CONFLICT,
            json!({
                "error": "deal is not ready for requester preimage release",
                "deal_id": deal_id,
                "status": deal.status,
            }),
        );
    }
    if settlement::current_unix_timestamp() > deal.artifact.payload.acceptance_deadline {
        return error_json(
            StatusCode::GONE,
            json!({
                "error": "deal acceptance_deadline already passed",
                "deal_id": deal_id,
                "acceptance_deadline": deal.artifact.payload.acceptance_deadline,
            }),
        );
    }

    let Some(result_hash) = deal.result_hash.clone() else {
        return error_json(
            StatusCode::CONFLICT,
            json!({ "error": "deal does not have a result_hash yet", "deal_id": deal_id }),
        );
    };
    if expected_result_hash
        .as_deref()
        .is_some_and(|expected| expected != result_hash)
    {
        return error_json(
            StatusCode::CONFLICT,
            json!({
                "error": "expected_result_hash does not match the persisted deal result",
                "deal_id": deal_id,
                "expected_result_hash": expected_result_hash,
                "result_hash": result_hash,
            }),
        );
    }

    let (success_preimage, payment_lock) =
        match validate_success_preimage_for_deal(&deal_id, &deal, success_preimage) {
            Ok(validated) => validated,
            Err(error) => return error_json(error.0, error.1),
        };

    let bundle =
        match load_validated_lightning_bundle_for_deal(state.clone(), &deal_id, &deal).await {
            Ok(bundle) => bundle,
            Err(error) => return error_json(error.0, error.1),
        };

    if !settlement::lightning_bundle_can_settle_success(&bundle) {
        return error_json(
            StatusCode::CONFLICT,
            json!({
                "error": "lightning invoice bundle is not ready for requester release",
                "deal_id": deal_id,
                "bundle_state": {
                    "base_state": bundle.base_state,
                    "success_state": bundle.success_state,
                }
            }),
        );
    }

    let settled_bundle = if bundle.success_state == InvoiceBundleLegState::Settled {
        bundle
    } else {
        match settlement::settle_lightning_success_hold_invoice(
            state.as_ref(),
            &bundle,
            &success_preimage,
        )
        .await
        {
            Ok(bundle) => bundle,
            Err(error) => {
                tracing::error!(
                    "Failed to settle success hold invoice for deal {}: {error}",
                    deal.deal_id
                );
                return error_json(
                    StatusCode::CONFLICT,
                    json!({ "error": "failed to settle success hold invoice", "details": error }),
                );
            }
        }
    };

    let release_evidence = json!({
        "session_id": settled_bundle.session_id,
        "success_preimage": success_preimage,
        "payment_hash": payment_lock.token_hash,
    });
    match persist_lightning_success_receipt(
        state.clone(),
        &deal,
        &settled_bundle,
        Some(release_evidence),
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => {}
        Err(error) => {
            tracing::error!(
                "Failed to persist released Lightning receipt for deal {deal_id}: {error}"
            );
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to persist receipt" }),
            );
        }
    }

    let reload_deal_id = deal_id.clone();
    match state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &reload_deal_id))
        .await
    {
        Ok(Some(updated)) => (StatusCode::OK, Json(json!(updated.public_record()))),
        Ok(None) => error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" })),
        Err(error) => {
            tracing::error!("Failed to reload deal {deal_id} after receipt finalization: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        }
    }
}

pub async fn verify_receipt(Json(payload): Json<VerifyReceiptRequest>) -> impl IntoResponse {
    let valid = protocol::verify_artifact(&payload.receipt)
        && protocol::validate_receipt_artifact(&payload.receipt).is_ok();
    (
        StatusCode::OK,
        Json(json!({
            "valid": valid,
            "receipt_hash": payload.receipt.hash,
            "deal_state": payload.receipt.payload.deal_state
        })),
    )
}

pub async fn verify_curated_list(
    Json(payload): Json<VerifyCuratedListRequest>,
) -> impl IntoResponse {
    let valid = protocol::verify_artifact(&payload.curated_list);
    (
        StatusCode::OK,
        Json(json!(VerifyCuratedListResponse {
            valid,
            list_hash: payload.curated_list.hash,
            curator_id: payload.curated_list.payload.curator_id,
            list_id: payload.curated_list.payload.list_id,
            expires_at: payload.curated_list.payload.expires_at,
        })),
    )
}

pub async fn verify_nostr_event(Json(payload): Json<VerifyNostrEventRequest>) -> impl IntoResponse {
    let valid = nostr::verify_event(&payload.event);
    (
        StatusCode::OK,
        Json(json!(VerifyNostrEventResponse {
            valid,
            event_id: payload.event.id,
            pubkey: payload.event.pubkey,
            kind: payload.event.kind,
        })),
    )
}

pub async fn verify_invoice_bundle(
    Json(payload): Json<VerifyInvoiceBundleRequest>,
) -> impl IntoResponse {
    let expected_requester_id = match payload.requester_id {
        Some(requester_id) => match normalize_hex_value("requester_id", requester_id, 64) {
            Ok(requester_id) => Some(requester_id),
            Err(error) => return error_json(StatusCode::BAD_REQUEST, error),
        },
        None => None,
    };

    let report = settlement::validate_lightning_invoice_bundle(
        &payload.bundle,
        &payload.quote,
        &payload.deal,
        expected_requester_id.as_deref(),
    );

    (
        StatusCode::OK,
        Json(json!(VerifyInvoiceBundleResponse {
            valid: report.valid,
            bundle_hash: report.bundle_hash,
            quote_hash: report.quote_hash,
            deal_hash: report.deal_hash,
            expected_requester_id: report.expected_requester_id,
            issues: report.issues,
        })),
    )
}

pub async fn publish_event(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PublishRequest>,
) -> impl IntoResponse {
    let event = payload.event;

    if event.content.len() > MAX_EVENT_CONTENT_BYTES {
        return error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({ "error": "event content too large" }),
        );
    }

    tracing::info!("Received Event Publish: {:?}", event.kind);

    if event.id != expected_node_event_id(&event) {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid event id" }),
        );
    }

    if !crypto::verify_message(&event.pubkey, &event.sig, &event.canonical_signing_bytes()) {
        tracing::warn!("Invalid signature for event: {}", event.id);
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid signature" }),
        );
    }

    match insert_event_db(state.as_ref(), event).await {
        Ok(true) => {}
        Ok(false) => {
            return error_json(
                StatusCode::CONFLICT,
                json!({ "error": "event already exists" }),
            );
        }
        Err(error) => {
            tracing::error!("Failed to insert event: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            );
        }
    }

    (
        StatusCode::CREATED,
        Json(json!({
            "status": "success",
            "message": "event parsed and stored successfully"
        })),
    )
}

pub async fn query_events(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<QueryRequest>,
) -> impl IntoResponse {
    tracing::info!("Received Event Query for {} kinds", payload.kinds.len());

    if let Err(response) = validate_event_query_kinds(&payload.kinds) {
        return response;
    }

    if let Some(response) = legacy_paid_endpoint_requires_protocol_deal(
        state.as_ref(),
        ServiceId::EventsQuery,
        "/v1/node/events/query",
    ) {
        return response;
    }

    let reservation = match settlement::prepare_payment(
        state.as_ref(),
        ServiceId::EventsQuery,
        payload.payment,
        None,
    )
    .await
    {
        Ok(reservation) => reservation,
        Err(error) => return error_json(error.status_code(), error.details()),
    };

    match query_events_with_capacity(state.as_ref(), payload.kinds, payload.limit).await {
        Ok(events) => {
            let receipt = match finalize_payment(state.as_ref(), reservation).await {
                Ok(receipt) => receipt,
                Err(response) => return response,
            };

            (
                StatusCode::OK,
                Json(json!({
                    "events": events,
                    "cursor": null,
                    "payment_receipt": receipt
                })),
            )
        }
        Err(e) => {
            let _ = release_payment(state.as_ref(), reservation).await;
            tracing::error!("Database query failed: {}", e);
            if e.contains(EVENTS_QUERY_CAPACITY_EXHAUSTED) {
                error_json(
                    StatusCode::SERVICE_UNAVAILABLE,
                    events_query_capacity_error(),
                )
            } else {
                error_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "database error" }),
                )
            }
        }
    }
}

pub async fn execute_wasm(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ExecuteWasmRequest>,
) -> impl IntoResponse {
    tracing::info!("Received Wasm Execution Request");

    if let Err(response) = validate_wasm_submission(&payload.submission) {
        return response;
    }

    if payload.submission.workload.abi_version == wasm::WASM_HOST_JSON_ABI_V1 {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": format!(
                    "{} requires the /v1/provider/quotes and /v1/provider/deals protocol flow",
                    wasm::WASM_HOST_JSON_ABI_V1
                ),
                "abi_version": wasm::WASM_HOST_JSON_ABI_V1,
                "quote_path": "/v1/provider/quotes",
                "deal_path": "/v1/provider/deals",
            }),
        );
    }

    if let Some(response) = legacy_paid_endpoint_requires_protocol_deal(
        state.as_ref(),
        ServiceId::ExecuteWasm,
        "/v1/node/execute/wasm",
    ) {
        return response;
    }

    let reservation = match settlement::prepare_payment(
        state.as_ref(),
        ServiceId::ExecuteWasm,
        payload.payment,
        None,
    )
    .await
    {
        Ok(reservation) => reservation,
        Err(error) => return error_json(error.status_code(), error.details()),
    };

    match run_job_spec_now(
        state.as_ref(),
        JobSpec::Wasm {
            submission: payload.submission,
        },
    )
    .await
    {
        Ok(result) => {
            let receipt = match finalize_payment(state.as_ref(), reservation).await {
                Ok(receipt) => receipt,
                Err(response) => return response,
            };

            (
                StatusCode::OK,
                Json(json!({
                    "status": "success",
                    "result": result,
                    "payment_receipt": receipt
                })),
            )
        }
        Err(error_message) => {
            let _ = release_payment(state.as_ref(), reservation).await;
            tracing::error!("Wasm Execution Failed: {}", error_message);
            error_json(StatusCode::BAD_REQUEST, json!({ "error": error_message }))
        }
    }
}

pub async fn create_job(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateJobRequest>,
) -> impl IntoResponse {
    if let Err(response) = validate_job_spec(&payload.spec) {
        return response;
    }

    if let Some(response) = legacy_paid_endpoint_requires_protocol_deal(
        state.as_ref(),
        payload.spec.service_id(),
        "/v1/node/jobs",
    ) {
        return response;
    }

    let idempotency_key = match normalize_idempotency_key(payload.idempotency_key) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let request_hash = match payload.spec.request_hash() {
        Ok(hash) => hash,
        Err(error) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to hash job request: {error}") }),
            );
        }
    };

    let service_id = payload.spec.service_id();

    if let Some(existing) = match find_existing_job(state.as_ref(), idempotency_key.clone()).await {
        Ok(job) => job,
        Err(response) => return response,
    } {
        if existing.request_hash != request_hash || existing.service_id != service_id.as_str() {
            return error_json(
                StatusCode::CONFLICT,
                json!({ "error": "idempotency key reused with different payload" }),
            );
        }

        return (StatusCode::OK, Json(json!(existing.public_record())));
    }

    let job_id = jobs::new_job_id();
    match settlement::prepare_payment(
        state.as_ref(),
        service_id,
        payload.payment,
        Some(job_id.clone()),
    )
    .await
    {
        Ok(_) => {}
        Err(error) => return error_json(error.status_code(), error.details()),
    }

    let new_job = NewJob {
        job_id: job_id.clone(),
        idempotency_key: idempotency_key.clone(),
        request_hash,
        service_id: service_id.as_str().to_string(),
        spec: payload.spec,
        created_at: settlement::current_unix_timestamp(),
    };

    let insert_outcome = match state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<jobs::InsertJobOutcome, String> {
                let insert_outcome = jobs::insert_or_get_job(conn, new_job)?;
                if insert_outcome.created {
                    let evidence_hash = db::insert_execution_evidence(
                        conn,
                        "job",
                        &insert_outcome.job.job_id,
                        "workload_spec",
                        &insert_outcome.job.spec,
                        insert_outcome.job.created_at,
                    )?;
                    jobs::set_job_workload_evidence_hash(
                        conn,
                        &insert_outcome.job.job_id,
                        &evidence_hash,
                    )?;
                }
                Ok(insert_outcome)
            })();

            let result = match operation {
                Ok(result) => result,
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(error);
                }
            };

            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(result)
        })
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            let status = if error.contains("idempotency key reused") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return error_json(status, json!({ "error": error }));
        }
    };

    if !insert_outcome.created {
        return (
            StatusCode::OK,
            Json(json!(insert_outcome.job.public_record())),
        );
    }

    tokio::spawn(process_job(state.clone(), job_id));
    (
        StatusCode::ACCEPTED,
        Json(json!(insert_outcome.job.public_record())),
    )
}

pub async fn get_job_status(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let lookup_job_id = job_id.clone();
    match state
        .db
        .with_read_conn(move |conn| jobs::get_job(conn, &lookup_job_id))
        .await
    {
        Ok(Some(job)) => (StatusCode::OK, Json(json!(job.public_record()))),
        Ok(None) => error_json(StatusCode::NOT_FOUND, json!({ "error": "job not found" })),
        Err(error) => {
            tracing::error!("Failed to fetch job {job_id}: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        }
    }
}

pub(crate) fn error_json(
    status: StatusCode,
    body: serde_json::Value,
) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(body))
}

fn legacy_paid_endpoint_requires_protocol_deal(
    state: &AppState,
    service_id: ServiceId,
    endpoint_path: &str,
) -> Option<(StatusCode, Json<serde_json::Value>)> {
    let price_sats = state.pricing.price_for(service_id);
    if !state
        .config
        .payment_backends
        .contains(&PaymentBackend::Lightning)
        || price_sats == 0
    {
        return None;
    }

    Some(error_json(
        StatusCode::CONFLICT,
        json!({
            "error": format!(
                "priced {} requests must use /v1/provider/quotes and /v1/provider/deals when the lightning backend is active",
                service_id.as_str()
            ),
            "service_id": service_id.as_str(),
            "price_sats": price_sats,
            "payment_backend": "lightning",
            "legacy_endpoint": endpoint_path,
            "quote_path": "/v1/provider/quotes",
            "deal_path": "/v1/provider/deals",
            "requires_protocol_deal": true
        }),
    ))
}

async fn handle_timeout_error(_: BoxError) -> impl IntoResponse {
    error_json(
        StatusCode::REQUEST_TIMEOUT,
        json!({ "error": "request timed out" }),
    )
}

async fn insert_event_db(state: &AppState, event: NodeEventEnvelope) -> Result<bool, String> {
    if let Some(ref writer) = state.event_batch_writer {
        return writer.insert(event).await;
    }
    state
        .db
        .with_write_conn(move |conn| db::insert_event(conn, &event))
        .await
}

const EVENTS_QUERY_CAPACITY_EXHAUSTED: &str = "events query capacity exhausted";

fn events_query_capacity_error() -> serde_json::Value {
    json!({
        "error": EVENTS_QUERY_CAPACITY_EXHAUSTED,
        "code": "capacity_exhausted",
    })
}

fn try_acquire_events_query_permit(
    state: &AppState,
) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
    state
        .events_query_semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| EVENTS_QUERY_CAPACITY_EXHAUSTED.to_string())
}

async fn query_events_db(
    state: &AppState,
    kinds: Vec<String>,
    limit: Option<usize>,
) -> Result<Vec<NodeEventEnvelope>, String> {
    if kinds.len() > db::MAX_EVENT_QUERY_KINDS {
        return Err(format!(
            "events query exceeds maximum of {} kinds",
            db::MAX_EVENT_QUERY_KINDS
        ));
    }

    state
        .db
        .with_read_conn(move |conn| db::query_events_by_kind(conn, &kinds, limit))
        .await
}

async fn query_events_with_capacity(
    state: &AppState,
    kinds: Vec<String>,
    limit: Option<usize>,
) -> Result<Vec<NodeEventEnvelope>, String> {
    let _permit = try_acquire_events_query_permit(state)?;
    query_events_db(state, kinds, limit).await
}

async fn dispatch_builtin_workload(
    state: &AppState,
    execution: &ExecutionWorkload,
    caller_id: Option<&str>,
) -> Result<Value, String> {
    let builtin_name = execution
        .builtin_name
        .as_deref()
        .ok_or("builtin execution requires builtin_name")?;
    if let Some(handler) = state.builtin_services.get(builtin_name) {
        let mut input = execution.input.clone();
        // Inject verified caller identity so handlers can enforce authorization.
        // This field is set by the execution engine, not by the user.
        if let (Some(obj), Some(cid)) = (input.as_object_mut(), caller_id) {
            obj.insert("_caller_id".to_string(), Value::String(cid.to_string()));
        }
        handler.execute(input).await
    } else if let Some((kinds, limit)) = execution.events_query_params() {
        Ok(json!({
            "events": query_events_with_capacity(state, kinds, limit).await?,
            "cursor": null
        }))
    } else {
        Err(format!("unsupported builtin service: {builtin_name}"))
    }
}

async fn ensure_protocol_root_artifacts(state: &AppState) -> Result<(), String> {
    current_descriptor_artifact(state).await?;
    current_offer_artifacts(state).await?;
    Ok(())
}

fn nostr_publication_linked_identity(state: &AppState) -> Result<LinkedIdentity, String> {
    let scope = vec![protocol::LINKED_IDENTITY_SCOPE_PUBLICATION_NOSTR.to_string()];
    let created_at = state.identity.nostr_publication_created_at();
    let identity = state.identity.nostr_publication_key_hex().to_string();
    let challenge = protocol::linked_identity_challenge_bytes(
        state.identity.node_id(),
        protocol::LINKED_IDENTITY_KIND_NOSTR,
        &identity,
        &scope,
        created_at,
        None,
    )?;

    Ok(LinkedIdentity {
        identity_kind: protocol::LINKED_IDENTITY_KIND_NOSTR.to_string(),
        identity,
        scope,
        created_at,
        expires_at: None,
        signature_algorithm: protocol::LINKED_IDENTITY_SIGNATURE_ALGORITHM_BIP340.to_string(),
        linked_signature: state
            .identity
            .sign_nostr_publication_message_hex(&challenge),
    })
}

fn descriptor_payload_equivalent(
    current: &DescriptorPayload,
    existing: &DescriptorPayload,
) -> bool {
    let mut current = current.clone();
    let mut existing = existing.clone();
    current.descriptor_seq = 0;
    existing.descriptor_seq = 0;
    current == existing
}

#[derive(Debug, Clone)]
struct ResolvedConfidentialProfile {
    config: ConfidentialProfileConfig,
    artifact: SignedArtifact<ConfidentialProfilePayload>,
}

#[derive(Debug, Clone)]
struct LoadedConfidentialSession {
    profile: SignedArtifact<ConfidentialProfilePayload>,
    session: SignedArtifact<ConfidentialSessionPayload>,
    attestation: AttestationBundle,
    private_material: SessionPrivateMaterial,
}

async fn current_confidential_profile_artifacts(
    state: &AppState,
) -> Result<Vec<ResolvedConfidentialProfile>, String> {
    let Some(policy) = state.confidential_policy.as_ref() else {
        return Ok(Vec::new());
    };

    let mut profiles = Vec::new();
    for (profile_id, config) in &policy.profiles {
        let artifact = persist_signed_artifact(
            state,
            ARTIFACT_KIND_CONFIDENTIAL_PROFILE,
            confidential::profile_payload_from_config(state.identity.node_id(), profile_id, config),
        )
        .await?;
        profiles.push(ResolvedConfidentialProfile {
            config: config.clone(),
            artifact,
        });
    }

    Ok(profiles)
}

async fn lookup_confidential_profile_artifact(
    state: &AppState,
    artifact_hash: &str,
) -> Result<Option<SignedArtifact<ConfidentialProfilePayload>>, String> {
    let _ = current_confidential_profile_artifacts(state).await?;
    let lookup_hash = artifact_hash.to_string();
    let artifact = state
        .db
        .with_read_conn(move |conn| db::get_artifact_document_by_hash(conn, &lookup_hash))
        .await?;
    match artifact {
        Some(document) if document.artifact_kind == ARTIFACT_KIND_CONFIDENTIAL_PROFILE => {
            let profile: SignedArtifact<ConfidentialProfilePayload> =
                serde_json::from_value(document.document).map_err(|error| error.to_string())?;
            Ok(Some(profile))
        }
        Some(_) => Ok(None),
        None => Ok(None),
    }
}

fn deserialize_evidence_content<T: DeserializeOwned>(
    evidence: &[db::ExecutionEvidenceRecord],
    evidence_kind: &str,
) -> Result<T, String> {
    let record = evidence
        .iter()
        .find(|record| record.evidence_kind == evidence_kind)
        .ok_or_else(|| format!("missing confidential session evidence '{evidence_kind}'"))?;
    serde_json::from_value(record.content.clone()).map_err(|error| error.to_string())
}

async fn load_confidential_session_by_hash(
    state: &AppState,
    confidential_session_hash: &str,
) -> Result<Option<LoadedConfidentialSession>, String> {
    let lookup_hash = confidential_session_hash.to_string();
    let session_document = state
        .db
        .with_read_conn(move |conn| db::get_artifact_document_by_hash(conn, &lookup_hash))
        .await?;
    let Some(session_document) = session_document else {
        return Ok(None);
    };
    if session_document.artifact_kind != ARTIFACT_KIND_CONFIDENTIAL_SESSION {
        return Ok(None);
    }
    let session: SignedArtifact<ConfidentialSessionPayload> =
        serde_json::from_value(session_document.document).map_err(|error| error.to_string())?;
    let session_id = session.payload.session_id.clone();
    let evidence = state
        .db
        .with_read_conn(move |conn| {
            db::list_execution_evidence_for_subject(conn, "confidential_session", &session_id)
        })
        .await?;
    let attestation: AttestationBundle =
        deserialize_evidence_content(&evidence, "attestation_bundle")?;
    let private_material: SessionPrivateMaterial =
        deserialize_evidence_content(&evidence, "session_private_material")?;
    if private_material.confidential_session_hash != session.hash {
        return Err(
            "confidential session private material does not match session hash".to_string(),
        );
    }
    let Some(profile) =
        lookup_confidential_profile_artifact(state, &session.payload.confidential_profile_hash)
            .await?
    else {
        return Err("confidential profile referenced by session is missing".to_string());
    };

    Ok(Some(LoadedConfidentialSession {
        profile,
        session,
        attestation,
        private_material,
    }))
}

async fn load_confidential_session_by_id(
    state: &AppState,
    session_id: &str,
) -> Result<Option<LoadedConfidentialSession>, String> {
    let lookup_id = session_id.to_string();
    let evidence = state
        .db
        .with_read_conn(move |conn| {
            db::list_execution_evidence_for_subject(conn, "confidential_session", &lookup_id)
        })
        .await?;
    let artifact_ref: Value = deserialize_evidence_content(&evidence, "session_artifact_ref")?;
    let Some(confidential_session_hash) = artifact_ref
        .get("artifact_hash")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Err("confidential session artifact ref is missing artifact_hash".to_string());
    };
    load_confidential_session_by_hash(state, &confidential_session_hash).await
}

pub async fn get_confidential_profile(
    State(state): State<Arc<AppState>>,
    Path(artifact_hash): Path<String>,
) -> impl IntoResponse {
    match lookup_confidential_profile_artifact(state.as_ref(), &artifact_hash).await {
        Ok(Some(profile)) => (StatusCode::OK, Json(json!(profile))),
        Ok(None) => error_json(
            StatusCode::NOT_FOUND,
            json!({ "error": "confidential profile not found", "artifact_hash": artifact_hash }),
        ),
        Err(error) => {
            tracing::error!(
                "Failed to load confidential profile {}: {error}",
                artifact_hash
            );
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to load confidential profile" }),
            )
        }
    }
}

pub async fn open_confidential_session(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ConfidentialSessionOpenRequest>,
) -> impl IntoResponse {
    let Some(policy) = state.confidential_policy.as_ref() else {
        return error_json(
            StatusCode::NOT_FOUND,
            json!({ "error": "confidential execution is not enabled on this provider" }),
        );
    };
    let requester_id = match normalize_hex_field("requester_id", payload.requester_id.clone(), 64) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let requester_public_key =
        match confidential::validate_public_key_hex(payload.requester_public_key.as_str()) {
            Ok(value) => value,
            Err(error) => {
                return error_json(StatusCode::BAD_REQUEST, json!({ "error": error }));
            }
        };
    let profiles = match current_confidential_profile_artifacts(state.as_ref()).await {
        Ok(profiles) => profiles,
        Err(error) => {
            tracing::error!("Failed to load confidential profiles: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to load confidential profiles" }),
            );
        }
    };
    let Some(profile) = profiles
        .iter()
        .find(|profile| profile.artifact.hash == payload.confidential_profile_hash)
        .cloned()
    else {
        return error_json(
            StatusCode::NOT_FOUND,
            json!({
                "error": "confidential profile not found",
                "confidential_profile_hash": payload.confidential_profile_hash,
            }),
        );
    };
    if profile.artifact.payload.allowed_workload_kind != payload.allowed_workload_kind {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "requested workload kind does not match confidential profile",
                "allowed_workload_kind": profile.artifact.payload.allowed_workload_kind,
                "requested_workload_kind": payload.allowed_workload_kind,
            }),
        );
    }
    if profile.artifact.payload.attestation_platform != policy.backend.platform {
        return error_json(
            StatusCode::CONFLICT,
            json!({
                "error": "configured confidential backend platform does not match profile",
                "backend_platform": policy.backend.platform,
                "profile_platform": profile.artifact.payload.attestation_platform,
            }),
        );
    }

    let now = settlement::current_unix_timestamp();
    let expires_at = now.saturating_add(state.config.confidential.session_ttl_secs as i64);
    let session_id = protocol::new_artifact_id();
    let (session_private_key, session_public_key) = confidential::generate_keypair();
    let attestation_provider = NvidiaMockAttestationProvider;
    let attestation = match attestation_provider.issue_attestation(
        &profile.artifact.payload,
        &session_public_key,
        now,
        expires_at,
    ) {
        Ok(attestation) => attestation,
        Err(error) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to issue confidential attestation: {error}") }),
            );
        }
    };
    let attestation_evidence_hash = match confidential::attestation_hash(&attestation) {
        Ok(hash) => hash,
        Err(error) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to hash confidential attestation: {error}") }),
            );
        }
    };
    let session = match sign_node_artifact(
        state.as_ref(),
        ARTIFACT_KIND_CONFIDENTIAL_SESSION,
        now,
        ConfidentialSessionPayload {
            provider_id: state.identity.node_id().to_string(),
            requester_id,
            session_id: session_id.clone(),
            confidential_profile_hash: profile.artifact.hash.clone(),
            allowed_workload_kind: profile.artifact.payload.allowed_workload_kind.clone(),
            execution_mode: profile.artifact.payload.execution_mode.clone(),
            attestation_platform: profile.artifact.payload.attestation_platform.clone(),
            measurement: profile.artifact.payload.measurement.clone(),
            attestation_evidence_hash,
            key_release_policy_hash: profile.artifact.payload.key_release_policy_hash.clone(),
            session_public_key: session_public_key.clone(),
            requester_public_key: requester_public_key.clone(),
            encryption_algorithm: confidential::ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1
                .to_string(),
            expires_at,
        },
    ) {
        Ok(session) => session,
        Err(error) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to sign confidential session: {error}") }),
            );
        }
    };
    let session_json = match serde_json::to_string(&session) {
        Ok(value) => value,
        Err(error) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to encode confidential session: {error}") }),
            );
        }
    };
    let session_private_material = SessionPrivateMaterial {
        confidential_session_hash: session.hash.clone(),
        confidential_profile_hash: profile.artifact.hash.clone(),
        session_id: session_id.clone(),
        session_private_key,
        session_public_key,
        requester_public_key,
        expires_at,
    };
    let session_hash = session.hash.clone();
    let payload_hash = session.payload_hash.clone();
    let actor_id = session.signer.clone();
    let session_for_db = session.clone();
    let attestation_for_db = attestation.clone();
    let persisted = state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|error| error.to_string())?;
            let operation = (|| -> Result<(), String> {
                db::insert_artifact_document(
                    conn,
                    &session_hash,
                    &payload_hash,
                    ARTIFACT_KIND_CONFIDENTIAL_SESSION,
                    &actor_id,
                    session_for_db.created_at,
                    &session_json,
                )?;
                let _ = db::insert_execution_evidence(
                    conn,
                    "confidential_session",
                    &session_id,
                    "session_artifact_ref",
                    &json!({ "artifact_hash": session_for_db.hash }),
                    now,
                )?;
                let _ = db::insert_execution_evidence(
                    conn,
                    "confidential_session",
                    &session_id,
                    "attestation_bundle",
                    &attestation_for_db,
                    now,
                )?;
                let _ = db::insert_execution_evidence(
                    conn,
                    "confidential_session",
                    &session_id,
                    "session_private_material",
                    &session_private_material,
                    now,
                )?;
                Ok(())
            })();

            if let Err(error) = operation {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error);
            }

            conn.execute_batch("COMMIT")
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .await;

    if let Err(error) = persisted {
        tracing::error!(
            "Failed to persist confidential session {}: {error}",
            session.hash
        );
        return error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to persist confidential session" }),
        );
    }

    (
        StatusCode::CREATED,
        Json(json!(ConfidentialSessionResponse {
            profile: profile.artifact,
            session,
            attestation,
        })),
    )
}

pub async fn get_confidential_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match load_confidential_session_by_id(state.as_ref(), &session_id).await {
        Ok(Some(loaded)) => (
            StatusCode::OK,
            Json(json!(ConfidentialSessionResponse {
                profile: loaded.profile,
                session: loaded.session,
                attestation: loaded.attestation,
            })),
        ),
        Ok(None) => error_json(
            StatusCode::NOT_FOUND,
            json!({ "error": "confidential session not found", "session_id": session_id }),
        ),
        Err(error) => {
            tracing::error!(
                "Failed to load confidential session {}: {error}",
                session_id
            );
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to load confidential session" }),
            )
        }
    }
}

async fn current_descriptor_artifact(
    state: &AppState,
) -> Result<SignedArtifact<DescriptorPayload>, String> {
    let transport_status = state.transport_status.lock().await.clone();
    let mut transport_endpoints = Vec::new();
    if let Some(uri) = transport_status.clearnet_url {
        transport_endpoints.push(protocol::TransportEndpoint {
            transport: transport_name_for_clearnet_uri(&uri).to_string(),
            uri,
            created_at: None,
            expires_at: None,
            priority: 10,
            features: vec![
                "quote_http".to_string(),
                "artifact_fetch".to_string(),
                "receipt_poll".to_string(),
            ],
        });
    }
    if let Some(uri) = transport_status.tor_onion_url {
        transport_endpoints.push(protocol::TransportEndpoint {
            transport: "tor".to_string(),
            uri,
            created_at: None,
            expires_at: None,
            priority: 20,
            features: vec![
                "quote_http".to_string(),
                "artifact_fetch".to_string(),
                "receipt_poll".to_string(),
            ],
        });
    }
    let active_offer_definitions = current_offer_definitions(state)
        .await?
        .into_iter()
        .filter(|definition| definition.publication_state == "active")
        .collect::<Vec<_>>();
    let mut service_kinds = active_offer_definitions
        .iter()
        .map(|definition| definition.offer_kind.clone())
        .collect::<Vec<_>>();
    let mut execution_runtimes = active_offer_definitions
        .iter()
        .map(|definition| definition.runtime.clone())
        .collect::<Vec<_>>();
    service_kinds.sort();
    service_kinds.dedup();
    execution_runtimes.sort();
    execution_runtimes.dedup();
    let mut payload = DescriptorPayload {
        provider_id: state.identity.node_id().to_string(),
        descriptor_seq: 0,
        protocol_version: protocol::FROGLET_SCHEMA_V1.to_string(),
        expires_at: None,
        linked_identities: vec![nostr_publication_linked_identity(state)?],
        transport_endpoints,
        capabilities: protocol::DescriptorCapabilities {
            service_kinds,
            execution_runtimes,
            max_concurrent_deals: Some(sandbox::wasm_concurrency_limit() as u32),
        },
        accepted_payment_methods: settlement::accepted_payment_methods(state),
    };

    let actor_id = state.identity.node_id().to_string();
    let latest_descriptor = state
        .db
        .with_read_conn(move |conn| {
            db::get_latest_artifact_by_actor_kind(conn, &actor_id, ARTIFACT_KIND_DESCRIPTOR)
        })
        .await?;

    if let Some(existing) = latest_descriptor {
        let existing: SignedArtifact<DescriptorPayload> =
            serde_json::from_value(existing.document).map_err(|e| e.to_string())?;
        if descriptor_payload_equivalent(&payload, &existing.payload) {
            return Ok(existing);
        }
        payload.descriptor_seq = existing.payload.descriptor_seq.saturating_add(1).max(1);
    } else {
        payload.descriptor_seq = 1;
    }

    persist_signed_artifact(state, ARTIFACT_KIND_DESCRIPTOR, payload).await
}

async fn current_offer_artifacts(
    state: &AppState,
) -> Result<Vec<SignedArtifact<OfferPayload>>, String> {
    Ok(current_offer_records(state, false)
        .await?
        .into_iter()
        .map(|record| record.offer)
        .collect())
}

async fn lookup_offer(
    state: &AppState,
    offer_id: &str,
) -> Result<Option<SignedArtifact<OfferPayload>>, String> {
    Ok(provider_control_offer_record(state, offer_id, false)
        .await?
        .map(|record| record.offer))
}

fn accepted_payment_methods(state: &AppState) -> Vec<String> {
    settlement::accepted_payment_methods(state)
}

fn grant_requested_capabilities_from_offer(
    spec: &WorkloadSpec,
    offer: &SignedArtifact<OfferPayload>,
) -> Result<Vec<String>, (StatusCode, Json<serde_json::Value>)> {
    let requested = spec.requested_capabilities();
    if requested.is_empty() {
        return Ok(Vec::new());
    }

    if spec.runtime() != Some("wasm") {
        return Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "requested_capabilities are only supported for wasm workloads" }),
        ));
    }

    for capability in requested {
        if !offer
            .payload
            .execution_profile
            .capabilities
            .iter()
            .any(|granted| granted == capability)
        {
            return Err(error_json(
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "offer does not grant requested capability",
                    "capability": capability,
                    "offer_id": offer.payload.offer_id,
                }),
            ));
        }
    }

    Ok(requested.to_vec())
}

fn local_wasm_capabilities_for_submission(
    state: &AppState,
    submission: &wasm::VerifiedWasmSubmission,
) -> Result<
    (
        Vec<String>,
        Option<Arc<crate::wasm_host::WasmHostEnvironment>>,
    ),
    String,
> {
    match submission.abi_version.as_str() {
        wasm::WASM_RUN_JSON_ABI_V1 => Ok((Vec::new(), None)),
        wasm::WASM_HOST_JSON_ABI_V1 => {
            let Some(host_environment) = state.wasm_host.clone() else {
                return Err("froglet.wasm.host_json.v1 is not enabled on this provider".to_string());
            };
            let offered = host_environment.advertised_capabilities();
            for capability in &submission.requested_capabilities {
                if !offered.iter().any(|offered| offered == capability) {
                    return Err(format!(
                        "requested_capability '{capability}' is not enabled on this provider"
                    ));
                }
            }
            Ok((
                submission.requested_capabilities.clone(),
                Some(host_environment),
            ))
        }
        other => Err(format!("unsupported wasm abi_version: {other}")),
    }
}

async fn quoted_settlement_terms(
    state: &AppState,
    price_sats: u64,
) -> Result<QuoteSettlementTerms, String> {
    if let Some(terms) = settlement::quoted_lightning_settlement_terms(state, price_sats).await? {
        return Ok(terms);
    }

    // Canonical free-service settlement terms: method "none" with empty
    // destination_identity and zero fees.  Do NOT fall through to the
    // Lightning method string with zero fees.
    Ok(QuoteSettlementTerms {
        method: "none".to_string(),
        destination_identity: String::new(),
        base_fee_msat: 0,
        success_fee_msat: 0,
        max_base_invoice_expiry_secs: 0,
        max_success_hold_expiry_secs: 0,
        min_final_cltv_expiry: 0,
    })
}

fn settlement_quote_expires_at(
    state: &AppState,
    created_at: i64,
    price_sats: u64,
    execution_window_secs: u64,
) -> i64 {
    settlement::lightning_quote_expires_at(state, created_at, price_sats, execution_window_secs)
}

fn sign_node_artifact<T: Serialize + Clone>(
    state: &AppState,
    kind: &str,
    created_at: i64,
    payload: T,
) -> Result<SignedArtifact<T>, String> {
    protocol::sign_artifact(
        state.identity.node_id(),
        |message| state.identity.sign_message_hex(message),
        kind,
        created_at,
        payload,
    )
}

async fn persist_signed_artifact<T>(
    state: &AppState,
    kind: &str,
    payload: T,
) -> Result<SignedArtifact<T>, String>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    let created_at = settlement::current_unix_timestamp();
    let artifact = sign_node_artifact(state, kind, created_at, payload)?;
    let actor_id = artifact.signer.clone();
    let kind = artifact.artifact_type.clone();
    let payload_hash = artifact.payload_hash.clone();
    let artifact_hash = artifact.hash.clone();
    let document_json = serde_json::to_string(&artifact).map_err(|e| e.to_string())?;

    let stored = state
        .db
        .with_write_conn(move |conn| {
            db::insert_artifact_document(
                conn,
                &artifact_hash,
                &payload_hash,
                &kind,
                &actor_id,
                created_at,
                &document_json,
            )?;
            db::get_artifact_by_actor_kind_payload(conn, &actor_id, &kind, &payload_hash)
        })
        .await?
        .ok_or_else(|| "artifact missing after insert".to_string())?;

    serde_json::from_value(stored.document).map_err(|e| e.to_string())
}

async fn find_existing_job(
    state: &AppState,
    idempotency_key: Option<String>,
) -> Result<Option<jobs::StoredJob>, (StatusCode, Json<serde_json::Value>)> {
    let Some(idempotency_key) = idempotency_key else {
        return Ok(None);
    };

    state
        .db
        .with_read_conn(move |conn| jobs::find_job_by_idempotency_key(conn, &idempotency_key))
        .await
        .map_err(|error| {
            tracing::error!("Failed to look up idempotent job: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        })
}

async fn find_existing_deal(
    state: &AppState,
    idempotency_key: Option<String>,
) -> Result<Option<deals::StoredDeal>, (StatusCode, Json<serde_json::Value>)> {
    let Some(idempotency_key) = idempotency_key else {
        return Ok(None);
    };

    state
        .db
        .with_read_conn(move |conn| deals::find_deal_by_idempotency_key(conn, &idempotency_key))
        .await
        .map_err(|error| {
            tracing::error!("Failed to look up idempotent deal: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        })
}

async fn find_existing_deal_by_artifact_hash(
    state: &AppState,
    deal_artifact_hash: &str,
) -> Result<Option<deals::StoredDeal>, (StatusCode, Json<serde_json::Value>)> {
    let deal_artifact_hash = deal_artifact_hash.to_string();
    state
        .db
        .with_read_conn(move |conn| deals::get_deal_by_artifact_hash(conn, &deal_artifact_hash))
        .await
        .map_err(|error| {
            tracing::error!("Failed to look up deal by artifact hash: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        })
}

pub(crate) fn require_bearer_token(
    headers: &HeaderMap,
    expected_token: &str,
    scope: &str,
) -> Result<(), ApiFailure> {
    runtime_auth::require_bearer_token(headers, expected_token, scope)
}

fn require_runtime_auth(headers: &HeaderMap, state: &AppState) -> Result<(), ApiFailure> {
    require_bearer_token(headers, &state.runtime_auth_token, "runtime")
}

fn require_provider_control_auth(headers: &HeaderMap, state: &AppState) -> Result<(), ApiFailure> {
    require_bearer_token(
        headers,
        &state.provider_control_auth_token,
        "provider-control",
    )
}

pub(crate) async fn publish_artifact(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ProviderControlPublishArtifactRequest>,
) -> impl IntoResponse {
    if let Err((status, body)) = require_provider_control_auth(&headers, &state) {
        return (status, Json(json!(body))).into_response();
    }
    let service_id = payload.service_id.clone();
    let definition = match artifact_provider_offer_definition(state.as_ref(), payload) {
        Ok(d) => d,
        Err((status, body)) => return (status, Json(json!(body))).into_response(),
    };
    match persist_provider_offer_mutation(
        state.as_ref(),
        definition,
        StatusCode::CREATED,
        format!("published artifact {service_id}"),
    )
    .await
    {
        Ok((status, json)) => (status, json).into_response(),
        Err((status, body)) => (status, Json(json!(body))).into_response(),
    }
}

pub(crate) fn provider_offer_limits(
    state: &AppState,
    runtime: &str,
) -> (usize, u64, usize, usize, u64) {
    match runtime {
        "builtin" => (
            MAX_BODY_BYTES,
            state.config.execution_timeout_secs.saturating_mul(1_000),
            0,
            MAX_BODY_BYTES,
            0,
        ),
        _ => (
            MAX_WASM_INPUT_BYTES,
            state.config.execution_timeout_secs.saturating_mul(1_000),
            sandbox::WASM_MAX_MEMORY_BYTES,
            sandbox::WASM_MAX_OUTPUT_BYTES,
            sandbox::WASM_FUEL_LIMIT,
        ),
    }
}

pub(crate) fn validate_provider_offer_definition(
    definition: &ProviderManagedOfferDefinition,
) -> Result<(), String> {
    normalize_short_id(&definition.offer_id)?;
    normalize_offer_publication_state(Some(&definition.publication_state))?;
    if let Some(service_id) = definition.service_id.as_deref() {
        normalize_short_id(service_id)?;
    }
    if let Some(project_id) = definition.project_id.as_deref() {
        normalize_short_id(project_id)?;
    }
    if definition.offer_kind.trim().is_empty() {
        return Err("offer_kind must be a non-empty string".to_string());
    }
    let runtime = ExecutionRuntime::parse(&definition.runtime)?;
    if runtime == ExecutionRuntime::Any {
        if definition.offer_kind != crate::execution::WORKLOAD_KIND_EXECUTION_V1 {
            return Err(
                "runtime=any is only supported for offer_kind=compute.execution.v1".to_string(),
            );
        }
        if !definition.package_kind.trim().is_empty()
            || !definition.entrypoint_kind.trim().is_empty()
            || !definition.entrypoint.trim().is_empty()
            || !definition.contract_version.trim().is_empty()
        {
            return Err(
                "runtime=any offers must leave package_kind, entrypoint_kind, entrypoint, and contract_version empty"
                    .to_string(),
            );
        }
        if !definition.capabilities.is_empty() {
            return Err("runtime=any offers cannot declare fixed capabilities".to_string());
        }
        if definition.source_kind.trim().is_empty() {
            return Err("source_kind must be a non-empty string".to_string());
        }
        return Ok(());
    }
    let package_kind = ExecutionPackageKind::parse(&definition.package_kind)?;
    let entrypoint_kind = ExecutionEntrypointKind::parse(&definition.entrypoint_kind)?;
    if definition.mode != "sync" && definition.mode != "async" {
        return Err("mode must be sync or async".to_string());
    }
    if definition.contract_version.trim().is_empty() {
        return Err("contract_version must be a non-empty string".to_string());
    }
    if definition.source_kind.trim().is_empty() {
        return Err("source_kind must be a non-empty string".to_string());
    }
    if runtime == ExecutionRuntime::Builtin && package_kind != ExecutionPackageKind::Builtin {
        return Err("builtin runtime requires package_kind=builtin".to_string());
    }
    if runtime == ExecutionRuntime::Builtin && entrypoint_kind != ExecutionEntrypointKind::Builtin {
        return Err("builtin runtime requires entrypoint_kind=builtin".to_string());
    }
    if definition.contract_version == wasm::WASM_RUN_JSON_ABI_V1
        && !definition.capabilities.is_empty()
    {
        return Err(format!(
            "{} does not permit requested capabilities",
            wasm::WASM_RUN_JSON_ABI_V1
        ));
    }
    Ok(())
}

fn offer_service_id(definition: &ProviderManagedOfferDefinition) -> String {
    definition
        .service_id
        .clone()
        .unwrap_or_else(|| definition.offer_id.clone())
}

fn effective_provider_offer_kind(definition: &ProviderManagedOfferDefinition) -> String {
    if definition.service_id.is_some()
        && let Ok(
            ExecutionRuntime::Wasm
            | ExecutionRuntime::Python
            | ExecutionRuntime::Container
            | ExecutionRuntime::Any,
        ) = ExecutionRuntime::parse(&definition.runtime)
    {
        return crate::execution::WORKLOAD_KIND_EXECUTION_V1.to_string();
    }
    definition.offer_kind.clone()
}

fn service_id_for_offer_definition(definition: &ProviderManagedOfferDefinition) -> ServiceId {
    match definition.runtime.as_str() {
        "builtin" => ServiceId::EventsQuery,
        _ => ServiceId::ExecuteWasm,
    }
}

fn payload_from_provider_offer_definition(
    state: &AppState,
    descriptor_hash: &str,
    definition: &ProviderManagedOfferDefinition,
) -> OfferPayload {
    let service_id = service_id_for_offer_definition(definition);
    let offer_kind = effective_provider_offer_kind(definition);
    let runtime = ExecutionRuntime::parse(&definition.runtime).unwrap_or(ExecutionRuntime::Wasm);
    let base_fee_msat: u64 = 0;
    let success_fee_msat: u64 = definition.price_sats.saturating_mul(1_000);
    let settlement_method = if base_fee_msat == 0 && success_fee_msat == 0 {
        "none".to_string()
    } else {
        "lightning.base_fee_plus_success_fee.v1".to_string()
    };
    OfferPayload {
        provider_id: state.identity.node_id().to_string(),
        offer_id: definition.offer_id.clone(),
        descriptor_hash: descriptor_hash.to_string(),
        expires_at: None,
        offer_kind,
        settlement_method,
        quote_ttl_secs: advertised_offer_timeout_secs(
            state,
            service_id,
            definition.price_sats,
            &accepted_payment_methods(state),
        ),
        execution_profile: protocol::OfferExecutionProfile {
            runtime,
            package_kind: definition.package_kind.clone(),
            contract_version: definition.contract_version.clone(),
            access_handles: definition.capabilities.clone(),
            abi_version: definition.contract_version.clone(),
            capabilities: definition.capabilities.clone(),
            max_input_bytes: definition.max_input_bytes,
            max_runtime_ms: definition.max_runtime_ms,
            max_memory_bytes: definition.max_memory_bytes,
            max_output_bytes: definition.max_output_bytes,
            fuel_limit: definition.fuel_limit,
        },
        price_schedule: protocol::OfferPriceSchedule {
            base_fee_msat,
            success_fee_msat,
        },
        terms_hash: definition.terms_hash.clone(),
        confidential_profile_hash: definition.confidential_profile_hash.clone(),
    }
}

pub(crate) fn provider_offer_record_from_parts(
    definition: &ProviderManagedOfferDefinition,
    offer: SignedArtifact<OfferPayload>,
) -> ProviderControlOfferRecord {
    ProviderControlOfferRecord {
        publication_state: definition.publication_state.clone(),
        service_id: definition.service_id.clone(),
        project_id: definition.project_id.clone(),
        source_kind: definition.source_kind.clone(),
        runtime: definition.runtime.clone(),
        package_kind: definition.package_kind.clone(),
        entrypoint_kind: definition.entrypoint_kind.clone(),
        entrypoint: definition.entrypoint.clone(),
        contract_version: definition.contract_version.clone(),
        mounts: definition.mounts.clone(),
        mode: definition.mode.clone(),
        summary: definition.summary.clone(),
        module_hash: definition.module_hash.clone(),
        binding_hash: definition.module_hash.clone(),
        starter: definition.starter.clone(),
        input_schema: definition.input_schema.clone(),
        output_schema: definition.output_schema.clone(),
        offer,
    }
}

fn builtin_provider_offer_definitions(
    state: &AppState,
    confidential_profiles: &[ResolvedConfidentialProfile],
) -> Vec<ProviderManagedOfferDefinition> {
    let wasm_host_capabilities = state
        .wasm_host
        .as_ref()
        .map(|host| host.advertised_capabilities())
        .unwrap_or_default();
    let builtin = |offer_id: &str,
                   offer_kind: &str,
                   runtime: &str,
                   contract_version: &str,
                   capabilities: Vec<String>,
                   price_sats: u64,
                   source_kind: &str,
                   summary: &str| {
        let is_events_query = offer_id == ServiceId::EventsQuery.as_str();
        let is_generic_compute = offer_kind == crate::execution::WORKLOAD_KIND_EXECUTION_V1
            && (offer_id == ServiceId::ExecuteWasm.as_str()
                || offer_id == EXECUTE_COMPUTE_GENERIC_OFFER_ID);
        let (max_input_bytes, max_runtime_ms, max_memory_bytes, max_output_bytes, fuel_limit) =
            provider_offer_limits(state, runtime);
        ProviderManagedOfferDefinition {
            offer_id: offer_id.to_string(),
            service_id: None,
            project_id: None,
            offer_kind: offer_kind.to_string(),
            runtime: runtime.to_string(),
            package_kind: if is_events_query {
                "builtin".to_string()
            } else if is_generic_compute {
                String::new()
            } else {
                "inline_module".to_string()
            },
            entrypoint_kind: if is_events_query {
                "builtin".to_string()
            } else if is_generic_compute {
                String::new()
            } else {
                "handler".to_string()
            },
            entrypoint: if is_events_query {
                "events.query".to_string()
            } else if is_generic_compute {
                String::new()
            } else {
                "run".to_string()
            },
            contract_version: contract_version.to_string(),
            mounts: Vec::new(),
            mode: default_service_mode(),
            capabilities,
            max_input_bytes,
            max_runtime_ms,
            max_memory_bytes,
            max_output_bytes,
            fuel_limit,
            price_sats,
            publication_state: default_offer_publication_state(),
            starter: None,
            module_hash: None,
            module_bytes_hex: None,
            inline_source: None,
            oci_reference: None,
            oci_digest: None,
            source_path: None,
            source_kind: source_kind.to_string(),
            summary: Some(summary.to_string()),
            input_schema: None,
            output_schema: None,
            terms_hash: None,
            confidential_profile_hash: None,
        }
    };

    let mut definitions = vec![
        builtin(
            ServiceId::EventsQuery.as_str(),
            "events.query",
            "builtin",
            CONTRACT_BUILTIN_EVENTS_QUERY_V1,
            Vec::new(),
            state.pricing.price_for(ServiceId::EventsQuery),
            "builtin",
            "Query Froglet events from the provider's local event store.",
        ),
        builtin(
            ServiceId::ExecuteWasm.as_str(),
            wasm::WORKLOAD_KIND_COMPUTE_WASM_V1,
            "wasm",
            wasm::WASM_RUN_JSON_ABI_V1,
            Vec::new(),
            state.pricing.price_for(ServiceId::ExecuteWasm),
            "builtin",
            "Run arbitrary Wasm compute using froglet.wasm.run_json.v1.",
        ),
        builtin(
            EXECUTE_COMPUTE_GENERIC_OFFER_ID,
            crate::execution::WORKLOAD_KIND_EXECUTION_V1,
            "any",
            "",
            Vec::new(),
            state.pricing.price_for(ServiceId::ExecuteWasm),
            "builtin",
            "Run compute with any supported runtime (Python, Container, Wasm) using compute.execution.v1.",
        ),
    ];
    if !wasm_host_capabilities.is_empty() {
        definitions.push(builtin(
            "execute.compute.host",
            wasm::WORKLOAD_KIND_COMPUTE_WASM_V1,
            "wasm",
            wasm::WASM_HOST_JSON_ABI_V1,
            wasm_host_capabilities,
            state.pricing.price_for(ServiceId::ExecuteWasm),
            "builtin",
            "Run compute with Wasm host capabilities enabled using froglet.wasm.host_json.v1.",
        ));
    }
    for profile in confidential_profiles {
        let (runtime, contract_version, summary) =
            match profile.config.allowed_workload_kind.as_str() {
                confidential::WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1 => (
                    "tee.service",
                    "froglet.confidential.service.v1",
                    "Confidential service execution offer",
                ),
                confidential::WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1 => (
                    "tee.wasm",
                    "froglet.confidential.attested_wasm.v1",
                    "Confidential attested Wasm execution offer",
                ),
                _ => continue,
            };
        definitions.push(ProviderManagedOfferDefinition {
            offer_id: profile.config.offer_id.clone(),
            service_id: None,
            project_id: None,
            offer_kind: profile.config.allowed_workload_kind.clone(),
            runtime: runtime.to_string(),
            package_kind: "builtin".to_string(),
            entrypoint_kind: "builtin".to_string(),
            entrypoint: if runtime == "tee.service" {
                "confidential.service".to_string()
            } else {
                "attested.wasm".to_string()
            },
            contract_version: contract_version.to_string(),
            mounts: Vec::new(),
            mode: default_service_mode(),
            capabilities: Vec::new(),
            max_input_bytes: profile.config.max_input_bytes,
            max_runtime_ms: profile.config.max_runtime_ms,
            max_memory_bytes: sandbox::WASM_MAX_MEMORY_BYTES,
            max_output_bytes: profile.config.max_output_bytes,
            fuel_limit: sandbox::WASM_FUEL_LIMIT,
            price_sats: profile.config.price_sats,
            publication_state: default_offer_publication_state(),
            starter: None,
            module_hash: None,
            module_bytes_hex: None,
            inline_source: None,
            oci_reference: None,
            oci_digest: None,
            source_path: None,
            source_kind: "confidential_profile".to_string(),
            summary: Some(summary.to_string()),
            input_schema: None,
            output_schema: None,
            terms_hash: profile.config.terms_hash.clone(),
            confidential_profile_hash: Some(profile.artifact.hash.clone()),
        });
    }
    definitions
}

pub(crate) async fn current_offer_definitions(
    state: &AppState,
) -> Result<Vec<ProviderManagedOfferDefinition>, String> {
    let confidential_profiles = current_confidential_profile_artifacts(state).await?;
    let mut definitions = BTreeMap::new();
    for definition in builtin_provider_offer_definitions(state, &confidential_profiles) {
        definitions.insert(definition.offer_id.clone(), definition);
    }
    let managed = state
        .db
        // Operator publications can arrive from a separate long-lived process.
        // Use the writer connection here so provider-facing snapshots reflect
        // the latest committed offer definitions immediately.
        .with_write_conn(db::list_provider_managed_offers)
        .await?;
    for record in managed {
        let definition: ProviderManagedOfferDefinition = serde_json::from_value(record.definition)
            .map_err(|error| {
                format!(
                    "provider_managed_offers {} contains invalid JSON: {error}",
                    record.offer_id
                )
            })?;
        validate_provider_offer_definition(&definition)?;
        definitions.insert(definition.offer_id.clone(), definition);
    }
    Ok(definitions.into_values().collect())
}

pub async fn current_advertised_services(
    state: &AppState,
) -> Result<Vec<crate::pricing::ServicePriceInfo>, String> {
    let mut services = current_offer_definitions(state)
        .await?
        .into_iter()
        .filter(|definition| definition.publication_state == "active")
        .map(|definition| crate::pricing::ServicePriceInfo {
            service_id: offer_service_id(&definition),
            price_sats: definition.price_sats,
            payment_required: definition.price_sats > 0,
        })
        .collect::<Vec<_>>();
    services.sort_by(|left, right| left.service_id.cmp(&right.service_id));
    Ok(services)
}

pub(crate) async fn current_offer_records(
    state: &AppState,
    include_hidden: bool,
) -> Result<Vec<ProviderControlOfferRecord>, String> {
    let descriptor = current_descriptor_artifact(state).await?;
    let descriptor_hash = descriptor.hash;
    let mut records = Vec::new();
    for definition in current_offer_definitions(state).await? {
        if !include_hidden && definition.publication_state == "hidden" {
            continue;
        }
        let payload = payload_from_provider_offer_definition(state, &descriptor_hash, &definition);
        let offer = persist_signed_artifact(state, ARTIFACT_KIND_OFFER, payload).await?;
        records.push(provider_offer_record_from_parts(&definition, offer));
    }
    Ok(records)
}

fn inline_module_bytes_hex(
    definition: &ProviderManagedOfferDefinition,
) -> Result<Option<String>, String> {
    if let Some(module_bytes_hex) = definition.module_bytes_hex.clone() {
        return Ok(Some(module_bytes_hex));
    }
    if definition.runtime != "wasm" || definition.package_kind != "inline_module" {
        return Ok(None);
    }
    let Some(source_path) = definition.source_path.as_ref() else {
        return Ok(None);
    };
    let module_bytes = fs::read(source_path)
        .map_err(|error| format!("failed to read service artifact {}: {error}", source_path))?;
    Ok(Some(hex::encode(module_bytes)))
}

fn provider_service_from_definition(
    state: &AppState,
    definition: &ProviderManagedOfferDefinition,
    include_binding: bool,
) -> Result<Option<ProviderServiceRecord>, String> {
    let Some(service_id) = definition.service_id.clone() else {
        return Ok(None);
    };
    Ok(Some(ProviderServiceRecord {
        service_id,
        offer_id: definition.offer_id.clone(),
        offer_kind: effective_provider_offer_kind(definition),
        resource_kind: provider_service_resource_kind(definition).to_string(),
        project_id: definition.project_id.clone(),
        summary: definition
            .summary
            .clone()
            .unwrap_or_else(|| format!("Froglet service {}", definition.offer_id)),
        runtime: definition.runtime.clone(),
        package_kind: definition.package_kind.clone(),
        entrypoint_kind: definition.entrypoint_kind.clone(),
        entrypoint: definition.entrypoint.clone(),
        contract_version: definition.contract_version.clone(),
        mounts: definition.mounts.clone(),
        mode: definition.mode.clone(),
        price_sats: definition.price_sats,
        publication_state: definition.publication_state.clone(),
        provider_id: state.identity.node_id().to_string(),
        module_hash: definition.module_hash.clone(),
        binding_hash: definition.module_hash.clone(),
        input_schema: definition.input_schema.clone(),
        output_schema: definition.output_schema.clone(),
        module_bytes_hex: if include_binding {
            inline_module_bytes_hex(definition)?
        } else {
            None
        },
        inline_source: if include_binding {
            definition.inline_source.clone()
        } else {
            None
        },
        oci_reference: if include_binding {
            definition.oci_reference.clone()
        } else {
            None
        },
        oci_digest: if include_binding {
            definition.oci_digest.clone()
        } else {
            None
        },
    }))
}

fn provider_service_resource_kind(definition: &ProviderManagedOfferDefinition) -> &'static str {
    match definition.runtime.as_str() {
        "builtin" if definition.offer_kind == "events.query" => "data",
        "tee.service" | "tee.wasm" | "tee.python" => "confidential",
        _ => "service",
    }
}

pub(crate) async fn current_service_records(
    state: &AppState,
    include_hidden: bool,
    include_binding: bool,
) -> Result<Vec<ProviderServiceRecord>, String> {
    let mut services = Vec::new();
    for definition in current_offer_definitions(state).await? {
        if !include_hidden && definition.publication_state == "hidden" {
            continue;
        }
        if let Some(service) =
            provider_service_from_definition(state, &definition, include_binding)?
        {
            services.push(service);
        }
    }
    services.sort_by(|left, right| left.service_id.cmp(&right.service_id));
    Ok(services)
}

pub(crate) async fn provider_service_record(
    state: &AppState,
    service_id: &str,
    include_hidden: bool,
    include_binding: bool,
) -> Result<Option<ProviderServiceRecord>, String> {
    let normalized_service_id = normalize_short_id(service_id)?;
    let services = current_service_records(state, include_hidden, include_binding).await?;
    Ok(services
        .into_iter()
        .find(|service| service.service_id == normalized_service_id))
}

pub(crate) async fn provider_control_offer_record(
    state: &AppState,
    offer_id: &str,
    include_hidden: bool,
) -> Result<Option<ProviderControlOfferRecord>, String> {
    let normalized_offer_id = normalize_short_id(offer_id)?;
    let offers = current_offer_records(state, include_hidden).await?;
    Ok(offers
        .into_iter()
        .find(|record| record.offer.payload.offer_id == normalized_offer_id))
}

pub(crate) fn persist_provider_offer_definition(
    conn: &rusqlite::Connection,
    definition: &ProviderManagedOfferDefinition,
    now: i64,
) -> Result<(), String> {
    let definition_json = serde_json::to_string(definition).map_err(|error| error.to_string())?;
    db::upsert_provider_managed_offer(conn, &definition.offer_id, &definition_json, now)
}

pub fn artifact_provider_offer_definition(
    state: &AppState,
    payload: ProviderControlPublishArtifactRequest,
) -> Result<ProviderManagedOfferDefinition, ApiFailure> {
    let service_id = normalize_short_id(&payload.service_id)
        .map_err(|error| (StatusCode::BAD_REQUEST, json!({ "error": error })))?;
    let offer_id = normalize_short_id(payload.offer_id.as_deref().unwrap_or(&service_id))
        .map_err(|error| (StatusCode::BAD_REQUEST, json!({ "error": error })))?;
    let publication_state = normalize_offer_publication_state(payload.publication_state.as_deref())
        .map_err(|error| (StatusCode::BAD_REQUEST, json!({ "error": error })))?;
    let runtime = if let Some(runtime) = payload.runtime.as_deref() {
        ExecutionRuntime::parse(runtime)
            .map_err(|error| (StatusCode::BAD_REQUEST, json!({ "error": error })))?
    } else if payload.inline_source.is_some() {
        ExecutionRuntime::Python
    } else if payload.wasm_module_hex.is_some() {
        ExecutionRuntime::Wasm
    } else if payload.oci_reference.is_some() || payload.oci_digest.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "runtime is required for OCI artifact publication",
                "details": "set runtime to python, container, or wasm when using oci_reference/oci_digest"
            }),
        ));
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "runtime is required when artifact publication cannot be inferred",
                "details": "set runtime/package_kind or provide inline_source/wasm_module_hex"
            }),
        ));
    };
    let package_kind = if let Some(package_kind) = payload.package_kind.as_deref() {
        ExecutionPackageKind::parse(package_kind)
            .map_err(|error| (StatusCode::BAD_REQUEST, json!({ "error": error })))?
    } else if payload.inline_source.is_some() {
        ExecutionPackageKind::InlineSource
    } else if payload.wasm_module_hex.is_some() {
        ExecutionPackageKind::InlineModule
    } else if payload.oci_reference.is_some() || payload.oci_digest.is_some() {
        ExecutionPackageKind::OciImage
    } else {
        match runtime {
            ExecutionRuntime::Builtin => ExecutionPackageKind::Builtin,
            ExecutionRuntime::Python => ExecutionPackageKind::InlineSource,
            _ => ExecutionPackageKind::InlineModule,
        }
    };
    let entrypoint_kind = if let Some(entrypoint_kind) = payload.entrypoint_kind.as_deref() {
        ExecutionEntrypointKind::parse(entrypoint_kind)
            .map_err(|error| (StatusCode::BAD_REQUEST, json!({ "error": error })))?
    } else {
        default_entrypoint_kind_for(&runtime)
    };
    let entrypoint = payload
        .entrypoint
        .clone()
        .unwrap_or_else(|| default_entrypoint_for(&runtime, &entrypoint_kind).to_string());
    let contract_version = payload.contract_version.clone().unwrap_or_else(|| {
        default_contract_version_for(&runtime, &package_kind, &entrypoint_kind).to_string()
    });
    let (
        offer_kind,
        module_hash,
        module_bytes_hex,
        inline_source,
        source_path,
        source_kind,
        oci_reference,
        oci_digest,
    ) = match (&runtime, &package_kind) {
        (ExecutionRuntime::Wasm, ExecutionPackageKind::InlineModule) => {
            let module_bytes = match (
                payload.artifact_path.as_ref(),
                payload.wasm_module_hex.as_ref(),
                payload.oci_reference.as_ref(),
                payload.oci_digest.as_ref(),
            ) {
                (Some(path), None, None, None) => fs::read(path).map_err(|error| {
                    (
                        StatusCode::BAD_REQUEST,
                        json!({
                            "error": "failed to read artifact_path",
                            "artifact_path": path,
                            "details": error.to_string(),
                        }),
                    )
                })?,
                (None, Some(module_hex), None, None) => {
                    hex::decode(module_hex).map_err(|error| {
                        (
                            StatusCode::BAD_REQUEST,
                            json!({ "error": format!("invalid wasm_module_hex: {error}") }),
                        )
                    })?
                }
                (Some(_), Some(_), _, _) => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "provide either artifact_path or wasm_module_hex, not both" }),
                    ));
                }
                (_, _, Some(_), _) | (_, _, _, Some(_)) => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "oci_reference and oci_digest require package_kind=oci_image" }),
                    ));
                }
                _ => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "artifact_path or wasm_module_hex is required for runtime=wasm package_kind=inline_module" }),
                    ));
                }
            };
            sandbox::validate_module_bytes_for_abi(&module_bytes, &contract_version).map_err(
                |error| {
                    (
                        StatusCode::BAD_REQUEST,
                        json!({
                            "error": "artifact validation failed",
                            "details": error,
                            "contract_version": contract_version,
                        }),
                    )
                },
            )?;
            (
                wasm::WORKLOAD_KIND_COMPUTE_WASM_V1.to_string(),
                Some(crypto::sha256_hex(&module_bytes)),
                Some(
                    payload
                        .wasm_module_hex
                        .unwrap_or_else(|| hex::encode(&module_bytes)),
                ),
                None,
                payload.artifact_path,
                "artifact".to_string(),
                None,
                None,
            )
        }
        (ExecutionRuntime::Wasm, ExecutionPackageKind::OciImage) => {
            let Some(oci_reference) = payload.oci_reference else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "oci_reference is required for runtime=wasm package_kind=oci_image" }),
                ));
            };
            let Some(oci_digest) = payload.oci_digest else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "oci_digest is required for runtime=wasm package_kind=oci_image" }),
                ));
            };
            if payload.artifact_path.is_some() || payload.wasm_module_hex.is_some() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "artifact_path and wasm_module_hex are not used for runtime=wasm package_kind=oci_image" }),
                ));
            }
            (
                wasm::WORKLOAD_KIND_COMPUTE_WASM_OCI_V1.to_string(),
                Some(oci_digest.clone()),
                None,
                None,
                None,
                "oci".to_string(),
                Some(oci_reference),
                Some(oci_digest),
            )
        }
        (ExecutionRuntime::Python, ExecutionPackageKind::InlineSource) => {
            let source_text = match (
                payload.inline_source.as_ref(),
                payload.artifact_path.as_ref(),
            ) {
                (Some(source), None) => source.clone(),
                (None, Some(path)) => fs::read_to_string(path).map_err(|error| {
                    (
                        StatusCode::BAD_REQUEST,
                        json!({
                            "error": "failed to read artifact_path",
                            "artifact_path": path,
                            "details": error.to_string(),
                        }),
                    )
                })?,
                (Some(_), Some(_)) => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "provide either artifact_path or inline_source, not both" }),
                    ));
                }
                _ => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "artifact_path or inline_source is required for runtime=python package_kind=inline_source" }),
                    ));
                }
            };
            (
                crate::execution::WORKLOAD_KIND_EXECUTION_V1.to_string(),
                Some(crypto::sha256_hex(source_text.as_bytes())),
                None,
                Some(source_text),
                payload.artifact_path,
                runtime.as_str().to_string(),
                None,
                None,
            )
        }
        (ExecutionRuntime::Python, ExecutionPackageKind::OciImage)
        | (ExecutionRuntime::Container, ExecutionPackageKind::OciImage) => {
            let Some(oci_reference) = payload.oci_reference else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "oci_reference is required for OCI execution" }),
                ));
            };
            let Some(oci_digest) = payload.oci_digest else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "oci_digest is required for OCI execution" }),
                ));
            };
            (
                crate::execution::WORKLOAD_KIND_EXECUTION_V1.to_string(),
                Some(oci_digest.clone()),
                None,
                None,
                None,
                "oci".to_string(),
                Some(oci_reference),
                Some(oci_digest),
            )
        }
        (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin) => {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({ "error": "builtin services are managed by Froglet and cannot be published via publish_artifact" }),
            ));
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "unsupported runtime/package_kind combination",
                    "runtime": runtime.as_str(),
                    "package_kind": package_kind.as_str(),
                }),
            ));
        }
    };

    let runtime_str = runtime.as_str().to_string();
    let (max_input_bytes, max_runtime_ms, max_memory_bytes, max_output_bytes, fuel_limit) =
        provider_offer_limits(state, &runtime_str);
    let definition = ProviderManagedOfferDefinition {
        offer_id: offer_id.clone(),
        service_id: Some(service_id),
        project_id: None,
        offer_kind,
        runtime: runtime_str,
        package_kind: package_kind.as_str().to_string(),
        entrypoint_kind: entrypoint_kind.as_str().to_string(),
        entrypoint,
        contract_version: contract_version.clone(),
        mounts: payload.mounts.unwrap_or_default(),
        mode: payload.mode.unwrap_or_else(default_service_mode),
        capabilities: Vec::new(),
        max_input_bytes,
        max_runtime_ms,
        max_memory_bytes,
        max_output_bytes,
        fuel_limit,
        price_sats: payload.price_sats,
        publication_state,
        starter: None,
        module_hash,
        module_bytes_hex,
        inline_source,
        oci_reference,
        oci_digest,
        source_path,
        source_kind,
        summary: Some(
            payload
                .summary
                .unwrap_or_else(|| format!("Froglet service {}", offer_id)),
        ),
        input_schema: payload.input_schema,
        output_schema: payload.output_schema,
        terms_hash: None,
        confidential_profile_hash: None,
    };
    validate_provider_offer_definition(&definition)
        .map_err(|error| (StatusCode::BAD_REQUEST, json!({ "error": error })))?;
    Ok(definition)
}

pub async fn persist_provider_offer_mutation(
    state: &AppState,
    definition: ProviderManagedOfferDefinition,
    status_code: StatusCode,
    summary: String,
) -> Result<(StatusCode, Json<ProviderControlMutationResponse>), ApiFailure> {
    let now = settlement::current_unix_timestamp();
    let persisted_definition = definition.clone();
    state
        .db
        .with_write_conn(move |conn| {
            persist_provider_offer_definition(conn, &persisted_definition, now)
        })
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to persist provider offer", "details": error }),
            )
        })?;
    let Some(offer_record) = provider_control_offer_record(state, &definition.offer_id, true)
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to materialize provider offer", "details": error }),
            )
        })?
    else {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "provider offer missing after persistence" }),
        ));
    };
    let response = ProviderControlMutationResponse {
        request_id: protocol::new_artifact_id(),
        status: "passed".to_string(),
        failure_kind: None,
        summary,
        artifacts: vec![
            ProviderControlArtifactRef {
                kind: ARTIFACT_KIND_DESCRIPTOR.to_string(),
                hash: offer_record.offer.payload.descriptor_hash.clone(),
            },
            ProviderControlArtifactRef {
                kind: ARTIFACT_KIND_OFFER.to_string(),
                hash: offer_record.offer.hash.clone(),
            },
        ],
        evidence: ProviderControlEvidence {
            provider_id: offer_record.offer.payload.provider_id.clone(),
            descriptor_hash: offer_record.offer.payload.descriptor_hash.clone(),
            offer_hash: offer_record.offer.hash.clone(),
            offer_id: offer_record.offer.payload.offer_id.clone(),
            service_id: offer_record.service_id.clone(),
        },
        offer: offer_record,
    };
    Ok((status_code, Json(response)))
}

async fn create_quote_record(
    state: Arc<AppState>,
    payload: CreateQuoteRequest,
) -> Result<SignedArtifact<QuotePayload>, ApiFailure> {
    if let Err(response) = validate_workload_spec(&payload.spec) {
        return Err((response.0, response.1.0));
    }

    let Some(offer) = lookup_offer(state.as_ref(), &payload.offer_id)
        .await
        .map_err(|error| {
            tracing::error!("Failed to look up offer {}: {error}", payload.offer_id);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to load offer" }),
            )
        })?
    else {
        return Err((
            StatusCode::NOT_FOUND,
            json!({ "error": "offer not found", "offer_id": payload.offer_id }),
        ));
    };

    let requester_id = normalize_hex_field("requester_id", payload.requester_id.clone(), 64)
        .map_err(|response| (response.0, response.1.0))?;
    let workload_kind = payload.spec.workload_kind().to_string();
    if offer.payload.offer_kind != workload_kind {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "offer does not match workload kind",
                "offer_kind": offer.payload.offer_kind,
                "requested_workload_kind": workload_kind,
            }),
        ));
    }

    if let Some(requested_runtime) = payload.spec.runtime()
        && offer.payload.execution_profile.runtime != ExecutionRuntime::Any
        && offer.payload.execution_profile.runtime.as_str() != requested_runtime
    {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "offer does not match workload runtime",
                "offer_runtime": offer.payload.execution_profile.runtime,
                "requested_runtime": requested_runtime,
            }),
        ));
    }
    if let Some(contract_version) = payload.spec.contract_version()
        && !offer
            .payload
            .execution_profile
            .contract_version
            .trim()
            .is_empty()
        && !offer
            .payload
            .execution_profile
            .abi_version
            .trim()
            .is_empty()
        && offer.payload.execution_profile.contract_version != contract_version
        && offer.payload.execution_profile.abi_version != contract_version
    {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "offer does not match workload contract_version",
                "offer_contract_version": offer.payload.execution_profile.contract_version,
                "requested_contract_version": contract_version,
            }),
        ));
    }
    if let Some(confidential_profile_hash) = offer.payload.confidential_profile_hash.as_deref() {
        let Some(confidential_session_hash) = payload.spec.confidential_session_hash() else {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({ "error": "confidential offers require confidential_session_hash" }),
            ));
        };
        let Some(session) =
            load_confidential_session_by_hash(state.as_ref(), confidential_session_hash)
                .await
                .map_err(|error| {
                    tracing::error!(
                        "Failed to load confidential session {}: {error}",
                        confidential_session_hash
                    );
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({ "error": "failed to load confidential session" }),
                    )
                })?
        else {
            return Err((
                StatusCode::NOT_FOUND,
                json!({
                    "error": "confidential session not found",
                    "confidential_session_hash": confidential_session_hash,
                }),
            ));
        };
        if session.session.payload.confidential_profile_hash != confidential_profile_hash {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({ "error": "confidential session does not match offer profile" }),
            ));
        }
        if session.session.payload.requester_id != requester_id {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({ "error": "confidential session requester_id does not match quote requester_id" }),
            ));
        }
        if session.session.payload.allowed_workload_kind != workload_kind {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({ "error": "confidential session workload kind does not match requested workload" }),
            ));
        }
        confidential::verify_attestation_bundle(
            &session.profile.payload,
            &session.session,
            &session.attestation,
            settlement::current_unix_timestamp(),
        )
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("invalid confidential session attestation: {error}") }),
            )
        })?;
    } else if payload.spec.confidential_session_hash().is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "confidential_session_hash requires an offer with confidential_profile_hash" }),
        ));
    }
    let capabilities_granted = grant_requested_capabilities_from_offer(&payload.spec, &offer)
        .map_err(|response| (response.0, response.1.0))?;

    let quoted_total_sats = (offer.payload.price_schedule.base_fee_msat
        + offer.payload.price_schedule.success_fee_msat)
        / 1_000;
    if let Some(max_price_sats) = payload.max_price_sats
        && quoted_total_sats > max_price_sats
    {
        return Err((
            StatusCode::CONFLICT,
            json!({
                "error": "offer price exceeds max_price_sats",
                "price_sats": quoted_total_sats,
                "max_price_sats": max_price_sats,
            }),
        ));
    }

    let workload_hash = payload.spec.request_hash().map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to hash workload: {error}") }),
        )
    })?;
    let created_at = settlement::current_unix_timestamp();
    let mut settlement_terms = quoted_settlement_terms(state.as_ref(), quoted_total_sats)
        .await
        .map_err(|error| {
            tracing::error!("Failed to resolve quote settlement terms: {error}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({ "error": "failed to resolve lightning settlement destination" }),
            )
        })?;
    settlement_terms.base_fee_msat = offer.payload.price_schedule.base_fee_msat;
    settlement_terms.success_fee_msat = offer.payload.price_schedule.success_fee_msat;
    let quote_expires_at = settlement_quote_expires_at(
        state.as_ref(),
        created_at,
        quoted_total_sats,
        deal_execution_window_secs(&ExecutionLimits {
            max_input_bytes: offer.payload.execution_profile.max_input_bytes,
            max_runtime_ms: offer.payload.execution_profile.max_runtime_ms,
            max_memory_bytes: offer.payload.execution_profile.max_memory_bytes,
            max_output_bytes: offer.payload.execution_profile.max_output_bytes,
            fuel_limit: offer.payload.execution_profile.fuel_limit,
        }),
    );
    let quote = sign_node_artifact(
        state.as_ref(),
        ARTIFACT_KIND_QUOTE,
        created_at,
        QuotePayload {
            provider_id: state.identity.node_id().to_string(),
            requester_id,
            descriptor_hash: offer.payload.descriptor_hash.clone(),
            offer_hash: offer.hash.clone(),
            expires_at: quote_expires_at,
            workload_kind,
            workload_hash,
            confidential_session_hash: payload.spec.confidential_session_hash().map(str::to_string),
            capabilities_granted,
            extension_refs: Vec::new(),
            quote_use: None,
            settlement_terms,
            execution_limits: ExecutionLimits {
                max_input_bytes: offer.payload.execution_profile.max_input_bytes,
                max_runtime_ms: offer.payload.execution_profile.max_runtime_ms,
                max_memory_bytes: offer.payload.execution_profile.max_memory_bytes,
                max_output_bytes: offer.payload.execution_profile.max_output_bytes,
                fuel_limit: offer.payload.execution_profile.fuel_limit,
            },
        },
    )
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to sign quote: {error}") }),
        )
    })?;

    let artifact_json = serde_json::to_string(&quote).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to encode quote: {error}") }),
        )
    })?;
    let quote_for_db = quote.clone();
    let quote_hash = quote.hash.clone();
    let payload_hash = quote.payload_hash.clone();
    let actor_id = quote.signer.clone();
    let quote_kind = quote.artifact_type.clone();
    let persisted = state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<(), String> {
                db::insert_artifact_document(
                    conn,
                    &quote_hash,
                    &payload_hash,
                    &quote_kind,
                    &actor_id,
                    quote_for_db.created_at,
                    &artifact_json,
                )?;
                if deals::get_quote(conn, &quote_hash)?.is_none() {
                    deals::insert_quote(conn, &quote_for_db)?;
                }
                Ok(())
            })();

            if let Err(error) = operation {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error);
            }

            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(())
        })
        .await;

    persisted.map_err(|error| {
        tracing::error!("Failed to persist quote {}: {error}", quote.hash);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to persist quote" }),
        )
    })?;

    Ok(quote)
}

async fn create_deal_record(
    state: Arc<AppState>,
    payload: CreateDealRequest,
) -> Result<(deals::DealRecord, StatusCode), ApiFailure> {
    if let Err(response) = validate_workload_spec(&payload.spec) {
        return Err((response.0, response.1.0));
    }

    let idempotency_key = normalize_idempotency_key(payload.idempotency_key.clone())
        .map_err(|response| (response.0, response.1.0))?;
    let workload_hash = payload.spec.request_hash().map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to hash workload: {error}") }),
        )
    })?;
    let canonical_quote_hash = protocol::artifact_hash(&payload.quote).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to hash quote artifact: {error}") }),
        )
    })?;
    let canonical_deal_hash = protocol::artifact_hash(&payload.deal).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to hash deal artifact: {error}") }),
        )
    })?;

    if !protocol::verify_artifact(&payload.quote) {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid quote signature" }),
        ));
    }

    if !protocol::verify_artifact(&payload.deal) {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid deal signature" }),
        ));
    }

    if payload.quote.signer != state.identity.node_id() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "quote was not issued by this provider" }),
        ));
    }

    if payload.quote.payload.provider_id != state.identity.node_id() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "quote provider_id does not match this provider" }),
        ));
    }

    if payload.quote.payload.workload_hash != workload_hash {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "quote does not match workload payload" }),
        ));
    }

    if payload.deal.signer != payload.deal.payload.requester_id {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "deal signer does not match deal requester_id" }),
        ));
    }

    if payload.deal.payload.provider_id != payload.quote.payload.provider_id {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "deal provider_id does not match quote provider_id" }),
        ));
    }

    if payload.deal.payload.requester_id != payload.quote.payload.requester_id {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "deal requester_id does not match quote requester_id" }),
        ));
    }

    if payload.deal.payload.quote_hash != canonical_quote_hash {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "deal quote_hash does not match the submitted quote" }),
        ));
    }

    if payload.deal.payload.workload_hash != workload_hash {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "deal does not match workload payload" }),
        ));
    }
    if payload.quote.payload.confidential_session_hash
        != payload.spec.confidential_session_hash().map(str::to_string)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "quote confidential_session_hash does not match workload payload" }),
        ));
    }
    if payload.deal.payload.confidential_session_hash
        != payload.quote.payload.confidential_session_hash
    {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "deal confidential_session_hash does not match quote confidential_session_hash" }),
        ));
    }

    let now = settlement::current_unix_timestamp();
    if payload.quote.payload.expires_at < now {
        return Err((
            StatusCode::GONE,
            json!({
                "error": "quote expired",
                "quote_hash": canonical_quote_hash,
                "expires_at": payload.quote.payload.expires_at,
            }),
        ));
    }
    if let Some(confidential_session_hash) =
        payload.quote.payload.confidential_session_hash.as_deref()
    {
        let Some(session) =
            load_confidential_session_by_hash(state.as_ref(), confidential_session_hash)
                .await
                .map_err(|error| {
                    tracing::error!(
                        "Failed to load confidential session {}: {error}",
                        confidential_session_hash
                    );
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({ "error": "failed to load confidential session" }),
                    )
                })?
        else {
            return Err((
                StatusCode::NOT_FOUND,
                json!({
                    "error": "confidential session not found",
                    "confidential_session_hash": confidential_session_hash,
                }),
            ));
        };
        if now > session.session.payload.expires_at {
            return Err((
                StatusCode::GONE,
                json!({ "error": "confidential session expired" }),
            ));
        }
        confidential::verify_attestation_bundle(
            &session.profile.payload,
            &session.session,
            &session.attestation,
            now,
        )
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("invalid confidential session attestation: {error}") }),
            )
        })?;
    }

    let quoted_total_msat = payload.quote.payload.settlement_terms.base_fee_msat
        + payload.quote.payload.settlement_terms.success_fee_msat;
    let quoted_total_sats = quoted_total_msat / 1_000;
    let uses_lightning_bundle = quoted_total_sats > 0
        && state
            .config
            .payment_backends
            .contains(&PaymentBackend::Lightning);
    if let Err((status, message)) =
        validate_deal_deadlines(&payload.quote, &payload.deal, now, uses_lightning_bundle)
    {
        return Err((status, json!({ "error": message })));
    }

    if let Some(existing) =
        find_existing_deal_by_artifact_hash(state.as_ref(), &canonical_deal_hash)
            .await
            .map_err(|response| (response.0, response.1.0))?
    {
        if existing.quote.hash != canonical_quote_hash {
            return Err((
                StatusCode::CONFLICT,
                json!({ "error": "deal artifact hash already exists with a different quote" }),
            ));
        }
        if let Some(idempotency_key) = idempotency_key.clone()
            && let Some(existing_by_key) = find_existing_deal(state.as_ref(), Some(idempotency_key))
                .await
                .map_err(|response| (response.0, response.1.0))?
            && existing_by_key.artifact.hash != canonical_deal_hash
        {
            return Err((
                StatusCode::CONFLICT,
                json!({ "error": "idempotency key reused with different deal payload" }),
            ));
        }
        return Ok((existing.public_record(), StatusCode::OK));
    }

    if let Some(existing) = find_existing_deal(state.as_ref(), idempotency_key.clone())
        .await
        .map_err(|response| (response.0, response.1.0))?
    {
        if existing.quote.hash != canonical_quote_hash
            || existing.artifact.hash != canonical_deal_hash
        {
            return Err((
                StatusCode::CONFLICT,
                json!({ "error": "idempotency key reused with different deal payload" }),
            ));
        }

        return Ok((existing.public_record(), StatusCode::OK));
    }

    if quoted_total_sats > 0 && payload.payment.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "paid deals use the signed deal plus invoice-bundle flow instead of inline payment tokens"
            }),
        ));
    }

    let deal_id = protocol::new_artifact_id();
    let reservation = None;
    let deal_artifact = payload.deal.clone();
    let mut reserved_execution_permit = None;
    let mut immediate_rejection: Option<(
        String,
        ReceiptFailure,
        SignedArtifact<ReceiptPayload>,
        String,
    )> = None;
    let deal_json = serde_json::to_string(&deal_artifact).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to encode deal: {error}") }),
        )
    })?;
    let quote_json = serde_json::to_string(&payload.quote).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to encode quote: {error}") }),
        )
    })?;
    let pending_materialization_request =
        uses_lightning_bundle.then(|| settlement::BuildLightningInvoiceBundleRequest {
            session_id: Some(deal_id.clone()),
            requester_id: payload.deal.payload.requester_id.clone(),
            quote_hash: canonical_quote_hash.clone(),
            deal_hash: canonical_deal_hash.clone(),
            admission_deadline: Some(payload.deal.payload.admission_deadline),
            success_payment_hash: payload.deal.payload.success_payment_hash.clone(),
            base_fee_msat: payload.quote.payload.settlement_terms.base_fee_msat,
            success_fee_msat: payload.quote.payload.settlement_terms.success_fee_msat,
            created_at: now,
        });

    if !uses_lightning_bundle && payload.spec.runtime() == Some("wasm") {
        match state.wasm_sandbox.try_acquire_execution_permit() {
            Ok(permit) => reserved_execution_permit = Some(permit),
            Err(error_message) => {
                let failure = receipt_failure("capacity_exhausted", error_message.clone());
                let rejected_deal = deals::StoredDeal {
                    deal_id: deal_id.clone(),
                    idempotency_key: idempotency_key.clone(),
                    quote: payload.quote.clone(),
                    spec: payload.spec.clone(),
                    artifact: deal_artifact.clone(),
                    status: deals::DEAL_STATUS_ACCEPTED.to_string(),
                    result: None,
                    result_hash: None,
                    error: None,
                    payment_method: None,
                    payment_token_hash: None,
                    payment_amount_sats: None,
                    receipt: None,
                    created_at: now,
                    updated_at: now,
                };
                let receipt = sign_deal_receipt(
                    state.as_ref(),
                    &rejected_deal,
                    now,
                    ReceiptSignSpec {
                        deal_state: "rejected",
                        execution_state: "not_started",
                        bundle: None,
                        result_hash: None,
                        result_format: None,
                        result_envelope_hash: None,
                        failure: Some(failure.clone()),
                    },
                )
                .map_err(|error| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({ "error": format!("failed to sign rejection receipt: {error}") }),
                    )
                })?;
                let receipt_json = serde_json::to_string(&receipt).map_err(|error| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({ "error": format!("failed to encode rejection receipt: {error}") }),
                    )
                })?;
                immediate_rejection = Some((error_message, failure, receipt, receipt_json));
            }
        }
    }

    let deal_hash = canonical_deal_hash.clone();
    let deal_payload_hash = deal_artifact.payload_hash.clone();
    let deal_actor_id = deal_artifact.signer.clone();
    let deal_artifact_hash = canonical_deal_hash.clone();
    let quote_hash = canonical_quote_hash.clone();
    let quote_payload_hash = payload.quote.payload_hash.clone();
    let quote_actor_id = payload.quote.signer.clone();
    let quote_id = canonical_quote_hash.clone();
    let deal_id_for_db = deal_id.clone();
    let spec_for_evidence = payload.spec.clone();
    let quote_artifact_ref = json!({ "artifact_hash": quote_hash.clone() });
    let deal_artifact_ref = json!({ "artifact_hash": deal_hash.clone() });
    let materialization_request_for_db = pending_materialization_request.clone();
    let immediate_rejection_for_db = immediate_rejection.clone();
    let insert_result = state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<deals::InsertDealOutcome, String> {
                db::insert_artifact_document(
                    conn,
                    &quote_hash,
                    &quote_payload_hash,
                    ARTIFACT_KIND_QUOTE,
                    &quote_actor_id,
                    payload.quote.created_at,
                    &quote_json,
                )?;

                match deals::get_quote(conn, &quote_id)? {
                    Some(stored) if stored.artifact.hash != quote_hash => {
                        return Err("quote hash already exists with different contents".to_string());
                    }
                    Some(_) => {}
                    None => deals::insert_quote(conn, &payload.quote)?,
                }

                db::insert_artifact_document(
                    conn,
                    &deal_hash,
                    &deal_payload_hash,
                    ARTIFACT_KIND_DEAL,
                    &deal_actor_id,
                    deal_artifact.created_at,
                    &deal_json,
                )?;

                let deal_for_db = NewDeal {
                    deal_id: deal_id_for_db.clone(),
                    idempotency_key: idempotency_key.clone(),
                    quote: payload.quote.clone(),
                    spec: payload.spec.clone(),
                    artifact: deal_artifact.clone(),
                    workload_evidence_hash: None,
                    deal_artifact_hash: deal_artifact_hash.clone(),
                    payment_method: uses_lightning_bundle.then(|| "lightning".to_string()),
                    payment_token_hash: uses_lightning_bundle
                        .then(|| payload.deal.payload.success_payment_hash.clone()),
                    payment_amount_sats: uses_lightning_bundle.then_some(quoted_total_sats),
                    initial_status: if uses_lightning_bundle {
                        deals::DEAL_STATUS_PAYMENT_PENDING.to_string()
                    } else {
                        deals::DEAL_STATUS_ACCEPTED.to_string()
                    },
                    created_at: now,
                };

                let insert_outcome = deals::insert_or_get_deal(conn, deal_for_db.clone())?;
                if insert_outcome.created {
                    let workload_evidence_hash = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &insert_outcome.deal.deal_id,
                        "workload_spec",
                        &spec_for_evidence,
                        now,
                    )?;
                    deals::set_deal_storage_refs(
                        conn,
                        &insert_outcome.deal.deal_id,
                        &workload_evidence_hash,
                        &deal_artifact_hash,
                    )?;
                    let _ = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &insert_outcome.deal.deal_id,
                        "quote_artifact_ref",
                        &quote_artifact_ref,
                        now,
                    )?;
                    let _ = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &insert_outcome.deal.deal_id,
                        "deal_artifact_ref",
                        &deal_artifact_ref,
                        now,
                    )?;
                    if let Some(materialization_request) = materialization_request_for_db.as_ref() {
                        let request_json = serde_json::to_string(materialization_request)
                            .map_err(|error| error.to_string())?;
                        db::insert_deal_settlement_materialization(
                            conn,
                            &insert_outcome.deal.deal_id,
                            "lightning_invoice_bundle",
                            &request_json,
                            now,
                        )?;
                    }
                    if let Some((error_message, failure, receipt, receipt_json)) =
                        immediate_rejection_for_db.as_ref()
                    {
                        let failure_evidence_hash = db::insert_execution_evidence(
                            conn,
                            "deal",
                            &insert_outcome.deal.deal_id,
                            "execution_failure",
                            failure,
                            now,
                        )?;
                        let rejected = deals::reject_deal_admission(
                            conn,
                            &insert_outcome.deal.deal_id,
                            error_message,
                            receipt,
                            Some(&failure_evidence_hash),
                            Some(&receipt.hash),
                            now,
                        )?;
                        if !rejected {
                            return Err(
                                "deal could not be rejected after capacity admission check"
                                    .to_string(),
                            );
                        }
                        db::insert_artifact_document(
                            conn,
                            &receipt.hash,
                            &receipt.payload_hash,
                            ARTIFACT_KIND_RECEIPT,
                            &receipt.signer,
                            receipt.created_at,
                            receipt_json,
                        )?;
                        let _ = db::insert_execution_evidence(
                            conn,
                            "deal",
                            &insert_outcome.deal.deal_id,
                            "receipt_artifact_ref",
                            &json!({ "artifact_hash": receipt.hash }),
                            now,
                        )?;
                    }
                }

                Ok(insert_outcome)
            })();

            let result = match operation {
                Ok(result) => result,
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(error);
                }
            };

            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(result)
        })
        .await;

    let insert_result = match insert_result {
        Ok(result) => result,
        Err(error) => {
            let _ = release_payment(state.as_ref(), reservation).await;
            let status = if error.contains("idempotency key reused") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return Err((status, json!({ "error": error })));
        }
    };

    if !insert_result.created {
        let _ = release_payment(state.as_ref(), reservation).await;
        return Ok((insert_result.deal.public_record(), StatusCode::OK));
    }

    if immediate_rejection.is_some() {
        let rejected_deal_id = deal_id.clone();
        let rejected = state
            .db
            .with_read_conn(move |conn| deals::get_deal(conn, &rejected_deal_id))
            .await
            .map_err(|error| {
                tracing::error!("Failed to reload rejected deal {deal_id}: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "failed to reload rejected deal" }),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "rejected deal missing after persistence" }),
                )
            })?;
        return Ok((rejected.public_record(), StatusCode::ACCEPTED));
    }

    if uses_lightning_bundle {
        if let Err(error) = materialize_pending_lightning_bundle(state.clone(), &deal_id).await {
            tracing::error!("Failed to materialize paid deal settlement for {deal_id}: {error}");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to materialize lightning invoice bundle" }),
            ));
        }

        let persisted_deal_id = deal_id.clone();
        let persisted = state
            .db
            .with_read_conn(move |conn| deals::get_deal(conn, &persisted_deal_id))
            .await
            .map_err(|error| {
                tracing::error!("Failed to reload paid deal {deal_id}: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "failed to reload persisted deal" }),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "persisted deal missing after settlement materialization" }),
                )
            })?;
        return Ok((persisted.public_record(), StatusCode::ACCEPTED));
    }

    if !uses_lightning_bundle {
        tokio::spawn(process_deal_with_reserved_permit(
            state,
            deal_id,
            reserved_execution_permit,
        ));
    }
    Ok((insert_result.deal.public_record(), StatusCode::ACCEPTED))
}

async fn fail_pending_deal_materialization(
    state: Arc<AppState>,
    deal: &deals::StoredDeal,
    failure_code: &str,
    error_message: String,
) -> Result<(), String> {
    let failure = receipt_failure(failure_code, error_message.clone());
    let completed_at = settlement::current_unix_timestamp();
    let receipt = sign_deal_receipt(
        state.as_ref(),
        deal,
        completed_at,
        ReceiptSignSpec {
            deal_state: "failed",
            execution_state: "not_started",
            bundle: None,
            result_hash: None,
            result_format: None,
            result_envelope_hash: None,
            failure: Some(failure.clone()),
        },
    )
    .map_err(|error| error.to_string())?;
    let receipt_json = serde_json::to_string(&receipt).map_err(|error| error.to_string())?;
    let deal_id = deal.deal_id.clone();
    let expected_status = deal.status.clone();
    let receipt_for_db = receipt.clone();
    let updated = state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|error| error.to_string())?;
            let operation = (|| -> Result<bool, String> {
                let _ = db::delete_deal_settlement_materialization(conn, &deal_id)?;
                let failure_evidence_hash = db::insert_execution_evidence(
                    conn,
                    "deal",
                    &deal_id,
                    "execution_failure",
                    &failure,
                    completed_at,
                )?;
                let updated = deals::complete_deal_failure_if_status(
                    conn,
                    deals::DealTerminalTransition {
                        deal_id: &deal_id,
                        expected_status: &expected_status,
                        error: &error_message,
                        receipt: &receipt_for_db,
                        failure_evidence_hash: Some(&failure_evidence_hash),
                        receipt_artifact_hash: Some(&receipt_for_db.hash),
                        now: completed_at,
                    },
                )?;

                if updated {
                    db::insert_artifact_document(
                        conn,
                        &receipt_for_db.hash,
                        &receipt_for_db.payload_hash,
                        ARTIFACT_KIND_RECEIPT,
                        &receipt_for_db.signer,
                        receipt_for_db.created_at,
                        &receipt_json,
                    )?;
                    let _ = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &deal_id,
                        "receipt_artifact_ref",
                        &json!({ "artifact_hash": receipt_for_db.hash }),
                        completed_at,
                    )?;
                }

                Ok(updated)
            })();

            let result = match operation {
                Ok(result) => result,
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(error);
                }
            };

            conn.execute_batch("COMMIT")
                .map_err(|error| error.to_string())?;
            Ok(result)
        })
        .await?;

    if !updated {
        tracing::warn!(
            "Deal {} changed state before settlement materialization failure could be persisted",
            deal.deal_id
        );
    }

    Ok(())
}

async fn materialize_pending_lightning_bundle(
    state: Arc<AppState>,
    deal_id: &str,
) -> Result<(), String> {
    let lookup_deal_id = deal_id.to_string();
    let (deal, materialization) = state
        .db
        .with_read_conn(
            move |conn| -> Result<
                (
                    Option<deals::StoredDeal>,
                    Option<db::DealSettlementMaterializationRecord>,
                ),
                String,
            > {
                Ok((
                    deals::get_deal(conn, &lookup_deal_id)?,
                    db::get_deal_settlement_materialization(conn, &lookup_deal_id)?,
                ))
            },
        )
        .await?;

    let Some(deal) = deal else {
        return Ok(());
    };
    let Some(materialization) = materialization else {
        return Ok(());
    };

    if materialization.materialization_kind != "lightning_invoice_bundle" {
        return Err(format!(
            "unsupported settlement materialization kind: {}",
            materialization.materialization_kind
        ));
    }

    let request: settlement::BuildLightningInvoiceBundleRequest =
        serde_json::from_str(&materialization.request_json).map_err(|error| {
            format!(
                "invalid settlement materialization payload for deal {}: {error}",
                deal.deal_id
            )
        })?;

    let bundle = match settlement::issue_lightning_invoice_bundle(state.as_ref(), request).await {
        Ok(bundle) => bundle,
        Err(error) => {
            fail_pending_deal_materialization(
                state,
                &deal,
                "lightning_invoice_bundle_materialization_failed",
                error.clone(),
            )
            .await?;
            return Err(error);
        }
    };

    let bundle_for_db = bundle.clone();
    let bundle_session_id = bundle.session_id.clone();
    let deal_id_for_db = deal.deal_id.clone();
    let deal_hash_for_db = deal.artifact.hash.clone();
    let persisted = state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|error| error.to_string())?;
            let operation = (|| -> Result<(), String> {
                if db::get_deal_settlement_materialization(conn, &deal_id_for_db)?.is_none() {
                    return Ok(());
                }

                if db::get_lightning_invoice_bundle_by_deal_hash(conn, &deal_hash_for_db)?.is_some()
                {
                    let _ = db::delete_deal_settlement_materialization(conn, &deal_id_for_db)?;
                    return Ok(());
                }

                db::insert_lightning_invoice_bundle(
                    conn,
                    &bundle_session_id,
                    &bundle_for_db.bundle,
                    bundle_for_db.base_state.clone(),
                    bundle_for_db.success_state.clone(),
                    bundle_for_db.created_at,
                )?;
                let _ = db::insert_execution_evidence(
                    conn,
                    "deal",
                    &deal_id_for_db,
                    "lightning_invoice_bundle_ref",
                    &json!({
                        "session_id": bundle_for_db.session_id,
                        "bundle_hash": bundle_for_db.bundle.hash,
                    }),
                    settlement::current_unix_timestamp(),
                )?;
                let _ = db::delete_deal_settlement_materialization(conn, &deal_id_for_db)?;
                Ok(())
            })();

            if let Err(error) = operation {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error);
            }

            conn.execute_batch("COMMIT")
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .await;

    if let Err(error) = persisted {
        let cancel_error =
            settlement::cancel_lightning_invoice_bundle(state.as_ref(), &bundle).await;
        let failure_message = match cancel_error {
            Ok(()) => error.clone(),
            Err(cancel_error) => format!(
                "{error}; additionally failed to cancel materialized lightning invoices: {cancel_error}"
            ),
        };
        fail_pending_deal_materialization(
            state,
            &deal,
            "lightning_invoice_bundle_persist_failed",
            failure_message.clone(),
        )
        .await?;
        return Err(failure_message);
    }

    Ok(())
}

fn validate_job_spec(spec: &JobSpec) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match spec {
        JobSpec::Execution { execution } => validate_execution_workload(execution),
        JobSpec::Wasm { submission } => validate_wasm_submission(submission),
        JobSpec::OciWasm { submission } => validate_oci_wasm_submission(submission),
    }
}

fn validate_wasm_submission(
    submission: &WasmSubmission,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if let Err(error) = submission.validate_limits(MAX_WASM_HEX_BYTES, MAX_WASM_INPUT_BYTES) {
        return Err(error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({ "error": error }),
        ));
    }

    if let Err(error) = submission.verify() {
        return Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": error }),
        ));
    }

    Ok(())
}

fn validate_oci_wasm_submission(
    submission: &crate::wasm::OciWasmSubmission,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if let Err(error) = submission.validate_limits(MAX_WASM_INPUT_BYTES) {
        return Err(error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({ "error": error }),
        ));
    }

    if let Err(error) = submission.verify() {
        return Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": error }),
        ));
    }

    Ok(())
}

fn validate_execution_workload(
    execution: &ExecutionWorkload,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    execution
        .validate_basic()
        .map_err(|error| error_json(StatusCode::BAD_REQUEST, json!({ "error": error })))?;

    if execution.is_service_addressed() {
        return match (&execution.runtime, &execution.package_kind) {
            (ExecutionRuntime::Python, ExecutionPackageKind::InlineSource)
            | (ExecutionRuntime::TeePython, ExecutionPackageKind::InlineSource) => {
                if execution.contract_version != CONTRACT_PYTHON_HANDLER_JSON_V1
                    && execution.contract_version != CONTRACT_PYTHON_SCRIPT_JSON_V1
                {
                    return Err(error_json(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "unsupported python contract_version", "contract_version": execution.contract_version }),
                    ));
                }
                Ok(())
            }
            (ExecutionRuntime::Container, ExecutionPackageKind::OciImage)
            | (ExecutionRuntime::Python, ExecutionPackageKind::OciImage) => {
                if execution.contract_version != CONTRACT_CONTAINER_JSON_V1 {
                    return Err(error_json(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "unsupported container contract_version", "contract_version": execution.contract_version }),
                    ));
                }
                Ok(())
            }
            (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin) => Ok(()),
            _ => Ok(()),
        };
    }

    match (&execution.runtime, &execution.package_kind) {
        (ExecutionRuntime::Wasm, ExecutionPackageKind::InlineModule)
        | (ExecutionRuntime::TeeWasm, ExecutionPackageKind::InlineModule) => {
            let submission = execution
                .to_wasm_submission()
                .map_err(|error| error_json(StatusCode::BAD_REQUEST, json!({ "error": error })))?;
            validate_wasm_submission(&submission)
        }
        (ExecutionRuntime::Wasm, ExecutionPackageKind::OciImage) => {
            let submission = execution
                .to_oci_wasm_submission()
                .map_err(|error| error_json(StatusCode::BAD_REQUEST, json!({ "error": error })))?;
            validate_oci_wasm_submission(&submission)
        }
        (ExecutionRuntime::Python, ExecutionPackageKind::InlineSource)
        | (ExecutionRuntime::TeePython, ExecutionPackageKind::InlineSource) => {
            if execution.contract_version != CONTRACT_PYTHON_HANDLER_JSON_V1
                && execution.contract_version != CONTRACT_PYTHON_SCRIPT_JSON_V1
            {
                return Err(error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "unsupported python contract_version", "contract_version": execution.contract_version }),
                ));
            }
            Ok(())
        }
        (ExecutionRuntime::Container, ExecutionPackageKind::OciImage)
        | (ExecutionRuntime::Python, ExecutionPackageKind::OciImage) => {
            if execution.contract_version != CONTRACT_CONTAINER_JSON_V1 {
                return Err(error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "unsupported container contract_version", "contract_version": execution.contract_version }),
                ));
            }
            Ok(())
        }
        (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin) => {
            // events.query has specific validation; other builtins are
            // validated structurally and rejected at dispatch time if no
            // handler is registered.
            if let Some((kinds, limit)) = execution.events_query_params() {
                if kinds.is_empty() {
                    return Err(error_json(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "events query must include at least one kind" }),
                    ));
                }
                if kinds.len() > db::MAX_EVENT_QUERY_KINDS {
                    return Err(error_json(
                        StatusCode::BAD_REQUEST,
                        json!({
                            "error": "events query includes too many kinds",
                            "max_kinds": db::MAX_EVENT_QUERY_KINDS,
                        }),
                    ));
                }
                if let Some(limit) = limit
                    && limit > 500
                {
                    return Err(error_json(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "events query limit exceeds maximum", "max_limit": 500 }),
                    ));
                }
                if execution.contract_version != CONTRACT_BUILTIN_EVENTS_QUERY_V1 {
                    return Err(error_json(
                        StatusCode::BAD_REQUEST,
                        json!({
                            "error": "unsupported builtin contract_version",
                            "contract_version": execution.contract_version
                        }),
                    ));
                }
            }
            Ok(())
        }
        (ExecutionRuntime::TeeService, ExecutionPackageKind::Builtin) => {
            let Some(request_envelope) = execution.security.request_envelope.as_ref() else {
                return Err(error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "tee.service execution requires request_envelope" }),
                ));
            };
            request_envelope
                .validate()
                .map_err(|error| error_json(StatusCode::BAD_REQUEST, json!({ "error": error })))
        }
        _ => Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "unsupported runtime/package_kind combination",
                "runtime": execution.runtime.as_str(),
                "package_kind": execution.package_kind.as_str(),
            }),
        )),
    }
}

/// Plan for how granted data-source mounts should reshape a workload's
/// sandbox + env. Produced by [`collect_data_mount_plan`] from the
/// intersection of (workload's declared mounts, granted capabilities,
/// operator-configured bindings).
#[derive(Debug, Default, Clone)]
struct DataMountPlan {
    /// `(env_name, value)` pairs to inject into the workload's environment.
    env: Vec<(String, String)>,
    /// Filesystem paths the sandbox must grant write access to. Used for
    /// file-based mounts (SQLite). Each path is either the DB file itself or
    /// its parent directory, depending on what landlock allows cleanly.
    writable_paths: Vec<std::path::PathBuf>,
    /// True when any granted mount requires outbound network. Flips the
    /// Python sandbox's `allow_network` flag.
    needs_network: bool,
}

/// Return the [`DataMountPlan`] for the given workload + granted capabilities.
///
/// A mount is included only when the workload declared it with a supported
/// `kind`, the capability string `mount.<kind>.<read|write>.<handle>` is in
/// `granted_access` (or `granted_access` is empty), and the operator
/// configured a binding via `FROGLET_MOUNT_<kind>_<handle>`. Handles are
/// matched case-insensitively.
///
/// Every kind injects:
/// - `FROGLET_MOUNT_<HANDLE>_URL` — operator-configured binding string
/// - `FROGLET_MOUNT_<HANDLE>_READ_ONLY` — `"true"` or `"false"`
///
/// Kind-specific behavior:
/// - `postgres`, `s3`, `redis`: `needs_network = true` (workload will open
///   outbound TCP to the backing service).
/// - `sqlite`: extends `writable_paths` with the DB file's parent directory
///   so the sandbox grants access to the `.db`, `.db-journal`, and `.db-wal`
///   files SQLite creates. No network needed.
fn collect_data_mount_plan(
    execution: &ExecutionWorkload,
    granted_access: &[String],
) -> DataMountPlan {
    const NETWORK_KINDS: &[&str] = &["postgres", "s3", "redis"];
    const FILE_KINDS: &[&str] = &["sqlite"];
    let mut plan = DataMountPlan::default();

    for mount in &execution.mounts {
        let kind = mount.kind.to_ascii_lowercase();
        let is_network = NETWORK_KINDS.iter().any(|k| k == &kind);
        let is_file = FILE_KINDS.iter().any(|k| k == &kind);
        if !is_network && !is_file {
            continue;
        }
        let capability = format!(
            "mount.{}.{}.{}",
            mount.kind,
            if mount.read_only { "read" } else { "write" },
            mount.handle
        );
        if !granted_access.is_empty() && !granted_access.iter().any(|c| c == &capability) {
            continue;
        }
        let env_key = format!("FROGLET_MOUNT_{kind}_{}", mount.handle);
        let Ok(binding) = std::env::var(&env_key) else {
            continue;
        };
        if binding.is_empty() {
            continue;
        }
        let safe_handle = mount.handle.to_ascii_uppercase();
        plan.env
            .push((format!("FROGLET_MOUNT_{safe_handle}_URL"), binding.clone()));
        plan.env.push((
            format!("FROGLET_MOUNT_{safe_handle}_READ_ONLY"),
            if mount.read_only { "true" } else { "false" }.to_string(),
        ));
        if is_network {
            plan.needs_network = true;
        }
        if is_file && kind == "sqlite" {
            // SQLite creates `-journal` and `-wal` files adjacent to the DB.
            // Grant the parent directory so all three are reachable. Operators
            // who want tighter isolation should give each mount its own
            // directory.
            let db_path = std::path::PathBuf::from(&binding);
            if let Some(parent) = db_path.parent()
                && !parent.as_os_str().is_empty()
            {
                plan.writable_paths.push(parent.to_path_buf());
            } else {
                // Bare filename — fall back to the file itself.
                plan.writable_paths.push(db_path);
            }
        }
    }
    plan
}

fn execution_mount_context(execution: &ExecutionWorkload, granted_access: &[String]) -> Value {
    let mounts = execution
        .mounts
        .iter()
        .filter(|mount| {
            let handle = format!(
                "mount.{}.{}.{}",
                mount.kind,
                if mount.read_only { "read" } else { "write" },
                mount.handle
            );
            granted_access.is_empty() || granted_access.iter().any(|value| value == &handle)
        })
        .map(|mount| {
            (
                mount.handle.clone(),
                json!({
                    "kind": mount.kind,
                    "read_only": mount.read_only,
                    "binding": mount.binding,
                }),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    Value::Object(mounts)
}

async fn run_python_execution(
    execution: &ExecutionWorkload,
    granted_access: &[String],
    timeout: Duration,
) -> Result<Value, String> {
    let source = execution
        .inline_source
        .as_ref()
        .ok_or_else(|| "python execution requires inline_source".to_string())?;
    let input_json = canonical_json::to_vec(&execution.input).map_err(|error| error.to_string())?;
    let mount_context = execution_mount_context(execution, granted_access);
    let runner = r#"
import json, os, sys, traceback
source_path = sys.argv[1]
entrypoint_kind = os.environ.get("FROGLET_ENTRYPOINT_KIND", "handler")
entrypoint = os.environ.get("FROGLET_ENTRYPOINT", "handler")
context = json.loads(os.environ.get("FROGLET_CONTEXT", "{}"))
event = json.load(sys.stdin)
namespace = {"__name__": "__froglet__", "__file__": source_path}
with open(source_path, "r", encoding="utf-8") as handle:
    source = handle.read()
exec(compile(source, source_path, "exec"), namespace)
if entrypoint_kind == "handler":
    fn = namespace.get(entrypoint)
    if not callable(fn):
        raise RuntimeError(f"missing handler: {entrypoint}")
    result = fn(event, context)
elif entrypoint_kind == "script":
    if "result" in namespace:
        result = namespace["result"]
    elif callable(namespace.get("main")):
        result = namespace["main"](event, context)
    else:
        raise RuntimeError("python script must define result or main(event, context)")
else:
    raise RuntimeError(f"unsupported entrypoint kind: {entrypoint_kind}")
json.dump(result, sys.stdout, separators=(",", ":"))
"#;
    let tempdir = private_runtime_tempdir("froglet-python")?;
    let source_path = tempdir.join("main.py");
    fs::write(&source_path, source)
        .map_err(|error| format!("failed to write python source: {error}"))?;
    let context = json!({
        "mounts": mount_context,
    });
    let source_path_string = source_path.display().to_string();
    let entrypoint_kind = execution.entrypoint.kind.as_str().to_string();
    let entrypoint = execution.entrypoint.value.clone();
    let context_json = context.to_string();
    let timeout_secs = timeout;
    let input_json_clone = input_json.clone();
    let kill_handle: ChildKillHandle = Arc::new(std::sync::Mutex::new(None));
    let kill_handle_clone = Arc::clone(&kill_handle);
    // Resolve operator-configured data-source mounts that this invocation
    // was granted. A mount is injected only when (a) the workload declared a
    // mount with a supported kind for this handle, (b) the capability list
    // granted access to it, and (c) the operator configured a binding for
    // the handle via FROGLET_MOUNT_<kind>_<handle>. See docs/MOUNTS.md.
    let mount_plan = collect_data_mount_plan(execution, granted_access);
    // Per-invocation sandbox policy: read Python stdlib + CA certs, write
    // only to the invocation tempdir, no outbound network by default. Data
    // mounts extend this — a network-backed mount (postgres / s3 / redis)
    // flips allow_network=true, and a file-backed mount (sqlite) adds the
    // DB parent directory to the sandbox's writable_paths. Both are
    // coarse-grained for v1; tightening is tracked as a hardening follow-up.
    let mut sandbox_config = crate::python_sandbox::SandboxConfig::for_python(&tempdir);
    if mount_plan.needs_network {
        sandbox_config.allow_network = true;
    }
    sandbox_config
        .writable_paths
        .extend(mount_plan.writable_paths.clone());
    let mount_env = mount_plan.env.clone();
    let result = run_wasm_with_timeout_and_kill(timeout_secs, Some(kill_handle), move || {
        let mut command = std::process::Command::new("python3");
        command
            .arg("-I")
            .arg("-c")
            .arg(runner)
            .arg(&source_path_string)
            .env("FROGLET_ENTRYPOINT_KIND", entrypoint_kind)
            .env("FROGLET_ENTRYPOINT", entrypoint)
            .env("FROGLET_CONTEXT", context_json)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in &mount_env {
            command.env(name, value);
        }
        crate::python_sandbox::harden_command(&mut command, sandbox_config.clone())
            .map_err(|error| format!("failed to install python sandbox: {error}"))?;
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to spawn python3: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&input_json_clone)
                .map_err(|error| format!("failed to write python input: {error}"))?;
        }
        // Take stdout/stderr pipes before storing child in kill handle.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        *kill_handle_clone
            .lock()
            .map_err(|_| "python kill handle lock poisoned".to_string())? = Some(child);
        // Read stdout and stderr concurrently to avoid pipe-backpressure deadlock.
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe {
                let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
            }
            buf
        });
        let mut stdout_buf = Vec::new();
        if let Some(mut pipe) = stdout_pipe {
            std::io::Read::read_to_end(&mut pipe, &mut stdout_buf)
                .map_err(|error| format!("failed to read python stdout: {error}"))?;
        }
        let stderr_buf = stderr_thread.join().unwrap_or_default();
        // Wait for child to exit; kill handle can kill it on timeout.
        let status = kill_handle_clone
            .lock()
            .map_err(|_| "python kill handle lock poisoned".to_string())?
            .as_mut()
            .map(|c| c.wait())
            .transpose()
            .map_err(|error| format!("failed waiting for python execution: {error}"))?;
        if let Some(status) = status
            && !status.success()
        {
            let stderr = String::from_utf8_lossy(&stderr_buf).trim().to_string();
            return Err(format!("python execution failed: {stderr}").into());
        }
        serde_json::from_slice::<Value>(&stdout_buf)
            .map_err(|error| format!("python execution returned invalid JSON: {error}").into())
    })
    .await;
    let _ = fs::remove_dir_all(&tempdir);
    result
}

fn detect_container_runner() -> Option<String> {
    for candidate in ["docker", "podman"] {
        if std::process::Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return Some(candidate.to_string());
        }
    }
    None
}

async fn run_container_execution(
    execution: &ExecutionWorkload,
    granted_access: &[String],
    timeout: Duration,
) -> Result<Value, String> {
    let runner = detect_container_runner().ok_or_else(|| {
        "no supported OCI/container runtime found (expected docker or podman)".to_string()
    })?;
    let image = execution
        .oci_reference
        .as_ref()
        .ok_or_else(|| "container execution requires oci_reference".to_string())?;
    let oci_digest = execution
        .oci_digest
        .as_ref()
        .ok_or_else(|| "container execution requires oci_digest".to_string())?;
    let image_ref = crate::execution::digest_pinned_oci_image_reference(image, oci_digest)?;
    let input_json = canonical_json::to_vec(&execution.input).map_err(|error| error.to_string())?;
    let mount_context = execution_mount_context(execution, granted_access);
    let mounts = execution.mounts.clone();
    let oci_digest = Some(oci_digest.to_string());
    let timeout_secs = timeout;
    let context_json = json!({ "mounts": mount_context }).to_string();
    let granted_access_clone = granted_access.to_vec();
    let kill_handle: ChildKillHandle = Arc::new(std::sync::Mutex::new(None));
    let kill_handle_clone = Arc::clone(&kill_handle);
    run_wasm_with_timeout_and_kill(timeout_secs, Some(kill_handle), move || {
        let mut command = std::process::Command::new(&runner);
        command
            .arg("run")
            .arg("--rm")
            .arg("-i")
            .arg("--network")
            .arg("none")
            .env("FROGLET_CONTEXT", &context_json);
        if let Some(oci_digest) = oci_digest.as_ref() {
            command.env("FROGLET_OCI_DIGEST", oci_digest);
        }
        for mount in mounts.iter() {
            let Some(binding) = mount.binding.as_ref() else {
                continue;
            };
            let capability = format!(
                "mount.{}.{}.{}",
                mount.kind,
                if mount.read_only { "read" } else { "write" },
                mount.handle
            );
            if !granted_access_clone.is_empty()
                && !granted_access_clone.iter().any(|v| v == &capability)
            {
                continue;
            }
            let target = format!("/froglet-mounts/{}", mount.handle);
            let mut volume = format!("{binding}:{target}");
            if mount.read_only {
                volume.push_str(":ro");
            }
            command.arg("-v").arg(volume);
        }
        command
            .arg(&image_ref)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to spawn container runtime: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&input_json)
                .map_err(|error| format!("failed to write container input: {error}"))?;
        }
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        *kill_handle_clone
            .lock()
            .map_err(|_| "container kill handle lock poisoned".to_string())? = Some(child);
        // Read stdout and stderr concurrently to avoid pipe-backpressure deadlock.
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe {
                let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
            }
            buf
        });
        let mut stdout_buf = Vec::new();
        if let Some(mut pipe) = stdout_pipe {
            std::io::Read::read_to_end(&mut pipe, &mut stdout_buf)
                .map_err(|error| format!("failed to read container stdout: {error}"))?;
        }
        let stderr_buf = stderr_thread.join().unwrap_or_default();
        let status = kill_handle_clone
            .lock()
            .map_err(|_| "container kill handle lock poisoned".to_string())?
            .as_mut()
            .map(|c| c.wait())
            .transpose()
            .map_err(|error| format!("failed waiting for container execution: {error}"))?;
        if let Some(status) = status
            && !status.success()
        {
            let stderr = String::from_utf8_lossy(&stderr_buf).trim().to_string();
            return Err(format!("container execution failed: {stderr}").into());
        }
        serde_json::from_slice::<Value>(&stdout_buf)
            .map_err(|error| format!("container execution returned invalid JSON: {error}").into())
    })
    .await
}

fn normalized_service_execution_profile(
    service: &ProviderServiceRecord,
) -> Result<
    (
        ExecutionRuntime,
        ExecutionPackageKind,
        ExecutionEntrypointKind,
        String,
        String,
    ),
    String,
> {
    let runtime = ExecutionRuntime::parse(&service.runtime)?;
    let package_kind = ExecutionPackageKind::parse(&service.package_kind)?;
    let entrypoint_kind = if service.entrypoint_kind.trim().is_empty() {
        default_entrypoint_kind_for(&runtime)
    } else {
        ExecutionEntrypointKind::parse(&service.entrypoint_kind)?
    };
    let use_default_entrypoint = service.entrypoint.trim().is_empty()
        || (matches!(entrypoint_kind, ExecutionEntrypointKind::Handler)
            && (service.entrypoint.contains('/')
                || service.entrypoint.ends_with(".py")
                || service.entrypoint.contains('\\')));
    let entrypoint = if use_default_entrypoint {
        default_entrypoint_for(&runtime, &entrypoint_kind).to_string()
    } else {
        service.entrypoint.clone()
    };
    let contract_version = if service.contract_version.trim().is_empty() {
        default_contract_version_for(&runtime, &package_kind, &entrypoint_kind).to_string()
    } else {
        service.contract_version.clone()
    };
    Ok((
        runtime,
        package_kind,
        entrypoint_kind,
        entrypoint,
        contract_version,
    ))
}

fn service_binding_hash(service: &ProviderServiceRecord) -> Option<&str> {
    service
        .binding_hash
        .as_deref()
        .or(service.module_hash.as_deref())
        .filter(|value| !value.trim().is_empty())
}

fn validate_service_addressed_execution_against_service(
    execution: &ExecutionWorkload,
    service: &ProviderServiceRecord,
) -> Result<(), String> {
    let requested_service_id = execution
        .service_id()
        .ok_or_else(|| "service-addressed execution requires security.service_id".to_string())?;
    if requested_service_id != service.service_id {
        return Err(
            "service-addressed execution service_id does not match local service".to_string(),
        );
    }
    let (runtime, package_kind, entrypoint_kind, entrypoint, contract_version) =
        normalized_service_execution_profile(service)?;
    if execution.runtime != runtime {
        return Err("service-addressed execution runtime does not match local service".to_string());
    }
    if execution.package_kind != package_kind {
        return Err(
            "service-addressed execution package_kind does not match local service".to_string(),
        );
    }
    if execution.entrypoint.kind != entrypoint_kind {
        return Err(
            "service-addressed execution entrypoint_kind does not match local service".to_string(),
        );
    }
    if execution.entrypoint.value != entrypoint {
        return Err(
            "service-addressed execution entrypoint does not match local service".to_string(),
        );
    }
    if execution.contract_version != contract_version {
        return Err(
            "service-addressed execution contract_version does not match local service".to_string(),
        );
    }
    if execution.mounts != service.mounts {
        return Err("service-addressed execution mounts do not match local service".to_string());
    }
    let Some(requested_binding_hash) = execution.binding_hash() else {
        return Err("service-addressed execution binding hash is missing".to_string());
    };
    let Some(local_binding_hash) = service_binding_hash(service) else {
        return Err("local service binding hash is missing".to_string());
    };
    if requested_binding_hash != local_binding_hash {
        return Err(
            "service-addressed execution binding hash does not match local service".to_string(),
        );
    }
    Ok(())
}

fn build_bound_workload_spec_from_service(
    service: &ProviderServiceRecord,
    input: Value,
) -> Result<WorkloadSpec, String> {
    let (runtime, package_kind, entrypoint_kind, entrypoint, contract_version) =
        normalized_service_execution_profile(service)?;
    match (&runtime, &package_kind) {
        (ExecutionRuntime::Wasm, ExecutionPackageKind::InlineModule) => {
            let module_bytes_hex = service
                .module_bytes_hex
                .clone()
                .ok_or_else(|| "service is missing module_bytes_hex binding".to_string())?;
            let module_bytes = hex::decode(&module_bytes_hex)
                .map_err(|error| format!("invalid module hex: {error}"))?;
            let workload = crate::wasm::ComputeWasmWorkload::new(&module_bytes, &input)?;
            Ok(WorkloadSpec::Wasm {
                submission: Box::new(crate::wasm::WasmSubmission {
                    schema_version: crate::wasm::FROGLET_SCHEMA_V1.to_string(),
                    submission_type: crate::wasm::WASM_SUBMISSION_TYPE_V1.to_string(),
                    workload: crate::wasm::ComputeWasmWorkload {
                        abi_version: contract_version,
                        requested_capabilities: Vec::new(),
                        ..workload
                    },
                    module_bytes_hex,
                    input,
                }),
            })
        }
        (ExecutionRuntime::Wasm, ExecutionPackageKind::OciImage) => {
            let oci_reference = service
                .oci_reference
                .clone()
                .ok_or_else(|| "service is missing oci_reference binding".to_string())?;
            let oci_digest = service
                .oci_digest
                .clone()
                .ok_or_else(|| "service is missing oci_digest binding".to_string())?;
            let input_hash = crypto::sha256_hex(
                canonical_json::to_vec(&input).map_err(|error| error.to_string())?,
            );
            Ok(WorkloadSpec::OciWasm {
                submission: Box::new(crate::wasm::OciWasmSubmission {
                    schema_version: crate::wasm::FROGLET_SCHEMA_V1.to_string(),
                    submission_type: crate::wasm::WASM_OCI_SUBMISSION_TYPE_V1.to_string(),
                    workload: crate::wasm::OciWasmWorkload {
                        schema_version: crate::wasm::FROGLET_SCHEMA_V1.to_string(),
                        workload_kind: crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_OCI_V1.to_string(),
                        abi_version: contract_version,
                        module_format: crate::wasm::WASM_MODULE_OCI_FORMAT.to_string(),
                        oci_reference,
                        oci_digest,
                        input_format: crate::wasm::JCS_JSON_FORMAT.to_string(),
                        input_hash,
                        requested_capabilities: Vec::new(),
                    },
                    input,
                }),
            })
        }
        (ExecutionRuntime::Python, ExecutionPackageKind::InlineSource) => {
            let source = service
                .inline_source
                .clone()
                .ok_or_else(|| "service is missing inline_source binding".to_string())?;
            let execution = match entrypoint_kind {
                ExecutionEntrypointKind::Script => {
                    ExecutionWorkload::python_inline_script(source, input)?
                }
                _ => ExecutionWorkload::python_inline_handler(source, entrypoint, input)?,
            };
            Ok(WorkloadSpec::Execution {
                execution: Box::new(execution),
            })
        }
        (ExecutionRuntime::Python, ExecutionPackageKind::OciImage)
        | (ExecutionRuntime::Container, ExecutionPackageKind::OciImage) => {
            let oci_reference = service
                .oci_reference
                .clone()
                .ok_or_else(|| "service is missing oci_reference binding".to_string())?;
            let oci_digest = service
                .oci_digest
                .clone()
                .ok_or_else(|| "service is missing oci_digest binding".to_string())?;
            let execution = ExecutionWorkload::container_oci(
                runtime,
                oci_reference,
                oci_digest,
                entrypoint_kind,
                entrypoint,
                input,
            )?;
            Ok(WorkloadSpec::Execution {
                execution: Box::new(execution),
            })
        }
        (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin) => {
            let builtin_name = service.offer_kind.as_str();
            if builtin_name == "events.query" {
                let Value::Object(object) = input else {
                    return Err("builtin events.query expects a JSON object input".to_string());
                };
                let kinds = match object.get("kinds") {
                    Some(Value::Array(values)) => values
                        .iter()
                        .map(|value| {
                            value.as_str().map(str::to_string).ok_or_else(|| {
                                "builtin events.query kinds must be strings".to_string()
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    Some(_) => {
                        return Err(
                            "builtin events.query kinds must be an array of strings".to_string()
                        );
                    }
                    None => Vec::new(),
                };
                let limit = match object.get("limit") {
                    Some(Value::Number(number)) => {
                        Some(number.as_u64().map(|value| value as usize).ok_or_else(|| {
                            "builtin events.query limit must be a non-negative integer".to_string()
                        })?)
                    }
                    Some(Value::Null) | None => None,
                    Some(_) => {
                        return Err(
                            "builtin events.query limit must be a non-negative integer".to_string()
                        );
                    }
                };
                let execution = ExecutionWorkload::builtin_events_query(kinds, limit)?;
                Ok(WorkloadSpec::Execution {
                    execution: Box::new(execution),
                })
            } else {
                let execution =
                    ExecutionWorkload::builtin_service(builtin_name.to_string(), input)?;
                Ok(WorkloadSpec::Execution {
                    execution: Box::new(execution),
                })
            }
        }
        _ => Err(format!(
            "unsupported service execution profile runtime={} package_kind={}",
            runtime.as_str(),
            package_kind.as_str()
        )),
    }
}

async fn resolve_service_addressed_workload_spec(
    state: &AppState,
    execution: &ExecutionWorkload,
    expected_offer_hash: Option<&str>,
) -> Result<WorkloadSpec, String> {
    let service_id = execution
        .service_id()
        .ok_or_else(|| "service-addressed execution requires security.service_id".to_string())?;
    let Some(service) = provider_service_record(state, service_id, true, true).await? else {
        return Err(format!("service not found: {service_id}"));
    };
    validate_service_addressed_execution_against_service(execution, &service)?;
    if let Some(expected_offer_hash) = expected_offer_hash {
        let Some(offer_record) =
            provider_control_offer_record(state, &service.offer_id, true).await?
        else {
            return Err(format!(
                "service-addressed execution offer is missing: {}",
                service.offer_id
            ));
        };
        if offer_record.offer.hash != expected_offer_hash {
            return Err(
                "service-addressed execution offer hash does not match the quoted offer"
                    .to_string(),
            );
        }
    }
    build_bound_workload_spec_from_service(&service, execution.input.clone())
}

fn validate_workload_spec(
    spec: &WorkloadSpec,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match spec {
        WorkloadSpec::Execution { execution } => validate_execution_workload(execution),
        WorkloadSpec::Wasm { submission } => validate_wasm_submission(submission),
        WorkloadSpec::ConfidentialService {
            confidential_session_hash,
            service_id,
            request_envelope,
        } => {
            if confidential_session_hash.trim().is_empty() {
                return Err(error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "confidential service requires confidential_session_hash" }),
                ));
            }
            if service_id.trim().is_empty() {
                return Err(error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "confidential service requires service_id" }),
                ));
            }
            request_envelope
                .validate()
                .map_err(|error| error_json(StatusCode::BAD_REQUEST, json!({ "error": error })))?;
            if request_envelope.confidential_session_hash != confidential_session_hash.as_str() {
                return Err(error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "encrypted envelope confidential_session_hash does not match the workload payload" }),
                ));
            }
            Ok(())
        }
        WorkloadSpec::AttestedWasm {
            confidential_session_hash,
            request_envelope,
        } => {
            if confidential_session_hash.trim().is_empty() {
                return Err(error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "attested wasm requires confidential_session_hash" }),
                ));
            }
            request_envelope
                .validate()
                .map_err(|error| error_json(StatusCode::BAD_REQUEST, json!({ "error": error })))?;
            if request_envelope.confidential_session_hash != confidential_session_hash.as_str() {
                return Err(error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "encrypted envelope confidential_session_hash does not match the workload payload" }),
                ));
            }
            Ok(())
        }
        WorkloadSpec::EventsQuery { kinds, limit: _ } if kinds.is_empty() => Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "events query must include at least one kind" }),
        )),
        WorkloadSpec::EventsQuery { kinds, .. } if kinds.len() > db::MAX_EVENT_QUERY_KINDS => {
            Err(error_json(
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "events query includes too many kinds",
                    "max_kinds": db::MAX_EVENT_QUERY_KINDS,
                }),
            ))
        }
        WorkloadSpec::EventsQuery {
            limit: Some(limit), ..
        } if *limit > 500 => Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "events query limit exceeds maximum", "max_limit": 500 }),
        )),
        _ => Ok(()),
    }
}

fn validate_event_query_kinds(
    kinds: &[String],
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if kinds.len() > db::MAX_EVENT_QUERY_KINDS {
        Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "events query includes too many kinds",
                "max_kinds": db::MAX_EVENT_QUERY_KINDS,
            }),
        ))
    } else {
        Ok(())
    }
}

fn transport_name_for_clearnet_uri(uri: &str) -> &'static str {
    if uri.starts_with("https://") {
        "https"
    } else {
        "http"
    }
}

fn node_event_id_preimage(event: &NodeEventEnvelope) -> Vec<u8> {
    canonical_json::to_vec(&json!([
        event.pubkey,
        event.created_at,
        event.kind,
        event.tags,
        event.content
    ]))
    .expect("node event id preimage should serialize canonically")
}

fn expected_node_event_id(event: &NodeEventEnvelope) -> String {
    crypto::sha256_hex(node_event_id_preimage(event))
}

fn normalize_idempotency_key(
    idempotency_key: Option<String>,
) -> Result<Option<String>, (StatusCode, Json<serde_json::Value>)> {
    match idempotency_key {
        Some(key) if key.trim().is_empty() => Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "idempotency_key must not be empty" }),
        )),
        Some(key) if key.len() > MAX_IDEMPOTENCY_KEY_BYTES => Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "idempotency_key is too large" }),
        )),
        other => Ok(other),
    }
}

fn normalize_hex_field(
    field_name: &str,
    value: String,
    expected_len: usize,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    normalize_hex_value(field_name, value, expected_len)
        .map_err(|error| error_json(StatusCode::BAD_REQUEST, error))
}

fn normalize_hex_value(
    field_name: &str,
    value: String,
    expected_len: usize,
) -> Result<String, serde_json::Value> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() != expected_len
        || !normalized
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(json!({
            "error": format!("{field_name} must be {expected_len} lowercase hex characters"),
            "field": field_name,
        }));
    }

    Ok(normalized)
}

fn validate_success_preimage_for_deal(
    deal_id: &str,
    deal: &deals::StoredDeal,
    success_preimage: String,
) -> Result<(String, crate::protocol::PaymentLock), ApiFailure> {
    let success_preimage = match normalize_hex_value("success_preimage", success_preimage, 64) {
        Ok(preimage) => preimage,
        Err(error) => return Err((StatusCode::BAD_REQUEST, error)),
    };

    let Some(payment_lock) = deal.payment_lock() else {
        return Err((
            StatusCode::CONFLICT,
            json!({ "error": "deal is missing its lightning payment lock", "deal_id": deal_id }),
        ));
    };
    let Ok(success_preimage_bytes) = hex::decode(&success_preimage) else {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "success_preimage must be valid lowercase hex" }),
        ));
    };
    let computed_payment_hash = crypto::sha256_hex(&success_preimage_bytes);
    if computed_payment_hash != payment_lock.token_hash {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "success_preimage does not match the deal payment lock",
                "deal_id": deal_id,
            }),
        ));
    }

    Ok((success_preimage, payment_lock))
}

async fn load_validated_lightning_bundle_for_deal(
    state: Arc<AppState>,
    deal_id: &str,
    deal: &deals::StoredDeal,
) -> Result<settlement::LightningInvoiceBundleSession, ApiFailure> {
    let synced_bundle = sync_and_maybe_promote_lightning_deal(state.clone(), deal)
        .await
        .map_err(|error| {
            tracing::error!("Failed to sync Lightning bundle for deal {deal_id}: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to sync lightning settlement state" }),
            )
        })?;
    let Some(bundle) = synced_bundle else {
        return Err((
            StatusCode::NOT_FOUND,
            json!({ "error": "lightning invoice bundle not found", "deal_id": deal_id }),
        ));
    };

    let report = settlement::validate_lightning_invoice_bundle(
        &bundle.bundle,
        &deal.quote,
        &deal.artifact,
        None,
    );
    if !report.valid {
        return Err((
            StatusCode::CONFLICT,
            json!({
                "error": "stored lightning invoice bundle failed commitment validation",
                "deal_id": deal_id,
                "validation": report,
            }),
        ));
    }

    Ok(bundle)
}

async fn deal_lightning_invoice_bundle(
    state: &AppState,
    deal: &deals::StoredDeal,
) -> Result<Option<settlement::LightningInvoiceBundleSession>, String> {
    if deal.payment_method.as_deref() != Some("lightning") {
        return Ok(None);
    }

    settlement::get_lightning_invoice_bundle_by_deal_hash(state, &deal.artifact.hash).await
}

fn runtime_payment_intent_path(deal_id: &str) -> String {
    format!("/v1/runtime/deals/{deal_id}/payment-intent")
}

async fn load_runtime_requester_deal_and_payment_intent(
    state: Arc<AppState>,
    deal_id: &str,
) -> Result<
    (
        requester_deals::RequesterDealRecord,
        Option<settlement::LightningWalletIntent>,
    ),
    ApiFailure,
> {
    let stored = sync_requester_deal_from_provider(state.clone(), deal_id).await?;

    if !quote_uses_lightning_bundle(state.as_ref(), &stored.quote) {
        return Ok((stored.public_record(), None));
    }

    let bundle: settlement::LightningInvoiceBundleSession = remote_json_request(
        state.as_ref(),
        reqwest::Method::GET,
        format!(
            "{}/v1/provider/deals/{}/invoice-bundle",
            stored.provider_url,
            urlencoding::encode(deal_id)
        ),
        Option::<&()>::None,
    )
    .await?;
    let report = settlement::validate_lightning_invoice_bundle(
        &bundle.bundle,
        &stored.quote,
        &stored.deal,
        None,
    );
    if !report.valid {
        return Err((
            StatusCode::CONFLICT,
            json!({
                "error": "provider invoice bundle failed commitment validation",
                "deal_id": deal_id,
                "validation": report,
            }),
        ));
    }

    let mut payment_intent = settlement::build_lightning_wallet_intent(
        state.as_ref(),
        &stored.deal_id,
        &stored.status,
        stored.result_hash.as_deref(),
        &bundle,
    );
    if let Some(mock_action) = payment_intent.mock_action.as_mut() {
        mock_action.endpoint_path = format!("/v1/runtime/deals/{deal_id}/mock-pay");
    }
    if let Some(release_action) = payment_intent.release_action.as_mut() {
        release_action.endpoint_path = format!("/v1/runtime/deals/{deal_id}/accept");
    }

    Ok((stored.public_record(), Some(payment_intent)))
}

async fn promote_lightning_deal_if_funded(
    state: Arc<AppState>,
    deal: &deals::StoredDeal,
    bundle: &settlement::LightningInvoiceBundleSession,
) -> Result<bool, String> {
    if deal.status != deals::DEAL_STATUS_PAYMENT_PENDING
        || !settlement::lightning_bundle_is_funded(bundle)
    {
        return Ok(false);
    }

    let reserved_execution_permit = if deal.spec.runtime() == Some("wasm") {
        match state.wasm_sandbox.try_acquire_execution_permit() {
            Ok(permit) => Some(permit),
            Err(error_message) => {
                reject_deal_before_execution(
                    &state,
                    deal,
                    deals::DEAL_STATUS_PAYMENT_PENDING,
                    error_message,
                )
                .await;
                return Ok(false);
            }
        }
    } else {
        None
    };

    let deal_id = deal.deal_id.clone();
    let promoted = state
        .db
        .with_write_conn(move |conn| {
            deals::try_mark_deal_accepted_from_payment_pending(
                conn,
                &deal_id,
                settlement::current_unix_timestamp(),
            )
        })
        .await?;

    if promoted {
        tokio::spawn(process_deal_with_reserved_permit(
            state,
            deal.deal_id.clone(),
            reserved_execution_permit,
        ));
    }

    Ok(promoted)
}

async fn sync_and_maybe_promote_lightning_deal(
    state: Arc<AppState>,
    deal: &deals::StoredDeal,
) -> Result<Option<settlement::LightningInvoiceBundleSession>, String> {
    let Some(bundle) = deal_lightning_invoice_bundle(state.as_ref(), deal).await? else {
        return Ok(None);
    };

    let _ = promote_lightning_deal_if_funded(state, deal, &bundle).await?;
    Ok(Some(bundle))
}

struct LightningSettlementFailureOutcome {
    deal_state: &'static str,
    execution_state: &'static str,
    failure: ReceiptFailure,
}

fn lightning_settlement_failure_details(
    deal: &deals::StoredDeal,
    bundle: &settlement::LightningInvoiceBundleSession,
) -> Option<LightningSettlementFailureOutcome> {
    let base_state = &bundle.base_state;
    let success_state = &bundle.success_state;

    if deal.status == deals::DEAL_STATUS_PAYMENT_PENDING {
        if matches!(base_state, InvoiceBundleLegState::Expired)
            || matches!(success_state, InvoiceBundleLegState::Expired)
        {
            return Some(LightningSettlementFailureOutcome {
                deal_state: "canceled",
                execution_state: "not_started",
                failure: receipt_failure(
                    "payment_expired",
                    "lightning payment window expired before deal admission",
                ),
            });
        }
        if matches!(base_state, InvoiceBundleLegState::Canceled)
            || matches!(success_state, InvoiceBundleLegState::Canceled)
        {
            return Some(LightningSettlementFailureOutcome {
                deal_state: "canceled",
                execution_state: "not_started",
                failure: receipt_failure(
                    "payment_canceled",
                    "lightning payment was canceled before deal admission",
                ),
            });
        }
    }

    if deal.status == deals::DEAL_STATUS_RESULT_READY {
        if matches!(success_state, InvoiceBundleLegState::Expired) {
            return Some(LightningSettlementFailureOutcome {
                deal_state: "canceled",
                execution_state: "succeeded",
                failure: receipt_failure(
                    "success_fee_expired_before_release",
                    "success-fee hold expired before requester release",
                ),
            });
        }
        if matches!(success_state, InvoiceBundleLegState::Canceled) {
            return Some(LightningSettlementFailureOutcome {
                deal_state: "canceled",
                execution_state: "succeeded",
                failure: receipt_failure(
                    "success_fee_canceled_before_release",
                    "success-fee hold was canceled before requester release",
                ),
            });
        }
    }

    None
}

async fn load_deal_record(
    state: &AppState,
    deal_id: &str,
) -> Result<Option<deals::StoredDeal>, String> {
    let deal_id = deal_id.to_string();
    state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &deal_id))
        .await
}

async fn persist_lightning_success_receipt(
    state: Arc<AppState>,
    deal: &deals::StoredDeal,
    bundle: &settlement::LightningInvoiceBundleSession,
    release_evidence: Option<serde_json::Value>,
) -> Result<bool, String> {
    let Some(result) = deal.result.clone() else {
        return Err("deal result is not available".to_string());
    };
    let Some(result_hash) = deal.result_hash.clone() else {
        return Err("deal result_hash is not available".to_string());
    };

    let completed_at = settlement::current_unix_timestamp();
    let receipt = sign_deal_receipt(
        state.as_ref(),
        deal,
        completed_at,
        ReceiptSignSpec {
            deal_state: "succeeded",
            execution_state: "succeeded",
            bundle: Some(bundle),
            result_hash: Some(result_hash),
            result_format: None,
            result_envelope_hash: None,
            failure: None,
        },
    )?;
    let receipt_json = serde_json::to_string(&receipt).map_err(|e| e.to_string())?;

    let deal_id = deal.deal_id.clone();
    let result_for_db = result.clone();
    let receipt_for_db = receipt.clone();
    state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<bool, String> {
                if let Some(release_evidence) = release_evidence.as_ref() {
                    let _ = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &deal_id,
                        "lightning_success_preimage_release",
                        release_evidence,
                        completed_at,
                    )?;
                }
                db::insert_artifact_document(
                    conn,
                    &receipt_for_db.hash,
                    &receipt_for_db.payload_hash,
                    ARTIFACT_KIND_RECEIPT,
                    &receipt_for_db.signer,
                    receipt_for_db.created_at,
                    &receipt_json,
                )?;
                let _ = db::insert_execution_evidence(
                    conn,
                    "deal",
                    &deal_id,
                    "receipt_artifact_ref",
                    &json!({ "artifact_hash": receipt_for_db.hash }),
                    completed_at,
                )?;
                deals::complete_deal_success_if_status(
                    conn,
                    deals::DealSuccessCompletion {
                        deal_id: &deal_id,
                        expected_status: deals::DEAL_STATUS_RESULT_READY,
                        result: &result_for_db,
                        explicit_result_hash: None,
                        receipt: &receipt_for_db,
                        result_evidence_hash: None,
                        receipt_artifact_hash: Some(&receipt_for_db.hash),
                        now: completed_at,
                    },
                )
            })();

            let result = match operation {
                Ok(result) => result,
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(error);
                }
            };

            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(result)
        })
        .await
}

async fn persist_deal_terminal_failure_receipt(
    state: Arc<AppState>,
    deal: &deals::StoredDeal,
    expected_status: &str,
    deal_state: &str,
    execution_state: &str,
    bundle: Option<&settlement::LightningInvoiceBundleSession>,
    failure: ReceiptFailure,
) -> Result<bool, String> {
    let completed_at = settlement::current_unix_timestamp();
    let error_message = failure.message.clone();
    let receipt = sign_deal_receipt(
        state.as_ref(),
        deal,
        completed_at,
        ReceiptSignSpec {
            deal_state,
            execution_state,
            bundle,
            result_hash: deal.result_hash.clone(),
            result_format: None,
            result_envelope_hash: None,
            failure: Some(failure.clone()),
        },
    )?;
    let receipt_json = serde_json::to_string(&receipt).map_err(|e| e.to_string())?;
    let deal_id = deal.deal_id.clone();
    let expected_status = expected_status.to_string();
    let receipt_for_db = receipt.clone();
    state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<bool, String> {
                let failure_evidence_hash = db::insert_execution_evidence(
                    conn,
                    "deal",
                    &deal_id,
                    "execution_failure",
                    &failure,
                    completed_at,
                )?;
                let _ = db::insert_execution_evidence(
                    conn,
                    "deal",
                    &deal_id,
                    "receipt_artifact_ref",
                    &json!({ "artifact_hash": receipt_for_db.hash }),
                    completed_at,
                )?;
                db::insert_artifact_document(
                    conn,
                    &receipt_for_db.hash,
                    &receipt_for_db.payload_hash,
                    ARTIFACT_KIND_RECEIPT,
                    &receipt_for_db.signer,
                    receipt_for_db.created_at,
                    &receipt_json,
                )?;
                deals::complete_deal_failure_if_status(
                    conn,
                    deals::DealTerminalTransition {
                        deal_id: &deal_id,
                        expected_status: &expected_status,
                        error: &error_message,
                        receipt: &receipt_for_db,
                        failure_evidence_hash: Some(&failure_evidence_hash),
                        receipt_artifact_hash: Some(&receipt_for_db.hash),
                        now: completed_at,
                    },
                )
            })();

            let result = match operation {
                Ok(result) => result,
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(error);
                }
            };

            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(result)
        })
        .await
}

async fn persist_lightning_terminal_failure_receipt(
    state: Arc<AppState>,
    deal: &deals::StoredDeal,
    bundle: &settlement::LightningInvoiceBundleSession,
    deal_state: &str,
    execution_state: &str,
    failure: ReceiptFailure,
) -> Result<bool, String> {
    persist_deal_terminal_failure_receipt(
        state,
        deal,
        &deal.status,
        deal_state,
        execution_state,
        Some(bundle),
        failure,
    )
    .await
}

async fn reconcile_lightning_deal(
    state: Arc<AppState>,
    deal: deals::StoredDeal,
) -> Result<(), String> {
    let Some(bundle) = sync_and_maybe_promote_lightning_deal(state.clone(), &deal).await? else {
        return Ok(());
    };

    let Some(current) = load_deal_record(state.as_ref(), &deal.deal_id).await? else {
        return Ok(());
    };

    if (current.status == deals::DEAL_STATUS_PAYMENT_PENDING
        || current.status == deals::DEAL_STATUS_RESULT_READY)
        && let Some(outcome) = lightning_settlement_failure_details(&current, &bundle)
    {
        let _ = persist_lightning_terminal_failure_receipt(
            state,
            &current,
            &bundle,
            outcome.deal_state,
            outcome.execution_state,
            outcome.failure,
        )
        .await?;
        return Ok(());
    }

    if current.status == deals::DEAL_STATUS_RESULT_READY
        && bundle.success_state == InvoiceBundleLegState::Settled
    {
        let _ = persist_lightning_success_receipt(state, &current, &bundle, None).await?;
    }

    Ok(())
}

pub async fn reconcile_lightning_settlement_once(state: Arc<AppState>) -> Result<(), String> {
    if !state
        .config
        .payment_backends
        .contains(&PaymentBackend::Lightning)
    {
        return Ok(());
    }

    let started_at = std::time::Instant::now();
    let watch_deals = state
        .db
        .with_read_conn(deals::list_lightning_watch_deals)
        .await?;
    stream::iter(watch_deals)
        .for_each_concurrent(8, |deal| {
            let state = state.clone();
            async move {
                if let Err(error) = reconcile_lightning_deal(state, deal).await {
                    tracing::error!("Failed to reconcile Lightning deal: {error}");
                }
            }
        })
        .await;
    tracing::info!(
        duration_ms = started_at.elapsed().as_millis() as u64,
        "Completed Lightning settlement reconciliation pass"
    );

    Ok(())
}

pub async fn run_lightning_settlement_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_millis(
        state.config.lightning.sync_interval_ms,
    ));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;
        if let Err(error) = reconcile_lightning_settlement_once(state.clone()).await {
            tracing::error!("Lightning settlement reconciliation failed: {error}");
        }
    }
}

async fn update_deal_lightning_bundle_state(
    state: &AppState,
    deal: &deals::StoredDeal,
    success_state: InvoiceBundleLegState,
) -> Result<Option<settlement::LightningInvoiceBundleSession>, String> {
    let Some(bundle) = deal_lightning_invoice_bundle(state, deal).await? else {
        return Ok(None);
    };

    if matches!(success_state, InvoiceBundleLegState::Canceled) {
        return settlement::cancel_and_sync_lightning_invoice_bundle(state, &bundle)
            .await
            .map(Some);
    }

    settlement::update_lightning_invoice_bundle_states(
        state,
        &bundle.session_id,
        bundle.base_state,
        success_state,
    )
    .await
}

fn collect_archive_artifact_hashes_for_deal(deal: &deals::StoredDeal) -> Vec<String> {
    let mut hashes = vec![deal.quote.hash.clone(), deal.artifact.hash.clone()];
    if let Some(receipt) = deal.receipt.as_ref() {
        hashes.push(receipt.hash.clone());
    }
    hashes.sort();
    hashes.dedup();
    hashes
}

fn collect_archive_artifact_hashes_for_requester_deal(
    deal: &requester_deals::StoredRequesterDeal,
) -> Vec<String> {
    let mut hashes = vec![deal.quote.hash.clone(), deal.deal.hash.clone()];
    if let Some(receipt) = deal.receipt.as_ref() {
        hashes.push(receipt.hash.clone());
    }
    hashes.sort();
    hashes.dedup();
    hashes
}

async fn build_runtime_archive_export(
    state: &AppState,
    subject_kind: &str,
    subject_id: &str,
) -> Result<Option<RuntimeArchiveExportResponse>, String> {
    enum ArchiveSubject {
        Deal {
            artifact_hashes: Vec<String>,
            deal_hash: String,
        },
        Job,
    }

    let subject_kind_owned = subject_kind.to_string();
    let subject_id_owned = subject_id.to_string();
    let subject = state
        .db
        .with_read_conn(move |conn| match subject_kind_owned.as_str() {
            "deal" => {
                if let Some(deal) = deals::get_deal(conn, &subject_id_owned)? {
                    return Ok(Some(ArchiveSubject::Deal {
                        artifact_hashes: collect_archive_artifact_hashes_for_deal(&deal),
                        deal_hash: deal.artifact.hash,
                    }));
                }
                let Some(deal) = requester_deals::get_requester_deal(conn, &subject_id_owned)?
                else {
                    return Ok(None);
                };
                Ok(Some(ArchiveSubject::Deal {
                    artifact_hashes: collect_archive_artifact_hashes_for_requester_deal(&deal),
                    deal_hash: deal.deal.hash,
                }))
            }
            "job" => {
                if jobs::get_job(conn, &subject_id_owned)?.is_none() {
                    return Ok(None);
                }
                Ok(Some(ArchiveSubject::Job))
            }
            _ => Err(format!(
                "unsupported archive subject kind: {subject_kind_owned}"
            )),
        })
        .await?;

    let Some(subject) = subject else {
        return Ok(None);
    };

    let mut artifact_documents = Vec::new();
    let mut artifact_feed = Vec::new();
    let mut lightning_invoice_bundles = Vec::new();

    match subject {
        ArchiveSubject::Deal {
            artifact_hashes,
            deal_hash,
        } => {
            let artifact_hashes_for_lookup = artifact_hashes.clone();
            let artifacts = state
                .db
                .with_read_conn(
                    move |conn| -> Result<
                        (
                            Vec<db::ArtifactDocumentRecord>,
                            Vec<db::ArtifactFeedEntryRecord>,
                        ),
                        String,
                    > {
                        let mut documents = Vec::new();
                        let mut feed_entries = Vec::new();
                        for artifact_hash in artifact_hashes_for_lookup {
                            if let Some(document) =
                                db::get_artifact_document_by_hash(conn, &artifact_hash)?
                            {
                                documents.push(document);
                            }
                            if let Some(feed_entry) =
                                db::get_artifact_feed_entry_by_hash(conn, &artifact_hash)?
                            {
                                feed_entries.push(feed_entry);
                            }
                        }
                        feed_entries.sort_by_key(|entry| entry.sequence);
                        let sequence_by_hash = feed_entries
                            .iter()
                            .map(|entry| (entry.artifact_hash.clone(), entry.sequence))
                            .collect::<std::collections::HashMap<_, _>>();
                        documents.sort_by_key(|document| {
                            sequence_by_hash
                                .get(&document.artifact_hash)
                                .copied()
                                .unwrap_or(i64::MAX)
                        });
                        Ok((documents, feed_entries))
                    },
                )
                .await?;
            artifact_documents = artifacts.0;
            artifact_feed = artifacts.1;

            let deal_hash_for_bundle = deal_hash.clone();
            let bundle = state
                .db
                .with_read_conn(move |conn| {
                    db::get_lightning_invoice_bundle_by_deal_hash(conn, &deal_hash_for_bundle)
                })
                .await?;
            if let Some(bundle) = bundle {
                lightning_invoice_bundles.push(bundle);
            }
        }
        ArchiveSubject::Job => {}
    }

    let subject_kind_for_evidence = subject_kind.to_string();
    let subject_id_for_evidence = subject_id.to_string();
    let execution_evidence = state
        .db
        .with_read_conn(move |conn| {
            db::list_execution_evidence_for_subject(
                conn,
                &subject_kind_for_evidence,
                &subject_id_for_evidence,
            )
        })
        .await?;

    Ok(Some(RuntimeArchiveExportResponse {
        schema_version: "froglet/v1".to_string(),
        export_type: "runtime_archive_bundle".to_string(),
        subject_kind: subject_kind.to_string(),
        subject_id: subject_id.to_string(),
        exported_at: settlement::current_unix_timestamp(),
        artifact_documents,
        artifact_feed,
        execution_evidence,
        lightning_invoice_bundles,
    }))
}

async fn finalize_payment(
    state: &AppState,
    reservation: Option<PaymentReservation>,
) -> Result<Option<PaymentReceipt>, (StatusCode, Json<serde_json::Value>)> {
    match reservation {
        Some(reservation) => settlement::commit_payment(state, reservation)
            .await
            .map(Some)
            .map_err(|error| error_json(error.status_code(), error.details())),
        None => Ok(None),
    }
}

async fn release_payment(
    state: &AppState,
    reservation: Option<PaymentReservation>,
) -> Result<(), String> {
    match reservation {
        Some(reservation) => settlement::release_payment(state, &reservation).await,
        None => Ok(()),
    }
}

fn canonical_result_hash(result: &Value) -> String {
    canonical_json::to_vec(result)
        .map(crypto::sha256_hex)
        .unwrap_or_else(|_| crypto::sha256_hex(b""))
}

fn execution_timeout(state: &AppState) -> Duration {
    Duration::from_secs(state.config.execution_timeout_secs)
}

fn is_lightning_payment_method(payment_method: Option<&str>) -> bool {
    payment_method == Some("lightning")
}

fn advertised_offer_timeout_secs(
    state: &AppState,
    service_id: ServiceId,
    price_sats: u64,
    payment_methods: &[String],
) -> u64 {
    let _ = (service_id, price_sats, payment_methods);
    state.config.execution_timeout_secs
}

fn workload_execution_timeout(
    state: &AppState,
    spec: &WorkloadSpec,
    payment_method: Option<&str>,
) -> Duration {
    match spec {
        WorkloadSpec::Execution { execution }
            if execution.runtime == ExecutionRuntime::Wasm
                && is_lightning_payment_method(payment_method) =>
        {
            execution_timeout(state)
        }
        WorkloadSpec::Wasm { .. } if is_lightning_payment_method(payment_method) => {
            execution_timeout(state)
        }
        _ => execution_timeout(state),
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn receipt_executor_for_deal(deal: &deals::StoredDeal) -> ReceiptExecutor {
    match &deal.spec {
        WorkloadSpec::Execution { execution } => ReceiptExecutor {
            runtime: execution.runtime_name().to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            execution_mode: (execution.security.mode == ExecutionSecurityMode::Tee)
                .then_some(crate::confidential::EXECUTION_MODE_TEE.to_string()),
            attestation_platform: (execution.security.mode == ExecutionSecurityMode::Tee)
                .then_some(crate::confidential::ATTESTATION_PLATFORM_NVIDIA.to_string()),
            measurement: None,
            abi_version: Some(execution.contract_version.clone()),
            module_hash: execution
                .module_hash
                .clone()
                .or_else(|| execution.oci_digest.clone())
                .or_else(|| execution.source_hash.clone()),
            capabilities_granted: deal.quote.payload.capabilities_granted.clone(),
        },
        WorkloadSpec::Wasm { submission } => ReceiptExecutor {
            runtime: "wasm".to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            execution_mode: None,
            attestation_platform: None,
            measurement: None,
            abi_version: Some(submission.workload.abi_version.clone()),
            module_hash: Some(submission.workload.module_hash.clone()),
            capabilities_granted: deal.quote.payload.capabilities_granted.clone(),
        },
        WorkloadSpec::OciWasm { submission } => ReceiptExecutor {
            runtime: "wasm".to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            execution_mode: None,
            attestation_platform: None,
            measurement: None,
            abi_version: Some(submission.workload.abi_version.clone()),
            module_hash: Some(submission.workload.oci_digest.clone()),
            capabilities_granted: deal.quote.payload.capabilities_granted.clone(),
        },
        WorkloadSpec::ConfidentialService { .. } => ReceiptExecutor {
            runtime: "confidential.service".to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            execution_mode: Some(crate::confidential::EXECUTION_MODE_TEE.to_string()),
            attestation_platform: Some(
                crate::confidential::ATTESTATION_PLATFORM_NVIDIA.to_string(),
            ),
            measurement: None,
            abi_version: None,
            module_hash: None,
            capabilities_granted: Vec::new(),
        },
        WorkloadSpec::AttestedWasm { .. } => ReceiptExecutor {
            runtime: "confidential.wasm".to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            execution_mode: Some(crate::confidential::EXECUTION_MODE_TEE.to_string()),
            attestation_platform: Some(
                crate::confidential::ATTESTATION_PLATFORM_NVIDIA.to_string(),
            ),
            measurement: None,
            abi_version: None,
            module_hash: None,
            capabilities_granted: Vec::new(),
        },
        WorkloadSpec::EventsQuery { .. } => ReceiptExecutor {
            runtime: "builtin.events_query".to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            execution_mode: None,
            attestation_platform: None,
            measurement: None,
            abi_version: None,
            module_hash: None,
            capabilities_granted: Vec::new(),
        },
    }
}

fn receipt_limits_for_spec(
    state: &AppState,
    spec: &WorkloadSpec,
    payment_method: Option<&str>,
) -> ReceiptLimitsApplied {
    let max_runtime_ms =
        duration_millis_u64(workload_execution_timeout(state, spec, payment_method));
    match spec {
        WorkloadSpec::Execution { execution } => match execution.runtime {
            ExecutionRuntime::Wasm | ExecutionRuntime::TeeWasm => ReceiptLimitsApplied {
                max_input_bytes: MAX_WASM_INPUT_BYTES,
                max_runtime_ms,
                max_memory_bytes: sandbox::WASM_MAX_MEMORY_BYTES,
                max_output_bytes: sandbox::WASM_MAX_OUTPUT_BYTES,
                fuel_limit: sandbox::WASM_FUEL_LIMIT,
            },
            ExecutionRuntime::Builtin | ExecutionRuntime::TeeService => ReceiptLimitsApplied {
                max_input_bytes: MAX_BODY_BYTES,
                max_runtime_ms,
                max_memory_bytes: 0,
                max_output_bytes: MAX_BODY_BYTES,
                fuel_limit: 0,
            },
            ExecutionRuntime::Any => ReceiptLimitsApplied {
                max_input_bytes: MAX_BODY_BYTES,
                max_runtime_ms,
                max_memory_bytes: 128 * 1024 * 1024,
                max_output_bytes: MAX_BODY_BYTES,
                fuel_limit: 0,
            },
            ExecutionRuntime::Python
            | ExecutionRuntime::Container
            | ExecutionRuntime::TeePython => ReceiptLimitsApplied {
                max_input_bytes: MAX_BODY_BYTES,
                max_runtime_ms,
                max_memory_bytes: 128 * 1024 * 1024,
                max_output_bytes: MAX_BODY_BYTES,
                fuel_limit: 0,
            },
        },
        WorkloadSpec::Wasm { .. } => ReceiptLimitsApplied {
            max_input_bytes: MAX_WASM_INPUT_BYTES,
            max_runtime_ms,
            max_memory_bytes: sandbox::WASM_MAX_MEMORY_BYTES,
            max_output_bytes: sandbox::WASM_MAX_OUTPUT_BYTES,
            fuel_limit: sandbox::WASM_FUEL_LIMIT,
        },
        WorkloadSpec::OciWasm { .. } => ReceiptLimitsApplied {
            max_input_bytes: MAX_WASM_INPUT_BYTES,
            max_runtime_ms,
            max_memory_bytes: sandbox::WASM_MAX_MEMORY_BYTES,
            max_output_bytes: sandbox::WASM_MAX_OUTPUT_BYTES,
            fuel_limit: sandbox::WASM_FUEL_LIMIT,
        },
        WorkloadSpec::ConfidentialService { .. } => ReceiptLimitsApplied {
            max_input_bytes: MAX_BODY_BYTES,
            max_runtime_ms,
            max_memory_bytes: 0,
            max_output_bytes: MAX_BODY_BYTES,
            fuel_limit: 0,
        },
        WorkloadSpec::AttestedWasm { .. } => ReceiptLimitsApplied {
            max_input_bytes: MAX_WASM_INPUT_BYTES,
            max_runtime_ms,
            max_memory_bytes: sandbox::WASM_MAX_MEMORY_BYTES,
            max_output_bytes: sandbox::WASM_MAX_OUTPUT_BYTES,
            fuel_limit: sandbox::WASM_FUEL_LIMIT,
        },
        WorkloadSpec::EventsQuery { .. } => ReceiptLimitsApplied {
            max_input_bytes: MAX_BODY_BYTES,
            max_runtime_ms,
            max_memory_bytes: 0,
            max_output_bytes: MAX_BODY_BYTES,
            fuel_limit: 0,
        },
    }
}

fn receipt_leg_state_from_invoice_state(state: &InvoiceBundleLegState) -> ReceiptLegState {
    match state {
        InvoiceBundleLegState::Open => ReceiptLegState::Open,
        InvoiceBundleLegState::Accepted => ReceiptLegState::Accepted,
        InvoiceBundleLegState::Settled => ReceiptLegState::Settled,
        InvoiceBundleLegState::Canceled => ReceiptLegState::Canceled,
        InvoiceBundleLegState::Expired => ReceiptLegState::Expired,
    }
}

fn empty_receipt_leg() -> ReceiptSettlementLeg {
    ReceiptSettlementLeg {
        amount_msat: 0,
        invoice_hash: String::new(),
        payment_hash: String::new(),
        state: ReceiptLegState::Canceled,
    }
}

fn settlement_refs_from_bundle(
    bundle: Option<&settlement::LightningInvoiceBundleSession>,
) -> ReceiptSettlement {
    match bundle {
        Some(bundle) => ReceiptSettlement {
            method: "lightning.base_fee_plus_success_fee.v1".to_string(),
            bundle_hash: Some(bundle.bundle.hash.clone()),
            destination_identity: bundle.bundle.payload.destination_identity.clone(),
            base_fee: ReceiptSettlementLeg {
                amount_msat: bundle.bundle.payload.base_fee.amount_msat,
                invoice_hash: bundle.bundle.payload.base_fee.invoice_hash.clone(),
                payment_hash: bundle.bundle.payload.base_fee.payment_hash.clone(),
                state: receipt_leg_state_from_invoice_state(&bundle.base_state),
            },
            success_fee: ReceiptSettlementLeg {
                amount_msat: bundle.bundle.payload.success_fee.amount_msat,
                invoice_hash: bundle.bundle.payload.success_fee.invoice_hash.clone(),
                payment_hash: bundle.bundle.payload.success_fee.payment_hash.clone(),
                state: receipt_leg_state_from_invoice_state(&bundle.success_state),
            },
        },
        None => ReceiptSettlement {
            method: "none".to_string(),
            bundle_hash: None,
            destination_identity: String::new(),
            base_fee: empty_receipt_leg(),
            success_fee: empty_receipt_leg(),
        },
    }
}

fn settlement_state_from_bundle(
    bundle: Option<&settlement::LightningInvoiceBundleSession>,
) -> String {
    match bundle {
        Some(bundle) => match bundle.success_state {
            InvoiceBundleLegState::Settled => "settled",
            InvoiceBundleLegState::Canceled => "canceled",
            InvoiceBundleLegState::Expired => "expired",
            InvoiceBundleLegState::Open | InvoiceBundleLegState::Accepted => "none",
        }
        .to_string(),
        None => "none".to_string(),
    }
}

fn receipt_started_at(deal: &deals::StoredDeal, execution_state: &str) -> Option<i64> {
    match execution_state {
        "not_started" => None,
        _ => Some(deal.created_at),
    }
}

fn receipt_failure(code: &str, message: impl Into<String>) -> ReceiptFailure {
    ReceiptFailure {
        code: code.to_string(),
        message: message.into(),
    }
}

#[derive(Debug, Clone)]
struct RecoveredDealResume {
    deal_id: String,
    previous_status: String,
    reset_running_status: bool,
}

#[derive(Debug, Clone)]
struct RecoveredJobResume {
    job_id: String,
    previous_status: String,
    reset_running_status: bool,
}

#[derive(Debug, Clone)]
struct RecoveredDealFailure {
    deal: deals::StoredDeal,
    bundle: Option<settlement::LightningInvoiceBundleSession>,
    error_message: String,
    failure: ReceiptFailure,
    receipt: SignedArtifact<ReceiptPayload>,
    receipt_json: String,
}

enum DealRecoveryDecision {
    Requeue(RecoveredDealResume),
    Fail(Box<RecoveredDealFailure>),
}

fn recovery_execution_state(deal: &deals::StoredDeal) -> &'static str {
    if deal.status == deals::DEAL_STATUS_RUNNING {
        "failed"
    } else {
        "not_started"
    }
}

fn build_recovered_deal_failure(
    state: &AppState,
    deal: deals::StoredDeal,
    recovered_at: i64,
    bundle: Option<settlement::LightningInvoiceBundleSession>,
    error_message: impl Into<String>,
    failure: ReceiptFailure,
) -> Result<RecoveredDealFailure, String> {
    let error_message = error_message.into();
    let receipt = sign_deal_receipt(
        state,
        &deal,
        recovered_at,
        ReceiptSignSpec {
            deal_state: "failed",
            execution_state: recovery_execution_state(&deal),
            bundle: bundle.as_ref(),
            result_hash: None,
            result_format: None,
            result_envelope_hash: None,
            failure: Some(failure.clone()),
        },
    )?;
    let receipt_json = serde_json::to_string(&receipt).map_err(|error| error.to_string())?;

    Ok(RecoveredDealFailure {
        deal,
        bundle,
        error_message,
        failure,
        receipt,
        receipt_json,
    })
}

async fn classify_deal_recovery(
    state: &Arc<AppState>,
    deal: deals::StoredDeal,
    recovered_at: i64,
) -> Result<DealRecoveryDecision, String> {
    let mut bundle = None;

    if deal.payment_method.as_deref() == Some("lightning") {
        let Some(existing_bundle) = deal_lightning_invoice_bundle(state.as_ref(), &deal).await?
        else {
            let failure = receipt_failure(
                "recovery_invariant_violation",
                "lightning deal is missing its invoice bundle during recovery",
            );
            return Ok(DealRecoveryDecision::Fail(Box::new(
                build_recovered_deal_failure(
                    state.as_ref(),
                    deal,
                    recovered_at,
                    None,
                    "lightning deal is missing its invoice bundle during recovery",
                    failure,
                )?,
            )));
        };

        let synced_bundle =
            settlement::sync_lightning_invoice_bundle_session(state.as_ref(), existing_bundle)
                .await?;

        if matches!(synced_bundle.success_state, InvoiceBundleLegState::Expired) {
            let failure = receipt_failure(
                "success_fee_expired_during_recovery",
                "lightning success hold expired before the deal could be recovered",
            );
            return Ok(DealRecoveryDecision::Fail(Box::new(
                build_recovered_deal_failure(
                    state.as_ref(),
                    deal,
                    recovered_at,
                    Some(synced_bundle),
                    "lightning success hold expired before the deal could be recovered",
                    failure,
                )?,
            )));
        }

        if matches!(synced_bundle.success_state, InvoiceBundleLegState::Canceled) {
            let failure = receipt_failure(
                "success_fee_canceled_during_recovery",
                "lightning success hold was canceled before the deal could be recovered",
            );
            return Ok(DealRecoveryDecision::Fail(Box::new(
                build_recovered_deal_failure(
                    state.as_ref(),
                    deal,
                    recovered_at,
                    Some(synced_bundle),
                    "lightning success hold was canceled before the deal could be recovered",
                    failure,
                )?,
            )));
        }

        let settled_success_can_finish_on_recovery = deal.status == deals::DEAL_STATUS_RESULT_READY
            && matches!(synced_bundle.success_state, InvoiceBundleLegState::Settled);
        let payment_pending_can_wait_for_funding = deal.status
            == deals::DEAL_STATUS_PAYMENT_PENDING
            && !settlement::lightning_bundle_is_funded(&synced_bundle);

        if !settled_success_can_finish_on_recovery
            && !payment_pending_can_wait_for_funding
            && (matches!(synced_bundle.success_state, InvoiceBundleLegState::Settled)
                || !settlement::lightning_bundle_is_funded(&synced_bundle))
        {
            // Cancel the bundle to ensure the success_fee leg reaches a
            // terminal state before we emit a receipt.  Without this,
            // non-terminal states (Open/Accepted) would leak into the
            // signed receipt's settlement_state via
            // settlement_state_from_bundle, violating the kernel rule that
            // receipts are terminal-only artifacts.
            let terminal_bundle = settlement::cancel_and_sync_lightning_invoice_bundle(
                state.as_ref(),
                &synced_bundle,
            )
            .await?;
            let failure = receipt_failure(
                "recovery_invariant_violation",
                "lightning settlement state is inconsistent with the persisted deal status",
            );
            return Ok(DealRecoveryDecision::Fail(Box::new(
                build_recovered_deal_failure(
                    state.as_ref(),
                    deal,
                    recovered_at,
                    Some(terminal_bundle),
                    "lightning settlement state is inconsistent with the persisted deal status",
                    failure,
                )?,
            )));
        }

        bundle = Some(synced_bundle);
    }

    if recovered_at > deal.artifact.payload.completion_deadline {
        if let Some(existing_bundle) = bundle.take() {
            bundle = Some(
                settlement::cancel_and_sync_lightning_invoice_bundle(
                    state.as_ref(),
                    &existing_bundle,
                )
                .await?,
            );
        }
        let failure = receipt_failure(
            "completion_deadline_elapsed_during_recovery",
            "deal completion_deadline elapsed while recovering from node restart",
        );
        return Ok(DealRecoveryDecision::Fail(Box::new(
            build_recovered_deal_failure(
                state.as_ref(),
                deal,
                recovered_at,
                bundle,
                "deal completion_deadline elapsed while recovering from node restart",
                failure,
            )?,
        )));
    }

    Ok(DealRecoveryDecision::Requeue(RecoveredDealResume {
        deal_id: deal.deal_id,
        previous_status: deal.status.clone(),
        reset_running_status: deal.status == deals::DEAL_STATUS_RUNNING,
    }))
}

async fn apply_recovery_plan(
    state: Arc<AppState>,
    recovered_jobs: Vec<RecoveredJobResume>,
    recovered_deals: Vec<RecoveredDealResume>,
    failed_deals: Vec<RecoveredDealFailure>,
    recovered_at: i64,
) -> Result<(), String> {
    let recovered_jobs_for_db = recovered_jobs.clone();
    let recovered_deals_for_db = recovered_deals.clone();
    state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<(), String> {
                for job in &recovered_jobs_for_db {
                    if job.reset_running_status
                        && !jobs::reset_running_job_to_queued(conn, &job.job_id, recovered_at)?
                    {
                        return Err(format!(
                            "job {} could not be returned to queued during recovery",
                            job.job_id
                        ));
                    }
                    let _ = db::insert_execution_evidence(
                        conn,
                        "job",
                        &job.job_id,
                        "recovery_action",
                        &json!({
                            "action": "requeued",
                            "previous_status": job.previous_status,
                        }),
                        recovered_at,
                    )?;
                }

                for deal in &recovered_deals_for_db {
                    if deal.reset_running_status
                        && !deals::reset_running_deal_to_accepted(
                            conn,
                            &deal.deal_id,
                            recovered_at,
                        )?
                    {
                        return Err(format!(
                            "deal {} could not be returned to accepted during recovery",
                            deal.deal_id
                        ));
                    }
                    let _ = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &deal.deal_id,
                        "recovery_action",
                        &json!({
                            "action": "requeued",
                            "previous_status": deal.previous_status,
                        }),
                        recovered_at,
                    )?;
                }

                for deal in &failed_deals {
                    if let Some(bundle) = deal.bundle.as_ref()
                        && !db::update_lightning_invoice_bundle_states(
                            conn,
                            &bundle.session_id,
                            bundle.base_state.clone(),
                            bundle.success_state.clone(),
                            recovered_at,
                        )?
                    {
                        return Err(format!(
                            "lightning invoice bundle {} disappeared during recovery",
                            bundle.session_id
                        ));
                    }

                    let failure_evidence_hash = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &deal.deal.deal_id,
                        "execution_failure",
                        &deal.failure,
                        recovered_at,
                    )?;
                    deals::complete_deal_failure(
                        conn,
                        &deal.deal.deal_id,
                        &deal.error_message,
                        &deal.receipt,
                        Some(&failure_evidence_hash),
                        Some(&deal.receipt.hash),
                        recovered_at,
                    )?;
                    db::insert_artifact_document(
                        conn,
                        &deal.receipt.hash,
                        &deal.receipt.payload_hash,
                        ARTIFACT_KIND_RECEIPT,
                        &deal.receipt.signer,
                        deal.receipt.created_at,
                        &deal.receipt_json,
                    )?;
                    let _ = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &deal.deal.deal_id,
                        "receipt_artifact_ref",
                        &json!({ "artifact_hash": deal.receipt.hash }),
                        recovered_at,
                    )?;
                }
                Ok(())
            })();

            if let Err(error) = operation {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error);
            }

            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(())
        })
        .await?;

    for job in recovered_jobs {
        tokio::spawn(process_job(state.clone(), job.job_id));
    }

    for deal in recovered_deals {
        tokio::spawn(process_deal_with_reserved_permit(
            state.clone(),
            deal.deal_id,
            None,
        ));
    }

    Ok(())
}

async fn delete_orphaned_deal_materialization_record(
    state: Arc<AppState>,
    orphaned_deal_id: String,
) -> Result<(), String> {
    state
        .db
        .with_write_conn(move |conn| -> Result<(), String> {
            db::delete_deal_settlement_materialization(conn, &orphaned_deal_id)?;
            Ok(())
        })
        .await
}

async fn recover_orphaned_deal_materializations_local(state: Arc<AppState>) -> Result<(), String> {
    let records = state
        .db
        .with_read_conn(db::list_deal_settlement_materializations)
        .await?;

    for record in records {
        let deal_id = record.deal_id.clone();
        let deal = state
            .db
            .with_read_conn(move |conn| deals::get_deal(conn, &deal_id))
            .await?;

        match deal {
            Some(deal) => {
                let requires_remote_cancellation = state
                    .config
                    .payment_backends
                    .contains(&PaymentBackend::Lightning)
                    && matches!(state.config.lightning.mode, LightningMode::LndRest)
                    && record.materialization_kind == "lightning_invoice_bundle"
                    && deal.payment_method.as_deref() == Some("lightning");
                if requires_remote_cancellation {
                    continue;
                }
                fail_pending_deal_materialization(
                    state.clone(),
                    &deal,
                    "settlement_materialization_interrupted_during_recovery",
                    "settlement materialization did not complete before node restart".to_string(),
                )
                .await?;
            }
            None => {
                delete_orphaned_deal_materialization_record(state.clone(), record.deal_id).await?;
            }
        }
    }

    Ok(())
}

async fn recover_orphaned_deal_materializations_remote(state: Arc<AppState>) -> Result<(), String> {
    if !state
        .config
        .payment_backends
        .contains(&PaymentBackend::Lightning)
    {
        return Ok(());
    }

    let records = state
        .db
        .with_read_conn(db::list_deal_settlement_materializations)
        .await?;

    for record in records {
        let deal_id = record.deal_id.clone();
        let deal = state
            .db
            .with_read_conn(move |conn| deals::get_deal(conn, &deal_id))
            .await?;

        match deal {
            Some(deal) => {
                if record.materialization_kind != "lightning_invoice_bundle"
                    || deal.payment_method.as_deref() != Some("lightning")
                {
                    continue;
                }

                let request: settlement::BuildLightningInvoiceBundleRequest =
                    serde_json::from_str(&record.request_json).map_err(|error| {
                        format!(
                            "invalid settlement materialization payload for deal {} during recovery: {error}",
                            deal.deal_id
                        )
                    })?;
                settlement::cancel_pending_lightning_materialization_request(
                    state.as_ref(),
                    &request,
                )
                .await?;
                fail_pending_deal_materialization(
                    state.clone(),
                    &deal,
                    "settlement_materialization_interrupted_during_recovery",
                    "settlement materialization did not complete before node restart".to_string(),
                )
                .await?;
            }
            None => {
                delete_orphaned_deal_materialization_record(state.clone(), record.deal_id).await?;
            }
        }
    }

    Ok(())
}

/// Register this provider's descriptor and offers with a marketplace.
///
/// Makes a `marketplace.register` deal with the marketplace at `marketplace_url`.
/// Uses a lightweight HTTP POST to submit the registration payload directly,
/// bypassing the full deal flow for bootstrap simplicity.
pub async fn register_with_marketplace(state: Arc<AppState>) -> Result<(), String> {
    let marketplace_url = match state.config.marketplace_url.as_deref() {
        Some(url) => url.to_string(),
        None => return Ok(()), // no marketplace configured
    };

    let descriptor = current_descriptor_artifact(state.as_ref()).await?;
    let offers = current_offer_artifacts(state.as_ref()).await?;

    let descriptor_json = serde_json::to_value(&descriptor).map_err(|e| e.to_string())?;
    let offer_jsons: Vec<serde_json::Value> = offers
        .iter()
        .map(|o| serde_json::to_value(o).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    // Determine our feed URL from transport status
    let transport_status = state.transport_status.lock().await.clone();
    let feed_url = transport_status
        .clearnet_url
        .or(transport_status.tor_onion_url);

    let registration_input = serde_json::json!({
        "descriptor": descriptor_json,
        "offers": offer_jsons,
        "feed_url": feed_url,
    });

    // Build a builtin execution workload for marketplace.register
    let execution = crate::execution::ExecutionWorkload::builtin_service(
        "marketplace.register".to_string(),
        registration_input,
    )?;

    // Call the marketplace's quote endpoint
    let quote_url = format!("{marketplace_url}/v1/provider/quotes");
    let quote_response: serde_json::Value = remote_json_request(
        state.as_ref(),
        reqwest::Method::POST,
        quote_url,
        Some(&serde_json::json!({
            "offer_id": "marketplace.register",
            "requester_id": state.identity.node_id(),
            "kind": "execution",
            "execution": execution,
        })),
    )
    .await
    .map_err(|(status, body)| format!("marketplace quote failed ({status}): {body}"))?;

    let quote_hash = quote_response
        .get("hash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "marketplace quote response missing 'hash' field".to_string())?
        .to_string();

    // Create and sign a deal referencing the quote
    let created_at = settlement::current_unix_timestamp();
    let workload_hash = quote_response
        .get("payload")
        .and_then(|p| p.get("workload_hash"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            "marketplace quote response missing 'payload.workload_hash' field".to_string()
        })?
        .to_string();
    let provider_id = quote_response
        .get("payload")
        .and_then(|p| p.get("provider_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            "marketplace quote response missing 'payload.provider_id' field".to_string()
        })?
        .to_string();
    let deal_payload = protocol::DealPayload {
        provider_id,
        requester_id: state.identity.node_id().to_string(),
        quote_hash: quote_hash.clone(),
        workload_hash,
        confidential_session_hash: None,
        extension_refs: Vec::new(),
        authority_ref: None,
        supersedes_deal_hash: None,
        client_nonce: None,
        success_payment_hash: crypto::sha256_hex(format!("mkt-reg-{created_at}")),
        admission_deadline: created_at + 60,
        completion_deadline: created_at + 90,
        acceptance_deadline: created_at + 120,
    };
    let deal = protocol::sign_artifact(
        state.identity.node_id(),
        |msg| state.identity.sign_message_hex(msg),
        protocol::ARTIFACT_TYPE_DEAL,
        created_at,
        deal_payload,
    )?;

    // Send deal to marketplace
    let deal_url = format!("{marketplace_url}/v1/provider/deals");
    let _deal_response: serde_json::Value = remote_json_request(
        state.as_ref(),
        reqwest::Method::POST,
        deal_url,
        Some(&serde_json::json!({
            "quote": quote_response,
            "deal": deal,
            "kind": "execution",
            "execution": execution,
        })),
    )
    .await
    .map_err(|(status, body)| format!("marketplace deal failed ({status}): {body}"))?;

    tracing::info!(
        marketplace = marketplace_url,
        offers = offers.len(),
        "registered with marketplace"
    );
    Ok(())
}

pub async fn recover_runtime_state_local(state: Arc<AppState>) -> Result<(), String> {
    recover_orphaned_deal_materializations_local(state.clone()).await?;

    let incomplete_deals = state
        .db
        .with_read_conn(deals::list_incomplete_deals)
        .await?;
    let incomplete_jobs = state.db.with_read_conn(jobs::list_incomplete_jobs).await?;
    let recovered_at = settlement::current_unix_timestamp();
    let mut recovered_deals = Vec::new();
    let mut failed_deals = Vec::new();

    for deal in incomplete_deals
        .into_iter()
        .filter(|deal| deal.payment_method.as_deref() != Some("lightning"))
    {
        match classify_deal_recovery(&state, deal, recovered_at).await? {
            DealRecoveryDecision::Requeue(resume) => recovered_deals.push(resume),
            DealRecoveryDecision::Fail(failure) => failed_deals.push(*failure),
        }
    }

    let recovered_jobs: Vec<RecoveredJobResume> = incomplete_jobs
        .into_iter()
        .map(|job| RecoveredJobResume {
            job_id: job.job_id,
            previous_status: job.status.clone(),
            reset_running_status: job.status == jobs::JOB_STATUS_RUNNING,
        })
        .collect();

    apply_recovery_plan(
        state,
        recovered_jobs,
        recovered_deals,
        failed_deals,
        recovered_at,
    )
    .await
}

pub async fn recover_runtime_state_remote(state: Arc<AppState>) -> Result<(), String> {
    if !state
        .config
        .payment_backends
        .contains(&PaymentBackend::Lightning)
    {
        return Ok(());
    }

    recover_orphaned_deal_materializations_remote(state.clone()).await?;

    let incomplete_deals = state
        .db
        .with_read_conn(deals::list_incomplete_deals)
        .await?;
    let recovered_at = settlement::current_unix_timestamp();
    let mut recovered_deals = Vec::new();
    let mut failed_deals = Vec::new();

    for deal in incomplete_deals
        .into_iter()
        .filter(|deal| deal.payment_method.as_deref() == Some("lightning"))
    {
        match classify_deal_recovery(&state, deal, recovered_at).await? {
            DealRecoveryDecision::Requeue(resume) => recovered_deals.push(resume),
            DealRecoveryDecision::Fail(failure) => failed_deals.push(*failure),
        }
    }

    apply_recovery_plan(
        state,
        Vec::new(),
        recovered_deals,
        failed_deals,
        recovered_at,
    )
    .await
}

/// Combined recovery function retained for test convenience. In production,
/// `recover_runtime_state_local` and `recover_runtime_state_remote` are invoked
/// separately (the remote path runs as a supervised background task). Do not
/// call this from production startup code to avoid double-running remote recovery.
#[cfg(test)]
pub async fn recover_runtime_state(state: Arc<AppState>) -> Result<(), String> {
    recover_runtime_state_local(state.clone()).await?;
    recover_runtime_state_remote(state).await
}

/// Shared slot for a child process, allowing timeout-based kill.
type ChildKillHandle = Arc<std::sync::Mutex<Option<std::process::Child>>>;

async fn run_wasm_with_timeout<F>(timeout: Duration, operation: F) -> Result<Value, String>
where
    F: FnOnce() -> Result<Value, Box<dyn std::error::Error + Send + Sync>> + Send + 'static,
{
    run_wasm_with_timeout_and_kill(timeout, None, operation).await
}

async fn run_wasm_with_timeout_and_kill<F>(
    timeout: Duration,
    kill_handle: Option<ChildKillHandle>,
    operation: F,
) -> Result<Value, String>
where
    F: FnOnce() -> Result<Value, Box<dyn std::error::Error + Send + Sync>> + Send + 'static,
{
    let handle = tokio::task::spawn_blocking(operation);
    match tokio::time::timeout(
        timeout.saturating_add(Duration::from_secs(BLOCKING_EXECUTION_TIMEOUT_GRACE_SECS)),
        handle,
    )
    .await
    {
        Ok(join_result) => {
            let result =
                join_result.map_err(|error| format!("execution thread panicked: {error}"))?;
            result.map_err(|error| error.to_string())
        }
        Err(_) => {
            if let Some(ref handle) = kill_handle
                && let Ok(mut guard) = handle.lock()
                && let Some(ref mut child) = *guard
            {
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(format!(
                "execution exceeded runtime deadline after {}s",
                timeout.as_secs()
            ))
        }
    }
}

/// Parse an OCI reference, pull the Wasm layer from the registry, and verify its digest.
/// Returns the raw Wasm module bytes on success.
async fn fetch_oci_wasm_module(
    submission: &crate::wasm::OciWasmSubmission,
) -> Result<Vec<u8>, String> {
    // Parse OCI references such as:
    // - "ghcr.io/org/module:tag"
    // - "ghcr.io/org/module@sha256:abc123"
    // - "http://127.0.0.1:5000/module:tag" for explicit local/test registries
    let oci_ref = submission.workload.oci_reference.trim();
    let (explicit_scheme, remainder) = if let Some(rest) = oci_ref.strip_prefix("https://") {
        (Some("https"), rest)
    } else if let Some(rest) = oci_ref.strip_prefix("http://") {
        (Some("http"), rest)
    } else {
        (None, oci_ref)
    };

    let parts: Vec<&str> = remainder.split('/').collect();
    if parts.len() < 2 {
        return Err("invalid oci_reference format, expected at least host/image".to_string());
    }

    let host = parts[0];
    let name_tag = parts[1..].join("/");

    // Handle both tag (:tag) and digest (@sha256:...) reference styles
    let (image, reference) = if let Some(at_pos) = name_tag.find('@') {
        (&name_tag[..at_pos], &name_tag[at_pos + 1..])
    } else {
        let colon_pos = name_tag.rfind(':');
        match colon_pos {
            Some(pos) => (&name_tag[..pos], &name_tag[pos + 1..]),
            None => (name_tag.as_str(), "latest"),
        }
    };

    // Registry URL mappings; fall back to https://{host} for OCI-compliant registries
    let (api_url, auth_url) = if let Some(scheme) = explicit_scheme {
        (
            format!("{scheme}://{host}"),
            format!("{scheme}://{host}/token"),
        )
    } else if host == "registry.hub.docker.com"
        || host == "docker.io"
        || host == "registry-1.docker.io"
    {
        (
            "https://registry-1.docker.io".to_string(),
            "https://auth.docker.io/token".to_string(),
        )
    } else if host == "ghcr.io" {
        (
            "https://ghcr.io".to_string(),
            "https://ghcr.io/token".to_string(),
        )
    } else {
        (format!("https://{host}"), format!("https://{host}/token"))
    };

    // 2. Setup Client
    use oci_registry_client::DockerRegistryClientV2;
    let mut client = DockerRegistryClientV2::new(host, &api_url, &auth_url);

    // 3. Authenticate (anonymous pull)
    match client.auth("repository", image, "pull").await {
        Ok(token) => client.set_auth_token(Some(token)),
        Err(err) => {
            tracing::warn!("OCI auth failed (might be public repo): {}", err);
        }
    }

    // 4. Fetch Manifest
    let manifest = client
        .manifest(image, reference)
        .await
        .map_err(|e| format!("failed to fetch OCI manifest: {:?}", e))?;

    // 5. Extract first WASM layer
    let wasm_layer = manifest
        .layers
        .iter()
        .find(|l| l.media_type == crate::wasm::WASM_MODULE_FORMAT || l.media_type.contains("wasm"))
        .ok_or_else(|| "no wasm layer found in OCI manifest".to_string())?;

    // 6. Download Blob (with size cap)
    let mut blob_stream = client
        .blob(image, &wasm_layer.digest)
        .await
        .map_err(|e| format!("failed to fetch OCI blob {}: {:?}", wasm_layer.digest, e))?;

    let mut module_bytes = Vec::new();
    loop {
        match blob_stream.chunk().await {
            Ok(Some(chunk)) => {
                module_bytes.extend_from_slice(&chunk);
                if module_bytes.len() > MAX_OCI_WASM_MODULE_BYTES {
                    return Err(format!(
                        "OCI module exceeds maximum size of {} bytes",
                        MAX_OCI_WASM_MODULE_BYTES
                    ));
                }
            }
            Ok(None) => break,
            Err(e) => return Err(format!("failed downloading blob chunk: {:?}", e)),
        }
    }

    // 7. Verify Workload Hash
    let computed_hash = crate::crypto::sha256_hex(&module_bytes);
    if computed_hash != submission.workload.oci_digest {
        return Err(format!(
            "OCI layer digest mismatch. expected: {}, got: {}",
            submission.workload.oci_digest, computed_hash
        ));
    }

    Ok(module_bytes)
}

async fn run_job_spec_now(state: &AppState, spec: JobSpec) -> Result<Value, String> {
    let timeout = execution_timeout(state);
    match spec {
        JobSpec::Execution { execution } => match (&execution.runtime, &execution.package_kind) {
            (ExecutionRuntime::Wasm, ExecutionPackageKind::InlineModule) => {
                let submission = execution.to_wasm_submission()?;
                let verified = submission.verify()?;
                let (capabilities_granted, host_environment) =
                    local_wasm_capabilities_for_submission(state, &verified)?;
                let wasm_sandbox = state.wasm_sandbox.clone();
                run_wasm_with_timeout(timeout, move || {
                    wasm_sandbox.execute_module_with_options(
                        &verified.module_bytes,
                        &verified.input,
                        sandbox::WasmExecutionOptions {
                            abi_version: verified.abi_version.clone(),
                            capabilities_granted,
                            host_environment,
                        },
                        timeout,
                    )
                })
                .await
            }
            (ExecutionRuntime::Wasm, ExecutionPackageKind::OciImage) => {
                let submission = execution.to_oci_wasm_submission()?;
                submission.verify()?;
                let module_bytes = fetch_oci_wasm_module(&submission).await?;
                let declared_capabilities = crate::wasm::normalize_requested_capabilities(
                    &submission.workload.requested_capabilities,
                )?;
                let (capabilities_granted, host_environment) =
                    local_wasm_capabilities_for_submission(
                        state,
                        &crate::wasm::VerifiedWasmSubmission {
                            module_bytes: module_bytes.clone(),
                            input: submission.input.clone(),
                            abi_version: submission.workload.abi_version.clone(),
                            requested_capabilities: declared_capabilities,
                        },
                    )?;
                let wasm_sandbox = state.wasm_sandbox.clone();
                let abi_version = submission.workload.abi_version.clone();
                let input = submission.input.clone();
                run_wasm_with_timeout(timeout, move || {
                    wasm_sandbox.execute_module_with_options(
                        &module_bytes,
                        &input,
                        sandbox::WasmExecutionOptions {
                            abi_version,
                            capabilities_granted,
                            host_environment,
                        },
                        timeout,
                    )
                })
                .await
            }
            (ExecutionRuntime::Python, ExecutionPackageKind::InlineSource) => {
                run_python_execution(&execution, &execution.requested_access, timeout).await
            }
            (ExecutionRuntime::Python, ExecutionPackageKind::OciImage)
            | (ExecutionRuntime::Container, ExecutionPackageKind::OciImage) => {
                run_container_execution(&execution, &execution.requested_access, timeout).await
            }
            (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin) => {
                dispatch_builtin_workload(state, &execution, None).await
            }
            _ => Err("unsupported execution runtime/package for job".to_string()),
        },
        JobSpec::Wasm { submission } => {
            let verified = submission.verify()?;
            let (capabilities_granted, host_environment) =
                local_wasm_capabilities_for_submission(state, &verified)?;
            let wasm_sandbox = state.wasm_sandbox.clone();
            run_wasm_with_timeout(timeout, move || {
                wasm_sandbox.execute_module_with_options(
                    &verified.module_bytes,
                    &verified.input,
                    sandbox::WasmExecutionOptions {
                        abi_version: verified.abi_version.clone(),
                        capabilities_granted,
                        host_environment,
                    },
                    timeout,
                )
            })
            .await
        }
        JobSpec::OciWasm { submission } => {
            submission.verify()?;
            let module_bytes = fetch_oci_wasm_module(&submission).await?;

            let declared_capabilities = crate::wasm::normalize_requested_capabilities(
                &submission.workload.requested_capabilities,
            )?;
            let (capabilities_granted, host_environment) = local_wasm_capabilities_for_submission(
                state,
                &crate::wasm::VerifiedWasmSubmission {
                    module_bytes: module_bytes.clone(),
                    input: submission.input.clone(),
                    abi_version: submission.workload.abi_version.clone(),
                    requested_capabilities: declared_capabilities,
                },
            )?;

            let wasm_sandbox = state.wasm_sandbox.clone();
            let abi_version = submission.workload.abi_version.clone();
            let input = submission.input.clone();

            run_wasm_with_timeout(timeout, move || {
                wasm_sandbox.execute_module_with_options(
                    &module_bytes,
                    &input,
                    sandbox::WasmExecutionOptions {
                        abi_version,
                        capabilities_granted,
                        host_environment,
                    },
                    timeout,
                )
            })
            .await
        }
    }
}

#[derive(Debug, Clone)]
struct WorkloadRunOutput {
    persisted_result: Value,
    result_hash: String,
    result_format: String,
    result_envelope_hash: Option<String>,
    result_evidence_kind: String,
    extra_evidence: Vec<(String, Value)>,
}

fn run_output_for_plain_result(result: Value) -> WorkloadRunOutput {
    WorkloadRunOutput {
        result_hash: canonical_result_hash(&result),
        persisted_result: result,
        result_format: wasm::JCS_JSON_FORMAT.to_string(),
        result_envelope_hash: None,
        result_evidence_kind: "execution_result".to_string(),
        extra_evidence: Vec::new(),
    }
}

fn confidential_execution_timeout(
    state: &AppState,
    profile: &ConfidentialProfilePayload,
) -> Duration {
    Duration::from_millis(
        profile
            .max_runtime_ms
            .min(duration_millis_u64(execution_timeout(state))),
    )
}

fn ensure_safe_attested_wasm_submission(
    submission: &crate::wasm::VerifiedWasmSubmission,
) -> Result<(), String> {
    if !submission.requested_capabilities.is_empty() {
        return Err(
            "attested confidential wasm currently requires empty requested_capabilities"
                .to_string(),
        );
    }
    Ok(())
}

async fn run_confidential_service_workload(
    state: &AppState,
    confidential_session_hash: &str,
    service_id: &str,
    request_envelope: &EncryptedEnvelope,
) -> Result<WorkloadRunOutput, String> {
    let loaded = load_confidential_session_by_hash(state, confidential_session_hash)
        .await?
        .ok_or_else(|| "confidential session not found".to_string())?;
    confidential::verify_attestation_bundle(
        &loaded.profile.payload,
        &loaded.session,
        &loaded.attestation,
        settlement::current_unix_timestamp(),
    )?;
    if loaded.profile.payload.service_id.as_deref() != Some(service_id) {
        return Err("confidential service_id does not match the session profile".to_string());
    }

    let key_release_provider = MockExternalKeyReleaseProvider;
    let key_release = key_release_provider.release_key(
        confidential_session_hash,
        &loaded.session.payload,
        &loaded.attestation,
        settlement::current_unix_timestamp(),
    )?;
    let input: Value = confidential::decrypt_request_envelope(
        confidential_session_hash,
        &loaded.private_material.session_private_key,
        &loaded.session.payload.requester_public_key,
        request_envelope,
    )?;
    let input_size = canonical_json::to_vec(&input)
        .map_err(|error| error.to_string())?
        .len();
    if input_size > loaded.profile.payload.max_input_bytes {
        return Err("confidential request exceeds profile max_input_bytes".to_string());
    }
    let executor = PolicyConfidentialExecutor {
        policy: state
            .confidential_policy
            .as_ref()
            .ok_or_else(|| "confidential policy is not enabled".to_string())?
            .as_ref()
            .clone(),
    };
    let timeout = confidential_execution_timeout(state, &loaded.profile.payload);
    let service_id = service_id.to_string();
    let context = ConfidentialExecutionContext {
        confidential_session_hash,
        now: settlement::current_unix_timestamp(),
    };
    let result = tokio::time::timeout(timeout, async move {
        executor.execute_service(&service_id, input, &context)
    })
    .await
    .map_err(|_| "confidential service execution timed out".to_string())??;
    let result_size = canonical_json::to_vec(&result)
        .map_err(|error| error.to_string())?
        .len();
    if result_size > loaded.profile.payload.max_output_bytes {
        return Err("confidential result exceeds profile max_output_bytes".to_string());
    }
    let result_hash = canonical_result_hash(&result);
    let result_envelope = confidential::encrypt_result_envelope(
        confidential_session_hash,
        &loaded.private_material.session_private_key,
        &loaded.session.payload.requester_public_key,
        &result,
        wasm::JCS_JSON_FORMAT,
    )?;
    let result_envelope_hash = result_envelope.envelope_hash()?;
    Ok(WorkloadRunOutput {
        persisted_result: json!(result_envelope),
        result_hash,
        result_format: wasm::JCS_JSON_FORMAT.to_string(),
        result_envelope_hash: Some(result_envelope_hash),
        result_evidence_kind: "execution_result_envelope".to_string(),
        extra_evidence: vec![
            ("attestation_bundle".to_string(), json!(loaded.attestation)),
            ("key_release_evidence".to_string(), json!(key_release)),
        ],
    })
}

async fn run_attested_wasm_workload(
    state: &AppState,
    confidential_session_hash: &str,
    request_envelope: &EncryptedEnvelope,
    permit: sandbox::ExecutionPermit,
) -> Result<WorkloadRunOutput, String> {
    let loaded = load_confidential_session_by_hash(state, confidential_session_hash)
        .await?
        .ok_or_else(|| "confidential session not found".to_string())?;
    confidential::verify_attestation_bundle(
        &loaded.profile.payload,
        &loaded.session,
        &loaded.attestation,
        settlement::current_unix_timestamp(),
    )?;
    let key_release_provider = MockExternalKeyReleaseProvider;
    let key_release = key_release_provider.release_key(
        confidential_session_hash,
        &loaded.session.payload,
        &loaded.attestation,
        settlement::current_unix_timestamp(),
    )?;
    let submission: crate::wasm::WasmSubmission = confidential::decrypt_request_envelope(
        confidential_session_hash,
        &loaded.private_material.session_private_key,
        &loaded.session.payload.requester_public_key,
        request_envelope,
    )?;
    submission.validate_limits(MAX_WASM_HEX_BYTES, MAX_WASM_INPUT_BYTES)?;
    let verified = submission.verify()?;
    ensure_safe_attested_wasm_submission(&verified)?;
    let timeout = confidential_execution_timeout(state, &loaded.profile.payload);
    let wasm_sandbox = state.wasm_sandbox.clone();
    let result = run_wasm_with_timeout(timeout, move || {
        wasm_sandbox.execute_module_with_options_and_permit(
            &verified.module_bytes,
            &verified.input,
            sandbox::WasmExecutionOptions {
                abi_version: verified.abi_version.clone(),
                capabilities_granted: Vec::new(),
                host_environment: None,
            },
            permit,
            timeout,
        )
    })
    .await?;
    let result_hash = canonical_result_hash(&result);
    let result_envelope = confidential::encrypt_result_envelope(
        confidential_session_hash,
        &loaded.private_material.session_private_key,
        &loaded.session.payload.requester_public_key,
        &result,
        wasm::JCS_JSON_FORMAT,
    )?;
    let result_envelope_hash = result_envelope.envelope_hash()?;
    Ok(WorkloadRunOutput {
        persisted_result: json!(result_envelope),
        result_hash,
        result_format: wasm::JCS_JSON_FORMAT.to_string(),
        result_envelope_hash: Some(result_envelope_hash),
        result_evidence_kind: "execution_result_envelope".to_string(),
        extra_evidence: vec![
            ("attestation_bundle".to_string(), json!(loaded.attestation)),
            ("key_release_evidence".to_string(), json!(key_release)),
        ],
    })
}

async fn run_workload_spec_with_admission(
    state: &AppState,
    spec: WorkloadSpec,
    capabilities_granted: Vec<String>,
    payment_method: Option<&str>,
    permit: Option<sandbox::ExecutionPermit>,
    expected_offer_hash: Option<&str>,
    caller_id: Option<&str>,
) -> Result<WorkloadRunOutput, String> {
    let timeout = workload_execution_timeout(state, &spec, payment_method);
    if let WorkloadSpec::Execution { execution } = &spec
        && execution.is_service_addressed()
    {
        let bound_spec =
            resolve_service_addressed_workload_spec(state, execution, expected_offer_hash).await?;
        return Box::pin(run_workload_spec_with_admission(
            state,
            bound_spec,
            capabilities_granted,
            payment_method,
            permit,
            expected_offer_hash,
            caller_id,
        ))
        .await;
    }
    match (spec, permit) {
        (WorkloadSpec::Execution { execution }, permit) => {
            match (&execution.runtime, &execution.package_kind, permit) {
                (ExecutionRuntime::Wasm, ExecutionPackageKind::InlineModule, Some(permit)) => {
                    let submission = execution.to_wasm_submission()?;
                    let verified = submission.verify()?;
                    let (_, host_environment) =
                        local_wasm_capabilities_for_submission(state, &verified)?;
                    let wasm_sandbox = state.wasm_sandbox.clone();
                    let result = run_wasm_with_timeout(timeout, move || {
                        wasm_sandbox.execute_module_with_options_and_permit(
                            &verified.module_bytes,
                            &verified.input,
                            sandbox::WasmExecutionOptions {
                                abi_version: verified.abi_version.clone(),
                                capabilities_granted,
                                host_environment,
                            },
                            permit,
                            timeout,
                        )
                    })
                    .await?;
                    Ok(run_output_for_plain_result(result))
                }
                (ExecutionRuntime::Wasm, ExecutionPackageKind::InlineModule, None) => {
                    Err("Wasm workloads require an execution permit".to_string())
                }
                (ExecutionRuntime::Wasm, ExecutionPackageKind::OciImage, permit) => {
                    let execution_clone = execution.as_ref().clone();
                    let result = run_job_spec_now(
                        state,
                        JobSpec::Execution {
                            execution: Box::new(execution_clone),
                        },
                    )
                    .await?;
                    drop(permit);
                    Ok(run_output_for_plain_result(result))
                }
                (ExecutionRuntime::Python, ExecutionPackageKind::InlineSource, _) => {
                    let result =
                        run_python_execution(execution.as_ref(), &capabilities_granted, timeout)
                            .await?;
                    Ok(run_output_for_plain_result(result))
                }
                (ExecutionRuntime::Python, ExecutionPackageKind::OciImage, _)
                | (ExecutionRuntime::Container, ExecutionPackageKind::OciImage, _) => {
                    let result =
                        run_container_execution(execution.as_ref(), &capabilities_granted, timeout)
                            .await?;
                    Ok(run_output_for_plain_result(result))
                }
                (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin, None) => {
                    let result = dispatch_builtin_workload(state, &execution, caller_id).await?;
                    Ok(run_output_for_plain_result(result))
                }
                (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin, Some(_)) => {
                    Err("builtin workloads do not use execution permits".to_string())
                }
                (ExecutionRuntime::TeeService, ExecutionPackageKind::Builtin, None) => {
                    let security = &execution.security;
                    run_confidential_service_workload(
                        state,
                        security
                            .confidential_session_hash
                            .as_deref()
                            .ok_or_else(|| "missing confidential_session_hash".to_string())?,
                        security
                            .service_id
                            .as_deref()
                            .ok_or_else(|| "missing tee service_id".to_string())?,
                        security
                            .request_envelope
                            .as_ref()
                            .ok_or_else(|| "missing request_envelope".to_string())?,
                    )
                    .await
                }
                (ExecutionRuntime::TeeWasm, ExecutionPackageKind::InlineModule, Some(permit)) => {
                    let security = &execution.security;
                    run_attested_wasm_workload(
                        state,
                        security
                            .confidential_session_hash
                            .as_deref()
                            .ok_or_else(|| "missing confidential_session_hash".to_string())?,
                        security
                            .request_envelope
                            .as_ref()
                            .ok_or_else(|| "missing request_envelope".to_string())?,
                        permit,
                    )
                    .await
                }
                (ExecutionRuntime::TeeWasm, ExecutionPackageKind::InlineModule, None) => Err(
                    "attested confidential wasm workloads require an execution permit".to_string(),
                ),
                _ => Err("unsupported execution runtime/package combination".to_string()),
            }
        }
        (WorkloadSpec::Wasm { submission }, Some(permit)) => {
            let verified = submission.verify()?;
            let (_, host_environment) = local_wasm_capabilities_for_submission(state, &verified)?;
            let wasm_sandbox = state.wasm_sandbox.clone();
            let result = run_wasm_with_timeout(timeout, move || {
                wasm_sandbox.execute_module_with_options_and_permit(
                    &verified.module_bytes,
                    &verified.input,
                    sandbox::WasmExecutionOptions {
                        abi_version: verified.abi_version.clone(),
                        capabilities_granted,
                        host_environment,
                    },
                    permit,
                    timeout,
                )
            })
            .await?;
            Ok(run_output_for_plain_result(result))
        }
        (WorkloadSpec::Wasm { .. }, None) => {
            Err("Wasm workloads require an execution permit".to_string())
        }
        (WorkloadSpec::OciWasm { submission }, None) => {
            let result = run_job_spec_now(
                state,
                JobSpec::OciWasm {
                    submission: *submission,
                },
            )
            .await?;
            Ok(run_output_for_plain_result(result))
        }
        (WorkloadSpec::OciWasm { submission }, Some(permit)) => {
            submission.verify()?;
            let module_bytes = fetch_oci_wasm_module(&submission).await?;

            let declared_capabilities = crate::wasm::normalize_requested_capabilities(
                &submission.workload.requested_capabilities,
            )?;
            let (_, host_environment) = local_wasm_capabilities_for_submission(
                state,
                &crate::wasm::VerifiedWasmSubmission {
                    module_bytes: module_bytes.clone(),
                    input: submission.input.clone(),
                    abi_version: submission.workload.abi_version.clone(),
                    requested_capabilities: declared_capabilities,
                },
            )?;

            let wasm_sandbox = state.wasm_sandbox.clone();
            let abi_version = submission.workload.abi_version.clone();
            let input = submission.input.clone();

            let result = run_wasm_with_timeout(timeout, move || {
                wasm_sandbox.execute_module_with_options_and_permit(
                    &module_bytes,
                    &input,
                    sandbox::WasmExecutionOptions {
                        abi_version,
                        capabilities_granted,
                        host_environment,
                    },
                    permit,
                    timeout,
                )
            })
            .await?;
            Ok(run_output_for_plain_result(result))
        }
        (WorkloadSpec::ConfidentialService { .. }, Some(_)) => {
            Err("confidential service workloads do not use execution permits".to_string())
        }
        (
            WorkloadSpec::ConfidentialService {
                confidential_session_hash,
                service_id,
                request_envelope,
            },
            None,
        ) => {
            run_confidential_service_workload(
                state,
                &confidential_session_hash,
                &service_id,
                request_envelope.as_ref(),
            )
            .await
        }
        (WorkloadSpec::AttestedWasm { .. }, None) => {
            Err("attested confidential wasm workloads require an execution permit".to_string())
        }
        (
            WorkloadSpec::AttestedWasm {
                confidential_session_hash,
                request_envelope,
            },
            Some(permit),
        ) => {
            run_attested_wasm_workload(
                state,
                &confidential_session_hash,
                request_envelope.as_ref(),
                permit,
            )
            .await
        }
        (WorkloadSpec::EventsQuery { kinds, limit }, None) => {
            let events = query_events_with_capacity(state, kinds, limit).await?;
            Ok(run_output_for_plain_result(json!({
                "events": events,
                "cursor": null
            })))
        }
        (WorkloadSpec::EventsQuery { .. }, Some(_)) => {
            Err("events workloads do not use execution permits".to_string())
        }
    }
}

async fn process_job(state: Arc<AppState>, job_id: String) {
    let started_job = state
        .db
        .with_write_conn(move |conn| {
            jobs::try_start_job(conn, &job_id, settlement::current_unix_timestamp())
        })
        .await;

    let job = match started_job {
        Ok(Some(job)) => job,
        Ok(None) => return,
        Err(error) => {
            tracing::error!("Failed to transition job into running state: {error}");
            return;
        }
    };

    match run_job_spec_now(state.as_ref(), job.spec.clone()).await {
        Ok(result) => {
            let job_for_commit = job.clone();
            let persisted = state
                .db
                .with_write_conn(move |conn| {
                    conn.execute_batch("BEGIN IMMEDIATE")
                        .map_err(|e| e.to_string())?;
                    let operation = (|| -> Result<(), String> {
                        let committed_at = settlement::current_unix_timestamp();
                        let result_evidence_hash = db::insert_execution_evidence(
                            conn,
                            "job",
                            &job_for_commit.job_id,
                            "execution_result",
                            &result,
                            committed_at,
                        )?;

                        jobs::complete_job_success(
                            conn,
                            &job_for_commit.job_id,
                            &result,
                            Some(&result_evidence_hash),
                            committed_at,
                        )?;
                        Ok(())
                    })();

                    if let Err(error) = operation {
                        let _ = conn.execute_batch("ROLLBACK");
                        return Err(error);
                    }

                    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                    Ok(())
                })
                .await;

            if let Err(error) = persisted {
                tracing::error!("Failed to persist successful job result: {error}");
                let job_id = job.job_id.clone();
                let _ = state
                    .db
                    .with_write_conn(move |conn| {
                        jobs::complete_job_failure(
                            conn,
                            &job_id,
                            "job completed but result could not be persisted",
                            None,
                            settlement::current_unix_timestamp(),
                        )
                    })
                    .await;
            }
        }
        Err(error_message) => {
            let job_id = job.job_id.clone();
            let persisted = state
                .db
                .with_write_conn(move |conn| {
                    conn.execute_batch("BEGIN IMMEDIATE")
                        .map_err(|e| e.to_string())?;
                    let operation = (|| -> Result<(), String> {
                        let failed_at = settlement::current_unix_timestamp();
                        let failure_evidence_hash = db::insert_execution_evidence(
                            conn,
                            "job",
                            &job_id,
                            "execution_failure",
                            &json!({ "message": error_message }),
                            failed_at,
                        )?;
                        jobs::complete_job_failure(
                            conn,
                            &job_id,
                            &error_message,
                            Some(&failure_evidence_hash),
                            failed_at,
                        )?;
                        Ok(())
                    })();

                    if let Err(error) = operation {
                        let _ = conn.execute_batch("ROLLBACK");
                        return Err(error);
                    }

                    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                    Ok(())
                })
                .await;

            if let Err(error) = persisted {
                tracing::error!("Failed to persist failed job result: {error}");
            }
        }
    }
}

fn classify_execution_failure(message: &str) -> &'static str {
    let normalized = message.to_ascii_lowercase();
    if message.contains(EVENTS_QUERY_CAPACITY_EXHAUSTED) || normalized.contains("concurrency limit")
    {
        "capacity_exhausted"
    } else if normalized.contains("timeout")
        || normalized.contains("deadline")
        || normalized.contains("interrupt")
    {
        "execution_timed_out"
    } else if normalized.contains("fuel")
        || normalized.contains("execution limit")
        || normalized.contains("limit exceeded")
    {
        "execution_limit_exceeded"
    } else {
        "execution_failed"
    }
}

struct ReceiptSignSpec<'a> {
    deal_state: &'a str,
    execution_state: &'a str,
    bundle: Option<&'a settlement::LightningInvoiceBundleSession>,
    result_hash: Option<String>,
    result_format: Option<String>,
    result_envelope_hash: Option<String>,
    failure: Option<ReceiptFailure>,
}

fn sign_deal_receipt(
    state: &AppState,
    deal: &deals::StoredDeal,
    finished_at: i64,
    spec: ReceiptSignSpec<'_>,
) -> Result<SignedArtifact<ReceiptPayload>, String> {
    let result_format = spec.result_format.or_else(|| {
        spec.result_hash
            .as_ref()
            .map(|_| wasm::JCS_JSON_FORMAT.to_string())
    });
    let settlement_refs = settlement_refs_from_bundle(spec.bundle);
    let settlement_state = settlement_state_from_bundle(spec.bundle);
    let failure_code = spec.failure.as_ref().map(|details| details.code.clone());
    let failure_message = spec.failure.as_ref().map(|details| details.message.clone());

    sign_node_artifact(
        state,
        ARTIFACT_KIND_RECEIPT,
        finished_at,
        ReceiptPayload {
            provider_id: deal.artifact.payload.provider_id.clone(),
            requester_id: deal.artifact.payload.requester_id.clone(),
            deal_hash: deal.artifact.hash.clone(),
            quote_hash: deal.quote.hash.clone(),
            extension_refs: Vec::new(),
            acceptance_ref: None,
            started_at: receipt_started_at(deal, spec.execution_state),
            finished_at,
            deal_state: spec.deal_state.to_string(),
            execution_state: spec.execution_state.to_string(),
            settlement_state,
            result_hash: spec.result_hash,
            confidential_session_hash: deal.spec.confidential_session_hash().map(str::to_string),
            result_envelope_hash: spec.result_envelope_hash,
            result_format,
            executor: receipt_executor_for_deal(deal),
            limits_applied: receipt_limits_for_spec(
                state,
                &deal.spec,
                deal.payment_method.as_deref(),
            ),
            settlement_refs,
            failure_code,
            failure_message,
            result_ref: None,
        },
    )
}

async fn reject_deal_before_execution(
    state: &Arc<AppState>,
    deal: &deals::StoredDeal,
    expected_status: &str,
    error_message: String,
) {
    let expected_status = expected_status.to_string();
    let completed_at = settlement::current_unix_timestamp();
    let failure = receipt_failure("capacity_exhausted", error_message.clone());
    let bundle = match update_deal_lightning_bundle_state(
        state.as_ref(),
        deal,
        InvoiceBundleLegState::Canceled,
    )
    .await
    {
        Ok(bundle) => bundle,
        Err(error) => {
            tracing::error!("Failed to update Lightning bundle for rejected deal: {error}");
            None
        }
    };
    let receipt = match sign_deal_receipt(
        state.as_ref(),
        deal,
        completed_at,
        ReceiptSignSpec {
            deal_state: "rejected",
            execution_state: "not_started",
            bundle: bundle.as_ref(),
            result_hash: None,
            result_format: None,
            result_envelope_hash: None,
            failure: Some(failure.clone()),
        },
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            tracing::error!("Failed to sign rejected deal receipt: {error}");
            return;
        }
    };
    let receipt_json = match serde_json::to_string(&receipt) {
        Ok(json) => json,
        Err(error) => {
            tracing::error!("Failed to encode rejected deal receipt: {error}");
            return;
        }
    };

    let deal_id = deal.deal_id.clone();
    let receipt_for_db = receipt.clone();
    let persisted = state
        .db
        .with_write_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<(), String> {
                let failure_evidence_hash = db::insert_execution_evidence(
                    conn,
                    "deal",
                    &deal_id,
                    "execution_failure",
                    &failure,
                    completed_at,
                )?;

                let rejected = deals::reject_deal_if_status(
                    conn,
                    deals::DealTerminalTransition {
                        deal_id: &deal_id,
                        expected_status: &expected_status,
                        error: &error_message,
                        receipt: &receipt_for_db,
                        failure_evidence_hash: Some(&failure_evidence_hash),
                        receipt_artifact_hash: Some(&receipt_for_db.hash),
                        now: completed_at,
                    },
                )?;

                if !rejected {
                    return Ok(());
                }

                db::insert_artifact_document(
                    conn,
                    &receipt_for_db.hash,
                    &receipt_for_db.payload_hash,
                    ARTIFACT_KIND_RECEIPT,
                    &receipt_for_db.signer,
                    receipt_for_db.created_at,
                    &receipt_json,
                )?;
                let _ = db::insert_execution_evidence(
                    conn,
                    "deal",
                    &deal_id,
                    "receipt_artifact_ref",
                    &json!({ "artifact_hash": receipt_for_db.hash }),
                    completed_at,
                )?;
                Ok(())
            })();

            if let Err(error) = operation {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error);
            }

            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(())
        })
        .await;

    if let Err(error) = persisted {
        tracing::error!("Failed to persist rejected deal receipt: {error}");
    }
}

async fn process_deal_with_reserved_permit(
    state: Arc<AppState>,
    deal_id: String,
    reserved_execution_permit: Option<sandbox::ExecutionPermit>,
) {
    let lookup_deal_id = deal_id.clone();
    let loaded_deal = state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
        .await;

    let deal = match loaded_deal {
        Ok(Some(deal)) if deal.status == deals::DEAL_STATUS_ACCEPTED => deal,
        Ok(None) => return,
        Ok(Some(_)) => return,
        Err(error) => {
            tracing::error!("Failed to load deal {deal_id} for execution: {error}");
            return;
        }
    };

    let execution_permit = match (&deal.spec, reserved_execution_permit) {
        (WorkloadSpec::Execution { execution }, maybe_permit) => {
            if execution.requires_wasm_permit() {
                match maybe_permit {
                    Some(permit) => Some(permit),
                    None => match state.wasm_sandbox.try_acquire_execution_permit() {
                        Ok(permit) => Some(permit),
                        Err(error_message) => {
                            reject_deal_before_execution(
                                &state,
                                &deal,
                                deals::DEAL_STATUS_ACCEPTED,
                                error_message,
                            )
                            .await;
                            return;
                        }
                    },
                }
            } else {
                drop(maybe_permit);
                None
            }
        }
        (WorkloadSpec::Wasm { .. }, Some(permit)) => Some(permit),
        (WorkloadSpec::Wasm { .. }, None) => {
            match state.wasm_sandbox.try_acquire_execution_permit() {
                Ok(permit) => Some(permit),
                Err(error_message) => {
                    reject_deal_before_execution(
                        &state,
                        &deal,
                        deals::DEAL_STATUS_ACCEPTED,
                        error_message,
                    )
                    .await;
                    return;
                }
            }
        }
        (WorkloadSpec::OciWasm { .. }, Some(permit)) => Some(permit),
        (WorkloadSpec::OciWasm { .. }, None) => {
            match state.wasm_sandbox.try_acquire_execution_permit() {
                Ok(permit) => Some(permit),
                Err(error_message) => {
                    reject_deal_before_execution(
                        &state,
                        &deal,
                        deals::DEAL_STATUS_ACCEPTED,
                        error_message,
                    )
                    .await;
                    return;
                }
            }
        }
        (WorkloadSpec::AttestedWasm { .. }, Some(permit)) => Some(permit),
        (WorkloadSpec::AttestedWasm { .. }, None) => {
            match state.wasm_sandbox.try_acquire_execution_permit() {
                Ok(permit) => Some(permit),
                Err(error_message) => {
                    reject_deal_before_execution(
                        &state,
                        &deal,
                        deals::DEAL_STATUS_ACCEPTED,
                        error_message,
                    )
                    .await;
                    return;
                }
            }
        }
        (WorkloadSpec::ConfidentialService { .. }, maybe_permit) => {
            drop(maybe_permit);
            None
        }
        (WorkloadSpec::EventsQuery { .. }, maybe_permit) => {
            drop(maybe_permit);
            None
        }
    };

    let start_deal_id = deal.deal_id.clone();
    let started = state
        .db
        .with_write_conn(move |conn| {
            deals::try_mark_deal_running(conn, &start_deal_id, settlement::current_unix_timestamp())
        })
        .await;

    match started {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            tracing::error!(
                "Failed to transition deal {} into running state: {error}",
                deal.deal_id
            );
            return;
        }
    }

    // Intersect the capabilities granted at quote-time with the provider's
    // current advertised capabilities, so that capabilities removed since the
    // quote was issued are no longer honoured at execution time.
    let effective_capabilities = if let Some(host_env) = state.wasm_host.as_ref() {
        let current = host_env.advertised_capabilities();
        deal.quote
            .payload
            .capabilities_granted
            .iter()
            .filter(|cap| current.iter().any(|c| c == *cap))
            .cloned()
            .collect()
    } else {
        deal.quote.payload.capabilities_granted.clone()
    };

    let deal_requester_id = deal.artifact.payload.requester_id.clone();
    match run_workload_spec_with_admission(
        state.as_ref(),
        deal.spec.clone(),
        effective_capabilities,
        deal.payment_method.as_deref(),
        execution_permit,
        Some(deal.quote.payload.offer_hash.as_str()),
        Some(&deal_requester_id),
    )
    .await
    {
        Ok(output) => {
            let completed_at = settlement::current_unix_timestamp();
            let result_for_db = output.persisted_result.clone();
            if deal.payment_method.as_deref() == Some("lightning") {
                let deal_for_stage = deal.clone();
                let output_for_stage = output.clone();
                let persisted = state
                    .db
                    .with_write_conn(move |conn| {
                        conn.execute_batch("BEGIN IMMEDIATE")
                            .map_err(|e| e.to_string())?;
                        let operation = (|| -> Result<(), String> {
                            let result_evidence_hash = db::insert_execution_evidence(
                                conn,
                                "deal",
                                &deal_for_stage.deal_id,
                                &output_for_stage.result_evidence_kind,
                                &result_for_db,
                                completed_at,
                            )?;
                            for (evidence_kind, evidence_value) in &output_for_stage.extra_evidence
                            {
                                let _ = db::insert_execution_evidence(
                                    conn,
                                    "deal",
                                    &deal_for_stage.deal_id,
                                    evidence_kind,
                                    evidence_value,
                                    completed_at,
                                )?;
                            }
                            let staged = deals::stage_deal_result_ready(
                                conn,
                                &deal_for_stage.deal_id,
                                &result_for_db,
                                Some(&output_for_stage.result_hash),
                                Some(&result_evidence_hash),
                                completed_at,
                            )?;
                            if !staged {
                                return Err("deal could not be staged as result_ready".to_string());
                            }
                            Ok(())
                        })();

                        if let Err(error) = operation {
                            let _ = conn.execute_batch("ROLLBACK");
                            return Err(error);
                        }

                        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                        Ok(())
                    })
                    .await;

                if let Err(error) = persisted {
                    tracing::error!(
                        "Failed to persist result-ready Lightning deal {}: {error}",
                        deal.deal_id
                    );
                }
            } else {
                let receipt = match sign_deal_receipt(
                    state.as_ref(),
                    &deal,
                    completed_at,
                    ReceiptSignSpec {
                        deal_state: "succeeded",
                        execution_state: "succeeded",
                        bundle: None,
                        result_hash: Some(output.result_hash.clone()),
                        result_format: Some(output.result_format.clone()),
                        result_envelope_hash: output.result_envelope_hash.clone(),
                        failure: None,
                    },
                ) {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        tracing::error!("Failed to sign successful deal receipt: {error}");
                        return;
                    }
                };

                let receipt_json = match serde_json::to_string(&receipt) {
                    Ok(json) => json,
                    Err(error) => {
                        tracing::error!("Failed to encode successful deal receipt: {error}");
                        return;
                    }
                };

                let deal_for_commit = deal.clone();
                let receipt_for_db = receipt.clone();
                let output_for_commit = output.clone();
                let persisted = state
                    .db
                    .with_write_conn(move |conn| {
                        conn.execute_batch("BEGIN IMMEDIATE")
                            .map_err(|e| e.to_string())?;
                        let operation = (|| -> Result<(), String> {
                            let result_evidence_hash = db::insert_execution_evidence(
                                conn,
                                "deal",
                                &deal_for_commit.deal_id,
                                &output_for_commit.result_evidence_kind,
                                &result_for_db,
                                completed_at,
                            )?;
                            for (evidence_kind, evidence_value) in &output_for_commit.extra_evidence
                            {
                                let _ = db::insert_execution_evidence(
                                    conn,
                                    "deal",
                                    &deal_for_commit.deal_id,
                                    evidence_kind,
                                    evidence_value,
                                    completed_at,
                                )?;
                            }
                            db::insert_artifact_document(
                                conn,
                                &receipt_for_db.hash,
                                &receipt_for_db.payload_hash,
                                ARTIFACT_KIND_RECEIPT,
                                &receipt_for_db.signer,
                                receipt_for_db.created_at,
                                &receipt_json,
                            )?;
                            let _ = db::insert_execution_evidence(
                                conn,
                                "deal",
                                &deal_for_commit.deal_id,
                                "receipt_artifact_ref",
                                &json!({ "artifact_hash": receipt_for_db.hash }),
                                completed_at,
                            )?;

                            deals::complete_deal_success(
                                conn,
                                deals::DealSuccessPersistence {
                                    deal_id: &deal_for_commit.deal_id,
                                    result: &result_for_db,
                                    explicit_result_hash: Some(&output_for_commit.result_hash),
                                    receipt: &receipt_for_db,
                                    result_evidence_hash: Some(&result_evidence_hash),
                                    receipt_artifact_hash: Some(&receipt_for_db.hash),
                                    now: completed_at,
                                },
                            )?;
                            Ok(())
                        })();

                        if let Err(error) = operation {
                            let _ = conn.execute_batch("ROLLBACK");
                            return Err(error);
                        }

                        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                        Ok(())
                    })
                    .await;

                if let Err(error) = persisted {
                    tracing::error!("Failed to persist successful deal result: {error}");
                }
            }
        }
        Err(error_message) => {
            let completed_at = settlement::current_unix_timestamp();
            let failure = receipt_failure(
                classify_execution_failure(&error_message),
                error_message.clone(),
            );
            let bundle = match update_deal_lightning_bundle_state(
                state.as_ref(),
                &deal,
                InvoiceBundleLegState::Canceled,
            )
            .await
            {
                Ok(bundle) => bundle,
                Err(error) => {
                    tracing::error!(
                        "Failed to update Lightning bundle for failed deal {}: {error}",
                        deal.deal_id
                    );
                    None
                }
            };
            let receipt = match sign_deal_receipt(
                state.as_ref(),
                &deal,
                completed_at,
                ReceiptSignSpec {
                    deal_state: "failed",
                    execution_state: "failed",
                    bundle: bundle.as_ref(),
                    result_hash: None,
                    result_format: None,
                    result_envelope_hash: None,
                    failure: Some(failure.clone()),
                },
            ) {
                Ok(receipt) => receipt,
                Err(error) => {
                    tracing::error!("Failed to sign failed deal receipt: {error}");
                    return;
                }
            };

            let receipt_json = match serde_json::to_string(&receipt) {
                Ok(json) => json,
                Err(error) => {
                    tracing::error!("Failed to encode failed deal receipt: {error}");
                    return;
                }
            };

            let deal_id = deal.deal_id.clone();
            let receipt_for_db = receipt.clone();
            let persisted = state
                .db
                .with_write_conn(move |conn| {
                    conn.execute_batch("BEGIN IMMEDIATE")
                        .map_err(|e| e.to_string())?;
                    let operation = (|| -> Result<(), String> {
                        let failure_evidence_hash = db::insert_execution_evidence(
                            conn,
                            "deal",
                            &deal_id,
                            "execution_failure",
                            &failure,
                            completed_at,
                        )?;
                        db::insert_artifact_document(
                            conn,
                            &receipt_for_db.hash,
                            &receipt_for_db.payload_hash,
                            ARTIFACT_KIND_RECEIPT,
                            &receipt_for_db.signer,
                            receipt_for_db.created_at,
                            &receipt_json,
                        )?;
                        let _ = db::insert_execution_evidence(
                            conn,
                            "deal",
                            &deal_id,
                            "receipt_artifact_ref",
                            &json!({ "artifact_hash": receipt_for_db.hash }),
                            completed_at,
                        )?;

                        deals::complete_deal_failure(
                            conn,
                            &deal_id,
                            &error_message,
                            &receipt_for_db,
                            Some(&failure_evidence_hash),
                            Some(&receipt_for_db.hash),
                            completed_at,
                        )?;
                        Ok(())
                    })();

                    if let Err(error) = operation {
                        let _ = conn.execute_batch("ROLLBACK");
                        return Err(error);
                    }

                    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                    Ok(())
                })
                .await;

            if let Err(error) = persisted {
                tracing::error!("Failed to persist failed deal result: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{
            IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
            PaymentBackend, PricingConfig, StorageConfig, WasmConfig,
        },
        crypto,
        db::DbPool,
        identity::NodeIdentity,
        pricing::PricingTable,
        sandbox::WasmSandbox,
        settlement::SettlementRegistry,
        state::TransportStatus,
        wasm::{
            ComputeWasmWorkload, FROGLET_SCHEMA_V1, JCS_JSON_FORMAT, OciWasmSubmission,
            OciWasmWorkload, WASM_MODULE_OCI_FORMAT, WASM_OCI_SUBMISSION_TYPE_V1,
            WASM_RUN_JSON_ABI_V1, WASM_SUBMISSION_TYPE_V1, WORKLOAD_KIND_COMPUTE_WASM_OCI_V1,
        },
    };
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode, header},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use tokio::{net::TcpListener, sync::Mutex};
    use tower::ServiceExt;

    static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(1);
    static TEST_ENV_LOCK: Mutex<()> = Mutex::const_new(());
    const VALID_WASM_HEX: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";
    const TEST_CONFIDENTIAL_POLICY_TOML: &str =
        include_str!("../../examples/confidential_policy.example.toml");

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "froglet-api-tests-{label}-{}-{unique}-{counter}",
            std::process::id()
        ))
    }

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<String>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            match self.previous.as_deref() {
                Some(value) => unsafe {
                    std::env::set_var(self.key, value);
                },
                None => unsafe {
                    std::env::remove_var(self.key);
                },
            }
        }
    }

    fn test_app_state_with_lightning_mode(
        payment_backend: PaymentBackend,
        lightning_mode: LightningMode,
    ) -> Arc<AppState> {
        let temp_dir = unique_temp_dir("runtime-recovery");
        let db_path = temp_dir.join("node.db");
        std::fs::create_dir_all(&temp_dir).expect("temp dir");

        let node_config = NodeConfig {
            network_mode: NetworkMode::Clearnet,
            listen_addr: "127.0.0.1:0".to_string(),
            public_base_url: None,
            runtime_listen_addr: "127.0.0.1:0".to_string(),
            runtime_allow_non_loopback: false,
            http_ca_cert_path: None,
            tor: crate::config::TorSidecarConfig {
                binary_path: "tor".to_string(),
                backend_listen_addr: "127.0.0.1:0".to_string(),
                startup_timeout_secs: 90,
            },
            identity: IdentityConfig {
                auto_generate: true,
            },
            pricing: PricingConfig {
                events_query: 10,
                execute_wasm: 30,
            },
            payment_backends: vec![payment_backend],
            execution_timeout_secs: 5,
            lightning: LightningConfig {
                mode: lightning_mode,
                destination_identity: matches!(lightning_mode, LightningMode::LndRest)
                    .then(|| format!("02{}", "99".repeat(32))),
                base_invoice_expiry_secs: 300,
                success_hold_expiry_secs: 300,
                min_final_cltv_expiry: 18,
                sync_interval_ms: 100,
                lnd_rest: None,
            },
            x402: None,
            stripe: None,
            storage: StorageConfig {
                data_dir: temp_dir.clone(),
                db_path: db_path.clone(),
                identity_dir: temp_dir.join("identity"),
                identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
                nostr_publication_seed_path: temp_dir
                    .join("identity/nostr-publication.secp256k1.seed"),
                runtime_dir: temp_dir.join("runtime"),
                runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
                consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
                provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
                tor_dir: temp_dir.join("tor"),
                host_readable_control_token: false,
            },
            wasm: WasmConfig {
                policy_path: None,
                policy: None,
            },
            confidential: crate::confidential::ConfidentialConfig {
                policy_path: None,
                policy: None,
                session_ttl_secs: 300,
            },
            marketplace_url: None,
            postgres_mounts: std::collections::BTreeMap::new(),
        };

        let db = DbPool::open(&db_path).expect("db pool");
        let events_query_capacity = db.read_connection_count().max(1);
        let identity = NodeIdentity::load_or_create(&node_config).expect("identity");
        let settlement_registry = SettlementRegistry::new(&node_config);

        Arc::new(AppState {
            db,
            transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
                &node_config,
            ))),
            wasm_sandbox: Arc::new(WasmSandbox::new(4).expect("sandbox")),
            config: node_config.clone(),
            identity: Arc::new(identity),
            pricing: PricingTable::from_config(node_config.pricing),
            http_client: crate::tls::build_reqwest_client(None).expect("http client"),
            wasm_host: None,
            confidential_policy: None,
            runtime_auth_token: "test-runtime-token".to_string(),
            runtime_auth_token_path: node_config.storage.runtime_auth_token_path.clone(),
            consumer_control_auth_token: "test-consumer-control-token".to_string(),
            consumer_control_auth_token_path: node_config
                .storage
                .consumer_control_auth_token_path
                .clone(),
            provider_control_auth_token: "test-provider-control-token".to_string(),
            provider_control_auth_token_path: node_config
                .storage
                .provider_control_auth_token_path
                .clone(),
            events_query_semaphore: Arc::new(tokio::sync::Semaphore::new(events_query_capacity)),
            lnd_rest_client: None,
            lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
            event_batch_writer: None,
            builtin_services: std::collections::HashMap::new(),
            settlement_registry,
        })
    }

    fn test_app_state(payment_backend: PaymentBackend) -> Arc<AppState> {
        test_app_state_with_lightning_mode(payment_backend, LightningMode::Mock)
    }

    fn test_app_state_with_free_pricing(payment_backend: PaymentBackend) -> Arc<AppState> {
        let mut state = test_app_state(payment_backend);
        let state_mut = Arc::get_mut(&mut state).expect("unique app state");
        state_mut.config.pricing.events_query = 0;
        state_mut.config.pricing.execute_wasm = 0;
        state_mut.pricing = PricingTable::from_config(state_mut.config.pricing);
        state
    }

    #[tokio::test]
    async fn node_capabilities_reports_all_job_runtimes() {
        let state = test_app_state(PaymentBackend::None);
        let response = public_router(state)
            .oneshot(runtime_request(
                Method::GET,
                "/v1/node/capabilities",
                None,
                None,
            ))
            .await
            .expect("node capabilities response");
        let (status, payload): (StatusCode, Value) = response_json(response).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["faas"]["jobs_api"], Value::Bool(true));
        assert_eq!(payload["faas"]["async_jobs"], Value::Bool(true));
        assert_eq!(payload["faas"]["idempotency_keys"], Value::Bool(true));
        assert_eq!(
            payload["faas"]["runtimes"],
            json!(crate::jobs::FaaSDescriptor::standard().runtimes)
        );
    }

    async fn publish_test_service(
        state: &Arc<AppState>,
        payload: ProviderControlPublishArtifactRequest,
    ) {
        let service_id = payload.service_id.clone();
        let definition = artifact_provider_offer_definition(state.as_ref(), payload)
            .expect("service definition");
        let (_status, _response) = persist_provider_offer_mutation(
            state.as_ref(),
            definition,
            StatusCode::CREATED,
            format!("published test service {service_id}"),
        )
        .await
        .expect("persist published service");
    }

    fn service_addressed_execution_from_record(
        service: &ProviderServiceRecord,
        input: Value,
    ) -> crate::execution::ExecutionWorkload {
        let (runtime, package_kind, entrypoint_kind, entrypoint, contract_version) =
            normalized_service_execution_profile(service).expect("normalized service profile");
        let input_hash =
            crypto::sha256_hex(canonical_json::to_vec(&input).expect("canonical input"));
        let requested_access = service
            .mounts
            .iter()
            .map(|mount| {
                format!(
                    "mount.{}.{}.{}",
                    mount.kind,
                    if mount.read_only { "read" } else { "write" },
                    mount.handle
                )
            })
            .collect::<Vec<_>>();
        let binding_hash = service
            .binding_hash
            .clone()
            .or_else(|| service.module_hash.clone())
            .expect("binding hash");
        let mut execution = crate::execution::ExecutionWorkload {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            workload_kind: crate::execution::WORKLOAD_KIND_EXECUTION_V1.to_string(),
            runtime,
            package_kind,
            entrypoint: crate::execution::ExecutionEntrypoint {
                kind: entrypoint_kind,
                value: entrypoint,
            },
            contract_version,
            input_format: JCS_JSON_FORMAT.to_string(),
            input_hash,
            requested_access,
            security: crate::execution::ExecutionSecurity {
                mode: crate::execution::ExecutionSecurityMode::Standard,
                confidential_session_hash: None,
                service_id: Some(service.service_id.clone()),
                request_envelope: None,
            },
            mounts: service.mounts.clone(),
            input,
            module_hash: None,
            module_bytes_hex: None,
            source_hash: None,
            inline_source: None,
            oci_reference: None,
            oci_digest: None,
            builtin_name: None,
        };
        match execution.package_kind {
            crate::execution::ExecutionPackageKind::InlineSource => {
                execution.source_hash = Some(binding_hash)
            }
            crate::execution::ExecutionPackageKind::InlineModule
            | crate::execution::ExecutionPackageKind::OciImage => {
                execution.module_hash = Some(binding_hash)
            }
            crate::execution::ExecutionPackageKind::Builtin => {}
        }
        execution
    }

    fn test_confidential_policy() -> crate::confidential::ConfidentialPolicy {
        let mut policy: crate::confidential::ConfidentialPolicy =
            toml::from_str(TEST_CONFIDENTIAL_POLICY_TOML).expect("parse confidential policy");
        if let Some(profile) = policy.profiles.get_mut("confidential_search") {
            profile.price_sats = 0;
        }
        if let Some(profile) = policy.profiles.get_mut("attested_wasm") {
            profile.price_sats = 0;
        }
        policy
    }

    fn test_app_state_with_confidential_policy(payment_backend: PaymentBackend) -> Arc<AppState> {
        let mut state = test_app_state(payment_backend);
        let policy = test_confidential_policy();
        crate::confidential::validate_policy(&policy, &state.config.storage.db_path)
            .expect("validate confidential policy");
        let state_mut = Arc::get_mut(&mut state).expect("unique app state");
        state_mut.config.confidential.policy = Some(policy.clone());
        state_mut.confidential_policy = Some(Arc::new(policy));
        state
    }

    fn test_wasm_submission() -> crate::wasm::WasmSubmission {
        test_wasm_submission_with_input(Value::Null)
    }

    fn test_wasm_submission_with_input(input: Value) -> crate::wasm::WasmSubmission {
        let module_bytes = hex::decode(VALID_WASM_HEX).expect("valid wasm hex");
        crate::wasm::WasmSubmission {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            submission_type: WASM_SUBMISSION_TYPE_V1.to_string(),
            workload: ComputeWasmWorkload::new(&module_bytes, &input).expect("workload"),
            module_bytes_hex: VALID_WASM_HEX.to_string(),
            input,
        }
    }

    async fn wait_for_job_status(
        state: &Arc<AppState>,
        job_id: &str,
        expected_status: &str,
    ) -> jobs::StoredJob {
        for _ in 0..100 {
            let lookup_job_id = job_id.to_string();
            let job = state
                .db
                .with_read_conn(move |conn| jobs::get_job(conn, &lookup_job_id))
                .await
                .expect("load job")
                .expect("job exists");
            if job.status == expected_status {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        panic!("timed out waiting for job {job_id} to reach {expected_status}");
    }

    async fn wait_for_deal_status(
        state: &Arc<AppState>,
        deal_id: &str,
        expected_status: &str,
    ) -> deals::StoredDeal {
        for _ in 0..100 {
            let lookup_deal_id = deal_id.to_string();
            let deal = state
                .db
                .with_read_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
                .await
                .expect("load deal")
                .expect("deal exists");
            if deal.status == expected_status {
                return deal;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        panic!("timed out waiting for deal {deal_id} to reach {expected_status}");
    }

    fn signed_quote(
        requester_id: String,
        created_at: i64,
        expires_at: i64,
        max_runtime_ms: u64,
        max_base_invoice_expiry_secs: u64,
        max_success_hold_expiry_secs: u64,
    ) -> SignedArtifact<QuotePayload> {
        let provider_key = crypto::generate_signing_key();
        let provider_id = crypto::public_key_hex(&provider_key);
        protocol::sign_artifact(
            &provider_id,
            |message| crypto::sign_message_hex(&provider_key, message),
            ARTIFACT_KIND_QUOTE,
            created_at,
            QuotePayload {
                provider_id: provider_id.clone(),
                requester_id,
                descriptor_hash: "aa".repeat(32),
                offer_hash: "bb".repeat(32),
                expires_at,
                workload_kind: "compute.wasm.v1".to_string(),
                workload_hash: "cc".repeat(32),
                confidential_session_hash: None,
                capabilities_granted: Vec::new(),
                extension_refs: Vec::new(),
                quote_use: None,
                settlement_terms: QuoteSettlementTerms {
                    method: "lightning.base_fee_plus_success_fee.v1".to_string(),
                    destination_identity: format!("02{}", "dd".repeat(32)),
                    base_fee_msat: 1_000,
                    success_fee_msat: 9_000,
                    max_base_invoice_expiry_secs,
                    max_success_hold_expiry_secs,
                    min_final_cltv_expiry: 18,
                },
                execution_limits: ExecutionLimits {
                    max_input_bytes: 1024,
                    max_runtime_ms,
                    max_memory_bytes: 4096,
                    max_output_bytes: 1024,
                    fuel_limit: 10_000,
                },
            },
        )
        .expect("quote")
    }

    fn lightning_payment_amount_sats(quote: &SignedArtifact<QuotePayload>) -> u64 {
        (quote.payload.settlement_terms.base_fee_msat
            + quote.payload.settlement_terms.success_fee_msat)
            / 1_000
    }

    fn test_lightning_bundle(
        state: &AppState,
        quote: &SignedArtifact<QuotePayload>,
        deal: &SignedArtifact<DealPayload>,
        requester_id: &str,
        created_at: i64,
    ) -> settlement::LightningInvoiceBundleSession {
        settlement::build_lightning_invoice_bundle(
            state,
            settlement::BuildLightningInvoiceBundleRequest {
                session_id: None,
                requester_id: requester_id.to_string(),
                quote_hash: quote.hash.clone(),
                deal_hash: deal.hash.clone(),
                admission_deadline: Some(deal.payload.admission_deadline),
                success_payment_hash: deal.payload.success_payment_hash.clone(),
                base_fee_msat: quote.payload.settlement_terms.base_fee_msat,
                success_fee_msat: quote.payload.settlement_terms.success_fee_msat,
                created_at,
            },
        )
        .expect("bundle")
    }

    fn signed_lightning_quote_for_state(
        state: &AppState,
        requester_id: String,
        created_at: i64,
        expires_at: i64,
        max_runtime_ms: u64,
        max_base_invoice_expiry_secs: u64,
        max_success_hold_expiry_secs: u64,
    ) -> SignedArtifact<QuotePayload> {
        let submission = test_wasm_submission();
        let spec = WorkloadSpec::Wasm {
            submission: Box::new(submission),
        };
        let workload_hash = spec.request_hash().expect("quote workload hash");

        sign_node_artifact(
            state,
            ARTIFACT_KIND_QUOTE,
            created_at,
            QuotePayload {
                provider_id: state.identity.node_id().to_string(),
                requester_id,
                descriptor_hash: "aa".repeat(32),
                offer_hash: "bb".repeat(32),
                expires_at,
                workload_kind: "compute.wasm.v1".to_string(),
                workload_hash,
                confidential_session_hash: None,
                capabilities_granted: Vec::new(),
                extension_refs: Vec::new(),
                quote_use: None,
                settlement_terms: QuoteSettlementTerms {
                    method: "lightning.base_fee_plus_success_fee.v1".to_string(),
                    destination_identity: state.identity.compressed_public_key_hex().to_string(),
                    base_fee_msat: 1_000,
                    success_fee_msat: 9_000,
                    max_base_invoice_expiry_secs,
                    max_success_hold_expiry_secs,
                    min_final_cltv_expiry: 18,
                },
                execution_limits: ExecutionLimits {
                    max_input_bytes: 1024,
                    max_runtime_ms,
                    max_memory_bytes: 4096,
                    max_output_bytes: 1024,
                    fuel_limit: 10_000,
                },
            },
        )
        .expect("quote")
    }

    struct SeededMockLightningDeal {
        deal_id: String,
        success_preimage: String,
    }

    struct TestHttpServer {
        base_url: String,
        join_handle: tokio::task::JoinHandle<()>,
    }

    impl Drop for TestHttpServer {
        fn drop(&mut self) {
            self.join_handle.abort();
        }
    }

    async fn spawn_http_test_server(app: Router) -> TestHttpServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server addr");
        let base_url = format!("http://{addr}");
        let join_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        TestHttpServer {
            base_url,
            join_handle,
        }
    }

    async fn spawn_public_test_server(state: Arc<AppState>) -> TestHttpServer {
        spawn_http_test_server(public_router(state)).await
    }

    #[derive(Clone)]
    struct OciRegistryState {
        module_bytes: Arc<Vec<u8>>,
        layer_digest: String,
        expected_image: String,
        expected_reference: String,
    }

    struct OciRegistryFixture {
        _server: TestHttpServer,
        oci_reference: String,
        oci_digest: String,
        module_bytes: Vec<u8>,
    }

    async fn oci_registry_token() -> impl IntoResponse {
        (
            StatusCode::OK,
            Json(json!({
                "token": "test-token",
                "access_token": "test-token"
            })),
        )
    }

    async fn oci_registry_manifest(
        State(state): State<Arc<OciRegistryState>>,
        Path((image, reference)): Path<(String, String)>,
    ) -> impl IntoResponse {
        assert_eq!(image, state.expected_image);
        assert_eq!(reference, state.expected_reference);
        (
            StatusCode::OK,
            Json(json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "config": {
                    "mediaType": "application/vnd.oci.image.config.v1+json",
                    "size": 2,
                    "digest": format!("sha256:{}", "00".repeat(32)),
                },
                "layers": [{
                    "mediaType": wasm::WASM_MODULE_FORMAT,
                    "size": state.module_bytes.len(),
                    "digest": state.layer_digest,
                }]
            })),
        )
    }

    async fn oci_registry_blob(
        State(state): State<Arc<OciRegistryState>>,
        Path((image, digest)): Path<(String, String)>,
    ) -> impl IntoResponse {
        assert_eq!(image, state.expected_image);
        assert_eq!(digest, state.layer_digest);
        (
            [(header::CONTENT_TYPE, wasm::WASM_MODULE_FORMAT)],
            (*state.module_bytes).clone(),
        )
    }

    async fn spawn_oci_registry_fixture(module_bytes: Vec<u8>) -> OciRegistryFixture {
        let expected_image = "module".to_string();
        let expected_reference = "latest".to_string();
        let layer_digest = format!("sha256:{}", crypto::sha256_hex(&module_bytes));
        let state = Arc::new(OciRegistryState {
            module_bytes: Arc::new(module_bytes.clone()),
            layer_digest: layer_digest.clone(),
            expected_image: expected_image.clone(),
            expected_reference: expected_reference.clone(),
        });
        let app = Router::new()
            .route("/token", get(oci_registry_token))
            .route(
                "/v2/:image/manifests/:reference",
                get(oci_registry_manifest),
            )
            .route("/v2/:image/blobs/:digest", get(oci_registry_blob))
            .with_state(state);
        let server = spawn_http_test_server(app).await;
        OciRegistryFixture {
            oci_reference: format!("{}/{expected_image}:{expected_reference}", server.base_url),
            oci_digest: crypto::sha256_hex(&module_bytes),
            module_bytes,
            _server: server,
        }
    }

    fn test_oci_wasm_submission(oci_reference: &str, oci_digest: &str) -> OciWasmSubmission {
        let input = Value::Null;
        let input_hash =
            crypto::sha256_hex(canonical_json::to_vec(&input).expect("canonical input"));
        OciWasmSubmission {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            submission_type: WASM_OCI_SUBMISSION_TYPE_V1.to_string(),
            workload: OciWasmWorkload {
                schema_version: FROGLET_SCHEMA_V1.to_string(),
                workload_kind: WORKLOAD_KIND_COMPUTE_WASM_OCI_V1.to_string(),
                abi_version: WASM_RUN_JSON_ABI_V1.to_string(),
                module_format: WASM_MODULE_OCI_FORMAT.to_string(),
                oci_reference: oci_reference.to_string(),
                oci_digest: oci_digest.to_string(),
                input_format: JCS_JSON_FORMAT.to_string(),
                input_hash,
                requested_capabilities: Vec::new(),
            },
            input,
        }
    }

    fn runtime_request(
        method: Method,
        uri: &str,
        runtime_auth_token: Option<&str>,
        body: Option<Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = runtime_auth_token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = if let Some(payload) = body {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&payload).expect("serialize request body"))
        } else {
            Body::empty()
        };
        builder.body(body).expect("build runtime request")
    }

    async fn response_json<T: serde::de::DeserializeOwned>(
        response: axum::response::Response,
    ) -> (StatusCode, T) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let payload = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "failed to decode JSON response {status}: {error}; body={}",
                String::from_utf8_lossy(&bytes)
            )
        });
        (status, payload)
    }

    async fn seed_mock_lightning_runtime_deal(
        state: &Arc<AppState>,
        provider_url: &str,
    ) -> SeededMockLightningDeal {
        let now = settlement::current_unix_timestamp();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_lightning_quote_for_state(
            state.as_ref(),
            requester_id.clone(),
            now - 5,
            now + 180,
            30_000,
            60,
            60,
        );
        let success_preimage = "66".repeat(32);
        let success_payment_hash =
            crypto::sha256_hex(hex::decode(&success_preimage).expect("success preimage bytes"));
        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_key,
            &success_payment_hash,
            now - 5,
            true,
        )
        .expect("deal");
        let bundle = test_lightning_bundle(state.as_ref(), &quote, &deal, &requester_id, now - 5);
        let deal_id = protocol::new_artifact_id();
        let deal_id_for_provider = deal_id.clone();
        let deal_id_for_requester = deal_id.clone();
        let provider_id = state.identity.node_id().to_string();
        let provider_url = provider_url.to_string();
        let spec = WorkloadSpec::Wasm {
            submission: Box::new(test_wasm_submission()),
        };

        state
            .db
            .with_write_conn({
                let quote = quote.clone();
                let deal = deal.clone();
                let bundle = bundle.clone();
                let provider_id = provider_id.clone();
                let provider_url = provider_url.clone();
                let spec = spec.clone();
                move |conn| -> Result<(), String> {
                    deals::insert_or_get_deal(
                        conn,
                        NewDeal {
                            deal_id: deal_id_for_provider.clone(),
                            idempotency_key: Some(format!("provider-{}", deal_id_for_provider)),
                            quote: quote.clone(),
                            spec: spec.clone(),
                            artifact: deal.clone(),
                            workload_evidence_hash: None,
                            deal_artifact_hash: deal.hash.clone(),
                            payment_method: Some("lightning".to_string()),
                            payment_token_hash: Some(deal.payload.success_payment_hash.clone()),
                            payment_amount_sats: Some(lightning_payment_amount_sats(&quote)),
                            initial_status: deals::DEAL_STATUS_PAYMENT_PENDING.to_string(),
                            created_at: now - 5,
                        },
                    )?;
                    requester_deals::insert_or_get_requester_deal(
                        conn,
                        NewRequesterDeal {
                            deal_id: deal_id_for_requester.clone(),
                            idempotency_key: Some(format!("requester-{}", deal_id_for_requester)),
                            provider_id,
                            provider_url,
                            spec,
                            quote,
                            deal: deal.clone(),
                            status: deals::DEAL_STATUS_PAYMENT_PENDING.to_string(),
                            success_preimage,
                            created_at: now - 5,
                        },
                    )?;
                    db::insert_lightning_invoice_bundle(
                        conn,
                        &bundle.session_id,
                        &bundle.bundle,
                        InvoiceBundleLegState::Open,
                        InvoiceBundleLegState::Open,
                        now - 5,
                    )?;
                    Ok(())
                }
            })
            .await
            .expect("seed mock lightning runtime deal");

        SeededMockLightningDeal {
            deal_id,
            success_preimage: "66".repeat(32),
        }
    }

    #[tokio::test]
    async fn oci_wasm_fetch_supports_explicit_http_registry_refs() {
        let module_bytes = hex::decode(VALID_WASM_HEX).expect("valid wasm bytes");
        let fixture = spawn_oci_registry_fixture(module_bytes.clone()).await;
        let submission = test_oci_wasm_submission(&fixture.oci_reference, &fixture.oci_digest);

        let fetched = fetch_oci_wasm_module(&submission)
            .await
            .expect("fetch OCI wasm module");

        assert_eq!(fetched, fixture.module_bytes);
    }

    #[tokio::test]
    async fn runtime_accessible_provider_url_rewrites_local_loopback_to_configured_base_url() {
        let _env_lock = TEST_ENV_LOCK.lock().await;
        let _env = ScopedEnvVar::set("FROGLET_RUNTIME_PROVIDER_BASE_URL", "http://provider:8080");
        let state = test_app_state(PaymentBackend::None);
        let local_node_id = state.identity.node_id().to_string();

        let provider_url = runtime_accessible_provider_url(
            state.as_ref(),
            "http://127.0.0.1:8080",
            Some(&local_node_id),
        )
        .await
        .expect("runtime-accessible provider url");

        assert_eq!(provider_url, "http://provider:8080");
    }

    #[tokio::test]
    async fn runtime_accessible_provider_url_accepts_configured_local_provider_base_url() {
        let _env_lock = TEST_ENV_LOCK.lock().await;
        let _env = ScopedEnvVar::set("FROGLET_RUNTIME_PROVIDER_BASE_URL", "http://provider:8080");
        let state = test_app_state(PaymentBackend::None);
        let local_node_id = state.identity.node_id().to_string();

        let provider_url = runtime_accessible_provider_url(
            state.as_ref(),
            "http://provider:8080",
            Some(&local_node_id),
        )
        .await
        .expect("runtime-accessible provider url");

        assert_eq!(provider_url, "http://provider:8080");
    }

    #[tokio::test]
    async fn runtime_accessible_provider_url_rejects_https_loopback_for_remote_provider() {
        let state = test_app_state(PaymentBackend::None);
        let error = runtime_accessible_provider_url(
            state.as_ref(),
            "https://127.0.0.1:8443",
            Some(&"11".repeat(32)),
        )
        .await
        .expect_err("remote https loopback should be rejected");

        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(
            error.1["error"]
                .as_str()
                .is_some_and(|value| value.contains("local or private-network")),
            "unexpected payload: {}",
            error.1
        );
    }

    #[tokio::test]
    async fn runtime_accessible_provider_url_rejects_loopback_without_provider_id() {
        let _env_lock = TEST_ENV_LOCK.lock().await;
        let _env = ScopedEnvVar::unset("FROGLET_RUNTIME_PROVIDER_BASE_URL");
        let state = test_app_state(PaymentBackend::None);
        let error = runtime_accessible_provider_url(state.as_ref(), "http://127.0.0.1:8080", None)
            .await
            .expect_err("missing provider_id loopback should be rejected");

        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(
            error.1["error"]
                .as_str()
                .is_some_and(|value| value.contains("local or private-network")),
            "unexpected payload: {}",
            error.1
        );
    }

    #[tokio::test]
    async fn runtime_accessible_provider_url_rejects_loopback_for_wrong_provider_id() {
        let _env_lock = TEST_ENV_LOCK.lock().await;
        let _env = ScopedEnvVar::unset("FROGLET_RUNTIME_PROVIDER_BASE_URL");
        let state = test_app_state(PaymentBackend::None);
        let error = runtime_accessible_provider_url(
            state.as_ref(),
            "http://127.0.0.1:8080",
            Some(&"11".repeat(32)),
        )
        .await
        .expect_err("wrong provider_id loopback should be rejected");

        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(
            error.1["error"]
                .as_str()
                .is_some_and(|value| value.contains("local or private-network")),
            "unexpected payload: {}",
            error.1
        );
    }

    #[tokio::test]
    async fn runtime_accessible_provider_url_requires_configured_base_url_for_local_provider() {
        let _env_lock = TEST_ENV_LOCK.lock().await;
        let _env = ScopedEnvVar::unset("FROGLET_RUNTIME_PROVIDER_BASE_URL");
        let state = test_app_state(PaymentBackend::None);
        let local_node_id = state.identity.node_id().to_string();
        let error = runtime_accessible_provider_url(
            state.as_ref(),
            "http://127.0.0.1:8080",
            Some(&local_node_id),
        )
        .await
        .expect_err("local provider loopback should require configured base url");

        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(
            error.1["error"]
                .as_str()
                .is_some_and(|value| value.contains("local or private-network")),
            "unexpected payload: {}",
            error.1
        );
    }

    #[tokio::test]
    async fn public_feed_excludes_withdrawn_hidden_offers() {
        let state = test_app_state(PaymentBackend::None);
        let service_id = "feed-visibility";
        publish_test_service(
            &state,
            ProviderControlPublishArtifactRequest {
                service_id: service_id.to_string(),
                offer_id: None,
                artifact_path: None,
                wasm_module_hex: None,
                oci_reference: None,
                oci_digest: None,
                runtime: Some("python".to_string()),
                package_kind: Some("inline_source".to_string()),
                entrypoint_kind: Some("handler".to_string()),
                entrypoint: Some("handler".to_string()),
                contract_version: Some(CONTRACT_PYTHON_HANDLER_JSON_V1.to_string()),
                mounts: None,
                inline_source: Some(
                    "def handler(event, context):\n    return {\"message\": \"visible\"}\n"
                        .to_string(),
                ),
                summary: Some("visible then hidden".to_string()),
                mode: Some("sync".to_string()),
                price_sats: 0,
                publication_state: Some("active".to_string()),
                input_schema: None,
                output_schema: None,
            },
        )
        .await;
        publish_test_service(
            &state,
            ProviderControlPublishArtifactRequest {
                service_id: service_id.to_string(),
                offer_id: None,
                artifact_path: None,
                wasm_module_hex: None,
                oci_reference: None,
                oci_digest: None,
                runtime: Some("python".to_string()),
                package_kind: Some("inline_source".to_string()),
                entrypoint_kind: Some("handler".to_string()),
                entrypoint: Some("handler".to_string()),
                contract_version: Some(CONTRACT_PYTHON_HANDLER_JSON_V1.to_string()),
                mounts: None,
                inline_source: Some(
                    "def handler(event, context):\n    return {\"message\": \"hidden\"}\n"
                        .to_string(),
                ),
                summary: Some("visible then hidden".to_string()),
                mode: Some("sync".to_string()),
                price_sats: 0,
                publication_state: Some("hidden".to_string()),
                input_schema: None,
                output_schema: None,
            },
        )
        .await;

        let response = public_router(state)
            .oneshot(runtime_request(
                Method::GET,
                "/v1/feed?limit=100",
                None,
                None,
            ))
            .await
            .expect("feed response");
        let (status, payload): (StatusCode, Value) = response_json(response).await;

        assert_eq!(status, StatusCode::OK);
        let offers = payload["artifacts"]
            .as_array()
            .expect("feed artifacts")
            .iter()
            .filter(|artifact| artifact["kind"] == Value::String(ARTIFACT_KIND_OFFER.to_string()))
            .filter(|artifact| {
                artifact["document"]["payload"]["offer_id"].as_str() == Some(service_id)
            })
            .collect::<Vec<_>>();
        assert!(
            offers.is_empty(),
            "hidden service offer leaked into public feed: {offers:?}"
        );
    }

    #[tokio::test]
    async fn public_feed_advances_cursor_when_only_filtered_artifacts_remain() {
        let state = test_app_state(PaymentBackend::None);
        let initial = public_router(state.clone())
            .oneshot(runtime_request(
                Method::GET,
                "/v1/feed?limit=100",
                None,
                None,
            ))
            .await
            .expect("initial feed response");
        let (status, initial_payload): (StatusCode, Value) = response_json(initial).await;
        assert_eq!(status, StatusCode::OK);
        let initial_cursor = initial_payload["next_cursor"]
            .as_i64()
            .expect("initial next_cursor");

        publish_test_service(
            &state,
            ProviderControlPublishArtifactRequest {
                service_id: "feed-hidden-only".to_string(),
                offer_id: None,
                artifact_path: None,
                wasm_module_hex: None,
                oci_reference: None,
                oci_digest: None,
                runtime: Some("python".to_string()),
                package_kind: Some("inline_source".to_string()),
                entrypoint_kind: Some("handler".to_string()),
                entrypoint: Some("handler".to_string()),
                contract_version: Some(CONTRACT_PYTHON_HANDLER_JSON_V1.to_string()),
                mounts: None,
                inline_source: Some(
                    "def handler(event, context):\n    return {\"hidden\": True}\n".to_string(),
                ),
                summary: Some("hidden only".to_string()),
                mode: Some("sync".to_string()),
                price_sats: 0,
                publication_state: Some("hidden".to_string()),
                input_schema: None,
                output_schema: None,
            },
        )
        .await;

        let response = public_router(state)
            .oneshot(runtime_request(
                Method::GET,
                &format!("/v1/feed?cursor={initial_cursor}&limit=10"),
                None,
                None,
            ))
            .await
            .expect("filtered feed response");
        let (status, payload): (StatusCode, Value) = response_json(response).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["artifacts"], Value::Array(Vec::new()));
        assert_eq!(payload["has_more"], Value::Bool(false));
        let advanced_cursor = payload["next_cursor"]
            .as_i64()
            .expect("advanced next_cursor");
        assert!(
            advanced_cursor > initial_cursor,
            "expected next_cursor to advance past filtered artifacts: {payload}"
        );
    }

    #[tokio::test]
    async fn public_provider_service_detail_hides_hidden_services_and_binding_fields() {
        let state = test_app_state(PaymentBackend::None);
        publish_test_service(
            &state,
            ProviderControlPublishArtifactRequest {
                service_id: "public-python".to_string(),
                offer_id: None,
                artifact_path: None,
                wasm_module_hex: None,
                oci_reference: None,
                oci_digest: None,
                runtime: Some("python".to_string()),
                package_kind: Some("inline_source".to_string()),
                entrypoint_kind: Some("handler".to_string()),
                entrypoint: Some("handler".to_string()),
                contract_version: Some(CONTRACT_PYTHON_HANDLER_JSON_V1.to_string()),
                mounts: None,
                inline_source: Some(
                    "def handler(event, context):\n    return {\"message\": \"pong\"}\n"
                        .to_string(),
                ),
                summary: Some("public python service".to_string()),
                mode: Some("sync".to_string()),
                price_sats: 0,
                publication_state: Some("active".to_string()),
                input_schema: None,
                output_schema: None,
            },
        )
        .await;
        publish_test_service(
            &state,
            ProviderControlPublishArtifactRequest {
                service_id: "hidden-python".to_string(),
                offer_id: None,
                artifact_path: None,
                wasm_module_hex: None,
                oci_reference: None,
                oci_digest: None,
                runtime: Some("python".to_string()),
                package_kind: Some("inline_source".to_string()),
                entrypoint_kind: Some("handler".to_string()),
                entrypoint: Some("handler".to_string()),
                contract_version: Some(CONTRACT_PYTHON_HANDLER_JSON_V1.to_string()),
                mounts: None,
                inline_source: Some(
                    "def handler(event, context):\n    return {\"hidden\": true}\n".to_string(),
                ),
                summary: Some("hidden python service".to_string()),
                mode: Some("sync".to_string()),
                price_sats: 0,
                publication_state: Some("hidden".to_string()),
                input_schema: None,
                output_schema: None,
            },
        )
        .await;

        let public_response = public_router(state.clone())
            .oneshot(runtime_request(
                Method::GET,
                "/v1/provider/services/public-python",
                None,
                None,
            ))
            .await
            .expect("public service response");
        let (public_status, public_payload): (StatusCode, Value) =
            response_json(public_response).await;
        assert_eq!(public_status, StatusCode::OK);
        assert_eq!(public_payload["service"]["service_id"], "public-python");
        assert_eq!(public_payload["service"]["publication_state"], "active");
        assert_eq!(public_payload["service"]["inline_source"], Value::Null);
        assert_eq!(public_payload["service"]["module_bytes_hex"], Value::Null);
        assert_eq!(public_payload["service"]["oci_reference"], Value::Null);
        assert_eq!(public_payload["service"]["oci_digest"], Value::Null);
        assert!(public_payload["service"]["binding_hash"].is_string());

        let hidden_response = public_router(state)
            .oneshot(runtime_request(
                Method::GET,
                "/v1/provider/services/hidden-python",
                None,
                None,
            ))
            .await
            .expect("hidden service response");
        let (hidden_status, hidden_payload): (StatusCode, Value) =
            response_json(hidden_response).await;
        assert_eq!(hidden_status, StatusCode::NOT_FOUND);
        assert_eq!(hidden_payload["error"], "service not found");
    }

    #[tokio::test]
    #[ignore = "requires landlock+seccomp syscalls unavailable on default CI runners; run via FROGLET_RUN_LINUX_SANDBOX_TESTS=1 scripts/strict_checks.sh"]
    async fn service_addressed_python_execution_runs_from_redacted_service_record() {
        // Python runtime needs the Linux landlock+seccomp sandbox. On macOS /
        // other dev hosts the sandbox refuses to run without this explicit
        // opt-out; set it so `cargo test` passes locally without weakening
        // the production default.
        // SAFETY: test-only env mutation; Rust 2024 edition marks set_var unsafe.
        unsafe {
            std::env::set_var("FROGLET_ALLOW_UNSANDBOXED_PYTHON", "1");
        }
        let state = test_app_state(PaymentBackend::None);
        publish_test_service(
            &state,
            ProviderControlPublishArtifactRequest {
                service_id: "public-python".to_string(),
                offer_id: None,
                artifact_path: None,
                wasm_module_hex: None,
                oci_reference: None,
                oci_digest: None,
                runtime: Some("python".to_string()),
                package_kind: Some("inline_source".to_string()),
                entrypoint_kind: Some("handler".to_string()),
                entrypoint: Some("handler".to_string()),
                contract_version: Some(CONTRACT_PYTHON_HANDLER_JSON_V1.to_string()),
                mounts: None,
                inline_source: Some(
                    "def handler(event, context):\n    return {\"message\": \"pong\", \"input\": event}\n"
                        .to_string(),
                ),
                summary: Some("public python service".to_string()),
                mode: Some("sync".to_string()),
                price_sats: 0,
                publication_state: Some("active".to_string()),
                input_schema: None,
                output_schema: None,
            },
        )
        .await;

        let service = provider_service_record(state.as_ref(), "public-python", false, false)
            .await
            .expect("provider service record")
            .expect("published service");
        assert!(service.inline_source.is_none());
        assert!(service.binding_hash.is_some());
        let offer = provider_control_offer_record(state.as_ref(), &service.offer_id, true)
            .await
            .expect("provider offer record")
            .expect("published offer");
        let execution =
            service_addressed_execution_from_record(&service, json!({ "origin": "test" }));

        let output = run_workload_spec_with_admission(
            state.as_ref(),
            WorkloadSpec::Execution {
                execution: Box::new(execution),
            },
            Vec::new(),
            None,
            Some(
                state
                    .wasm_sandbox
                    .try_acquire_execution_permit()
                    .expect("wasm execution permit"),
            ),
            Some(offer.offer.hash.as_str()),
            None,
        )
        .await
        .expect("service-addressed python execution");

        assert_eq!(
            output.persisted_result,
            json!({ "message": "pong", "input": { "origin": "test" } })
        );
    }

    #[tokio::test]
    async fn service_addressed_wasm_execution_runs_from_redacted_service_record() {
        let state = test_app_state(PaymentBackend::None);
        publish_test_service(
            &state,
            ProviderControlPublishArtifactRequest {
                service_id: "public-wasm".to_string(),
                offer_id: None,
                artifact_path: None,
                wasm_module_hex: Some(VALID_WASM_HEX.to_string()),
                oci_reference: None,
                oci_digest: None,
                runtime: Some("wasm".to_string()),
                package_kind: Some("inline_module".to_string()),
                entrypoint_kind: None,
                entrypoint: None,
                contract_version: Some(WASM_RUN_JSON_ABI_V1.to_string()),
                mounts: None,
                inline_source: None,
                summary: Some("public wasm service".to_string()),
                mode: Some("sync".to_string()),
                price_sats: 0,
                publication_state: Some("active".to_string()),
                input_schema: None,
                output_schema: None,
            },
        )
        .await;

        let service = provider_service_record(state.as_ref(), "public-wasm", false, false)
            .await
            .expect("provider service record")
            .expect("published service");
        assert!(service.module_bytes_hex.is_none());
        assert!(service.binding_hash.is_some());
        let offer = provider_control_offer_record(state.as_ref(), &service.offer_id, true)
            .await
            .expect("provider offer record")
            .expect("published offer");
        let execution = service_addressed_execution_from_record(&service, Value::Null);

        let output = run_workload_spec_with_admission(
            state.as_ref(),
            WorkloadSpec::Execution {
                execution: Box::new(execution),
            },
            Vec::new(),
            None,
            Some(
                state
                    .wasm_sandbox
                    .try_acquire_execution_permit()
                    .expect("wasm execution permit"),
            ),
            Some(offer.offer.hash.as_str()),
            None,
        )
        .await
        .expect("service-addressed wasm execution");

        assert_eq!(output.persisted_result, json!(42));
    }

    #[tokio::test]
    async fn service_addressed_oci_wasm_execution_runs_from_redacted_service_record() {
        let state = test_app_state(PaymentBackend::None);
        let module_bytes = hex::decode(VALID_WASM_HEX).expect("valid wasm bytes");
        let fixture = spawn_oci_registry_fixture(module_bytes).await;
        publish_test_service(
            &state,
            ProviderControlPublishArtifactRequest {
                service_id: "public-oci-wasm".to_string(),
                offer_id: None,
                artifact_path: None,
                wasm_module_hex: None,
                oci_reference: Some(fixture.oci_reference.clone()),
                oci_digest: Some(fixture.oci_digest.clone()),
                runtime: Some("wasm".to_string()),
                package_kind: Some("oci_image".to_string()),
                entrypoint_kind: None,
                entrypoint: None,
                contract_version: Some(WASM_RUN_JSON_ABI_V1.to_string()),
                mounts: None,
                inline_source: None,
                summary: Some("public OCI wasm service".to_string()),
                mode: Some("sync".to_string()),
                price_sats: 0,
                publication_state: Some("active".to_string()),
                input_schema: None,
                output_schema: None,
            },
        )
        .await;

        let service = provider_service_record(state.as_ref(), "public-oci-wasm", false, false)
            .await
            .expect("provider service record")
            .expect("published service");
        assert!(service.oci_reference.is_none());
        assert!(service.oci_digest.is_none());
        assert_eq!(
            service.binding_hash.as_deref(),
            Some(fixture.oci_digest.as_str())
        );
        let offer = provider_control_offer_record(state.as_ref(), &service.offer_id, true)
            .await
            .expect("provider offer record")
            .expect("published offer");
        let execution = service_addressed_execution_from_record(&service, Value::Null);

        let output = run_workload_spec_with_admission(
            state.as_ref(),
            WorkloadSpec::Execution {
                execution: Box::new(execution),
            },
            Vec::new(),
            None,
            None,
            Some(offer.offer.hash.as_str()),
            None,
        )
        .await
        .expect("service-addressed OCI wasm execution");

        assert_eq!(output.persisted_result, json!(42));
    }

    #[tokio::test]
    async fn service_addressed_execution_rejects_metadata_and_offer_mismatches() {
        let state = test_app_state(PaymentBackend::None);
        publish_test_service(
            &state,
            ProviderControlPublishArtifactRequest {
                service_id: "validated-python".to_string(),
                offer_id: None,
                artifact_path: None,
                wasm_module_hex: None,
                oci_reference: None,
                oci_digest: None,
                runtime: Some("python".to_string()),
                package_kind: Some("inline_source".to_string()),
                entrypoint_kind: Some("handler".to_string()),
                entrypoint: Some("handler".to_string()),
                contract_version: Some(CONTRACT_PYTHON_HANDLER_JSON_V1.to_string()),
                mounts: Some(vec![crate::execution::ExecutionMount {
                    handle: "fixtures".to_string(),
                    kind: "fs".to_string(),
                    read_only: true,
                    binding: Some("/tmp".to_string()),
                }]),
                inline_source: Some("def handler(event, context):\n    return event\n".to_string()),
                summary: Some("validated python service".to_string()),
                mode: Some("sync".to_string()),
                price_sats: 0,
                publication_state: Some("active".to_string()),
                input_schema: None,
                output_schema: None,
            },
        )
        .await;

        let service = provider_service_record(state.as_ref(), "validated-python", false, false)
            .await
            .expect("provider service record")
            .expect("published service");
        let offer = provider_control_offer_record(state.as_ref(), &service.offer_id, true)
            .await
            .expect("provider offer record")
            .expect("published offer");
        let execution = service_addressed_execution_from_record(&service, Value::Null);

        let mut wrong_runtime = execution.clone();
        wrong_runtime.runtime = crate::execution::ExecutionRuntime::Wasm;
        let runtime_error = resolve_service_addressed_workload_spec(
            state.as_ref(),
            &wrong_runtime,
            Some(offer.offer.hash.as_str()),
        )
        .await
        .expect_err("runtime mismatch should fail");
        assert!(runtime_error.contains("runtime does not match"));

        let mut wrong_entrypoint = execution.clone();
        wrong_entrypoint.entrypoint.value = "different".to_string();
        let entrypoint_error = resolve_service_addressed_workload_spec(
            state.as_ref(),
            &wrong_entrypoint,
            Some(offer.offer.hash.as_str()),
        )
        .await
        .expect_err("entrypoint mismatch should fail");
        assert!(entrypoint_error.contains("entrypoint does not match"));

        let mut wrong_contract = execution.clone();
        wrong_contract.contract_version = WASM_RUN_JSON_ABI_V1.to_string();
        let contract_error = resolve_service_addressed_workload_spec(
            state.as_ref(),
            &wrong_contract,
            Some(offer.offer.hash.as_str()),
        )
        .await
        .expect_err("contract mismatch should fail");
        assert!(contract_error.contains("contract_version does not match"));

        let mut wrong_mounts = execution.clone();
        wrong_mounts.mounts.clear();
        wrong_mounts.requested_access.clear();
        let mounts_error = resolve_service_addressed_workload_spec(
            state.as_ref(),
            &wrong_mounts,
            Some(offer.offer.hash.as_str()),
        )
        .await
        .expect_err("mount mismatch should fail");
        assert!(mounts_error.contains("mounts do not match"));

        let mut wrong_binding = execution.clone();
        wrong_binding.source_hash = Some("00".repeat(32));
        let binding_error = resolve_service_addressed_workload_spec(
            state.as_ref(),
            &wrong_binding,
            Some(offer.offer.hash.as_str()),
        )
        .await
        .expect_err("binding mismatch should fail");
        assert!(binding_error.contains("binding hash does not match"));

        let offer_error = resolve_service_addressed_workload_spec(
            state.as_ref(),
            &execution,
            Some(&"11".repeat(32)),
        )
        .await
        .expect_err("quoted offer mismatch should fail");
        assert!(offer_error.contains("offer hash does not match"));
    }

    #[tokio::test]
    async fn oci_wasm_fetch_rejects_digest_mismatch() {
        let module_bytes = hex::decode(VALID_WASM_HEX).expect("valid wasm bytes");
        let fixture = spawn_oci_registry_fixture(module_bytes).await;
        let submission =
            test_oci_wasm_submission(&fixture.oci_reference, &format!("{}1", "00".repeat(31)));

        let error = fetch_oci_wasm_module(&submission)
            .await
            .expect_err("digest mismatch should fail");

        assert!(
            error.contains("OCI layer digest mismatch"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn oci_wasm_execution_matches_inline_wasm_execution() {
        let state = test_app_state(PaymentBackend::None);
        let inline_submission = test_wasm_submission();
        let module_bytes = hex::decode(VALID_WASM_HEX).expect("valid wasm bytes");
        let fixture = spawn_oci_registry_fixture(module_bytes).await;
        let oci_submission = test_oci_wasm_submission(&fixture.oci_reference, &fixture.oci_digest);

        let inline_result = run_job_spec_now(
            state.as_ref(),
            JobSpec::Wasm {
                submission: inline_submission,
            },
        )
        .await
        .expect("inline wasm execution");
        let oci_result = run_job_spec_now(
            state.as_ref(),
            JobSpec::OciWasm {
                submission: oci_submission,
            },
        )
        .await
        .expect("OCI wasm execution");

        assert_eq!(oci_result, inline_result);
    }

    #[tokio::test]
    async fn create_job_reuses_existing_job_for_canonical_wasm_submission() {
        let state = test_app_state_with_free_pricing(PaymentBackend::None);
        let public = router(state.clone());

        let first_submission = test_wasm_submission_with_input(json!({ "b": 2, "a": 1 }));
        let mut second_submission = test_wasm_submission_with_input(json!({ "a": 1, "b": 2 }));
        second_submission.module_bytes_hex = VALID_WASM_HEX.to_ascii_uppercase();

        let first_request_hash = JobSpec::Wasm {
            submission: first_submission.clone(),
        }
        .request_hash()
        .expect("first request hash");
        let second_request_hash = JobSpec::Wasm {
            submission: second_submission.clone(),
        }
        .request_hash()
        .expect("second request hash");
        assert_eq!(
            first_request_hash, second_request_hash,
            "canonical workload hashing should ignore transport-level hex casing and input order"
        );

        let first_response = public
            .clone()
            .oneshot(runtime_request(
                Method::POST,
                "/v1/node/jobs",
                None,
                Some(json!({
                    "kind": "wasm",
                    "submission": first_submission,
                    "idempotency_key": "rust-canonical-wasm-idempotency",
                })),
            ))
            .await
            .expect("first jobs response");
        let (first_status, first_payload): (StatusCode, Value) =
            response_json(first_response).await;
        assert_eq!(
            first_status,
            StatusCode::ACCEPTED,
            "unexpected first payload: {first_payload}"
        );

        let second_response = public
            .oneshot(runtime_request(
                Method::POST,
                "/v1/node/jobs",
                None,
                Some(json!({
                    "kind": "wasm",
                    "submission": second_submission,
                    "idempotency_key": "rust-canonical-wasm-idempotency",
                })),
            ))
            .await
            .expect("second jobs response");
        let (second_status, second_payload): (StatusCode, Value) =
            response_json(second_response).await;
        assert_eq!(
            second_status,
            StatusCode::OK,
            "unexpected second payload: {second_payload}"
        );
        assert_eq!(first_payload["job_id"], second_payload["job_id"]);

        let job_id = first_payload["job_id"]
            .as_str()
            .expect("job_id string")
            .to_string();
        let completed = wait_for_job_status(&state, &job_id, jobs::JOB_STATUS_SUCCEEDED).await;
        assert_eq!(completed.request_hash, first_request_hash);
        assert_eq!(completed.result, Some(json!(42)));
    }

    #[tokio::test]
    async fn create_execution_job_persists_request_hash_and_evidence() {
        let state = test_app_state_with_free_pricing(PaymentBackend::None);
        let public = router(state.clone());
        let created_at = settlement::current_unix_timestamp();
        let mut event = NodeEventEnvelope {
            id: String::new(),
            pubkey: "11".repeat(32),
            created_at,
            kind: "market.listing".to_string(),
            tags: vec![vec!["t".to_string(), "froglet".to_string()]],
            content: "hello".to_string(),
            sig: "22".repeat(64),
        };
        event.id = expected_node_event_id(&event);
        insert_event_db(state.as_ref(), event.clone())
            .await
            .expect("insert event");

        let execution = crate::execution::ExecutionWorkload::builtin_events_query(
            vec!["market.listing".to_string()],
            Some(1),
        )
        .expect("builtin execution");
        let request_hash = execution.request_hash().expect("execution request hash");

        let response = public
            .oneshot(runtime_request(
                Method::POST,
                "/v1/node/jobs",
                None,
                Some(json!({
                    "kind": "execution",
                    "execution": execution,
                    "idempotency_key": "rust-builtin-events-query",
                })),
            ))
            .await
            .expect("create execution job response");
        let (status, payload): (StatusCode, Value) = response_json(response).await;
        assert_eq!(status, StatusCode::ACCEPTED);

        let job_id = payload["job_id"]
            .as_str()
            .expect("job_id string")
            .to_string();
        let completed = wait_for_job_status(&state, &job_id, jobs::JOB_STATUS_SUCCEEDED).await;
        assert_eq!(completed.request_hash, request_hash);
        assert_eq!(
            completed.service_id,
            crate::pricing::ServiceId::EventsQuery.as_str()
        );
        assert_eq!(
            completed.result,
            Some(json!({
                "events": [event],
                "cursor": null
            }))
        );

        let evidence = state
            .db
            .with_read_conn({
                let job_id = job_id.clone();
                move |conn| db::list_execution_evidence_for_subject(conn, "job", &job_id)
            })
            .await
            .expect("job evidence");
        assert!(
            evidence
                .iter()
                .any(|record| record.evidence_kind == "workload_spec"),
            "expected workload_spec evidence: {evidence:?}"
        );
        assert!(
            evidence
                .iter()
                .any(|record| record.evidence_kind == "execution_result"),
            "expected execution_result evidence: {evidence:?}"
        );
    }

    #[tokio::test]
    async fn builtin_execute_compute_offer_is_wasm_execution() {
        #[derive(serde::Deserialize)]
        struct OffersResponse {
            offers: Vec<SignedArtifact<OfferPayload>>,
        }

        let state = test_app_state(PaymentBackend::None);
        let response = public_router(state)
            .oneshot(runtime_request(
                Method::GET,
                "/v1/provider/offers",
                None,
                None,
            ))
            .await
            .expect("offers response");
        let (status, payload): (StatusCode, OffersResponse) = response_json(response).await;

        assert_eq!(status, StatusCode::OK);
        let offer = payload
            .offers
            .into_iter()
            .find(|offer| offer.payload.offer_id == ServiceId::ExecuteWasm.as_str())
            .expect("execute.compute offer");
        assert_eq!(
            offer.payload.offer_kind,
            wasm::WORKLOAD_KIND_COMPUTE_WASM_V1
        );
        assert_eq!(
            offer.payload.execution_profile.runtime,
            ExecutionRuntime::Wasm
        );
        assert_eq!(
            offer.payload.execution_profile.contract_version,
            wasm::WASM_RUN_JSON_ABI_V1
        );
        assert_eq!(
            offer.payload.execution_profile.abi_version,
            wasm::WASM_RUN_JSON_ABI_V1
        );
    }

    #[tokio::test]
    async fn generic_python_compute_is_rejected_by_builtin_execute_compute_offer() {
        let state = test_app_state(PaymentBackend::None);
        let spec = WorkloadSpec::Execution {
            execution: Box::new(
                crate::execution::ExecutionWorkload::python_inline_script(
                    "result = 4 + 3".to_string(),
                    Value::Null,
                )
                .expect("python inline script workload"),
            ),
        };

        let (status, payload) = create_quote_record(
            state,
            CreateQuoteRequest {
                offer_id: ServiceId::ExecuteWasm.as_str().to_string(),
                requester_id: "11".repeat(32),
                spec,
                max_price_sats: None,
            },
        )
        .await
        .expect_err("quote should reject non-wasm workload kind");

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(payload["error"], "offer does not match workload kind");
        assert_eq!(payload["offer_kind"], wasm::WORKLOAD_KIND_COMPUTE_WASM_V1);
        assert_eq!(
            payload["requested_workload_kind"],
            crate::execution::WORKLOAD_KIND_EXECUTION_V1
        );
    }

    #[test]
    fn postgres_mount_injects_env_when_capability_granted_and_dsn_configured() {
        use crate::execution::{ExecutionMount, ExecutionWorkload};
        // SAFETY: test-only env mutation; Rust 2024 marks set_var unsafe.
        unsafe {
            std::env::set_var(
                "FROGLET_MOUNT_postgres_analytics",
                "postgres://user:pass@db.local:5432/analytics",
            );
        }
        let execution = ExecutionWorkload {
            mounts: vec![ExecutionMount {
                handle: "analytics".to_string(),
                kind: "postgres".to_string(),
                read_only: true,
                binding: None,
            }],
            ..execution_for_mount_tests()
        };
        let plan =
            collect_data_mount_plan(&execution, &["mount.postgres.read.analytics".to_string()]);
        assert_eq!(plan.env.len(), 2);
        assert!(plan.needs_network, "postgres mount must enable network");
        assert!(plan.writable_paths.is_empty());
        assert!(plan.env.contains(&(
            "FROGLET_MOUNT_ANALYTICS_URL".to_string(),
            "postgres://user:pass@db.local:5432/analytics".to_string()
        )));
        assert!(plan.env.contains(&(
            "FROGLET_MOUNT_ANALYTICS_READ_ONLY".to_string(),
            "true".to_string()
        )));
        // SAFETY: test-only env cleanup.
        unsafe {
            std::env::remove_var("FROGLET_MOUNT_postgres_analytics");
        }
    }

    #[test]
    fn postgres_mount_omits_env_when_capability_not_granted() {
        use crate::execution::{ExecutionMount, ExecutionWorkload};
        unsafe {
            std::env::set_var("FROGLET_MOUNT_postgres_finance", "postgres://nope");
        }
        let execution = ExecutionWorkload {
            mounts: vec![ExecutionMount {
                handle: "finance".to_string(),
                kind: "postgres".to_string(),
                read_only: true,
                binding: None,
            }],
            ..execution_for_mount_tests()
        };
        let plan =
            collect_data_mount_plan(&execution, &["mount.postgres.read.analytics".to_string()]);
        assert!(plan.env.is_empty());
        assert!(!plan.needs_network);
        unsafe {
            std::env::remove_var("FROGLET_MOUNT_postgres_finance");
        }
    }

    #[test]
    fn postgres_mount_omits_env_when_dsn_not_configured() {
        use crate::execution::{ExecutionMount, ExecutionWorkload};
        unsafe {
            std::env::remove_var("FROGLET_MOUNT_postgres_unset");
        }
        let execution = ExecutionWorkload {
            mounts: vec![ExecutionMount {
                handle: "unset".to_string(),
                kind: "postgres".to_string(),
                read_only: false,
                binding: None,
            }],
            ..execution_for_mount_tests()
        };
        let plan = collect_data_mount_plan(&execution, &["mount.postgres.write.unset".to_string()]);
        assert!(plan.env.is_empty());
    }

    #[test]
    fn sqlite_mount_injects_env_and_grants_parent_dir_write() {
        use crate::execution::{ExecutionMount, ExecutionWorkload};
        unsafe {
            std::env::set_var(
                "FROGLET_MOUNT_sqlite_cache",
                "/var/lib/froglet/cache.sqlite",
            );
        }
        let execution = ExecutionWorkload {
            mounts: vec![ExecutionMount {
                handle: "cache".to_string(),
                kind: "sqlite".to_string(),
                read_only: false,
                binding: None,
            }],
            ..execution_for_mount_tests()
        };
        let plan = collect_data_mount_plan(&execution, &["mount.sqlite.write.cache".to_string()]);
        assert_eq!(plan.env.len(), 2);
        assert!(!plan.needs_network, "sqlite mount must NOT enable network");
        assert_eq!(plan.writable_paths.len(), 1);
        assert_eq!(
            plan.writable_paths[0],
            std::path::PathBuf::from("/var/lib/froglet")
        );
        assert!(plan.env.contains(&(
            "FROGLET_MOUNT_CACHE_URL".to_string(),
            "/var/lib/froglet/cache.sqlite".to_string()
        )));
        assert!(plan.env.contains(&(
            "FROGLET_MOUNT_CACHE_READ_ONLY".to_string(),
            "false".to_string()
        )));
        unsafe {
            std::env::remove_var("FROGLET_MOUNT_sqlite_cache");
        }
    }

    #[test]
    fn s3_mount_injects_env_and_enables_network_only() {
        use crate::execution::{ExecutionMount, ExecutionWorkload};
        unsafe {
            std::env::set_var(
                "FROGLET_MOUNT_s3_backups",
                "s3://AKIA:secret@s3.example.com/froglet-backups",
            );
        }
        let execution = ExecutionWorkload {
            mounts: vec![ExecutionMount {
                handle: "backups".to_string(),
                kind: "s3".to_string(),
                read_only: true,
                binding: None,
            }],
            ..execution_for_mount_tests()
        };
        let plan = collect_data_mount_plan(&execution, &["mount.s3.read.backups".to_string()]);
        assert_eq!(plan.env.len(), 2);
        assert!(plan.needs_network);
        assert!(plan.writable_paths.is_empty());
        assert!(plan.env.contains(&(
            "FROGLET_MOUNT_BACKUPS_URL".to_string(),
            "s3://AKIA:secret@s3.example.com/froglet-backups".to_string()
        )));
        unsafe {
            std::env::remove_var("FROGLET_MOUNT_s3_backups");
        }
    }

    #[test]
    fn data_mount_plan_composes_multiple_kinds_in_one_invocation() {
        use crate::execution::{ExecutionMount, ExecutionWorkload};
        unsafe {
            std::env::set_var(
                "FROGLET_MOUNT_postgres_events",
                "postgres://u:p@db.example/events",
            );
            std::env::set_var("FROGLET_MOUNT_sqlite_local", "/opt/froglet/local.sqlite");
        }
        let execution = ExecutionWorkload {
            mounts: vec![
                ExecutionMount {
                    handle: "events".to_string(),
                    kind: "postgres".to_string(),
                    read_only: true,
                    binding: None,
                },
                ExecutionMount {
                    handle: "local".to_string(),
                    kind: "sqlite".to_string(),
                    read_only: false,
                    binding: None,
                },
            ],
            ..execution_for_mount_tests()
        };
        let plan = collect_data_mount_plan(
            &execution,
            &[
                "mount.postgres.read.events".to_string(),
                "mount.sqlite.write.local".to_string(),
            ],
        );
        assert_eq!(plan.env.len(), 4);
        assert!(plan.needs_network, "postgres contribution requires network");
        assert_eq!(plan.writable_paths.len(), 1);
        assert_eq!(
            plan.writable_paths[0],
            std::path::PathBuf::from("/opt/froglet")
        );
        unsafe {
            std::env::remove_var("FROGLET_MOUNT_postgres_events");
            std::env::remove_var("FROGLET_MOUNT_sqlite_local");
        }
    }

    fn execution_for_mount_tests() -> crate::execution::ExecutionWorkload {
        use crate::execution::{
            ExecutionEntrypoint, ExecutionEntrypointKind, ExecutionPackageKind, ExecutionSecurity,
        };
        use froglet_protocol::ExecutionRuntime;
        crate::execution::ExecutionWorkload {
            schema_version: "froglet/v1".to_string(),
            workload_kind: "execution.v1".to_string(),
            runtime: ExecutionRuntime::Python,
            package_kind: ExecutionPackageKind::InlineSource,
            entrypoint: ExecutionEntrypoint {
                kind: ExecutionEntrypointKind::Handler,
                value: "handler".to_string(),
            },
            contract_version: "froglet.python.handler_json.v1".to_string(),
            input_format: "application/json".to_string(),
            input_hash: "00".repeat(32),
            requested_access: Vec::new(),
            security: ExecutionSecurity::default(),
            mounts: Vec::new(),
            inline_source: Some("def handler(event, ctx):\n    return event\n".to_string()),
            module_hash: None,
            module_bytes_hex: None,
            source_hash: None,
            oci_reference: None,
            oci_digest: None,
            builtin_name: None,
            input: serde_json::Value::Null,
        }
    }

    #[test]
    fn lightning_runtime_deal_builder_aligns_deadlines_with_quote_expiry() {
        let created_at = 1_700_000_000;
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_quote(requester_id, created_at, created_at + 150, 30_000, 30, 60);

        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_key,
            &"11".repeat(32),
            created_at,
            true,
        )
        .expect("deal");

        assert_eq!(deal.payload.admission_deadline, created_at + 60);
        assert_eq!(deal.payload.completion_deadline, created_at + 90);
        assert_eq!(deal.payload.acceptance_deadline, quote.payload.expires_at);
    }

    #[test]
    fn lightning_runtime_deal_builder_clamps_to_quote_budget_after_delay() {
        let quote_created_at = 1_700_000_000;
        let deal_created_at = quote_created_at + 20;
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_quote(
            requester_id,
            quote_created_at,
            quote_created_at + 150,
            30_000,
            30,
            60,
        );

        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_key,
            &"22".repeat(32),
            deal_created_at,
            true,
        )
        .expect("deal");

        assert_eq!(deal.payload.admission_deadline, quote_created_at + 60);
        assert_eq!(deal.payload.completion_deadline, quote_created_at + 90);
        assert_eq!(deal.payload.acceptance_deadline, quote.payload.expires_at);
        validate_deal_deadlines(&quote, &deal, deal_created_at, true).expect("deadlines valid");
    }

    #[test]
    fn validate_deal_deadlines_rejects_lightning_windows_that_outlive_the_quote() {
        let created_at = 1_700_000_000;
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_quote(
            requester_id.clone(),
            created_at,
            created_at + 150,
            30_000,
            30,
            60,
        );
        let deal = protocol::sign_artifact(
            &requester_id,
            |message| crypto::sign_message_hex(&requester_key, message),
            ARTIFACT_KIND_DEAL,
            created_at,
            DealPayload {
                requester_id: requester_id.clone(),
                provider_id: quote.payload.provider_id.clone(),
                quote_hash: quote.hash.clone(),
                workload_hash: quote.payload.workload_hash.clone(),
                confidential_session_hash: None,
                extension_refs: Vec::new(),
                authority_ref: None,
                supersedes_deal_hash: None,
                client_nonce: None,
                success_payment_hash: "11".repeat(32),
                admission_deadline: created_at + 60,
                completion_deadline: created_at + 120,
                acceptance_deadline: quote.payload.expires_at + 1,
            },
        )
        .expect("deal");

        let error = validate_deal_deadlines(&quote, &deal, created_at, true).unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(error.1.contains("acceptance_deadline"));
    }

    #[tokio::test]
    async fn provider_mock_pay_rejects_wrong_preimage() {
        let state = test_app_state(PaymentBackend::Lightning);
        let seeded = seed_mock_lightning_runtime_deal(&state, "https://provider.example").await;

        let response = public_router(state.clone())
            .oneshot(runtime_request(
                Method::POST,
                &format!("/v1/provider/deals/{}/mock-pay", seeded.deal_id),
                None,
                Some(json!({ "success_preimage": "11".repeat(32) })),
            ))
            .await
            .expect("provider mock-pay response");
        let (status, payload): (StatusCode, Value) = response_json(response).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            payload.get("error").and_then(Value::as_str),
            Some("success_preimage does not match the deal payment lock")
        );
    }

    #[tokio::test]
    async fn provider_mock_pay_marks_bundle_funded_and_starts_execution() {
        let state = test_app_state(PaymentBackend::Lightning);
        let seeded = seed_mock_lightning_runtime_deal(&state, "https://provider.example").await;

        let response = public_router(state.clone())
            .oneshot(runtime_request(
                Method::POST,
                &format!("/v1/provider/deals/{}/mock-pay", seeded.deal_id),
                None,
                Some(json!({ "success_preimage": seeded.success_preimage })),
            ))
            .await
            .expect("provider mock-pay response");
        let (status, payload): (StatusCode, deals::DealRecord) = response_json(response).await;

        assert_eq!(status, StatusCode::OK);
        assert_ne!(payload.status, deals::DEAL_STATUS_PAYMENT_PENDING);

        let result_ready =
            wait_for_deal_status(&state, &seeded.deal_id, deals::DEAL_STATUS_RESULT_READY).await;
        let bundle = deal_lightning_invoice_bundle(state.as_ref(), &result_ready)
            .await
            .expect("load lightning bundle")
            .expect("lightning bundle exists");
        assert_eq!(bundle.base_state, InvoiceBundleLegState::Settled);
        assert_eq!(bundle.success_state, InvoiceBundleLegState::Accepted);
    }

    #[tokio::test]
    async fn provider_mock_pay_is_idempotent_after_progress() {
        let state = test_app_state(PaymentBackend::Lightning);
        let seeded = seed_mock_lightning_runtime_deal(&state, "https://provider.example").await;

        let first = public_router(state.clone())
            .oneshot(runtime_request(
                Method::POST,
                &format!("/v1/provider/deals/{}/mock-pay", seeded.deal_id),
                None,
                Some(json!({ "success_preimage": seeded.success_preimage.clone() })),
            ))
            .await
            .expect("first provider mock-pay response");
        let (first_status, _): (StatusCode, deals::DealRecord) = response_json(first).await;
        assert_eq!(first_status, StatusCode::OK);

        let _ =
            wait_for_deal_status(&state, &seeded.deal_id, deals::DEAL_STATUS_RESULT_READY).await;

        let second = public_router(state.clone())
            .oneshot(runtime_request(
                Method::POST,
                &format!("/v1/provider/deals/{}/mock-pay", seeded.deal_id),
                None,
                Some(json!({ "success_preimage": seeded.success_preimage })),
            ))
            .await
            .expect("second provider mock-pay response");
        let (second_status, payload): (StatusCode, deals::DealRecord) = response_json(second).await;

        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(payload.status, deals::DEAL_STATUS_RESULT_READY);
    }

    #[tokio::test]
    async fn runtime_mock_pay_requires_runtime_auth() {
        let state = test_app_state(PaymentBackend::Lightning);
        let provider = spawn_public_test_server(state.clone()).await;
        let seeded = seed_mock_lightning_runtime_deal(&state, &provider.base_url).await;
        let runtime = runtime_router(state.clone());

        let missing = runtime
            .clone()
            .oneshot(runtime_request(
                Method::POST,
                &format!("/v1/runtime/deals/{}/mock-pay", seeded.deal_id),
                None,
                None,
            ))
            .await
            .expect("missing-auth response");
        let (missing_status, _): (StatusCode, Value) = response_json(missing).await;
        assert_eq!(missing_status, StatusCode::UNAUTHORIZED);

        let invalid = runtime
            .oneshot(runtime_request(
                Method::POST,
                &format!("/v1/runtime/deals/{}/mock-pay", seeded.deal_id),
                Some("wrong-token"),
                None,
            ))
            .await
            .expect("invalid-auth response");
        let (invalid_status, _): (StatusCode, Value) = response_json(invalid).await;
        assert_eq!(invalid_status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn runtime_mock_pay_rejects_lnd_rest_mode() {
        let state =
            test_app_state_with_lightning_mode(PaymentBackend::Lightning, LightningMode::LndRest);
        let runtime = runtime_router(state);
        let response = runtime
            .oneshot(runtime_request(
                Method::POST,
                "/v1/runtime/deals/deal-1/mock-pay",
                Some("test-runtime-token"),
                None,
            ))
            .await
            .expect("runtime mock-pay response");
        let (status, payload): (StatusCode, Value) = response_json(response).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            payload.get("error").and_then(Value::as_str),
            Some("runtime mock-pay is only available for lightning mock mode")
        );
    }

    #[tokio::test]
    async fn runtime_mock_pay_rejects_missing_bundle() {
        let state = test_app_state(PaymentBackend::Lightning);
        let provider = spawn_public_test_server(state.clone()).await;
        let seeded = seed_mock_lightning_runtime_deal(&state, &provider.base_url).await;
        state
            .db
            .with_write_conn(|conn| -> Result<(), String> {
                conn.execute("DELETE FROM lightning_invoice_bundles", [])
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .await
            .expect("delete lightning bundles");

        let response = runtime_router(state.clone())
            .oneshot(runtime_request(
                Method::POST,
                &format!("/v1/runtime/deals/{}/mock-pay", seeded.deal_id),
                Some("test-runtime-token"),
                None,
            ))
            .await
            .expect("runtime mock-pay response");
        let (status, payload): (StatusCode, Value) = response_json(response).await;

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            payload.get("error").and_then(Value::as_str),
            Some("upstream request failed")
        );
        assert_eq!(
            payload.get("upstream_status").and_then(Value::as_u64),
            Some(404)
        );
    }

    #[tokio::test]
    async fn runtime_mock_pay_succeeds_and_is_idempotent() {
        let state = test_app_state(PaymentBackend::Lightning);
        let provider = spawn_public_test_server(state.clone()).await;
        let seeded = seed_mock_lightning_runtime_deal(&state, &provider.base_url).await;

        let first = runtime_router(state.clone())
            .oneshot(runtime_request(
                Method::POST,
                &format!("/v1/runtime/deals/{}/mock-pay", seeded.deal_id),
                Some("test-runtime-token"),
                None,
            ))
            .await
            .expect("first runtime mock-pay response");
        let (first_status, first_payload): (StatusCode, RuntimeMockPayDealResponse) =
            response_json(first).await;
        assert_eq!(first_status, StatusCode::OK);
        assert_ne!(
            first_payload.deal.status,
            deals::DEAL_STATUS_PAYMENT_PENDING
        );
        let expected_payment_intent_path =
            format!("/v1/runtime/deals/{}/payment-intent", seeded.deal_id);
        assert_eq!(
            first_payload.payment_intent_path.as_deref(),
            Some(expected_payment_intent_path.as_str())
        );

        let _ =
            wait_for_deal_status(&state, &seeded.deal_id, deals::DEAL_STATUS_RESULT_READY).await;

        let second = runtime_router(state.clone())
            .oneshot(runtime_request(
                Method::POST,
                &format!("/v1/runtime/deals/{}/mock-pay", seeded.deal_id),
                Some("test-runtime-token"),
                None,
            ))
            .await
            .expect("second runtime mock-pay response");
        let (second_status, second_payload): (StatusCode, RuntimeMockPayDealResponse) =
            response_json(second).await;
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(second_payload.deal.status, deals::DEAL_STATUS_RESULT_READY);
        assert!(second_payload.payment_intent.is_some());
    }

    #[test]
    fn attested_confidential_wasm_rejects_requested_capabilities() {
        let mut submission = test_wasm_submission();
        submission.workload.abi_version = crate::wasm::WASM_HOST_JSON_ABI_V1.to_string();
        submission.workload.requested_capabilities = vec!["db.sqlite.query.read.demo".to_string()];

        let verified = submission
            .verify()
            .expect("host abi submission with a generic capability should verify");
        let error =
            ensure_safe_attested_wasm_submission(&verified).expect_err("expected rejection");
        assert!(error.contains("requested_capabilities"));
    }

    #[tokio::test]
    async fn attested_confidential_wasm_executes_via_provider_session_and_deal_flow() {
        #[derive(serde::Deserialize)]
        struct OffersResponse {
            offers: Vec<SignedArtifact<OfferPayload>>,
        }

        let state = test_app_state_with_confidential_policy(PaymentBackend::None);
        let public = public_router(state.clone());
        let inline_submission = test_wasm_submission();
        let inline_result = run_job_spec_now(
            state.as_ref(),
            JobSpec::Wasm {
                submission: inline_submission.clone(),
            },
        )
        .await
        .expect("inline wasm execution");

        let offers_response = public
            .clone()
            .oneshot(runtime_request(
                Method::GET,
                "/v1/provider/offers",
                None,
                None,
            ))
            .await
            .expect("offers response");
        let (offers_status, offers_payload): (StatusCode, OffersResponse) =
            response_json(offers_response).await;
        assert_eq!(offers_status, StatusCode::OK);
        let offer = offers_payload
            .offers
            .into_iter()
            .find(|offer| {
                offer.payload.offer_kind
                    == crate::confidential::WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1
            })
            .expect("attested wasm offer");
        let confidential_profile_hash = offer
            .payload
            .confidential_profile_hash
            .clone()
            .expect("offer confidential profile hash");

        let profile_response = public
            .clone()
            .oneshot(runtime_request(
                Method::GET,
                &format!(
                    "/v1/provider/confidential/profiles/{}",
                    confidential_profile_hash
                ),
                None,
                None,
            ))
            .await
            .expect("profile response");
        let (profile_status, profile): (
            StatusCode,
            SignedArtifact<crate::confidential::ConfidentialProfilePayload>,
        ) = response_json(profile_response).await;
        assert_eq!(profile_status, StatusCode::OK);
        assert_eq!(
            profile.payload.allowed_workload_kind,
            crate::confidential::WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1
        );

        let requester_signing_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_signing_key);
        let (requester_private_key, requester_public_key) = crate::confidential::generate_keypair();
        let open_session_request = crate::confidential::ConfidentialSessionOpenRequest {
            requester_id: requester_id.clone(),
            confidential_profile_hash: confidential_profile_hash.clone(),
            allowed_workload_kind: crate::confidential::WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1
                .to_string(),
            requester_public_key: requester_public_key.clone(),
        };

        let open_session_response = public
            .clone()
            .oneshot(runtime_request(
                Method::POST,
                "/v1/provider/confidential/sessions",
                None,
                Some(
                    serde_json::to_value(&open_session_request)
                        .expect("serialize confidential session request"),
                ),
            ))
            .await
            .expect("open session response");
        let (open_status, opened_session): (
            StatusCode,
            crate::confidential::ConfidentialSessionOpenResponse,
        ) = response_json(open_session_response).await;
        assert_eq!(open_status, StatusCode::CREATED);
        assert_eq!(opened_session.profile.hash, profile.hash);
        crate::confidential::verify_attestation_bundle(
            &opened_session.profile.payload,
            &opened_session.session,
            &opened_session.attestation,
            settlement::current_unix_timestamp(),
        )
        .expect("valid confidential attestation");

        let session_response = public
            .clone()
            .oneshot(runtime_request(
                Method::GET,
                &format!(
                    "/v1/provider/confidential/sessions/{}",
                    opened_session.session.payload.session_id
                ),
                None,
                None,
            ))
            .await
            .expect("get session response");
        let (session_status, persisted_session): (
            StatusCode,
            crate::confidential::ConfidentialSessionOpenResponse,
        ) = response_json(session_response).await;
        assert_eq!(session_status, StatusCode::OK);
        assert_eq!(persisted_session.session.hash, opened_session.session.hash);

        let request_envelope = crate::confidential::encrypt_request_envelope(
            &opened_session.session.hash,
            &requester_private_key,
            &opened_session.session.payload.session_public_key,
            &inline_submission,
            JCS_JSON_FORMAT,
        )
        .expect("encrypt attested wasm request");
        let spec = WorkloadSpec::AttestedWasm {
            confidential_session_hash: opened_session.session.hash.clone(),
            request_envelope: Box::new(request_envelope),
        };

        let quote = create_quote_record(
            state.clone(),
            CreateQuoteRequest {
                offer_id: offer.payload.offer_id.clone(),
                requester_id: requester_id.clone(),
                spec: spec.clone(),
                max_price_sats: Some(0),
            },
        )
        .await
        .expect("create attested wasm quote");
        assert_eq!(
            quote.payload.confidential_session_hash.as_deref(),
            Some(opened_session.session.hash.as_str())
        );

        let created_at = settlement::current_unix_timestamp();
        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_signing_key,
            &"77".repeat(32),
            created_at,
            false,
        )
        .expect("attested wasm deal");
        let (accepted, status) = create_deal_record(
            state.clone(),
            CreateDealRequest {
                quote: quote.clone(),
                deal,
                spec,
                idempotency_key: Some("attested-confidential-wasm-e2e".to_string()),
                payment: None,
            },
        )
        .await
        .expect("create attested wasm deal");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(accepted.status, deals::DEAL_STATUS_ACCEPTED);

        let succeeded =
            wait_for_deal_status(&state, &accepted.deal_id, deals::DEAL_STATUS_SUCCEEDED).await;
        let result_envelope: crate::confidential::EncryptedEnvelope =
            serde_json::from_value(succeeded.result.clone().expect("encrypted result envelope"))
                .expect("decode encrypted result envelope");
        let decrypted_result: Value = crate::confidential::decrypt_result_envelope(
            &opened_session.session.hash,
            &requester_private_key,
            &opened_session.session.payload.session_public_key,
            &result_envelope,
        )
        .expect("decrypt confidential result");
        assert_eq!(decrypted_result, inline_result);
        assert_eq!(
            succeeded.result_hash.as_deref(),
            Some(canonical_result_hash(&inline_result).as_str())
        );
        assert!(succeeded.receipt.is_some(), "expected success receipt");
    }

    #[tokio::test]
    async fn confidential_service_executes_via_provider_session_and_deal_flow() {
        #[derive(serde::Deserialize)]
        struct OffersResponse {
            offers: Vec<SignedArtifact<OfferPayload>>,
        }

        let state = test_app_state_with_confidential_policy(PaymentBackend::None);
        let public = public_router(state.clone());

        let offers_response = public
            .clone()
            .oneshot(runtime_request(
                Method::GET,
                "/v1/provider/offers",
                None,
                None,
            ))
            .await
            .expect("offers response");
        let (offers_status, offers_payload): (StatusCode, OffersResponse) =
            response_json(offers_response).await;
        assert_eq!(offers_status, StatusCode::OK);
        let offer = offers_payload
            .offers
            .into_iter()
            .find(|offer| {
                offer.payload.offer_kind
                    == crate::confidential::WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1
            })
            .expect("confidential service offer");
        let confidential_profile_hash = offer
            .payload
            .confidential_profile_hash
            .clone()
            .expect("offer confidential profile hash");

        let profile_response = public
            .clone()
            .oneshot(runtime_request(
                Method::GET,
                &format!(
                    "/v1/provider/confidential/profiles/{}",
                    confidential_profile_hash
                ),
                None,
                None,
            ))
            .await
            .expect("profile response");
        let (profile_status, profile): (
            StatusCode,
            SignedArtifact<crate::confidential::ConfidentialProfilePayload>,
        ) = response_json(profile_response).await;
        assert_eq!(profile_status, StatusCode::OK);
        assert_eq!(
            profile.payload.allowed_workload_kind,
            crate::confidential::WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1
        );
        assert_eq!(profile.payload.service_id.as_deref(), Some("json_search"));

        let requester_signing_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_signing_key);
        let (requester_private_key, requester_public_key) = crate::confidential::generate_keypair();
        let open_session_request = crate::confidential::ConfidentialSessionOpenRequest {
            requester_id: requester_id.clone(),
            confidential_profile_hash: confidential_profile_hash.clone(),
            allowed_workload_kind: crate::confidential::WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1
                .to_string(),
            requester_public_key: requester_public_key.clone(),
        };

        let open_session_response = public
            .clone()
            .oneshot(runtime_request(
                Method::POST,
                "/v1/provider/confidential/sessions",
                None,
                Some(
                    serde_json::to_value(&open_session_request)
                        .expect("serialize confidential session request"),
                ),
            ))
            .await
            .expect("open session response");
        let (open_status, opened_session): (
            StatusCode,
            crate::confidential::ConfidentialSessionOpenResponse,
        ) = response_json(open_session_response).await;
        assert_eq!(open_status, StatusCode::CREATED);
        crate::confidential::verify_attestation_bundle(
            &opened_session.profile.payload,
            &opened_session.session,
            &opened_session.attestation,
            settlement::current_unix_timestamp(),
        )
        .expect("valid confidential attestation");

        let request_payload = json!({
            "query": "NemoClaw",
            "limit": 10,
        });
        let request_envelope = crate::confidential::encrypt_request_envelope(
            &opened_session.session.hash,
            &requester_private_key,
            &opened_session.session.payload.session_public_key,
            &request_payload,
            JCS_JSON_FORMAT,
        )
        .expect("encrypt confidential service request");
        let spec = WorkloadSpec::ConfidentialService {
            confidential_session_hash: opened_session.session.hash.clone(),
            service_id: "json_search".to_string(),
            request_envelope: Box::new(request_envelope),
        };

        let quote = create_quote_record(
            state.clone(),
            CreateQuoteRequest {
                offer_id: offer.payload.offer_id.clone(),
                requester_id: requester_id.clone(),
                spec: spec.clone(),
                max_price_sats: Some(0),
            },
        )
        .await
        .expect("create confidential service quote");
        assert_eq!(
            quote.payload.confidential_session_hash.as_deref(),
            Some(opened_session.session.hash.as_str())
        );

        let created_at = settlement::current_unix_timestamp();
        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_signing_key,
            &"88".repeat(32),
            created_at,
            false,
        )
        .expect("confidential service deal");
        let (accepted, status) = create_deal_record(
            state.clone(),
            CreateDealRequest {
                quote: quote.clone(),
                deal,
                spec,
                idempotency_key: Some("confidential-service-e2e".to_string()),
                payment: None,
            },
        )
        .await
        .expect("create confidential service deal");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(accepted.status, deals::DEAL_STATUS_ACCEPTED);

        let succeeded =
            wait_for_deal_status(&state, &accepted.deal_id, deals::DEAL_STATUS_SUCCEEDED).await;
        let result_envelope: crate::confidential::EncryptedEnvelope =
            serde_json::from_value(succeeded.result.clone().expect("encrypted result envelope"))
                .expect("decode encrypted result envelope");
        let decrypted_result: Value = crate::confidential::decrypt_result_envelope(
            &opened_session.session.hash,
            &requester_private_key,
            &opened_session.session.payload.session_public_key,
            &result_envelope,
        )
        .expect("decrypt confidential result");

        assert_eq!(decrypted_result["query"], "NemoClaw");
        assert_eq!(decrypted_result["returned"], 1);
        assert_eq!(decrypted_result["matches"][0]["id"], "doc-2");
        assert_eq!(
            decrypted_result["matches"][0]["title"],
            "NemoClaw integration"
        );
        assert_eq!(
            succeeded.result_hash.as_deref(),
            Some(canonical_result_hash(&decrypted_result).as_str())
        );
        assert!(succeeded.receipt.is_some(), "expected success receipt");
    }

    #[test]
    fn clearnet_transport_matches_uri_scheme() {
        assert_eq!(
            transport_name_for_clearnet_uri("http://127.0.0.1:8080"),
            "http"
        );
        assert_eq!(
            transport_name_for_clearnet_uri("https://node.example"),
            "https"
        );
    }

    #[test]
    fn node_event_ids_hash_full_event_commitment() {
        let event = NodeEventEnvelope {
            id: String::new(),
            pubkey: "11".repeat(32),
            created_at: 123,
            kind: "market.listing".to_string(),
            tags: vec![vec!["t".to_string(), "froglet".to_string()]],
            content: "hello".to_string(),
            sig: "22".repeat(64),
        };

        let expected = canonical_json::to_vec(&json!([
            event.pubkey,
            event.created_at,
            event.kind,
            event.tags,
            event.content
        ]))
        .expect("event id preimage");
        assert_eq!(expected_node_event_id(&event), crypto::sha256_hex(expected));
    }

    #[tokio::test]
    async fn runtime_and_public_routers_are_separated() {
        let state = test_app_state(PaymentBackend::None);
        let public = public_router(state.clone());
        let runtime = runtime_router(state);

        let public_runtime = public
            .oneshot(
                Request::builder()
                    .uri("/v1/runtime/provider/start")
                    .body(Body::empty())
                    .expect("runtime request"),
            )
            .await
            .expect("public response");
        assert_eq!(public_runtime.status(), StatusCode::NOT_FOUND);

        let runtime_public = runtime
            .oneshot(
                Request::builder()
                    .uri("/v1/descriptor")
                    .body(Body::empty())
                    .expect("descriptor request"),
            )
            .await
            .expect("runtime response");
        assert_eq!(runtime_public.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn recover_runtime_state_requeues_running_work() {
        let state = test_app_state(PaymentBackend::None);
        let now = settlement::current_unix_timestamp();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_quote(requester_id.clone(), now - 5, now + 60, 1_000, 30, 30);
        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_key,
            &"11".repeat(32),
            now - 5,
            false,
        )
        .expect("deal");
        let spec = WorkloadSpec::EventsQuery {
            kinds: vec!["market.listing".to_string()],
            limit: Some(1),
        };
        let running_job_id = jobs::new_job_id();
        let running_deal_id = protocol::new_artifact_id();
        let running_job_id_for_seed = running_job_id.clone();
        let running_deal_id_for_seed = running_deal_id.clone();
        let request_hash = JobSpec::Wasm {
            submission: test_wasm_submission(),
        }
        .request_hash()
        .expect("job hash");

        state
            .db
            .with_write_conn({
                let quote = quote.clone();
                let deal = deal.clone();
                let spec = spec.clone();
                move |conn| -> Result<(), String> {
                    jobs::insert_or_get_job(
                        conn,
                        NewJob {
                            job_id: running_job_id_for_seed.clone(),
                            idempotency_key: Some("recovery-job".to_string()),
                            request_hash,
                            service_id: ServiceId::ExecuteWasm.as_str().to_string(),
                            spec: JobSpec::Wasm {
                                submission: test_wasm_submission(),
                            },
                            created_at: now - 5,
                        },
                    )?;
                    let _ = jobs::try_start_job(conn, &running_job_id_for_seed, now - 4)?;

                    deals::insert_or_get_deal(
                        conn,
                        NewDeal {
                            deal_id: running_deal_id_for_seed.clone(),
                            idempotency_key: Some("recovery-deal".to_string()),
                            quote,
                            spec,
                            artifact: deal.clone(),
                            workload_evidence_hash: None,
                            deal_artifact_hash: deal.hash.clone(),
                            payment_method: None,
                            payment_token_hash: None,
                            payment_amount_sats: None,
                            initial_status: deals::DEAL_STATUS_RUNNING.to_string(),
                            created_at: now - 5,
                        },
                    )?;
                    Ok(())
                }
            })
            .await
            .expect("seed work");

        recover_runtime_state(state.clone())
            .await
            .expect("recover runtime state");

        let recovered_job =
            wait_for_job_status(&state, &running_job_id, jobs::JOB_STATUS_SUCCEEDED).await;
        let recovered_deal =
            wait_for_deal_status(&state, &running_deal_id, deals::DEAL_STATUS_SUCCEEDED).await;

        let job_evidence = state
            .db
            .with_read_conn({
                let running_job_id = running_job_id.clone();
                move |conn| db::list_execution_evidence_for_subject(conn, "job", &running_job_id)
            })
            .await
            .expect("job evidence");
        let deal_evidence = state
            .db
            .with_read_conn({
                let running_deal_id = running_deal_id.clone();
                move |conn| db::list_execution_evidence_for_subject(conn, "deal", &running_deal_id)
            })
            .await
            .expect("deal evidence");

        assert_eq!(recovered_job.status, jobs::JOB_STATUS_SUCCEEDED);
        assert_eq!(recovered_deal.status, deals::DEAL_STATUS_SUCCEEDED);
        assert!(
            job_evidence
                .iter()
                .any(|record| record.evidence_kind == "recovery_action")
        );
        assert!(
            deal_evidence
                .iter()
                .any(|record| record.evidence_kind == "recovery_action")
        );
    }

    #[tokio::test]
    async fn recover_runtime_state_fails_expired_inflight_deals() {
        let state = test_app_state(PaymentBackend::None);
        let now = settlement::current_unix_timestamp();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_quote(requester_id.clone(), now - 120, now - 90, 1_000, 30, 30);
        let expired_deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_key,
            &"22".repeat(32),
            now - 120,
            false,
        )
        .expect("deal");
        let expired_deal_id = protocol::new_artifact_id();
        let expired_deal_id_for_seed = expired_deal_id.clone();

        state
            .db
            .with_write_conn({
                let quote = quote.clone();
                let expired_deal = expired_deal.clone();
                move |conn| -> Result<(), String> {
                    deals::insert_or_get_deal(
                        conn,
                        NewDeal {
                            deal_id: expired_deal_id_for_seed.clone(),
                            idempotency_key: Some("expired-deal".to_string()),
                            quote,
                            spec: WorkloadSpec::EventsQuery {
                                kinds: vec!["market.listing".to_string()],
                                limit: Some(1),
                            },
                            artifact: expired_deal.clone(),
                            workload_evidence_hash: None,
                            deal_artifact_hash: expired_deal.hash.clone(),
                            payment_method: None,
                            payment_token_hash: None,
                            payment_amount_sats: None,
                            initial_status: deals::DEAL_STATUS_RUNNING.to_string(),
                            created_at: now - 120,
                        },
                    )?;
                    Ok(())
                }
            })
            .await
            .expect("seed expired deal");

        recover_runtime_state(state.clone())
            .await
            .expect("recover runtime state");

        let failed_deal =
            wait_for_deal_status(&state, &expired_deal_id, deals::DEAL_STATUS_FAILED).await;
        assert!(
            failed_deal
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("completion_deadline elapsed"),
            "unexpected recovery error: {:?}",
            failed_deal.error
        );
        assert!(failed_deal.receipt.is_some(), "expected recovery receipt");
        assert_eq!(
            failed_deal
                .receipt
                .as_ref()
                .and_then(|receipt| receipt.payload.failure_code.as_deref()),
            Some("completion_deadline_elapsed_during_recovery")
        );
    }

    #[tokio::test]
    async fn recover_runtime_state_keeps_funded_lightning_deals_recoverable() {
        let state = test_app_state(PaymentBackend::Lightning);
        let now = settlement::current_unix_timestamp();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_quote(requester_id.clone(), now - 5, now + 180, 30_000, 30, 60);
        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_key,
            &"33".repeat(32),
            now - 5,
            true,
        )
        .expect("deal");
        let bundle = test_lightning_bundle(state.as_ref(), &quote, &deal, &requester_id, now - 5);
        let deal_id = protocol::new_artifact_id();
        let deal_id_for_seed = deal_id.clone();

        state
            .db
            .with_write_conn({
                let quote = quote.clone();
                let deal = deal.clone();
                let bundle = bundle.clone();
                move |conn| -> Result<(), String> {
                    deals::insert_or_get_deal(
                        conn,
                        NewDeal {
                            deal_id: deal_id_for_seed.clone(),
                            idempotency_key: Some("lightning-payment-pending".to_string()),
                            quote: quote.clone(),
                            spec: WorkloadSpec::Wasm {
                                submission: Box::new(test_wasm_submission()),
                            },
                            artifact: deal.clone(),
                            workload_evidence_hash: None,
                            deal_artifact_hash: deal.hash.clone(),
                            payment_method: Some("lightning".to_string()),
                            payment_token_hash: Some(deal.payload.success_payment_hash.clone()),
                            payment_amount_sats: Some(lightning_payment_amount_sats(&quote)),
                            initial_status: deals::DEAL_STATUS_PAYMENT_PENDING.to_string(),
                            created_at: now - 5,
                        },
                    )?;
                    db::insert_lightning_invoice_bundle(
                        conn,
                        &bundle.session_id,
                        &bundle.bundle,
                        InvoiceBundleLegState::Settled,
                        InvoiceBundleLegState::Accepted,
                        now - 5,
                    )?;
                    Ok(())
                }
            })
            .await
            .expect("seed lightning deal");

        recover_runtime_state(state.clone())
            .await
            .expect("recover runtime state");

        let recovery_evidence = state
            .db
            .with_read_conn({
                let deal_id = deal_id.clone();
                move |conn| db::list_execution_evidence_for_subject(conn, "deal", &deal_id)
            })
            .await
            .expect("deal evidence");
        assert!(
            recovery_evidence
                .iter()
                .any(|record| record.evidence_kind == "recovery_action")
        );

        reconcile_lightning_settlement_once(state.clone())
            .await
            .expect("reconcile lightning");

        let resumed_deal =
            wait_for_deal_status(&state, &deal_id, deals::DEAL_STATUS_RESULT_READY).await;
        assert_eq!(resumed_deal.status, deals::DEAL_STATUS_RESULT_READY);
        assert!(
            resumed_deal.result.is_some(),
            "expected preserved execution result"
        );
    }

    #[tokio::test]
    async fn recover_runtime_state_keeps_unfunded_payment_pending_lightning_deals_recoverable() {
        let state = test_app_state(PaymentBackend::Lightning);
        let now = settlement::current_unix_timestamp();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_quote(requester_id.clone(), now - 5, now + 180, 30_000, 30, 60);
        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_key,
            &"34".repeat(32),
            now - 5,
            true,
        )
        .expect("deal");
        let bundle = test_lightning_bundle(state.as_ref(), &quote, &deal, &requester_id, now - 5);
        let deal_id = protocol::new_artifact_id();
        let deal_id_for_seed = deal_id.clone();

        state
            .db
            .with_write_conn({
                let quote = quote.clone();
                let deal = deal.clone();
                let bundle = bundle.clone();
                move |conn| -> Result<(), String> {
                    deals::insert_or_get_deal(
                        conn,
                        NewDeal {
                            deal_id: deal_id_for_seed.clone(),
                            idempotency_key: Some("lightning-payment-pending-unfunded".to_string()),
                            quote: quote.clone(),
                            spec: WorkloadSpec::Wasm {
                                submission: Box::new(test_wasm_submission()),
                            },
                            artifact: deal.clone(),
                            workload_evidence_hash: None,
                            deal_artifact_hash: deal.hash.clone(),
                            payment_method: Some("lightning".to_string()),
                            payment_token_hash: Some(deal.payload.success_payment_hash.clone()),
                            payment_amount_sats: Some(lightning_payment_amount_sats(&quote)),
                            initial_status: deals::DEAL_STATUS_PAYMENT_PENDING.to_string(),
                            created_at: now - 5,
                        },
                    )?;
                    db::insert_lightning_invoice_bundle(
                        conn,
                        &bundle.session_id,
                        &bundle.bundle,
                        InvoiceBundleLegState::Open,
                        InvoiceBundleLegState::Open,
                        now - 5,
                    )?;
                    Ok(())
                }
            })
            .await
            .expect("seed unfunded lightning deal");

        recover_runtime_state(state.clone())
            .await
            .expect("recover runtime state");

        tokio::time::sleep(Duration::from_millis(100)).await;

        let recovered_deal = state
            .db
            .with_read_conn({
                let deal_id = deal_id.clone();
                move |conn| deals::get_deal(conn, &deal_id)
            })
            .await
            .expect("load recovered deal")
            .expect("recovered deal");
        assert_eq!(recovered_deal.status, deals::DEAL_STATUS_PAYMENT_PENDING);
        assert!(recovered_deal.error.is_none(), "unexpected recovery error");
        assert!(
            recovered_deal.receipt.is_none(),
            "unexpected recovery receipt"
        );

        let funded_bundle = settlement::update_lightning_invoice_bundle_states(
            state.as_ref(),
            &bundle.session_id,
            InvoiceBundleLegState::Settled,
            InvoiceBundleLegState::Accepted,
        )
        .await
        .expect("fund bundle")
        .expect("updated bundle");
        let promoted =
            promote_lightning_deal_if_funded(state.clone(), &recovered_deal, &funded_bundle)
                .await
                .expect("promote funded deal");
        assert!(promoted, "expected funded recovered deal to promote");

        let resumed_deal =
            wait_for_deal_status(&state, &deal_id, deals::DEAL_STATUS_RESULT_READY).await;
        assert_eq!(resumed_deal.status, deals::DEAL_STATUS_RESULT_READY);
        assert!(
            resumed_deal.result.is_some(),
            "expected recovered deal to execute after funding"
        );
    }

    #[tokio::test]
    async fn recover_runtime_state_fails_result_ready_lightning_deals_with_canceled_success_hold() {
        let state = test_app_state(PaymentBackend::Lightning);
        let now = settlement::current_unix_timestamp();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_quote(requester_id.clone(), now - 5, now + 180, 30_000, 30, 60);
        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_key,
            &"44".repeat(32),
            now - 5,
            true,
        )
        .expect("deal");
        let bundle = test_lightning_bundle(state.as_ref(), &quote, &deal, &requester_id, now - 5);
        let deal_id = protocol::new_artifact_id();
        let deal_id_for_seed = deal_id.clone();

        state
            .db
            .with_write_conn({
                let quote = quote.clone();
                let deal = deal.clone();
                let bundle = bundle.clone();
                move |conn| -> Result<(), String> {
                    deals::insert_or_get_deal(
                        conn,
                        NewDeal {
                            deal_id: deal_id_for_seed.clone(),
                            idempotency_key: Some("lightning-result-ready".to_string()),
                            quote: quote.clone(),
                            spec: WorkloadSpec::Wasm {
                                submission: Box::new(test_wasm_submission()),
                            },
                            artifact: deal.clone(),
                            workload_evidence_hash: None,
                            deal_artifact_hash: deal.hash.clone(),
                            payment_method: Some("lightning".to_string()),
                            payment_token_hash: Some(deal.payload.success_payment_hash.clone()),
                            payment_amount_sats: Some(lightning_payment_amount_sats(&quote)),
                            initial_status: deals::DEAL_STATUS_RUNNING.to_string(),
                            created_at: now - 5,
                        },
                    )?;
                    if !deals::stage_deal_result_ready(
                        conn,
                        &deal_id_for_seed,
                        &json!({ "ok": true }),
                        None,
                        None,
                        now - 4,
                    )? {
                        return Err("failed to stage test deal as result_ready".to_string());
                    }
                    db::insert_lightning_invoice_bundle(
                        conn,
                        &bundle.session_id,
                        &bundle.bundle,
                        InvoiceBundleLegState::Settled,
                        InvoiceBundleLegState::Canceled,
                        now - 5,
                    )?;
                    Ok(())
                }
            })
            .await
            .expect("seed result-ready lightning deal");

        recover_runtime_state(state.clone())
            .await
            .expect("recover runtime state");

        let failed_deal = wait_for_deal_status(&state, &deal_id, deals::DEAL_STATUS_FAILED).await;
        assert!(
            failed_deal
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("success hold was canceled"),
            "unexpected recovery error: {:?}",
            failed_deal.error
        );
        assert!(failed_deal.receipt.is_some(), "expected recovery receipt");
        assert_eq!(
            failed_deal
                .receipt
                .as_ref()
                .and_then(|receipt| receipt.payload.failure_code.as_deref()),
            Some("success_fee_canceled_during_recovery")
        );
    }

    #[tokio::test]
    async fn recover_runtime_state_allows_settled_result_ready_lightning_deals_to_finish() {
        let state = test_app_state(PaymentBackend::Lightning);
        let now = settlement::current_unix_timestamp();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let quote = signed_quote(requester_id.clone(), now - 5, now + 180, 30_000, 30, 60);
        let deal = build_requester_signed_deal_artifact(
            &quote,
            &requester_key,
            &"55".repeat(32),
            now - 5,
            true,
        )
        .expect("deal");
        let bundle = test_lightning_bundle(state.as_ref(), &quote, &deal, &requester_id, now - 5);
        let deal_id = protocol::new_artifact_id();
        let deal_id_for_seed = deal_id.clone();

        state
            .db
            .with_write_conn({
                let quote = quote.clone();
                let deal = deal.clone();
                let bundle = bundle.clone();
                move |conn| -> Result<(), String> {
                    deals::insert_or_get_deal(
                        conn,
                        NewDeal {
                            deal_id: deal_id_for_seed.clone(),
                            idempotency_key: Some("lightning-result-ready-settled".to_string()),
                            quote: quote.clone(),
                            spec: WorkloadSpec::Wasm {
                                submission: Box::new(test_wasm_submission()),
                            },
                            artifact: deal.clone(),
                            workload_evidence_hash: None,
                            deal_artifact_hash: deal.hash.clone(),
                            payment_method: Some("lightning".to_string()),
                            payment_token_hash: Some(deal.payload.success_payment_hash.clone()),
                            payment_amount_sats: Some(lightning_payment_amount_sats(&quote)),
                            initial_status: deals::DEAL_STATUS_RUNNING.to_string(),
                            created_at: now - 5,
                        },
                    )?;
                    if !deals::stage_deal_result_ready(
                        conn,
                        &deal_id_for_seed,
                        &json!({ "ok": true }),
                        None,
                        None,
                        now - 4,
                    )? {
                        return Err("failed to stage test deal as result_ready".to_string());
                    }
                    db::insert_lightning_invoice_bundle(
                        conn,
                        &bundle.session_id,
                        &bundle.bundle,
                        InvoiceBundleLegState::Settled,
                        InvoiceBundleLegState::Settled,
                        now - 5,
                    )?;
                    Ok(())
                }
            })
            .await
            .expect("seed settled result-ready lightning deal");

        recover_runtime_state(state.clone())
            .await
            .expect("recover runtime state");
        reconcile_lightning_settlement_once(state.clone())
            .await
            .expect("reconcile lightning");

        let succeeded_deal =
            wait_for_deal_status(&state, &deal_id, deals::DEAL_STATUS_SUCCEEDED).await;
        assert_eq!(succeeded_deal.status, deals::DEAL_STATUS_SUCCEEDED);
        assert!(succeeded_deal.receipt.is_some(), "expected success receipt");
    }
}
