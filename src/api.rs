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
    canonical_json, crypto, db,
    deals::{self, NewDeal},
    ecash,
    jobs::{self, JobPaymentReceipt, JobSpec, NewJob},
    payments::{self, PaymentReceipt, ProvidedPayment},
    pricing::{PricingInfo, ServiceId},
    protocol::{
        self, ARTIFACT_KIND_DEAL, ARTIFACT_KIND_DESCRIPTOR, ARTIFACT_KIND_OFFER,
        ARTIFACT_KIND_QUOTE, ARTIFACT_KIND_RECEIPT, DealPayload, DescriptorPayload, FeedDescriptor,
        OfferConstraints, OfferPayload, QuotePayload, ReceiptFailure, ReceiptPayload,
        ReceiptSettlement, SettlementDescriptor, SettlementStatus, SignedArtifact,
        TransportEndpoints, WorkloadSpec,
    },
    sandbox, settlement,
    state::AppState,
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
    pub lua: LuaInfo,
    pub wasm: WasmInfo,
}

#[derive(Debug, Serialize)]
pub struct LuaInfo {
    pub enabled: bool,
    pub instruction_limit: u32,
    pub supports_json_input: bool,
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
    pub lua_script_limit_bytes: usize,
    pub wasm_hex_limit_bytes: usize,
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
pub struct ExecuteLuaRequest {
    pub script: String,
    #[serde(default)]
    pub input: Option<Value>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteWasmRequest {
    pub wasm_hex: String,
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

const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_EVENT_CONTENT_BYTES: usize = 64 * 1024;
const MAX_LUA_SCRIPT_BYTES: usize = 16 * 1024;
const MAX_WASM_HEX_BYTES: usize = 512 * 1024;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
type ApiFailure = (StatusCode, serde_json::Value);

pub fn router(state: Arc<AppState>) -> Router {
    let publish_routes = Router::new()
        .route("/v1/node/events/publish", post(publish_event))
        .route_layer(ConcurrencyLimitLayer::new(32));

    let exec_routes = Router::new()
        .route("/v1/node/execute/lua", post(execute_lua))
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
            lua: LuaInfo {
                enabled: true,
                instruction_limit: 50_000_000,
                supports_json_input: true,
            },
            wasm: WasmInfo {
                enabled: true,
                fuel_limit: 50_000_000,
                entrypoints: vec!["run".to_string(), "main".to_string()],
            },
        },
        limits: LimitsInfo {
            events_query_limit_default: 100,
            events_query_limit_max: 500,
            body_limit_bytes: MAX_BODY_BYTES,
            lua_script_limit_bytes: MAX_LUA_SCRIPT_BYTES,
            wasm_hex_limit_bytes: MAX_WASM_HEX_BYTES,
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
            runtimes: vec!["lua".to_string(), "wasm".to_string()],
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
        if wait_for_receipt && !terminal {
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
        },
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return error_json(error.0, error.1),
    };

    let mut terminal = false;
    if wait_for_receipt {
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

pub async fn execute_lua(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ExecuteLuaRequest>,
) -> impl IntoResponse {
    if payload.script.as_bytes().len() > MAX_LUA_SCRIPT_BYTES {
        return error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({ "error": "lua script too large" }),
        );
    }

    tracing::info!("Received Lua Execution Request");

    let reservation = match payments::prepare_payment(
        state.as_ref(),
        ServiceId::ExecuteLua,
        payload.payment,
        None,
    )
    .await
    {
        Ok(reservation) => reservation,
        Err(error) => return error_json(error.status_code(), error.details()),
    };

    match run_job_spec_now(state.as_ref(), JobSpec::Lua {
        script: payload.script,
        input: payload.input,
    })
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
            tracing::error!("Lua Execution Failed: {}", error_message);
            error_json(StatusCode::BAD_REQUEST, json!({ "error": error_message }))
        }
    }
}

pub async fn execute_wasm(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ExecuteWasmRequest>,
) -> impl IntoResponse {
    tracing::info!("Received Wasm Execution Request");

    if payload.wasm_hex.as_bytes().len() > MAX_WASM_HEX_BYTES {
        return error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({ "error": "wasm module too large" }),
        );
    }

    if hex::decode(&payload.wasm_hex).is_err() {
        return error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid hex encoding" }),
        );
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

    match run_job_spec_now(state.as_ref(), JobSpec::Wasm {
        wasm_hex: payload.wasm_hex,
    })
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
        .with_conn(move |conn| jobs::insert_or_get_job(conn, new_job))
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
            runtimes: vec!["lua".to_string(), "wasm".to_string()],
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
            ServiceId::ExecuteLua,
            "compute",
            Some("lua"),
            OfferConstraints {
                max_body_bytes: Some(MAX_LUA_SCRIPT_BYTES),
                max_query_limit: None,
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
    let quote = sign_node_artifact(
        state.as_ref(),
        ARTIFACT_KIND_QUOTE,
        created_at,
        QuotePayload {
            quote_id: protocol::new_artifact_id(),
            offer_id: offer.payload.offer_id.clone(),
            service_id: offer.payload.service_id.clone(),
            workload_kind: payload.spec.kind().to_string(),
            workload_hash,
            price_sats: offer.payload.price_sats,
            payment_method: offer
                .payload
                .payment_required
                .then(|| offer.payload.payment_methods.first().cloned())
                .flatten(),
            expires_at: created_at + 60,
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

    let idempotency_key = normalize_idempotency_key(payload.idempotency_key)
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

    let deal_id = protocol::new_artifact_id();
    let reservation = payments::prepare_payment_for_amount(
        state.as_ref(),
        service_id,
        payload.quote.payload.price_sats,
        payload.payment,
        Some(deal_id.clone()),
    )
    .await
    .map_err(|error| (error.status_code(), error.details()))?;

    let payment_lock = reservation
        .as_ref()
        .map(|reservation| protocol::PaymentLock {
            kind: reservation.method.clone(),
            token_hash: reservation.token_hash.clone(),
            amount_sats: reservation.amount_sats,
        });
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
        payment_token_hash: reservation
            .as_ref()
            .map(|payment| payment.token_hash.clone()),
        payment_amount_sats: reservation.as_ref().map(|payment| payment.amount_sats),
        created_at: now,
    };

    let deal_hash = deal_artifact.hash.clone();
    let deal_payload_hash = deal_artifact.payload_hash.clone();
    let deal_actor_id = deal_artifact.actor_id.clone();
    let quote_hash = payload.quote.hash.clone();
    let quote_payload_hash = payload.quote.payload_hash.clone();
    let quote_actor_id = payload.quote.actor_id.clone();
    let quote_id = payload.quote.payload.quote_id.clone();
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

                deals::insert_or_get_deal(conn, deal_for_db.clone())
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

    tokio::spawn(process_deal(state, deal_id));
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
        JobSpec::Lua { script, .. } if script.as_bytes().len() > MAX_LUA_SCRIPT_BYTES => {
            Err(error_json(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({ "error": "lua script too large" }),
            ))
        }
        JobSpec::Wasm { wasm_hex } if wasm_hex.as_bytes().len() > MAX_WASM_HEX_BYTES => {
            Err(error_json(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({ "error": "wasm module too large" }),
            ))
        }
        JobSpec::Wasm { wasm_hex } if hex::decode(wasm_hex).is_err() => Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid hex encoding" }),
        )),
        _ => Ok(()),
    }
}

fn validate_workload_spec(
    spec: &WorkloadSpec,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match spec {
        WorkloadSpec::Lua { script, .. } if script.as_bytes().len() > MAX_LUA_SCRIPT_BYTES => {
            Err(error_json(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({ "error": "lua script too large" }),
            ))
        }
        WorkloadSpec::Wasm { wasm_hex } if wasm_hex.as_bytes().len() > MAX_WASM_HEX_BYTES => {
            Err(error_json(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({ "error": "wasm module too large" }),
            ))
        }
        WorkloadSpec::Wasm { wasm_hex } if hex::decode(wasm_hex).is_err() => Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid hex encoding" }),
        )),
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
    let incomplete_deals = state
        .db
        .with_conn(deals::list_incomplete_deals)
        .await?;
    let completed_at = payments::current_unix_timestamp();
    let deal_recovery_message = "node restarted before deal completion".to_string();
    let recovery_receipts = incomplete_deals
        .into_iter()
        .map(|deal| {
            let receipt = sign_deal_receipt(
                state.as_ref(),
                &deal,
                completed_at,
                deals::DEAL_STATUS_FAILED,
                Some(SettlementStatus::Expired),
                0,
                None,
                Some(receipt_failure(
                    "node_restarted",
                    deal_recovery_message.clone(),
                )),
            )?;
            let receipt_json = serde_json::to_string(&receipt).map_err(|e| e.to_string())?;
            Ok((deal, receipt, receipt_json))
        })
        .collect::<Result<Vec<_>, String>>()?;

    state
        .db
        .with_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<(), String> {
                let _ = db::expire_reserved_payment_tokens(conn, completed_at)?;
                jobs::fail_incomplete_jobs(conn, "node restarted before job completion", completed_at)?;

                for (deal, receipt, receipt_json) in &recovery_receipts {
                    deals::complete_deal_failure(
                        conn,
                        &deal.deal_id,
                        &deal_recovery_message,
                        receipt,
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
        JobSpec::Lua { script, input } => {
            let result = tokio::task::spawn_blocking(move || {
                sandbox::execute_lua_script(&script, input.as_ref(), timeout)
            })
            .await
            .map_err(|e| format!("execution thread panicked: {e}"))?;
            result.map_err(|e| e.to_string())
        }
        JobSpec::Wasm { wasm_hex } => {
            let wasm_bytes =
                hex::decode(&wasm_hex).map_err(|_| "invalid hex encoding".to_string())?;
            let result = tokio::task::spawn_blocking(move || {
                sandbox::execute_wasm_module(&wasm_bytes, timeout)
            })
            .await
            .map_err(|e| format!("execution thread panicked: {e}"))?;
            result.map(|value| json!(value)).map_err(|e| e.to_string())
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
        (WorkloadSpec::Lua { script, input }, Some(permit)) => {
            let result = tokio::task::spawn_blocking(move || {
                sandbox::execute_lua_script_with_permit(&script, input.as_ref(), permit, timeout)
            })
            .await
            .map_err(|e| format!("execution thread panicked: {e}"))?;
            result.map_err(|e| e.to_string())
        }
        (WorkloadSpec::Lua { script, input }, None) => {
            run_job_spec_now(state, JobSpec::Lua { script, input }).await
        }
        (WorkloadSpec::Wasm { wasm_hex }, Some(permit)) => {
            let wasm_bytes =
                hex::decode(&wasm_hex).map_err(|_| "invalid hex encoding".to_string())?;
            let result = tokio::task::spawn_blocking(move || {
                sandbox::execute_wasm_module_with_permit(&wasm_bytes, permit, timeout)
            })
            .await
            .map_err(|e| format!("execution thread panicked: {e}"))?;
            result.map(|value| json!(value)).map_err(|e| e.to_string())
        }
        (WorkloadSpec::Wasm { wasm_hex }, None) => {
            run_job_spec_now(state, JobSpec::Wasm { wasm_hex }).await
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

                    jobs::complete_job_success(
                        conn,
                        &job_for_commit.job_id,
                        &result,
                        payment_receipt.as_ref(),
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

                    jobs::complete_job_failure(
                        conn,
                        &job_id,
                        &error_message,
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
    result_hash: Option<String>,
    failure: Option<ReceiptFailure>,
) -> Result<SignedArtifact<ReceiptPayload>, String> {
    let settlement = settlement_status
        .and_then(|settlement_status| {
            receipt_settlement_from_deal(deal, settlement_status, committed_amount_sats, None)
        });
    let payment_lock = deal.payment_lock();
    let error = failure.as_ref().map(|details| details.message.clone());

    sign_node_artifact(
        state,
        ARTIFACT_KIND_RECEIPT,
        completed_at,
        ReceiptPayload {
            receipt_id: protocol::new_artifact_id(),
            deal_id: deal.deal_id.clone(),
            quote_id: deal.quote.payload.quote_id.clone(),
            offer_id: deal.artifact.payload.offer_id.clone(),
            service_id: deal.artifact.payload.service_id.clone(),
            workload_hash: deal.artifact.payload.workload_hash.clone(),
            status: status.to_string(),
            amount_paid_sats: (committed_amount_sats > 0).then_some(committed_amount_sats),
            payment_lock,
            settlement,
            result_hash,
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
    let receipt = match sign_deal_receipt(
        state.as_ref(),
        deal,
        completed_at,
        deals::DEAL_STATUS_REJECTED,
        Some(SettlementStatus::Released),
        0,
        None,
        Some(failure),
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
    let receipt_for_db = receipt.clone();
    let persisted = state
        .db
        .with_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let operation = (|| -> Result<(), String> {
                if let Some(token_hash) = token_hash {
                    let _ = db::release_payment_token(conn, &token_hash, &deal_id, completed_at)?;
                }

                let rejected = deals::reject_deal_admission(
                    conn,
                    &deal_id,
                    &error_message,
                    &receipt_for_db,
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
        WorkloadSpec::Lua { .. } => match sandbox::try_acquire_lua_execution_permit() {
            Ok(permit) => Some(permit),
            Err(error_message) => {
                reject_deal_admission(&state, &deal, error_message).await;
                return;
            }
        },
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
            let receipt = match sign_deal_receipt(
                state.as_ref(),
                &deal,
                completed_at,
                deals::DEAL_STATUS_SUCCEEDED,
                Some(SettlementStatus::Committed),
                committed_amount_sats,
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

                        deals::complete_deal_success(
                            conn,
                            &deal_for_commit.deal_id,
                            &result_for_db,
                            &receipt_for_db,
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
            let receipt = match sign_deal_receipt(
                state.as_ref(),
                &deal,
                completed_at,
                deals::DEAL_STATUS_FAILED,
                Some(SettlementStatus::Released),
                0,
                None,
                Some(receipt_failure(
                    classify_execution_failure(&error_message),
                    error_message.clone(),
                )),
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
            let receipt_for_db = receipt.clone();
            let persisted = state
                .db
                .with_conn(move |conn| {
                    conn.execute_batch("BEGIN IMMEDIATE")
                        .map_err(|e| e.to_string())?;
                    let operation = (|| -> Result<(), String> {
                        if let Some(token_hash) = token_hash {
                            let _ = db::release_payment_token(
                                conn,
                                &token_hash,
                                &deal_id,
                                completed_at,
                            )?;
                        }

                        deals::complete_deal_failure(
                            conn,
                            &deal_id,
                            &error_message,
                            &receipt_for_db,
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
