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
use subtle::ConstantTimeEq;
use tower::{BoxError, ServiceBuilder, limit::ConcurrencyLimitLayer, timeout::TimeoutLayer};

use crate::{
    canonical_json,
    config::PaymentBackend,
    crypto, db,
    deals::{self, NewDeal},
    jobs::{self, JobSpec, NewJob},
    nostr,
    pricing::{PricingInfo, ServiceId},
    protocol::{
        self, ARTIFACT_KIND_CURATED_LIST, ARTIFACT_KIND_DEAL, ARTIFACT_KIND_DESCRIPTOR,
        ARTIFACT_KIND_OFFER, ARTIFACT_KIND_QUOTE, ARTIFACT_KIND_RECEIPT, CuratedListEntry,
        CuratedListPayload, DealPayload, DescriptorPayload, ExecutionLimits, InvoiceBundleLegState,
        InvoiceBundlePayload, LinkedIdentity, OfferPayload, QuotePayload, QuoteSettlementTerms,
        ReceiptExecutor, ReceiptFailure, ReceiptLegState, ReceiptLimitsApplied, ReceiptPayload,
        ReceiptSettlement, ReceiptSettlementLeg, SignedArtifact, WorkloadSpec,
    },
    sandbox,
    settlement::{self, PaymentReceipt, PaymentReservation, ProvidedPayment},
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
        canonical_json::to_vec(&json!([
            self.id,
            self.pubkey,
            self.created_at,
            self.kind,
            self.tags,
            self.content
        ]))
        .expect("node event signing bytes should serialize canonically")
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
    pub requester_id: String,
    #[serde(flatten)]
    pub spec: WorkloadSpec,
    #[serde(default)]
    pub max_price_sats: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateDealRequest {
    pub quote: SignedArtifact<QuotePayload>,
    pub deal: SignedArtifact<DealPayload>,
    #[serde(flatten)]
    pub spec: WorkloadSpec,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseDealPreimageRequest {
    pub success_preimage: String,
    #[serde(default)]
    pub expected_result_hash: Option<String>,
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
    #[serde(flatten)]
    pub spec: WorkloadSpec,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
    #[serde(default)]
    pub quote: Option<SignedArtifact<QuotePayload>>,
    #[serde(default)]
    pub deal: Option<SignedArtifact<DealPayload>>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_intent_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_intent: Option<settlement::LightningWalletIntent>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IssueCuratedListRequest {
    #[serde(default)]
    pub list_id: Option<String>,
    pub expires_at: i64,
    pub entries: Vec<CuratedListEntry>,
}

#[derive(Debug, Serialize)]
pub struct IssueCuratedListResponse {
    pub curated_list: SignedArtifact<CuratedListPayload>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyCuratedListRequest {
    pub curated_list: SignedArtifact<CuratedListPayload>,
}

#[derive(Debug, Serialize)]
pub struct VerifyCuratedListResponse {
    pub valid: bool,
    pub list_hash: String,
    pub curator_id: String,
    pub list_id: String,
    pub expires_at: i64,
}

#[derive(Debug, Serialize)]
pub struct RuntimeNostrProviderPublicationsResponse {
    pub descriptor_summary: nostr::NostrEvent,
    pub offer_summaries: Vec<nostr::NostrEvent>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeNostrReceiptPublicationResponse {
    pub receipt_summary: nostr::NostrEvent,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyNostrEventRequest {
    pub event: nostr::NostrEvent,
}

#[derive(Debug, Serialize)]
pub struct VerifyNostrEventResponse {
    pub valid: bool,
    pub event_id: String,
    pub pubkey: String,
    pub kind: u32,
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

#[derive(Debug, Serialize)]
pub struct RuntimeDealPaymentIntentResponse {
    pub payment_intent: settlement::LightningWalletIntent,
}

const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_EVENT_CONTENT_BYTES: usize = 64 * 1024;
const MAX_WASM_HEX_BYTES: usize = 512 * 1024;
const MAX_WASM_INPUT_BYTES: usize = 128 * 1024;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
const BLOCKING_EXECUTION_TIMEOUT_GRACE_SECS: u64 = 1;
const DEFAULT_ROUTE_TIMEOUT_SECS: u64 = 10;
const DEFAULT_RUNTIME_WAIT_TIMEOUT_SECS: u64 = 15;
const MAX_RUNTIME_WAIT_TIMEOUT_SECS: u64 = 60;
const RUNTIME_WAIT_ROUTE_TIMEOUT_SECS: u64 = MAX_RUNTIME_WAIT_TIMEOUT_SECS + 5;
const _: () = assert!(RUNTIME_WAIT_ROUTE_TIMEOUT_SECS > MAX_RUNTIME_WAIT_TIMEOUT_SECS);
type ApiFailure = (StatusCode, serde_json::Value);

fn publish_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/node/events/publish", post(publish_event))
        .route_layer(ConcurrencyLimitLayer::new(32))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(
                    DEFAULT_ROUTE_TIMEOUT_SECS,
                ))),
        )
}

fn exec_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/node/execute/wasm", post(execute_wasm))
        .route("/v1/node/jobs", post(create_job))
        .route("/v1/node/jobs/:job_id", get(get_job_status))
        .route_layer(ConcurrencyLimitLayer::new(16))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(
                    DEFAULT_ROUTE_TIMEOUT_SECS,
                ))),
        )
}

fn protocol_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/descriptor", get(protocol_descriptor))
        .route("/v1/offers", get(list_offers))
        .route("/v1/feed", get(get_feed))
        .route("/v1/artifacts/:artifact_hash", get(get_artifact))
        .route("/v1/quotes", post(create_quote))
        .route("/v1/deals", post(create_deal))
        .route("/v1/deals/:deal_id", get(get_deal_status))
        .route(
            "/v1/deals/:deal_id/release-preimage",
            post(release_deal_preimage),
        )
        .route(
            "/v1/deals/:deal_id/invoice-bundle",
            get(get_deal_invoice_bundle),
        )
        .route("/v1/invoice-bundles/verify", post(verify_invoice_bundle))
        .route("/v1/curated-lists/verify", post(verify_curated_list))
        .route("/v1/nostr/events/verify", post(verify_nostr_event))
        .route("/v1/receipts/verify", post(verify_receipt))
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
        .route("/v1/runtime/wallet/balance", get(runtime_wallet_balance))
        .route("/v1/runtime/provider/start", post(runtime_provider_start))
        .route(
            "/v1/runtime/services/publish",
            post(runtime_services_publish),
        )
        .route(
            "/v1/runtime/discovery/curated-lists/issue",
            post(runtime_issue_curated_list),
        )
        .route(
            "/v1/runtime/nostr/publications/provider",
            get(runtime_nostr_provider_publications),
        )
        .route(
            "/v1/runtime/nostr/publications/deals/:deal_id/receipt",
            get(runtime_nostr_receipt_publication),
        )
        .route(
            "/v1/runtime/deals/:deal_id/payment-intent",
            get(runtime_deal_payment_intent),
        )
        .route(
            "/v1/runtime/archive/:subject_kind/:subject_id",
            get(runtime_archive_subject),
        )
        .route(
            "/v1/runtime/lightning/invoice-bundles/:session_id/state",
            post(runtime_update_lightning_bundle_state),
        )
        .route_layer(ConcurrencyLimitLayer::new(16))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(
                    DEFAULT_ROUTE_TIMEOUT_SECS,
                ))),
        )
}

fn runtime_wait_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/runtime/services/buy", post(runtime_services_buy))
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
        .route("/v1/node/events/query", post(query_events))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::SERVER,
            HeaderValue::from_static("nginx/1.18.0"),
        ))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

pub fn public_router(state: Arc<AppState>) -> Router {
    common_routes()
        .merge(protocol_routes())
        .merge(publish_routes())
        .merge(exec_routes())
        .with_state(state)
}

pub fn runtime_router(state: Arc<AppState>) -> Router {
    common_routes()
        .merge(runtime_wait_routes())
        .merge(runtime_routes())
        .with_state(state)
}

pub fn router(state: Arc<AppState>) -> Router {
    common_routes()
        .merge(runtime_wait_routes())
        .merge(runtime_routes())
        .merge(protocol_routes())
        .merge(publish_routes())
        .merge(exec_routes())
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

fn runtime_buy_wait_timeout_secs(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(DEFAULT_RUNTIME_WAIT_TIMEOUT_SECS)
        .clamp(1, MAX_RUNTIME_WAIT_TIMEOUT_SECS)
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
    let wait_timeout_secs = runtime_buy_wait_timeout_secs(payload.wait_timeout_secs);

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

        if existing.artifact.payload.workload_hash != workload_hash
            || payload
                .quote
                .as_ref()
                .is_some_and(|quote| quote.hash != existing.quote.hash)
            || payload
                .deal
                .as_ref()
                .is_some_and(|deal| deal.hash != existing.artifact.hash)
        {
            return error_json(
                StatusCode::CONFLICT,
                json!({ "error": "idempotency key reused with different service request" }),
            );
        }

        let mut deal = existing.public_record();
        let mut terminal = is_terminal_deal_status(&deal.status);
        if wait_for_receipt && !terminal && !is_wait_blocking_deal_status(&deal.status) {
            match wait_for_terminal_deal(state.clone(), &deal.deal_id, wait_timeout_secs).await {
                Ok(terminal_deal) => {
                    deal = terminal_deal;
                    terminal = is_terminal_deal_status(&deal.status);
                }
                Err(error) => return error_json(error.0, error.1),
            }
        }

        let (deal, payment_intent) = if existing.payment_method.as_deref() == Some("lightning") {
            match load_runtime_deal_and_payment_intent(state.clone(), &deal.deal_id).await {
                Ok(result) => result,
                Err(error) => return error_json(error.0, error.1),
            }
        } else {
            (deal, None)
        };

        return (
            StatusCode::OK,
            Json(json!(RuntimeBuyServiceResponse {
                quote: existing.quote,
                deal,
                terminal,
                payment_intent_path: payment_intent
                    .as_ref()
                    .map(|intent| runtime_payment_intent_path(&intent.deal_id)),
                payment_intent,
            })),
        );
    }

    let Some(quote) = payload.quote.clone() else {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "runtime buy requires a pre-signed quote artifact",
                "quote_path": "/v1/quotes",
            }),
        );
    };
    let Some(deal_artifact) = payload.deal.clone() else {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "runtime buy requires a pre-signed deal artifact",
                "deal_path": "/v1/deals",
            }),
        );
    };

    let (mut deal, _) = match create_deal_record(
        state.clone(),
        CreateDealRequest {
            quote: quote.clone(),
            deal: deal_artifact,
            spec: payload.spec,
            idempotency_key: payload.idempotency_key,
            payment: payload.payment,
        },
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return error_json(error.0, error.1),
    };

    let mut terminal = false;
    if wait_for_receipt && !is_wait_blocking_deal_status(&deal.status) {
        match wait_for_terminal_deal(state.clone(), &deal.deal_id, wait_timeout_secs).await {
            Ok(terminal_deal) => {
                deal = terminal_deal;
                terminal = is_terminal_deal_status(&deal.status);
            }
            Err(error) => return error_json(error.0, error.1),
        }
    }

    let (deal, payment_intent) = if quote_uses_lightning_bundle(state.as_ref(), &quote) {
        match load_runtime_deal_and_payment_intent(state.clone(), &deal.deal_id).await {
            Ok(result) => result,
            Err(error) => return error_json(error.0, error.1),
        }
    } else {
        (deal, None)
    };

    (
        StatusCode::OK,
        Json(json!(RuntimeBuyServiceResponse {
            quote,
            deal,
            terminal,
            payment_intent_path: payment_intent
                .as_ref()
                .map(|intent| runtime_payment_intent_path(&intent.deal_id)),
            payment_intent,
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
        created_at
            .saturating_add(lightning_admission_window_secs(&quote.payload.settlement_terms) as i64)
    } else {
        quote.payload.expires_at
    };
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
    state.config.payment_backend == PaymentBackend::Lightning
        && total_msat > 0
        && quote.payload.settlement_terms.method == "lightning.base_fee_plus_success_fee.v1"
}

pub async fn runtime_issue_curated_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<IssueCuratedListRequest>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    if payload.entries.is_empty() {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "curated list must contain at least one entry" }),
        );
    }

    let created_at = settlement::current_unix_timestamp();
    if payload.expires_at <= created_at {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "expires_at must be in the future" }),
        );
    }

    match sign_node_artifact(
        state.as_ref(),
        ARTIFACT_KIND_CURATED_LIST,
        created_at,
        CuratedListPayload {
            schema_version: "froglet/v1".to_string(),
            list_type: "curated_list".to_string(),
            curator_id: state.identity.node_id().to_string(),
            list_id: payload.list_id.unwrap_or_else(protocol::new_artifact_id),
            created_at,
            expires_at: payload.expires_at,
            entries: payload.entries,
        },
    ) {
        Ok(curated_list) => (
            StatusCode::CREATED,
            Json(json!(IssueCuratedListResponse { curated_list })),
        ),
        Err(error) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to sign curated list: {error}") }),
        ),
    }
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

    match load_runtime_deal_and_payment_intent(state, &deal_id).await {
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

    if settlement::lightning_bundle_is_funded(&updated) {
        let deal_hash = updated.bundle.payload.deal_hash.clone();
        match state
            .db
            .with_read_conn(move |conn| deals::get_deal_by_artifact_hash(conn, &deal_hash))
            .await
        {
            Ok(Some(deal)) => {
                if let Err(error) =
                    promote_lightning_deal_if_funded(state.clone(), &deal, &updated).await
                {
                    tracing::error!(
                        "Failed to promote Lightning deal {} after invoice update: {error}",
                        deal.deal_id
                    );
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
        .with_read_conn(move |conn| db::list_artifacts(conn, Some(applied_cursor), limit))
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

pub async fn release_deal_preimage(
    State(state): State<Arc<AppState>>,
    Path(deal_id): Path<String>,
    Json(payload): Json<ReleaseDealPreimageRequest>,
) -> impl IntoResponse {
    let success_preimage =
        match normalize_hex_value("success_preimage", payload.success_preimage, 64) {
            Ok(preimage) => preimage,
            Err(error) => return error_json(StatusCode::BAD_REQUEST, error),
        };
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

    let Some(payment_lock) = deal.payment_lock() else {
        return error_json(
            StatusCode::CONFLICT,
            json!({ "error": "deal is missing its lightning payment lock", "deal_id": deal_id }),
        );
    };
    let Ok(success_preimage_bytes) = hex::decode(&success_preimage) else {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "success_preimage must be valid lowercase hex" }),
        );
    };
    let computed_payment_hash = crypto::sha256_hex(&success_preimage_bytes);
    if computed_payment_hash != payment_lock.token_hash {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "success_preimage does not match the deal payment lock",
                "deal_id": deal_id,
            }),
        );
    }

    let synced_bundle = match sync_and_maybe_promote_lightning_deal(state.clone(), &deal).await {
        Ok(bundle) => bundle,
        Err(error) => {
            tracing::error!("Failed to sync Lightning bundle for deal {deal_id}: {error}");
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to sync lightning settlement state" }),
            );
        }
    };
    let Some(bundle) = synced_bundle else {
        return error_json(
            StatusCode::NOT_FOUND,
            json!({ "error": "lightning invoice bundle not found", "deal_id": deal_id }),
        );
    };

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
    let valid = protocol::verify_artifact(&payload.receipt);
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

    match query_events_db(state.as_ref(), payload.kinds, payload.limit).await {
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

async fn insert_event_db(state: &AppState, event: NodeEventEnvelope) -> Result<bool, String> {
    state
        .db
        .with_write_conn(move |conn| db::insert_event(conn, &event))
        .await
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
    let mut payload = DescriptorPayload {
        provider_id: state.identity.node_id().to_string(),
        descriptor_seq: 0,
        protocol_version: protocol::FROGLET_SCHEMA_V1.to_string(),
        expires_at: None,
        linked_identities: vec![nostr_publication_linked_identity(state)?],
        transport_endpoints,
        capabilities: protocol::DescriptorCapabilities {
            service_kinds: vec![
                crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_V1.to_string(),
                "events.query".to_string(),
            ],
            execution_runtimes: vec!["wasm".to_string()],
            max_concurrent_deals: Some(sandbox::wasm_concurrency_limit() as u32),
        },
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
    let descriptor = current_descriptor_artifact(state).await?;
    let descriptor_hash = descriptor.hash.clone();
    let mut offers = Vec::new();
    for payload in current_offer_payloads(state, &descriptor_hash) {
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

fn current_offer_payloads(state: &AppState, descriptor_hash: &str) -> Vec<OfferPayload> {
    let provider_id = state.identity.node_id().to_string();
    let priced_offer = |service_id: ServiceId,
                        offer_kind: &str,
                        runtime: &str,
                        abi_version: &str,
                        max_input_bytes: usize,
                        max_runtime_ms: u64,
                        max_memory_bytes: usize,
                        max_output_bytes: usize,
                        fuel_limit: u64| {
        let price_sats = state.pricing.price_for(service_id);
        OfferPayload {
            provider_id: provider_id.clone(),
            offer_id: service_id.as_str().to_string(),
            descriptor_hash: descriptor_hash.to_string(),
            expires_at: None,
            offer_kind: offer_kind.to_string(),
            settlement_method: "lightning.base_fee_plus_success_fee.v1".to_string(),
            quote_ttl_secs: advertised_offer_timeout_secs(
                state,
                service_id,
                price_sats,
                &accepted_payment_methods(state),
            ),
            execution_profile: protocol::OfferExecutionProfile {
                runtime: runtime.to_string(),
                abi_version: abi_version.to_string(),
                capabilities: Vec::new(),
                max_input_bytes,
                max_runtime_ms,
                max_memory_bytes,
                max_output_bytes,
                fuel_limit,
            },
            price_schedule: protocol::OfferPriceSchedule {
                base_fee_msat: 0,
                success_fee_msat: price_sats.saturating_mul(1_000),
            },
            terms_hash: None,
        }
    };

    vec![
        priced_offer(
            ServiceId::EventsQuery,
            "events.query",
            "events_query",
            "froglet.events.query.v1",
            MAX_BODY_BYTES,
            state.config.execution_timeout_secs.saturating_mul(1_000),
            0,
            MAX_BODY_BYTES,
            0,
        ),
        priced_offer(
            ServiceId::ExecuteWasm,
            crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_V1,
            "wasm",
            crate::wasm::WASM_RUN_JSON_ABI_V1,
            MAX_WASM_INPUT_BYTES,
            state.config.execution_timeout_secs.saturating_mul(1_000),
            sandbox::WASM_MAX_MEMORY_BYTES,
            sandbox::WASM_MAX_OUTPUT_BYTES,
            sandbox::WASM_FUEL_LIMIT,
        ),
    ]
}

fn accepted_payment_methods(state: &AppState) -> Vec<String> {
    settlement::accepted_payment_methods(state)
}

async fn quoted_settlement_terms(
    state: &AppState,
    price_sats: u64,
) -> Result<QuoteSettlementTerms, String> {
    if let Some(terms) = settlement::quoted_lightning_settlement_terms(state, price_sats).await? {
        return Ok(terms);
    }

    Ok(QuoteSettlementTerms {
        method: "lightning.base_fee_plus_success_fee.v1".to_string(),
        destination_identity: state.identity.compressed_public_key_hex().to_string(),
        base_fee_msat: 0,
        success_fee_msat: price_sats.saturating_mul(1_000),
        max_base_invoice_expiry_secs: state.config.lightning.base_invoice_expiry_secs,
        max_success_hold_expiry_secs: state.config.lightning.success_hold_expiry_secs,
        min_final_cltv_expiry: state.config.lightning.min_final_cltv_expiry,
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

    let valid = token.len() == state.runtime_auth_token.len()
        && token
            .as_bytes()
            .ct_eq(state.runtime_auth_token.as_bytes())
            .unwrap_u8()
            == 1;
    if !valid {
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

    if payload.spec.runtime() == Some("wasm") && offer.payload.execution_profile.runtime != "wasm" {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "offer does not match workload runtime",
                "offer_runtime": offer.payload.execution_profile.runtime,
                "requested_runtime": payload.spec.runtime(),
            }),
        ));
    }

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

    let quoted_total_msat = payload.quote.payload.settlement_terms.base_fee_msat
        + payload.quote.payload.settlement_terms.success_fee_msat;
    let quoted_total_sats = quoted_total_msat / 1_000;
    let uses_lightning_bundle =
        quoted_total_sats > 0 && state.config.payment_backend == PaymentBackend::Lightning;
    if let Err((status, message)) =
        validate_deal_deadlines(&payload.quote, &payload.deal, now, uses_lightning_bundle)
    {
        return Err((status, json!({ "error": message })));
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

    let invoice_bundle_session = if uses_lightning_bundle {
        Some(
            settlement::issue_lightning_invoice_bundle(
                state.as_ref(),
                settlement::BuildLightningInvoiceBundleRequest {
                    session_id: None,
                    requester_id: payload.deal.payload.requester_id.clone(),
                    quote_hash: canonical_quote_hash.clone(),
                    deal_hash: canonical_deal_hash.clone(),
                    admission_deadline: Some(payload.deal.payload.admission_deadline),
                    success_payment_hash: payload.deal.payload.success_payment_hash.clone(),
                    base_fee_msat: payload.quote.payload.settlement_terms.base_fee_msat,
                    success_fee_msat: payload.quote.payload.settlement_terms.success_fee_msat,
                    created_at: now,
                },
            )
            .await
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

    let deal_for_db = NewDeal {
        deal_id: deal_id.clone(),
        idempotency_key: idempotency_key.clone(),
        quote: payload.quote.clone(),
        spec: payload.spec.clone(),
        artifact: deal_artifact.clone(),
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

    let deal_hash = canonical_deal_hash.clone();
    let deal_payload_hash = deal_artifact.payload_hash.clone();
    let deal_actor_id = deal_artifact.signer.clone();
    let deal_artifact_hash = canonical_deal_hash.clone();
    let quote_hash = canonical_quote_hash.clone();
    let quote_payload_hash = payload.quote.payload_hash.clone();
    let quote_actor_id = payload.quote.signer.clone();
    let quote_id = canonical_quote_hash.clone();
    let spec_for_evidence = payload.spec.clone();
    let quote_artifact_ref = json!({ "artifact_hash": quote_hash.clone() });
    let deal_artifact_ref = json!({ "artifact_hash": deal_hash.clone() });
    let invoice_bundle_session_for_db = invoice_bundle_session.clone();
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
            if let Some(bundle) = invoice_bundle_session.as_ref()
                && let Err(cancel_error) =
                    settlement::cancel_lightning_invoice_bundle(state.as_ref(), bundle).await
            {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({
                        "error": format!(
                            "{error}; additionally failed to cancel issued lightning invoices: {cancel_error}"
                        ),
                    }),
                ));
            }
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
        if let Some(bundle) = invoice_bundle_session.as_ref()
            && let Err(cancel_error) =
                settlement::cancel_lightning_invoice_bundle(state.as_ref(), bundle).await
        {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({
                    "error": format!(
                        "failed to cancel issued lightning invoices for duplicate deal: {cancel_error}"
                    ),
                }),
            ));
        }
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

    if !uses_lightning_bundle {
        tokio::spawn(process_deal_with_reserved_permit(
            state,
            deal_id,
            reserved_execution_permit,
        ));
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
            .with_read_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
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

        if is_terminal_deal_status(&deal.status) || deal.status == deals::DEAL_STATUS_RESULT_READY {
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

async fn deal_lightning_invoice_bundle(
    state: &AppState,
    deal: &deals::StoredDeal,
) -> Result<Option<settlement::LightningInvoiceBundleSession>, String> {
    if deal.payment_method.as_deref() != Some("lightning") {
        return Ok(None);
    }

    settlement::get_lightning_invoice_bundle_by_deal_hash(state, &deal.artifact.hash).await
}

fn is_terminal_deal_status(status: &str) -> bool {
    matches!(
        status,
        deals::DEAL_STATUS_SUCCEEDED | deals::DEAL_STATUS_FAILED | deals::DEAL_STATUS_REJECTED
    )
}

fn is_wait_blocking_deal_status(status: &str) -> bool {
    matches!(
        status,
        deals::DEAL_STATUS_PAYMENT_PENDING | deals::DEAL_STATUS_RESULT_READY
    )
}

fn runtime_payment_intent_path(deal_id: &str) -> String {
    format!("/v1/runtime/deals/{deal_id}/payment-intent")
}

async fn load_runtime_deal_and_payment_intent(
    state: Arc<AppState>,
    deal_id: &str,
) -> Result<(deals::DealRecord, Option<settlement::LightningWalletIntent>), ApiFailure> {
    let lookup_deal_id = deal_id.to_string();
    let stored = state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &lookup_deal_id))
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("database error: {error}") }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                json!({ "error": "deal not found", "deal_id": deal_id }),
            )
        })?;

    if stored.payment_method.as_deref() != Some("lightning") {
        return Ok((stored.public_record(), None));
    }

    let Some(bundle) = sync_and_maybe_promote_lightning_deal(state.clone(), &stored)
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to sync lightning settlement state", "details": error }),
            )
        })?
    else {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "error": "lightning deal is missing its wallet payment intent",
                "deal_id": deal_id,
            }),
        ));
    };

    let reload_deal_id = deal_id.to_string();
    let current = state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &reload_deal_id))
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("database error: {error}") }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                json!({ "error": "deal not found", "deal_id": deal_id }),
            )
        })?;

    let report = settlement::validate_lightning_invoice_bundle(
        &bundle.bundle,
        &current.quote,
        &current.artifact,
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

    let payment_intent = settlement::build_lightning_wallet_intent(
        state.as_ref(),
        &current.deal_id,
        &current.status,
        current.result_hash.as_deref(),
        &bundle,
    );

    Ok((current.public_record(), Some(payment_intent)))
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

async fn persist_lightning_terminal_failure_receipt(
    state: Arc<AppState>,
    deal: &deals::StoredDeal,
    bundle: &settlement::LightningInvoiceBundleSession,
    deal_state: &str,
    execution_state: &str,
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
            bundle: Some(bundle),
            result_hash: deal.result_hash.clone(),
            failure: Some(failure.clone()),
        },
    )?;
    let receipt_json = serde_json::to_string(&receipt).map_err(|e| e.to_string())?;
    let deal_id = deal.deal_id.clone();
    let expected_status = deal.status.clone();
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
    if state.config.payment_backend != PaymentBackend::Lightning {
        return Ok(());
    }

    let watch_deals = state
        .db
        .with_read_conn(deals::list_lightning_watch_deals)
        .await?;
    for deal in watch_deals {
        if let Err(error) = reconcile_lightning_deal(state.clone(), deal).await {
            tracing::error!("Failed to reconcile Lightning deal: {error}");
        }
    }

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
        .with_read_conn(move |conn| match subject_kind_owned.as_str() {
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
        WorkloadSpec::Wasm { .. } if is_lightning_payment_method(payment_method) => {
            execution_timeout(state)
        }
        _ => execution_timeout(state),
    }
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

fn receipt_limits_for_spec(
    state: &AppState,
    spec: &WorkloadSpec,
    payment_method: Option<&str>,
) -> ReceiptLimitsApplied {
    let max_runtime_ms =
        duration_millis_u64(workload_execution_timeout(state, spec, payment_method));
    match spec {
        WorkloadSpec::Wasm { .. } => ReceiptLimitsApplied {
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

fn cancel_recovery_bundle_if_pending(
    bundle: &mut settlement::LightningInvoiceBundleSession,
    updated_at: i64,
) {
    if matches!(
        bundle.success_state,
        InvoiceBundleLegState::Open | InvoiceBundleLegState::Accepted
    ) {
        bundle.success_state = InvoiceBundleLegState::Canceled;
        bundle.updated_at = updated_at;
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

        if !settled_success_can_finish_on_recovery
            && (matches!(synced_bundle.success_state, InvoiceBundleLegState::Settled)
                || !settlement::lightning_bundle_is_funded(&synced_bundle))
        {
            let failure = receipt_failure(
                "recovery_invariant_violation",
                "lightning settlement state is inconsistent with the persisted deal status",
            );
            return Ok(DealRecoveryDecision::Fail(Box::new(
                build_recovered_deal_failure(
                    state.as_ref(),
                    deal,
                    recovered_at,
                    Some(synced_bundle),
                    "lightning settlement state is inconsistent with the persisted deal status",
                    failure,
                )?,
            )));
        }

        bundle = Some(synced_bundle);
    }

    if recovered_at > deal.artifact.payload.completion_deadline {
        if let Some(bundle) = bundle.as_mut() {
            cancel_recovery_bundle_if_pending(bundle, recovered_at);
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

pub async fn recover_runtime_state(state: Arc<AppState>) -> Result<(), String> {
    let incomplete_deals = state
        .db
        .with_read_conn(deals::list_incomplete_deals)
        .await?;
    let incomplete_jobs = state.db.with_read_conn(jobs::list_incomplete_jobs).await?;
    let recovered_at = settlement::current_unix_timestamp();
    let mut recovered_deals = Vec::new();
    let mut failed_deals = Vec::new();

    for deal in incomplete_deals {
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

async fn run_wasm_with_timeout<F>(timeout: Duration, operation: F) -> Result<Value, String>
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
        Err(_) => Err(format!(
            "execution exceeded runtime deadline after {}s",
            timeout.as_secs()
        )),
    }
}

async fn run_job_spec_now(state: &AppState, spec: JobSpec) -> Result<Value, String> {
    let timeout = execution_timeout(state);
    match spec {
        JobSpec::Wasm { submission } => {
            let verified = submission.verify()?;
            let wasm_sandbox = state.wasm_sandbox.clone();
            run_wasm_with_timeout(timeout, move || {
                wasm_sandbox.execute_module(&verified.module_bytes, &verified.input, timeout)
            })
            .await
        }
    }
}

async fn run_workload_spec_with_admission(
    state: &AppState,
    spec: WorkloadSpec,
    payment_method: Option<&str>,
    permit: Option<sandbox::ExecutionPermit>,
) -> Result<Value, String> {
    let timeout = workload_execution_timeout(state, &spec, payment_method);
    match (spec, permit) {
        (WorkloadSpec::Wasm { submission }, Some(permit)) => {
            let verified = submission.verify()?;
            let wasm_sandbox = state.wasm_sandbox.clone();
            run_wasm_with_timeout(timeout, move || {
                wasm_sandbox.execute_module_with_permit(
                    &verified.module_bytes,
                    &verified.input,
                    permit,
                    timeout,
                )
            })
            .await
        }
        (WorkloadSpec::Wasm { submission }, None) => {
            run_job_spec_now(
                state,
                JobSpec::Wasm {
                    submission: *submission,
                },
            )
            .await
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

struct ReceiptSignSpec<'a> {
    deal_state: &'a str,
    execution_state: &'a str,
    bundle: Option<&'a settlement::LightningInvoiceBundleSession>,
    result_hash: Option<String>,
    failure: Option<ReceiptFailure>,
}

fn sign_deal_receipt(
    state: &AppState,
    deal: &deals::StoredDeal,
    finished_at: i64,
    spec: ReceiptSignSpec<'_>,
) -> Result<SignedArtifact<ReceiptPayload>, String> {
    let result_format = spec
        .result_hash
        .as_ref()
        .map(|_| wasm::JCS_JSON_FORMAT.to_string());
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
            started_at: receipt_started_at(deal, spec.execution_state),
            finished_at,
            deal_state: spec.deal_state.to_string(),
            execution_state: spec.execution_state.to_string(),
            settlement_state,
            result_hash: spec.result_hash,
            result_format,
            executor: receipt_executor_for_spec(&deal.spec),
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

    match run_workload_spec_with_admission(
        state.as_ref(),
        deal.spec.clone(),
        deal.payment_method.as_deref(),
        execution_permit,
    )
    .await
    {
        Ok(result) => {
            let completed_at = settlement::current_unix_timestamp();
            let result_for_db = result.clone();
            if deal.payment_method.as_deref() == Some("lightning") {
                let deal_for_stage = deal.clone();
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
                                "execution_result",
                                &result_for_db,
                                completed_at,
                            )?;
                            let staged = deals::stage_deal_result_ready(
                                conn,
                                &deal_for_stage.deal_id,
                                &result_for_db,
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
                        result_hash: Some(canonical_result_hash(&result)),
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
                                "execution_result",
                                &result_for_db,
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
            DiscoveryMode, IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
            PaymentBackend, PricingConfig, StorageConfig,
        },
        crypto,
        db::DbPool,
        identity::NodeIdentity,
        pricing::PricingTable,
        sandbox::WasmSandbox,
        state::{MarketplaceStatus, TransportStatus},
        wasm::{ComputeWasmWorkload, FROGLET_SCHEMA_V1, WASM_SUBMISSION_TYPE_V1},
    };
    use axum::{body::Body, http::Request};
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use tower::ServiceExt;

    static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(1);
    const VALID_WASM_HEX: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";

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

    fn test_app_state(payment_backend: PaymentBackend) -> Arc<AppState> {
        let temp_dir = unique_temp_dir("runtime-recovery");
        let db_path = temp_dir.join("node.db");
        std::fs::create_dir_all(&temp_dir).expect("temp dir");

        let node_config = NodeConfig {
            network_mode: NetworkMode::Clearnet,
            listen_addr: "127.0.0.1:0".to_string(),
            runtime_listen_addr: "127.0.0.1:0".to_string(),
            tor: crate::config::TorSidecarConfig {
                binary_path: "tor".to_string(),
                backend_listen_addr: "127.0.0.1:0".to_string(),
                startup_timeout_secs: 90,
            },
            discovery_mode: DiscoveryMode::None,
            identity: IdentityConfig {
                auto_generate: true,
            },
            marketplace: None,
            pricing: PricingConfig {
                events_query: 10,
                execute_wasm: 30,
            },
            payment_backend,
            execution_timeout_secs: 5,
            lightning: LightningConfig {
                mode: LightningMode::Mock,
                destination_identity: None,
                base_invoice_expiry_secs: 300,
                success_hold_expiry_secs: 300,
                min_final_cltv_expiry: 18,
                sync_interval_ms: 100,
                lnd_rest: None,
            },
            storage: StorageConfig {
                data_dir: temp_dir.clone(),
                db_path: db_path.clone(),
                identity_dir: temp_dir.join("identity"),
                identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
                nostr_publication_seed_path: temp_dir
                    .join("identity/nostr-publication.secp256k1.seed"),
                runtime_dir: temp_dir.join("runtime"),
                runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
                tor_dir: temp_dir.join("tor"),
            },
        };

        let db = DbPool::open(&db_path).expect("db pool");
        let identity = NodeIdentity::load_or_create(&node_config).expect("identity");

        Arc::new(AppState {
            db,
            transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
                &node_config,
            ))),
            marketplace_status: Arc::new(tokio::sync::Mutex::new(MarketplaceStatus::from_config(
                &node_config,
            ))),
            wasm_sandbox: Arc::new(WasmSandbox::new(4).expect("sandbox")),
            config: node_config.clone(),
            identity: Arc::new(identity),
            pricing: PricingTable::from_config(node_config.pricing),
            http_client: reqwest::Client::new(),
            runtime_auth_token: "test-runtime-token".to_string(),
            runtime_auth_token_path: node_config.storage.runtime_auth_token_path.clone(),
        })
    }

    fn test_wasm_submission() -> crate::wasm::WasmSubmission {
        let module_bytes = hex::decode(VALID_WASM_HEX).expect("valid wasm hex");
        let input = Value::Null;
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

    #[test]
    fn runtime_buy_route_timeout_exceeds_the_max_wait_budget() {
        assert_eq!(
            runtime_buy_wait_timeout_secs(None),
            DEFAULT_RUNTIME_WAIT_TIMEOUT_SECS
        );
        assert_eq!(
            runtime_buy_wait_timeout_secs(Some(MAX_RUNTIME_WAIT_TIMEOUT_SECS + 10)),
            MAX_RUNTIME_WAIT_TIMEOUT_SECS
        );
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
                            artifact: deal,
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
                            artifact: expired_deal,
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
