use axum::{
    Json, Router,
    error_handling::HandleErrorLayer,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tower::{BoxError, ServiceBuilder, limit::ConcurrencyLimitLayer, timeout::TimeoutLayer};

use crate::{
    canonical_json,
    config::PaymentBackend,
    crypto, db,
    deals::{self, NewDeal},
    ecash,
    jobs::{self, JobPaymentReceipt, JobSpec, NewJob},
    payments::{self, PaymentReceipt, ProvidedPayment},
    pricing::{PricingInfo, ServiceId},
    protocol::{
        self, ARTIFACT_KIND_DEAL, ARTIFACT_KIND_DESCRIPTOR, ARTIFACT_KIND_OFFER,
        ARTIFACT_KIND_QUOTE, ARTIFACT_KIND_RECEIPT, DealPayload, DescriptorPayload, FeedDescriptor,
        InvoiceBundleLegState, InvoiceBundlePayload, OfferConstraints, OfferPayload, QuotePayload,
        QuoteSettlementTerms, ReceiptExecutor, ReceiptFailure, ReceiptLimitsApplied,
        ReceiptPayload, ReceiptSettlement, SettlementDescriptor, SettlementStatus, SignedArtifact,
        TransportEndpoints, WorkloadSpec,
    },
    sandbox, settlement,
    state::AppState,
    wasm::{self, WasmSubmission},
};

#[derive(Debug, Serialize)]
pub struct NodeCapabilities {
    pub api_version: String,
    pub version: String,
    pub identity: IdentityInfo,
    pub discovery: DiscoveryInfo,
    pub marketplace: MarketplaceInfo,
    pub transports: TransportsInfo,
    pub execution: ExecutionInfo,
    pub limits: LimitsInfo,
    pub pricing: PricingInfo,
    pub payments: PaymentsInfo,
    pub faas: FaaSInfo,
}

#[derive(Debug, Serialize)]
pub struct IdentityInfo {
    pub node_id: String,
    pub public_key: String,
}

#[derive(Debug, Serialize)]
pub struct DiscoveryInfo {
    pub mode: String,
}

#[derive(Debug, Serialize)]
pub struct MarketplaceInfo {
    pub enabled: bool,
    pub publish_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_register_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TransportsInfo {
    pub clearnet: ClearnetInfo,
    pub tor: TorInfo,
}

#[derive(Debug, Serialize)]
pub struct ClearnetInfo {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TorInfo {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onion_url: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ExecutionInfo {
    pub wasm: WasmInfo,
}

#[derive(Debug, Serialize)]
pub struct WasmInfo {
    pub enabled: bool,
    pub fuel_limit: u64,
    pub entrypoints: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LimitsInfo {
    pub events_query_limit_default: usize,
    pub events_query_limit_max: usize,
    pub body_limit_bytes: usize,
    pub wasm_hex_limit_bytes: usize,
    pub wasm_input_limit_bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct PaymentsInfo {
    pub ecash_verification: bool,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifier_mode: Option<String>,
    pub accepted_payment_methods: Vec<String>,
    pub reservations: bool,
    pub receipts: bool,
}

#[derive(Debug, Serialize)]
pub struct FaaSInfo {
    pub jobs_api: bool,
    pub async_jobs: bool,
    pub idempotency_keys: bool,
    pub runtimes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteWasmRequest {
    pub submission: WasmSubmission,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyEcashRequest {
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeEventEnvelope {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: String,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

impl NodeEventEnvelope {
    pub fn canonical_signing_bytes(&self) -> Vec<u8> {
        json!([
            self.id,
            self.pubkey,
            self.created_at,
            self.kind,
            self.tags,
            self.content
        ])
        .to_string()
        .into_bytes()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishRequest {
    pub event: NodeEventEnvelope,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryRequest {
    pub kinds: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateJobRequest {
    #[serde(flatten)]
    pub spec: JobSpec,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateQuoteRequest {
    pub offer_id: String,
    #[serde(flatten)]
    pub spec: WorkloadSpec,
    #[serde(default)]
    pub max_price_sats: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateDealRequest {
    pub quote: SignedArtifact<QuotePayload>,
    #[serde(flatten)]
    pub spec: WorkloadSpec,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
    #[serde(default)]
    pub requester_id: Option<String>,
    #[serde(default)]
    pub success_payment_hash: Option<String>,
}

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

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyReceiptRequest {
    pub receipt: SignedArtifact<ReceiptPayload>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyInvoiceBundleRequest {
    pub bundle: SignedArtifact<InvoiceBundlePayload>,
    pub quote: SignedArtifact<QuotePayload>,
    pub deal: SignedArtifact<DealPayload>,
    #[serde(default)]
    pub requester_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VerifyInvoiceBundleResponse {
    pub valid: bool,
    pub bundle_hash: String,
    pub quote_hash: String,
    pub deal_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_requester_id: Option<String>,
    pub issues: Vec<settlement::InvoiceBundleValidationIssue>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeWalletBalanceResponse {
    pub backend: String,
    pub mode: String,
    pub balance_known: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_sats: Option<u64>,
    pub accepted_payment_methods: Vec<String>,
    pub reservations: bool,
    pub receipts: bool,
}

#[derive(Debug, Serialize)]
pub struct RuntimeProviderResponse {
    pub status: String,
    pub descriptor: SignedArtifact<DescriptorPayload>,
    pub offers: Vec<SignedArtifact<OfferPayload>>,
    pub runtime_auth: RuntimeAuthInfo,
}

#[derive(Debug, Serialize)]
pub struct RuntimeAuthInfo {
    pub scheme: String,
    pub token_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeBuyServiceRequest {
    pub offer_id: String,
    #[serde(flatten)]
    pub spec: WorkloadSpec,
    #[serde(default)]
    pub max_price_sats: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
    #[serde(default)]
    pub requester_id: Option<String>,
    #[serde(default)]
    pub success_payment_hash: Option<String>,
    #[serde(default)]
    pub wait_for_receipt: bool,
    #[serde(default)]
    pub wait_timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeBuyServiceResponse {
    pub quote: SignedArtifact<QuotePayload>,
    pub deal: deals::DealRecord,
    pub terminal: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateLightningInvoiceBundleStateRequest {
    pub base_state: InvoiceBundleLegState,
    pub success_state: InvoiceBundleLegState,
}

#[derive(Debug, Serialize)]
pub struct RuntimeArchiveExportResponse {
    pub schema_version: String,
    pub export_type: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub exported_at: i64,
    pub artifact_documents: Vec<db::ArtifactDocumentRecord>,
    pub artifact_feed: Vec<db::ArtifactFeedEntryRecord>,
    pub execution_evidence: Vec<db::ExecutionEvidenceRecord>,
    pub lightning_invoice_bundles: Vec<db::LightningInvoiceBundleRecord>,
}

const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_EVENT_CONTENT_BYTES: usize = 64 * 1024;
const MAX_WASM_HEX_BYTES: usize = 512 * 1024;
const MAX_WASM_INPUT_BYTES: usize = 128 * 1024;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
type ApiFailure = (StatusCode, serde_json::Value);

pub fn router(state: Arc<AppState>) -> Router {
    let publish_routes = Router::new()
        .route("/v1/node/events/publish", post(publish_event))
        .route_layer(ConcurrencyLimitLayer::new(32));

    let exec_routes = Router::new()
        .route("/v1/node/execute/wasm", post(execute_wasm))
        .route("/v1/node/jobs", post(create_job))
        .route("/v1/node/jobs/:job_id", get(get_job_status))
        .route_layer(ConcurrencyLimitLayer::new(16));

    let protocol_routes = Router::new()
        .route("/v1/descriptor", get(protocol_descriptor))
        .route("/v1/offers", get(list_offers))
        .route("/v1/feed", get(get_feed))
        .route("/v1/artifacts/:artifact_hash", get(get_artifact))
        .route("/v1/quotes", post(create_quote))
        .route("/v1/deals", post(create_deal))
        .route("/v1/deals/:deal_id", get(get_deal_status))
        .route(
            "/v1/deals/:deal_id/invoice-bundle",
            get(get_deal_invoice_bundle),
        )
        .route("/v1/invoice-bundles/verify", post(verify_invoice_bundle))
        .route("/v1/receipts/verify", post(verify_receipt))
        .route_layer(ConcurrencyLimitLayer::new(16));

    let runtime_routes = Router::new()
        .route("/v1/runtime/wallet/balance", get(runtime_wallet_balance))
        .route("/v1/runtime/provider/start", post(runtime_provider_start))
        .route(
            "/v1/runtime/services/publish",
            post(runtime_services_publish),
        )
        .route("/v1/runtime/services/buy", post(runtime_services_buy))
        .route(
            "/v1/runtime/archive/:subject_kind/:subject_id",
            get(runtime_archive_subject),
        )
        .route(
            "/v1/runtime/lightning/invoice-bundles/:session_id/state",
            post(runtime_update_lightning_bundle_state),
        )
        .route_layer(ConcurrencyLimitLayer::new(16));

    Router::new()
        .route("/health", get(health_check))
        .route("/v1/node/capabilities", get(node_capabilities))
        .route("/v1/node/identity", get(node_identity))
        .route("/v1/node/events/query", post(query_events))
        .route("/v1/node/pay/ecash", post(verify_ecash))
        .merge(runtime_routes)
        .merge(protocol_routes)
        .merge(publish_routes)
        .merge(exec_routes)
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(10))),
        )
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::SERVER,
            HeaderValue::from_static("nginx/1.18.0"),
        ))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::DATE,
            HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
        ))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

pub async fn health_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({"status": "ok", "service": "froglet"})),
    )
}

pub async fn node_capabilities(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let transport_status = state.transport_status.lock().await.clone();
    let marketplace_status = state.marketplace_status.lock().await.clone();
    let settlement_descriptor = settlement::driver_descriptor(state.as_ref());

    let capabilities = NodeCapabilities {
        api_version: "v1".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        identity: IdentityInfo {
            node_id: state.identity.node_id().to_string(),
            public_key: state.identity.public_key_hex().to_string(),
        },
        discovery: DiscoveryInfo {
            mode: state.config.discovery_mode.to_string(),
        },
        marketplace: MarketplaceInfo {
            enabled: state.config.marketplace.is_some(),
            publish_enabled: marketplace_status.publish_enabled,
            url: state
                .config
                .marketplace
                .as_ref()
                .map(|marketplace| marketplace.url.clone()),
            connected: marketplace_status.connected,
            last_register_at: marketplace_status.last_register_at,
            last_heartbeat_at: marketplace_status.last_heartbeat_at,
            last_error: marketplace_status.last_error,
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
            ecash_verification: true,
            backend: settlement_descriptor.backend,
            verifier_mode: (settlement_descriptor.mode != "disabled")
                .then(|| settlement_descriptor.mode.clone()),
            accepted_payment_methods: settlement_descriptor.accepted_payment_methods,
            reservations: settlement_descriptor.reservations,
            receipts: settlement_descriptor.receipts,
        },
        faas: FaaSInfo {
            jobs_api: true,
            async_jobs: true,
            idempotency_keys: true,
            runtimes: vec!["wasm".to_string()],
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

pub async fn runtime_provider_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    match runtime_provider_snapshot(state.as_ref()).await {
        Ok(snapshot) => (StatusCode::OK, Json(json!(snapshot))),
        Err(error) => error_json(error.0, error.1),
    }
}

pub async fn runtime_services_publish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    match runtime_provider_snapshot(state.as_ref()).await {
        Ok(snapshot) => (StatusCode::OK, Json(json!(snapshot))),
        Err(error) => error_json(error.0, error.1),
    }
}

pub async fn runtime_services_buy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<RuntimeBuyServiceRequest>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    let wait_for_receipt = payload.wait_for_receipt;
    let wait_timeout_secs = payload.wait_timeout_secs.unwrap_or(15).clamp(1, 60);

    if let Err(response) = validate_workload_spec(&payload.spec) {
        return response;
    }

    if let Some(existing) =
        match find_existing_deal(state.as_ref(), payload.idempotency_key.clone()).await {
            Ok(existing) => existing,
            Err(error) => return error_json(error.0, error.1.0),
        }
    {
        let workload_hash = match payload.spec.request_hash() {
            Ok(hash) => hash,
            Err(error) => {
                return error_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to hash workload: {error}") }),
                );
            }
        };

        if existing.artifact.payload.offer_id != payload.offer_id
            || existing.artifact.payload.workload_hash != workload_hash
        {
            return error_json(
                StatusCode::CONFLICT,
                json!({ "error": "idempotency key reused with different service request" }),
            );
        }

        let mut deal = existing.public_record();
        let mut terminal = matches!(
            deal.status.as_str(),
            deals::DEAL_STATUS_SUCCEEDED | deals::DEAL_STATUS_FAILED | deals::DEAL_STATUS_REJECTED
        );
        if wait_for_receipt && !terminal && deal.status != deals::DEAL_STATUS_PAYMENT_PENDING {
            match wait_for_terminal_deal(state.clone(), &deal.deal_id, wait_timeout_secs).await {
                Ok(terminal_deal) => {
                    deal = terminal_deal;
                    terminal = true;
                }
                Err(error) => return error_json(error.0, error.1),
            }
        }

        return (
            StatusCode::OK,
            Json(json!(RuntimeBuyServiceResponse {
                quote: existing.quote,
                deal,
                terminal,
            })),
        );
    }

    let quote = match create_quote_record(
        state.clone(),
        CreateQuoteRequest {
            offer_id: payload.offer_id,
            spec: payload.spec.clone(),
            max_price_sats: payload.max_price_sats,
        },
    )
    .await
    {
        Ok(quote) => quote,
        Err(error) => return error_json(error.0, error.1),
    };

    let (mut deal, _) = match create_deal_record(
        state.clone(),
        CreateDealRequest {
            quote: quote.clone(),
            spec: payload.spec,
            idempotency_key: payload.idempotency_key,
            payment: payload.payment,
            requester_id: payload.requester_id,
            success_payment_hash: payload.success_payment_hash,
        },
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return error_json(error.0, error.1),
    };

    let mut terminal = false;
    if wait_for_receipt && deal.status != deals::DEAL_STATUS_PAYMENT_PENDING {
        match wait_for_terminal_deal(state.clone(), &deal.deal_id, wait_timeout_secs).await {
            Ok(terminal_deal) => {
                deal = terminal_deal;
                terminal = true;
            }
            Err(error) => return error_json(error.0, error.1),
        }
    }

    (
        StatusCode::OK,
        Json(json!(RuntimeBuyServiceResponse {
            quote,
            deal,
            terminal,
        })),
    )
}

pub async fn runtime_update_lightning_bundle_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(payload): Json<UpdateLightningInvoiceBundleStateRequest>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    let updated = match settlement::update_lightning_invoice_bundle_states(
        state.as_ref(),
        &session_id,
        payload.base_state.clone(),
        payload.success_state.clone(),
    )
    .await
    {
        Ok(Some(updated)) => updated,
        Ok(None) => {
            return error_json(
                StatusCode::NOT_FOUND,
                json!({ "error": "invoice bundle not found", "session_id": session_id }),
            );
        }
        Err(error) => {
            tracing::error!("Failed to update invoice bundle {session_id}: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to update invoice bundle" }),
            );
        }
    };

    if payload.base_state == InvoiceBundleLegState::Settled
        && matches!(
            payload.success_state,
            InvoiceBundleLegState::Accepted | InvoiceBundleLegState::Settled
        )
    {
        let deal_hash = updated.bundle.payload.deal_hash.clone();
        match state
            .db
            .with_conn(move |conn| deals::get_deal_by_artifact_hash(conn, &deal_hash))
            .await
        {
            Ok(Some(deal)) => {
                let deal_id = deal.deal_id.clone();
                let deal_id_for_update = deal_id.clone();
                let promoted = state
                    .db
                    .with_conn(move |conn| {
                        deals::try_mark_deal_accepted_from_payment_pending(
                            conn,
                            &deal_id_for_update,
                            payments::current_unix_timestamp(),
                        )
                    })
                    .await;

                match promoted {
                    Ok(true) => {
                        tokio::spawn(process_deal(state.clone(), deal_id));
                    }
                    Ok(false) => {}
                    Err(error) => {
                        tracing::error!(
                            "Failed to promote Lightning deal {} after invoice update: {error}",
                            deal.deal_id
                        );
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(
                    "Failed to load deal for Lightning invoice bundle {}: {error}",
                    updated.session_id
                );
            }
        }
    }

    (StatusCode::OK, Json(json!(updated)))
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
            let status = if error.contains("unsupported archive subject kind") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            tracing::error!(
                "Failed to build archive export for {} {}: {error}",
                subject_kind,
                subject_id
            );
            error_json(status, json!({ "error": error }))
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

    match state
        .db
        .with_conn(move |conn| db::list_artifacts(conn, Some(applied_cursor), limit))
        .await
    {
        Ok((artifacts, has_more)) => {
            let next_cursor = artifacts.last().map(|artifact| artifact.cursor);
            (
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
            )
        }
        Err(error) => {
            tracing::error!("Failed to read feed: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        }
    }
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
        .with_conn(move |conn| db::get_artifact_by_hash(conn, &lookup_hash))
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
    match state
        .db
        .with_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
        .await
    {
        Ok(Some(deal)) => (StatusCode::OK, Json(json!(deal.public_record()))),
        Ok(None) => error_json(StatusCode::NOT_FOUND, json!({ "error": "deal not found" })),
        Err(error) => {
            tracing::error!("Failed to fetch deal {deal_id}: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        }
    }
}

pub async fn get_deal_invoice_bundle(
    State(state): State<Arc<AppState>>,
    Path(deal_id): Path<String>,
) -> impl IntoResponse {
    let lookup_deal_id = deal_id.clone();
    let deal = match state
        .db
        .with_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
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

    match deal_lightning_invoice_bundle(state.as_ref(), &deal).await {
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

pub async fn verify_receipt(Json(payload): Json<VerifyReceiptRequest>) -> impl IntoResponse {
    let valid = protocol::verify_artifact(&payload.receipt);
    (
        StatusCode::OK,
        Json(json!({
            "valid": valid,
            "receipt_hash": payload.receipt.hash,
            "status": payload.receipt.payload.status
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

    if event.content.as_bytes().len() > MAX_EVENT_CONTENT_BYTES {
        return error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({ "error": "event content too large" }),
        );
    }

    tracing::info!("Received Event Publish: {:?}", event.kind);

    if !crypto::verify_message(&event.pubkey, &event.sig, &event.canonical_signing_bytes()) {
        tracing::warn!("Invalid signature for event: {}", event.id);
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid signature" }),
        );
    }

    if let Err(e) = insert_event_db(state.as_ref(), event).await {
        tracing::error!("Failed to insert event: {}", e);
        return error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "database error" }),
        );
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
    tracing::info!("Received Event Query for Kinds: {:?}", payload.kinds);

    if let Some(response) = legacy_paid_endpoint_requires_protocol_deal(
        state.as_ref(),
        ServiceId::EventsQuery,
        "/v1/node/events/query",
    ) {
        return response;
    }

    let reservation = match payments::prepare_payment(
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

    let response = match query_events_db(state.as_ref(), payload.kinds, payload.limit).await {
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
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        }
    };

    response
}

pub async fn execute_wasm(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ExecuteWasmRequest>,
) -> impl IntoResponse {
    tracing::info!("Received Wasm Execution Request");

    if let Err(response) = validate_wasm_submission(&payload.submission) {
        return response;
    }

    if let Some(response) = legacy_paid_endpoint_requires_protocol_deal(
        state.as_ref(),
        ServiceId::ExecuteWasm,
        "/v1/node/execute/wasm",
    ) {
        return response;
    }

    let reservation = match payments::prepare_payment(
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

pub async fn verify_ecash(Json(payload): Json<VerifyEcashRequest>) -> impl IntoResponse {
    tracing::info!("Received Ecash Verification Request");

    match ecash::inspect_cashu_token(&payload.token) {
        Ok(info) => (
            StatusCode::OK,
            Json(json!({
                "status": "success",
                "amount_satoshis": info.amount_satoshis,
                "token_hash": info.token_hash,
                "mint_url": info.mint_url,
                "proof_count": info.proof_ys.len(),
                "has_spend_conditions": info.has_spend_conditions,
                "p2pk_pubkeys": info.p2pk_pubkeys,
                "p2pk_refund_pubkeys": info.p2pk_refund_pubkeys,
            })),
        ),
        Err(e) => {
            tracing::error!("Ecash Verification Failed: {}", e);
            error_json(StatusCode::BAD_REQUEST, json!({ "error": e.to_string() }))
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
    let reservation = match payments::prepare_payment(
        state.as_ref(),
        service_id,
        payload.payment,
        Some(job_id.clone()),
    )
    .await
    {
        Ok(reservation) => reservation,
        Err(error) => return error_json(error.status_code(), error.details()),
    };

    let new_job = NewJob {
        job_id: job_id.clone(),
        idempotency_key: idempotency_key.clone(),
        request_hash,
        service_id: service_id.as_str().to_string(),
        spec: payload.spec,
        payment_token_hash: reservation
            .as_ref()
            .map(|payment| payment.token_hash.clone()),
        payment_amount_sats: reservation.as_ref().map(|payment| payment.amount_sats),
        created_at: payments::current_unix_timestamp(),
    };

    let insert_outcome = match state
        .db
        .with_conn(move |conn| {
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
            let _ = release_payment(state.as_ref(), reservation).await;
            let status = if error.contains("idempotency key reused") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return error_json(status, json!({ "error": error }));
        }
    };

    if !insert_outcome.created {
        let _ = release_payment(state.as_ref(), reservation).await;
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
        .with_conn(move |conn| jobs::get_job(conn, &lookup_job_id))
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

fn error_json(
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
    if state.config.payment_backend != PaymentBackend::Lightning || price_sats == 0 {
        return None;
    }

    Some(error_json(
        StatusCode::CONFLICT,
        json!({
            "error": format!(
                "priced {} requests must use /v1/quotes and /v1/deals when the lightning backend is active",
                service_id.as_str()
            ),
            "service_id": service_id.as_str(),
            "price_sats": price_sats,
            "payment_backend": "lightning",
            "legacy_endpoint": endpoint_path,
            "quote_path": "/v1/quotes",
            "deal_path": "/v1/deals",
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

async fn insert_event_db(state: &AppState, event: NodeEventEnvelope) -> Result<(), String> {
    state
        .db
        .with_conn(move |conn| db::insert_event(conn, &event))
        .await
}

async fn query_events_db(
    state: &AppState,
    kinds: Vec<String>,
    limit: Option<usize>,
) -> Result<Vec<NodeEventEnvelope>, String> {
    state
        .db
        .with_conn(move |conn| db::query_events_by_kind(conn, &kinds, limit))
        .await
}

async fn ensure_protocol_root_artifacts(state: &AppState) -> Result<(), String> {
    current_descriptor_artifact(state).await?;
    current_offer_artifacts(state).await?;
    Ok(())
}

async fn current_descriptor_artifact(
    state: &AppState,
) -> Result<SignedArtifact<DescriptorPayload>, String> {
    let transport_status = state.transport_status.lock().await.clone();
    let settlement_descriptor = settlement::driver_descriptor(state);

    persist_signed_artifact(
        state,
        ARTIFACT_KIND_DESCRIPTOR,
        DescriptorPayload {
            protocol_version: "v0.2".to_string(),
            node_id: state.identity.node_id().to_string(),
            public_key: state.identity.public_key_hex().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            discovery_mode: state.config.discovery_mode.to_string(),
            transports: TransportEndpoints {
                clearnet_url: transport_status.clearnet_url,
                onion_url: transport_status.tor_onion_url,
                tor_status: transport_status.tor_status,
            },
            settlement: SettlementDescriptor {
                methods: settlement_descriptor.accepted_payment_methods,
                reservations: settlement_descriptor.reservations,
                receipts: settlement_descriptor.receipts,
            },
            feeds: FeedDescriptor {
                pull_api: true,
                cursor_type: "artifact_sequence".to_string(),
                cursor_semantics: "exclusive_after".to_string(),
                feed_path: "/v1/feed".to_string(),
                artifact_path_template: "/v1/artifacts/{artifact_hash}".to_string(),
                max_page_size: 100,
                artifact_kinds: vec![
                    ARTIFACT_KIND_DESCRIPTOR.to_string(),
                    ARTIFACT_KIND_OFFER.to_string(),
                    ARTIFACT_KIND_QUOTE.to_string(),
                    ARTIFACT_KIND_DEAL.to_string(),
                    ARTIFACT_KIND_RECEIPT.to_string(),
                ],
            },
            runtimes: vec!["wasm".to_string()],
        },
    )
    .await
}

async fn current_offer_artifacts(
    state: &AppState,
) -> Result<Vec<SignedArtifact<OfferPayload>>, String> {
    let mut offers = Vec::new();
    for payload in current_offer_payloads(state) {
        offers.push(persist_signed_artifact(state, ARTIFACT_KIND_OFFER, payload).await?);
    }
    Ok(offers)
}

async fn lookup_offer(
    state: &AppState,
    offer_id: &str,
) -> Result<Option<SignedArtifact<OfferPayload>>, String> {
    let offers = current_offer_artifacts(state).await?;
    Ok(offers
        .into_iter()
        .find(|offer| offer.payload.offer_id == offer_id))
}

fn current_offer_payloads(state: &AppState) -> Vec<OfferPayload> {
    let settlement_methods = accepted_payment_methods(state);
    let execution_timeout_secs = state.config.execution_timeout_secs;
    let priced_offer = |service_id: ServiceId,
                        resource_kind: &str,
                        runtime: Option<&str>,
                        constraints: OfferConstraints| {
        let price_sats = state.pricing.price_for(service_id);
        OfferPayload {
            offer_id: service_id.as_str().to_string(),
            service_id: service_id.as_str().to_string(),
            resource_kind: resource_kind.to_string(),
            runtime: runtime.map(|runtime| runtime.to_string()),
            price_sats,
            payment_required: price_sats > 0,
            payment_methods: if price_sats > 0 {
                settlement_methods.clone()
            } else {
                Vec::new()
            },
            constraints,
            expires_at: None,
        }
    };

    vec![
        priced_offer(
            ServiceId::EventsQuery,
            "data",
            None,
            OfferConstraints {
                max_body_bytes: Some(MAX_BODY_BYTES),
                max_query_limit: Some(500),
                timeout_secs: Some(execution_timeout_secs),
            },
        ),
        priced_offer(
            ServiceId::ExecuteWasm,
            "compute",
            Some("wasm"),
            OfferConstraints {
                max_body_bytes: Some(MAX_WASM_HEX_BYTES),
                max_query_limit: None,
                timeout_secs: Some(execution_timeout_secs),
            },
        ),
    ]
}

fn accepted_payment_methods(state: &AppState) -> Vec<String> {
    settlement::accepted_payment_methods(state)
}

fn quoted_settlement_terms(state: &AppState, price_sats: u64) -> Option<QuoteSettlementTerms> {
    settlement::quoted_lightning_settlement_terms(state, price_sats)
}

fn settlement_quote_expires_at(state: &AppState, created_at: i64, price_sats: u64) -> i64 {
    settlement::lightning_quote_expires_at(state, created_at, price_sats)
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
    let created_at = payments::current_unix_timestamp();
    let artifact = sign_node_artifact(state, kind, created_at, payload)?;
    let actor_id = artifact.actor_id.clone();
    let kind = artifact.kind.clone();
    let payload_hash = artifact.payload_hash.clone();
    let artifact_hash = artifact.hash.clone();
    let document_json = serde_json::to_string(&artifact).map_err(|e| e.to_string())?;

    let stored = state
        .db
        .with_conn(move |conn| {
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
        .with_conn(move |conn| jobs::find_job_by_idempotency_key(conn, &idempotency_key))
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
        .with_conn(move |conn| deals::find_deal_by_idempotency_key(conn, &idempotency_key))
        .await
        .map_err(|error| {
            tracing::error!("Failed to look up idempotent deal: {error}");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "database error" }),
            )
        })
}

fn require_runtime_auth(headers: &HeaderMap, state: &AppState) -> Result<(), ApiFailure> {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Err((
            StatusCode::UNAUTHORIZED,
            json!({ "error": "missing runtime authorization" }),
        ));
    };

    let Ok(value) = value.to_str() else {
        return Err((
            StatusCode::UNAUTHORIZED,
            json!({ "error": "invalid runtime authorization header" }),
        ));
    };

    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err((
            StatusCode::UNAUTHORIZED,
            json!({ "error": "runtime authorization must use bearer auth" }),
        ));
    };

    if token != state.runtime_auth_token {
        return Err((
            StatusCode::UNAUTHORIZED,
            json!({ "error": "invalid runtime authorization token" }),
        ));
    }

    Ok(())
}

async fn runtime_provider_snapshot(
    state: &AppState,
) -> Result<RuntimeProviderResponse, ApiFailure> {
    let descriptor = current_descriptor_artifact(state).await.map_err(|error| {
        tracing::error!("Failed to build runtime descriptor snapshot: {error}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to build descriptor" }),
        )
    })?;

    let offers = current_offer_artifacts(state).await.map_err(|error| {
        tracing::error!("Failed to build runtime offer snapshot: {error}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to build offers" }),
        )
    })?;

    Ok(RuntimeProviderResponse {
        status: "running".to_string(),
        descriptor,
        offers,
        runtime_auth: RuntimeAuthInfo {
            scheme: "bearer".to_string(),
            token_path: state.runtime_auth_token_path.display().to_string(),
        },
    })
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

    let service_id = payload.spec.service_id();
    if offer.payload.service_id != service_id.as_str() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "offer does not match workload service",
                "offer_service_id": offer.payload.service_id,
                "requested_service_id": service_id.as_str(),
            }),
        ));
    }

    if offer.payload.resource_kind != payload.spec.resource_kind() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "offer does not match workload resource kind",
                "offer_resource_kind": offer.payload.resource_kind,
                "requested_resource_kind": payload.spec.resource_kind(),
            }),
        ));
    }

    if offer.payload.runtime.as_deref() != payload.spec.runtime() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "offer does not match workload runtime",
                "offer_runtime": offer.payload.runtime,
                "requested_runtime": payload.spec.runtime(),
            }),
        ));
    }

    if let Some(max_price_sats) = payload.max_price_sats {
        if offer.payload.price_sats > max_price_sats {
            return Err((
                StatusCode::CONFLICT,
                json!({
                    "error": "offer price exceeds max_price_sats",
                    "price_sats": offer.payload.price_sats,
                    "max_price_sats": max_price_sats,
                }),
            ));
        }
    }

    let workload_hash = payload.spec.request_hash().map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to hash workload: {error}") }),
        )
    })?;
    let created_at = payments::current_unix_timestamp();
    let settlement_terms = quoted_settlement_terms(state.as_ref(), offer.payload.price_sats);
    let quote_expires_at =
        settlement_quote_expires_at(state.as_ref(), created_at, offer.payload.price_sats);
    let quote = sign_node_artifact(
        state.as_ref(),
        ARTIFACT_KIND_QUOTE,
        created_at,
        QuotePayload {
            quote_id: protocol::new_artifact_id(),
            offer_id: offer.payload.offer_id.clone(),
            service_id: offer.payload.service_id.clone(),
            workload_kind: payload.spec.workload_kind().to_string(),
            workload_hash,
            price_sats: offer.payload.price_sats,
            payment_method: offer
                .payload
                .payment_required
                .then(|| offer.payload.payment_methods.first().cloned())
                .flatten(),
            settlement_terms,
            expires_at: quote_expires_at,
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
    let actor_id = quote.actor_id.clone();
    let quote_kind = quote.kind.clone();
    let persisted = state
        .db
        .with_conn(move |conn| {
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
                if deals::get_quote(conn, &quote_for_db.payload.quote_id)?.is_none() {
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
        tracing::error!(
            "Failed to persist quote {}: {error}",
            quote.payload.quote_id
        );
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
    let service_id = payload.spec.service_id();

    if !protocol::verify_artifact(&payload.quote) {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid quote signature" }),
        ));
    }

    if payload.quote.actor_id != state.identity.node_id() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "quote was not issued by this provider" }),
        ));
    }

    if payload.quote.payload.service_id != service_id.as_str() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "quote does not match workload service",
                "quote_service_id": payload.quote.payload.service_id,
                "requested_service_id": service_id.as_str(),
            }),
        ));
    }

    if payload.quote.payload.workload_hash != workload_hash {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "quote does not match workload payload" }),
        ));
    }

    let now = payments::current_unix_timestamp();
    if payload.quote.payload.expires_at < now {
        return Err((
            StatusCode::GONE,
            json!({
                "error": "quote expired",
                "quote_id": payload.quote.payload.quote_id,
                "expires_at": payload.quote.payload.expires_at,
            }),
        ));
    }

    if let Some(existing) = find_existing_deal(state.as_ref(), idempotency_key.clone())
        .await
        .map_err(|response| (response.0, response.1.0))?
    {
        if existing.quote.hash != payload.quote.hash
            || existing.artifact.payload.workload_hash != workload_hash
        {
            return Err((
                StatusCode::CONFLICT,
                json!({ "error": "idempotency key reused with different deal payload" }),
            ));
        }

        return Ok((existing.public_record(), StatusCode::OK));
    }

    let uses_lightning_bundle = state.config.payment_backend == PaymentBackend::Lightning
        && payload.quote.payload.price_sats > 0;
    let lightning_terms = if uses_lightning_bundle {
        if payload.quote.payload.payment_method.as_deref() != Some("lightning") {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({ "error": "lightning-backed quotes must advertise payment_method=lightning" }),
            ));
        }
        match payload.quote.payload.settlement_terms.clone() {
            Some(terms) => Some(terms),
            None => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "lightning-backed quotes must include settlement_terms" }),
                ));
            }
        }
    } else {
        None
    };
    if uses_lightning_bundle && payload.payment.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "lightning-backed deals use invoice bundles instead of inline payment tokens"
            }),
        ));
    }

    let requester_id = if uses_lightning_bundle {
        let requester_id = payload.requester_id.clone().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                json!({ "error": "requester_id is required for lightning-backed deals" }),
            )
        })?;
        Some(
            normalize_hex_field("requester_id", requester_id, 64)
                .map_err(|response| (response.0, response.1.0))?,
        )
    } else {
        None
    };
    let success_payment_hash = if uses_lightning_bundle {
        let success_payment_hash = payload.success_payment_hash.clone().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                json!({ "error": "success_payment_hash is required for lightning-backed deals" }),
            )
        })?;
        Some(
            normalize_hex_field("success_payment_hash", success_payment_hash, 64)
                .map_err(|response| (response.0, response.1.0))?,
        )
    } else {
        None
    };

    let deal_id = protocol::new_artifact_id();
    let reservation = if uses_lightning_bundle {
        None
    } else {
        payments::prepare_payment_for_amount(
            state.as_ref(),
            service_id,
            payload.quote.payload.price_sats,
            payload.payment.clone(),
            Some(deal_id.clone()),
        )
        .await
        .map_err(|error| (error.status_code(), error.details()))?
    };

    let payment_lock = if let Some(success_payment_hash) = success_payment_hash.as_ref() {
        Some(protocol::PaymentLock {
            kind: "lightning".to_string(),
            token_hash: success_payment_hash.clone(),
            amount_sats: payload.quote.payload.price_sats,
        })
    } else {
        reservation
            .as_ref()
            .map(|reservation| protocol::PaymentLock {
                kind: reservation.method.clone(),
                token_hash: reservation.token_hash.clone(),
                amount_sats: reservation.amount_sats,
            })
    };

    let deal_artifact = sign_node_artifact(
        state.as_ref(),
        ARTIFACT_KIND_DEAL,
        now,
        DealPayload {
            deal_id: deal_id.clone(),
            quote_id: payload.quote.payload.quote_id.clone(),
            offer_id: payload.quote.payload.offer_id.clone(),
            service_id: payload.quote.payload.service_id.clone(),
            workload_hash: workload_hash.clone(),
            payment_lock,
            idempotency_key: idempotency_key.clone(),
            deadline: payload.quote.payload.expires_at,
        },
    )
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to sign deal: {error}") }),
        )
    })?;

    let invoice_bundle_session = if uses_lightning_bundle {
        Some(
            settlement::build_lightning_invoice_bundle(
                state.as_ref(),
                settlement::BuildLightningInvoiceBundleRequest {
                    session_id: None,
                    requester_id: requester_id.clone().expect("validated requester_id"),
                    quote_hash: payload.quote.hash.clone(),
                    deal_hash: deal_artifact.hash.clone(),
                    success_payment_hash: success_payment_hash
                        .clone()
                        .expect("validated success_payment_hash"),
                    base_fee_msat: lightning_terms
                        .as_ref()
                        .expect("validated lightning terms")
                        .base_fee_msat,
                    success_fee_msat: lightning_terms
                        .as_ref()
                        .expect("validated lightning terms")
                        .success_fee_msat,
                    created_at: now,
                },
            )
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to build lightning invoice bundle: {error}") }),
                )
            })?,
        )
    } else {
        None
    };

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

    let deal_for_db = NewDeal {
        deal_id: deal_id.clone(),
        idempotency_key: idempotency_key.clone(),
        quote: payload.quote.clone(),
        spec: payload.spec.clone(),
        artifact: deal_artifact.clone(),
        payment_method: if uses_lightning_bundle {
            Some("lightning".to_string())
        } else {
            reservation.as_ref().map(|payment| payment.method.clone())
        },
        payment_token_hash: success_payment_hash.clone().or_else(|| {
            reservation
                .as_ref()
                .map(|payment| payment.token_hash.clone())
        }),
        payment_amount_sats: if uses_lightning_bundle {
            Some(payload.quote.payload.price_sats)
        } else {
            reservation.as_ref().map(|payment| payment.amount_sats)
        },
        initial_status: if uses_lightning_bundle {
            deals::DEAL_STATUS_PAYMENT_PENDING.to_string()
        } else {
            deals::DEAL_STATUS_ACCEPTED.to_string()
        },
        created_at: now,
    };

    let deal_hash = deal_artifact.hash.clone();
    let deal_payload_hash = deal_artifact.payload_hash.clone();
    let deal_actor_id = deal_artifact.actor_id.clone();
    let deal_artifact_hash = deal_artifact.hash.clone();
    let quote_hash = payload.quote.hash.clone();
    let quote_payload_hash = payload.quote.payload_hash.clone();
    let quote_actor_id = payload.quote.actor_id.clone();
    let quote_id = payload.quote.payload.quote_id.clone();
    let spec_for_evidence = payload.spec.clone();
    let quote_artifact_ref = json!({ "artifact_hash": quote_hash.clone() });
    let deal_artifact_ref = json!({ "artifact_hash": deal_hash.clone() });
    let invoice_bundle_session_for_db = invoice_bundle_session.clone();
    let insert_result = state
        .db
        .with_conn(move |conn| {
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
                        return Err("quote id already exists with different contents".to_string());
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
                    deals::set_deal_storage_refs(
                        conn,
                        &insert_outcome.deal.deal_id,
                        &workload_evidence_hash,
                        &deal_artifact_hash,
                    )?;
                    if let Some(invoice_bundle_session) = invoice_bundle_session_for_db.as_ref() {
                        db::insert_lightning_invoice_bundle(
                            conn,
                            &invoice_bundle_session.session_id,
                            &invoice_bundle_session.bundle,
                            invoice_bundle_session.base_state.clone(),
                            invoice_bundle_session.success_state.clone(),
                            invoice_bundle_session.created_at,
                        )?;
                        let _ = db::insert_execution_evidence(
                            conn,
                            "deal",
                            &insert_outcome.deal.deal_id,
                            "lightning_invoice_bundle_ref",
                            &json!({
                                "session_id": invoice_bundle_session.session_id,
                                "bundle_hash": invoice_bundle_session.bundle.hash,
                            }),
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

    if !uses_lightning_bundle {
        tokio::spawn(process_deal(state, deal_id));
    }
    Ok((insert_result.deal.public_record(), StatusCode::ACCEPTED))
}

async fn wait_for_terminal_deal(
    state: Arc<AppState>,
    deal_id: &str,
    timeout_secs: u64,
) -> Result<deals::DealRecord, ApiFailure> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        let lookup_deal_id = deal_id.to_string();
        let deal = state
            .db
            .with_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
            .await
            .map_err(|error| {
                tracing::error!("Failed to poll deal {deal_id}: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "database error" }),
                )
            })?;

        let Some(deal) = deal else {
            return Err((StatusCode::NOT_FOUND, json!({ "error": "deal not found" })));
        };

        if matches!(
            deal.status.as_str(),
            deals::DEAL_STATUS_SUCCEEDED | deals::DEAL_STATUS_FAILED | deals::DEAL_STATUS_REJECTED
        ) {
            return Ok(deal.public_record());
        }

        if tokio::time::Instant::now() >= deadline {
            return Err((
                StatusCode::REQUEST_TIMEOUT,
                json!({ "error": "timed out waiting for terminal deal status", "deal_id": deal_id }),
            ));
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn validate_job_spec(spec: &JobSpec) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match spec {
        JobSpec::Wasm { submission } => validate_wasm_submission(submission),
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

fn validate_workload_spec(
    spec: &WorkloadSpec,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match spec {
        WorkloadSpec::Wasm { submission } => validate_wasm_submission(submission),
        WorkloadSpec::EventsQuery { kinds, limit } if kinds.is_empty() => Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "events query must include at least one kind" }),
        )),
        WorkloadSpec::EventsQuery {
            limit: Some(limit), ..
        } if *limit > 500 => Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "events query limit exceeds maximum", "max_limit": 500 }),
        )),
        _ => Ok(()),
    }
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

async fn deal_lightning_invoice_bundle(
    state: &AppState,
    deal: &deals::StoredDeal,
) -> Result<Option<settlement::LightningInvoiceBundleSession>, String> {
    if deal.payment_method.as_deref() != Some("lightning") {
        return Ok(None);
    }

    settlement::get_lightning_invoice_bundle_by_deal_hash(state, &deal.artifact.hash).await
}

async fn update_deal_lightning_bundle_state(
    state: &AppState,
    deal: &deals::StoredDeal,
    success_state: InvoiceBundleLegState,
) -> Result<Option<settlement::LightningInvoiceBundleSession>, String> {
    let Some(bundle) = deal_lightning_invoice_bundle(state, deal).await? else {
        return Ok(None);
    };

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
        .with_conn(move |conn| match subject_kind_owned.as_str() {
            "deal" => {
                let Some(deal) = deals::get_deal(conn, &subject_id_owned)? else {
                    return Ok(None);
                };
                Ok(Some(ArchiveSubject::Deal {
                    artifact_hashes: collect_archive_artifact_hashes_for_deal(&deal),
                    deal_hash: deal.artifact.hash,
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
                .with_conn(
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
                .with_conn(move |conn| {
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
        .with_conn(move |conn| {
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
        exported_at: payments::current_unix_timestamp(),
        artifact_documents,
        artifact_feed,
        execution_evidence,
        lightning_invoice_bundles,
    }))
}

async fn finalize_payment(
    state: &AppState,
    reservation: Option<payments::PaymentReservation>,
) -> Result<Option<PaymentReceipt>, (StatusCode, Json<serde_json::Value>)> {
    match reservation {
        Some(reservation) => payments::commit_payment(state, reservation)
            .await
            .map(Some)
            .map_err(|error| error_json(error.status_code(), error.details())),
        None => Ok(None),
    }
}

async fn release_payment(
    state: &AppState,
    reservation: Option<payments::PaymentReservation>,
) -> Result<(), String> {
    match reservation {
        Some(reservation) => payments::release_payment(state, &reservation).await,
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

fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn receipt_executor_for_spec(spec: &WorkloadSpec) -> ReceiptExecutor {
    match spec {
        WorkloadSpec::Wasm { submission } => ReceiptExecutor {
            runtime: "wasm".to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            abi_version: Some(submission.workload.abi_version.clone()),
            module_hash: Some(submission.workload.module_hash.clone()),
            capabilities_granted: Vec::new(),
        },
        WorkloadSpec::EventsQuery { .. } => ReceiptExecutor {
            runtime: "builtin.events_query".to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            abi_version: None,
            module_hash: None,
            capabilities_granted: Vec::new(),
        },
    }
}

fn receipt_limits_for_spec(state: &AppState, spec: &WorkloadSpec) -> ReceiptLimitsApplied {
    match spec {
        WorkloadSpec::Wasm { .. } => ReceiptLimitsApplied {
            max_input_bytes: Some(MAX_WASM_INPUT_BYTES),
            max_runtime_ms: Some(duration_millis_u64(execution_timeout(state))),
            max_memory_bytes: Some(sandbox::WASM_MAX_MEMORY_BYTES),
            max_output_bytes: Some(sandbox::WASM_MAX_OUTPUT_BYTES),
            fuel_limit: Some(sandbox::WASM_FUEL_LIMIT),
        },
        WorkloadSpec::EventsQuery { .. } => ReceiptLimitsApplied {
            max_input_bytes: None,
            max_runtime_ms: Some(duration_millis_u64(execution_timeout(state))),
            max_memory_bytes: None,
            max_output_bytes: None,
            fuel_limit: None,
        },
    }
}

fn receipt_settlement_from_deal(
    deal: &deals::StoredDeal,
    settlement_status: SettlementStatus,
    committed_amount_sats: u64,
    settlement_reference: Option<String>,
) -> Option<ReceiptSettlement> {
    deal.payment_lock().map(|payment_lock| ReceiptSettlement {
        method: payment_lock.kind.clone(),
        status: Some(settlement_status),
        reserved_amount_sats: payment_lock.amount_sats,
        committed_amount_sats,
        payment_lock,
        settlement_reference,
    })
}

fn receipt_failure(code: &str, message: impl Into<String>) -> ReceiptFailure {
    ReceiptFailure {
        code: code.to_string(),
        message: message.into(),
    }
}

pub async fn recover_runtime_state(state: Arc<AppState>) -> Result<(), String> {
    let incomplete_deals = state.db.with_conn(deals::list_incomplete_deals).await?;
    let completed_at = payments::current_unix_timestamp();
    let deal_recovery_message = "node restarted before deal completion".to_string();
    let mut recovery_receipts = Vec::new();
    for deal in incomplete_deals {
        let settlement_reference = match update_deal_lightning_bundle_state(
            state.as_ref(),
            &deal,
            InvoiceBundleLegState::Expired,
        )
        .await?
        {
            Some(bundle) => Some(bundle.session_id),
            None => None,
        };

        let receipt = sign_deal_receipt(
            state.as_ref(),
            &deal,
            completed_at,
            deals::DEAL_STATUS_FAILED,
            Some(SettlementStatus::Expired),
            0,
            settlement_reference,
            None,
            Some(receipt_failure(
                "node_restarted",
                deal_recovery_message.clone(),
            )),
        )?;
        let receipt_json = serde_json::to_string(&receipt).map_err(|e| e.to_string())?;
        recovery_receipts.push((deal, receipt, receipt_json));
    }

    state
        .db
        .with_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<(), String> {
                let _ = db::expire_reserved_payment_tokens(conn, completed_at)?;
                jobs::fail_incomplete_jobs(
                    conn,
                    "node restarted before job completion",
                    completed_at,
                )?;

                for (deal, receipt, receipt_json) in &recovery_receipts {
                    let failure_evidence_hash = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &deal.deal_id,
                        "execution_failure",
                        &json!({
                            "code": "node_restarted",
                            "message": deal_recovery_message.clone(),
                        }),
                        completed_at,
                    )?;
                    deals::complete_deal_failure(
                        conn,
                        &deal.deal_id,
                        &deal_recovery_message,
                        receipt,
                        Some(&failure_evidence_hash),
                        Some(&receipt.hash),
                        completed_at,
                    )?;
                    db::insert_artifact_document(
                        conn,
                        &receipt.hash,
                        &receipt.payload_hash,
                        ARTIFACT_KIND_RECEIPT,
                        &receipt.actor_id,
                        receipt.created_at,
                        receipt_json,
                    )?;
                    let _ = db::insert_execution_evidence(
                        conn,
                        "deal",
                        &deal.deal_id,
                        "receipt_artifact_ref",
                        &json!({ "artifact_hash": receipt.hash }),
                        completed_at,
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
        .await
}

async fn run_job_spec_now(state: &AppState, spec: JobSpec) -> Result<Value, String> {
    let timeout = execution_timeout(state);
    match spec {
        JobSpec::Wasm { submission } => {
            let verified = submission.verify()?;
            let result = tokio::task::spawn_blocking(move || {
                sandbox::execute_wasm_module(&verified.module_bytes, &verified.input, timeout)
            })
            .await
            .map_err(|e| format!("execution thread panicked: {e}"))?;
            result.map_err(|e| e.to_string())
        }
    }
}

async fn run_workload_spec_with_admission(
    state: &AppState,
    spec: WorkloadSpec,
    permit: Option<sandbox::ExecutionPermit>,
) -> Result<Value, String> {
    let timeout = execution_timeout(state);
    match (spec, permit) {
        (WorkloadSpec::Wasm { submission }, Some(permit)) => {
            let verified = submission.verify()?;
            let result = tokio::task::spawn_blocking(move || {
                sandbox::execute_wasm_module_with_permit(
                    &verified.module_bytes,
                    &verified.input,
                    permit,
                    timeout,
                )
            })
            .await
            .map_err(|e| format!("execution thread panicked: {e}"))?;
            result.map_err(|e| e.to_string())
        }
        (WorkloadSpec::Wasm { submission }, None) => {
            run_job_spec_now(state, JobSpec::Wasm { submission }).await
        }
        (WorkloadSpec::EventsQuery { kinds, limit }, None) => {
            let events = query_events_db(state, kinds, limit).await?;
            Ok(json!({
                "events": events,
                "cursor": null
            }))
        }
        (WorkloadSpec::EventsQuery { .. }, Some(_)) => {
            Err("events workloads do not use execution permits".to_string())
        }
    }
}

async fn process_job(state: Arc<AppState>, job_id: String) {
    let started_job = state
        .db
        .with_conn(move |conn| {
            jobs::try_start_job(conn, &job_id, payments::current_unix_timestamp())
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
                .with_conn(move |conn| {
                    let payment_receipt = if let Some(token_hash) =
                        job_for_commit.payment_token_hash.clone()
                    {
                        let committed = db::commit_payment_token(
                            conn,
                            &token_hash,
                            &job_for_commit.job_id,
                            payments::current_unix_timestamp(),
                        )?;

                        if !committed {
                            return Err("payment reservation could not be committed".to_string());
                        }

                        Some(JobPaymentReceipt {
                            service_id: job_for_commit.service_id.clone(),
                            amount_sats: job_for_commit.payment_amount_sats.unwrap_or_default(),
                            token_hash,
                            settlement_status: SettlementStatus::Committed,
                        })
                    } else {
                        None
                    };
                    let result_evidence_hash = db::insert_execution_evidence(
                        conn,
                        "job",
                        &job_for_commit.job_id,
                        "execution_result",
                        &result,
                        payments::current_unix_timestamp(),
                    )?;

                    jobs::complete_job_success(
                        conn,
                        &job_for_commit.job_id,
                        &result,
                        payment_receipt.as_ref(),
                        Some(&result_evidence_hash),
                        payments::current_unix_timestamp(),
                    )
                })
                .await;

            if let Err(error) = persisted {
                tracing::error!("Failed to persist successful job result: {error}");
                let job_id = job.job_id.clone();
                let _ = state
                    .db
                    .with_conn(move |conn| {
                        jobs::complete_job_failure(
                            conn,
                            &job_id,
                            "job completed but result could not be persisted",
                            None,
                            payments::current_unix_timestamp(),
                        )
                    })
                    .await;
            }
        }
        Err(error_message) => {
            let job_id = job.job_id.clone();
            let token_hash = job.payment_token_hash.clone();
            let persisted = state
                .db
                .with_conn(move |conn| {
                    if let Some(token_hash) = token_hash {
                        let _ = db::release_payment_token(
                            conn,
                            &token_hash,
                            &job_id,
                            payments::current_unix_timestamp(),
                        )?;
                    }

                    let failure_evidence_hash = db::insert_execution_evidence(
                        conn,
                        "job",
                        &job_id,
                        "execution_failure",
                        &json!({ "message": error_message }),
                        payments::current_unix_timestamp(),
                    )?;
                    jobs::complete_job_failure(
                        conn,
                        &job_id,
                        &error_message,
                        Some(&failure_evidence_hash),
                        payments::current_unix_timestamp(),
                    )
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
    if normalized.contains("timeout")
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

fn sign_deal_receipt(
    state: &AppState,
    deal: &deals::StoredDeal,
    completed_at: i64,
    status: &str,
    settlement_status: Option<SettlementStatus>,
    committed_amount_sats: u64,
    settlement_reference: Option<String>,
    result_hash: Option<String>,
    failure: Option<ReceiptFailure>,
) -> Result<SignedArtifact<ReceiptPayload>, String> {
    let settlement = settlement_status.and_then(|settlement_status| {
        receipt_settlement_from_deal(
            deal,
            settlement_status,
            committed_amount_sats,
            settlement_reference,
        )
    });
    let payment_lock = deal.payment_lock();
    let error = failure.as_ref().map(|details| details.message.clone());
    let result_format = result_hash
        .as_ref()
        .map(|_| wasm::JCS_JSON_FORMAT.to_string());
    let executor = Some(receipt_executor_for_spec(&deal.spec));
    let limits_applied = Some(receipt_limits_for_spec(state, &deal.spec));

    sign_node_artifact(
        state,
        ARTIFACT_KIND_RECEIPT,
        completed_at,
        ReceiptPayload {
            receipt_id: protocol::new_artifact_id(),
            deal_id: deal.deal_id.clone(),
            deal_hash: Some(deal.artifact.hash.clone()),
            quote_id: deal.quote.payload.quote_id.clone(),
            offer_id: deal.artifact.payload.offer_id.clone(),
            service_id: deal.artifact.payload.service_id.clone(),
            workload_hash: deal.artifact.payload.workload_hash.clone(),
            status: status.to_string(),
            amount_paid_sats: (committed_amount_sats > 0).then_some(committed_amount_sats),
            payment_lock,
            settlement,
            result_hash,
            result_format,
            executor,
            limits_applied,
            failure,
            error,
            completed_at,
        },
    )
}

async fn reject_deal_admission(
    state: &Arc<AppState>,
    deal: &deals::StoredDeal,
    error_message: String,
) {
    let completed_at = payments::current_unix_timestamp();
    let failure = receipt_failure("capacity_exhausted", error_message.clone());
    let settlement_reference = match update_deal_lightning_bundle_state(
        state.as_ref(),
        deal,
        InvoiceBundleLegState::Canceled,
    )
    .await
    {
        Ok(Some(bundle)) => Some(bundle.session_id),
        Ok(None) => None,
        Err(error) => {
            tracing::error!("Failed to update Lightning bundle for rejected deal: {error}");
            None
        }
    };
    let receipt = match sign_deal_receipt(
        state.as_ref(),
        deal,
        completed_at,
        deals::DEAL_STATUS_REJECTED,
        Some(SettlementStatus::Released),
        0,
        settlement_reference,
        None,
        Some(failure.clone()),
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
    let token_hash = deal.payment_token_hash.clone();
    let payment_method = deal.payment_method.clone();
    let receipt_for_db = receipt.clone();
    let persisted = state
        .db
        .with_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<(), String> {
                if payment_method.as_deref() == Some("cashu") {
                    if let Some(token_hash) = token_hash {
                        let _ =
                            db::release_payment_token(conn, &token_hash, &deal_id, completed_at)?;
                    }
                }
                let failure_evidence_hash = db::insert_execution_evidence(
                    conn,
                    "deal",
                    &deal_id,
                    "execution_failure",
                    &failure,
                    completed_at,
                )?;

                let rejected = deals::reject_deal_admission(
                    conn,
                    &deal_id,
                    &error_message,
                    &receipt_for_db,
                    Some(&failure_evidence_hash),
                    Some(&receipt_for_db.hash),
                    completed_at,
                )?;

                if !rejected {
                    return Ok(());
                }

                db::insert_artifact_document(
                    conn,
                    &receipt_for_db.hash,
                    &receipt_for_db.payload_hash,
                    ARTIFACT_KIND_RECEIPT,
                    &receipt_for_db.actor_id,
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

async fn process_deal(state: Arc<AppState>, deal_id: String) {
    let lookup_deal_id = deal_id.clone();
    let loaded_deal = state
        .db
        .with_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
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

    let execution_permit = match &deal.spec {
        WorkloadSpec::Wasm { .. } => match sandbox::try_acquire_wasm_execution_permit() {
            Ok(permit) => Some(permit),
            Err(error_message) => {
                reject_deal_admission(&state, &deal, error_message).await;
                return;
            }
        },
        WorkloadSpec::EventsQuery { .. } => None,
    };

    let start_deal_id = deal.deal_id.clone();
    let started = state
        .db
        .with_conn(move |conn| {
            deals::try_mark_deal_running(conn, &start_deal_id, payments::current_unix_timestamp())
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

    match run_workload_spec_with_admission(state.as_ref(), deal.spec.clone(), execution_permit)
        .await
    {
        Ok(result) => {
            let completed_at = payments::current_unix_timestamp();
            let committed_amount_sats = deal.payment_amount_sats.unwrap_or_default();
            let settlement_reference = match update_deal_lightning_bundle_state(
                state.as_ref(),
                &deal,
                InvoiceBundleLegState::Settled,
            )
            .await
            {
                Ok(Some(bundle)) => Some(bundle.session_id),
                Ok(None) => None,
                Err(error) => {
                    tracing::error!(
                        "Failed to update Lightning bundle for successful deal {}: {error}",
                        deal.deal_id
                    );
                    None
                }
            };
            let receipt = match sign_deal_receipt(
                state.as_ref(),
                &deal,
                completed_at,
                deals::DEAL_STATUS_SUCCEEDED,
                Some(SettlementStatus::Committed),
                committed_amount_sats,
                settlement_reference,
                Some(canonical_result_hash(&result)),
                None,
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

            let result_for_db = result.clone();
            let deal_for_commit = deal.clone();
            let receipt_for_db = receipt.clone();
            let persisted = state
                .db
                .with_conn(move |conn| {
                    conn.execute_batch("BEGIN IMMEDIATE")
                        .map_err(|e| e.to_string())?;
                    let operation = (|| -> Result<(), String> {
                        if deal_for_commit.payment_method.as_deref() == Some("cashu") {
                            if let Some(token_hash) = deal_for_commit.payment_token_hash.clone() {
                                let committed = db::commit_payment_token(
                                    conn,
                                    &token_hash,
                                    &deal_for_commit.deal_id,
                                    completed_at,
                                )?;

                                if !committed {
                                    return Err(
                                        "payment reservation could not be committed".to_string()
                                    );
                                }
                            }
                        }
                        let result_evidence_hash = db::insert_execution_evidence(
                            conn,
                            "deal",
                            &deal_for_commit.deal_id,
                            "execution_result",
                            &result_for_db,
                            completed_at,
                        )?;
                        db::insert_artifact_document(
                            conn,
                            &receipt_for_db.hash,
                            &receipt_for_db.payload_hash,
                            ARTIFACT_KIND_RECEIPT,
                            &receipt_for_db.actor_id,
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
                            &deal_for_commit.deal_id,
                            &result_for_db,
                            &receipt_for_db,
                            Some(&result_evidence_hash),
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
                tracing::error!("Failed to persist successful deal result: {error}");
            }
        }
        Err(error_message) => {
            let completed_at = payments::current_unix_timestamp();
            let failure = receipt_failure(
                classify_execution_failure(&error_message),
                error_message.clone(),
            );
            let settlement_reference = match update_deal_lightning_bundle_state(
                state.as_ref(),
                &deal,
                InvoiceBundleLegState::Canceled,
            )
            .await
            {
                Ok(Some(bundle)) => Some(bundle.session_id),
                Ok(None) => None,
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
                deals::DEAL_STATUS_FAILED,
                Some(SettlementStatus::Released),
                0,
                settlement_reference,
                None,
                Some(failure.clone()),
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
            let token_hash = deal.payment_token_hash.clone();
            let payment_method = deal.payment_method.clone();
            let receipt_for_db = receipt.clone();
            let persisted = state
                .db
                .with_conn(move |conn| {
                    conn.execute_batch("BEGIN IMMEDIATE")
                        .map_err(|e| e.to_string())?;
                    let operation = (|| -> Result<(), String> {
                        if payment_method.as_deref() == Some("cashu") {
                            if let Some(token_hash) = token_hash {
                                let _ = db::release_payment_token(
                                    conn,
                                    &token_hash,
                                    &deal_id,
                                    completed_at,
                                )?;
                            }
                        }
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
                            &receipt_for_db.actor_id,
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
