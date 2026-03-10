use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower::limit::ConcurrencyLimitLayer;
use tower::timeout::TimeoutLayer;
use std::time::Duration;

use crate::{
    crypto, db, ecash,
    payments::{self, CASHU_VERIFIER_MODE, ProvidedPayment},
    pricing::{PricingInfo, ServiceId},
    sandbox,
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
}

#[derive(Debug, Serialize)]
pub struct PaymentsInfo {
    pub ecash_verification: bool,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifier_mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteLuaRequest {
    pub script: String,
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

const MAX_EVENT_CONTENT_BYTES: usize = 64 * 1024;
const MAX_LUA_SCRIPT_BYTES: usize = 16 * 1024;
const MAX_WASM_HEX_BYTES: usize = 512 * 1024;

pub fn router(state: Arc<AppState>) -> Router {
    let publish_routes = Router::new()
        .route("/v1/node/events/publish", post(publish_event))
        .route_layer(ConcurrencyLimitLayer::new(32));

    let exec_routes = Router::new()
        .route("/v1/node/execute/lua", post(execute_lua))
        .route("/v1/node/execute/wasm", post(execute_wasm))
        .route_layer(ConcurrencyLimitLayer::new(16));

    Router::new()
        .route("/health", get(health_check))
        .route("/v1/node/capabilities", get(node_capabilities))
        .route("/v1/node/identity", get(node_identity))
        .route("/v1/node/events/query", post(query_events))
        .route("/v1/node/pay/ecash", post(verify_ecash))
        .merge(publish_routes)
        .merge(exec_routes)
        .layer(TimeoutLayer::new(Duration::from_secs(10)))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::SERVER,
            HeaderValue::from_static("nginx/1.18.0"),
        ))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::DATE,
            HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
        ))
        .with_state(state)
}

pub async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "🐸 Froglet is Running")
}

pub async fn node_capabilities(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let transport_status = state.transport_status.lock().await.clone();
    let marketplace_status = state.marketplace_status.lock().await.clone();

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
                instruction_limit: 100_000,
            },
            wasm: WasmInfo {
                enabled: true,
                fuel_limit: 1_000_000,
                entrypoints: vec!["run".to_string(), "main".to_string()],
            },
        },
        limits: LimitsInfo {
            events_query_limit_default: 100,
            events_query_limit_max: 500,
        },
        pricing: state.pricing.info().clone(),
        payments: PaymentsInfo {
            ecash_verification: true,
            backend: state.config.payment_backend.to_string(),
            verifier_mode: matches!(
                state.config.payment_backend,
                crate::config::PaymentBackend::Cashu
            )
            .then(|| CASHU_VERIFIER_MODE.to_string()),
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

pub async fn publish_event(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PublishRequest>,
) -> impl IntoResponse {
    let event = payload.event;

    if event.content.as_bytes().len() > MAX_EVENT_CONTENT_BYTES {
        return error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            serde_json::json!({ "error": "event content too large" }),
        );
    }

    tracing::info!("Received Event Publish: {:?}", event.kind);

    if !crypto::verify_signature(&event.pubkey, &event.sig, &event.content) {
        tracing::warn!("Invalid signature for event: {}", event.id);
        return error_json(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "invalid signature" }),
        );
    }

    if let Err(e) = insert_event_db(state.as_ref(), event).await {
        tracing::error!("Failed to insert event: {}", e);
        return error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": "database error" }),
        );
    }

    (
        StatusCode::CREATED,
        Json(
            serde_json::json!({ "status": "success", "message": "event parsed and stored successfully" }),
        ),
    )
}

pub async fn query_events(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<QueryRequest>,
) -> impl IntoResponse {
    tracing::info!("Received Event Query for Kinds: {:?}", payload.kinds);

    if let Err(error) =
        payments::enforce_payment(state.as_ref(), ServiceId::EventsQuery, payload.payment).await
    {
        return error_json(error.status_code(), error.details());
    }

    match query_events_db(state.as_ref(), payload.kinds, payload.limit).await {
        Ok(events) => (
            StatusCode::OK,
            Json(serde_json::json!({ "events": events, "cursor": null })),
        ),
        Err(e) => {
            tracing::error!("Database query failed: {}", e);
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({ "error": "database error" }),
            )
        }
    }
}

pub async fn execute_lua(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ExecuteLuaRequest>,
) -> impl IntoResponse {
    if payload.script.as_bytes().len() > MAX_LUA_SCRIPT_BYTES {
        return error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            serde_json::json!({ "error": "lua script too large" }),
        );
    }

    tracing::info!("Received Lua Execution Request");

    if let Err(error) =
        payments::enforce_payment(state.as_ref(), ServiceId::ExecuteLua, payload.payment).await
    {
        return error_json(error.status_code(), error.details());
    }

    let script = payload.script;
    let res = tokio::task::spawn_blocking(move || sandbox::execute_lua_script(&script)).await;

    match res {
        Ok(Ok(result)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "success", "result": result })),
        ),
        Ok(Err(e)) => {
            tracing::error!("Lua Execution Failed: {}", e);
            error_json(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": e.to_string() }),
            )
        }
        Err(_) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": "Execution Thread Panicked or timed out" }),
        ),
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
            serde_json::json!({ "error": "wasm module too large" }),
        );
    }

    if let Err(error) =
        payments::enforce_payment(state.as_ref(), ServiceId::ExecuteWasm, payload.payment).await
    {
        return error_json(error.status_code(), error.details());
    }

    let wasm_bytes = match hex::decode(&payload.wasm_hex) {
        Ok(b) => b,
        Err(_) => {
            return error_json(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "invalid hex encoding" }),
            );
        }
    };

    let res = tokio::task::spawn_blocking(move || sandbox::execute_wasm_module(&wasm_bytes)).await;

    match res {
        Ok(Ok(result)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "success", "result": result })),
        ),
        Ok(Err(e)) => {
            tracing::error!("Wasm Execution Failed: {}", e);
            error_json(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": e.to_string() }),
            )
        }
        Err(_) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": "Execution Thread Panicked or timed out" }),
        ),
    }
}

pub async fn verify_ecash(Json(payload): Json<VerifyEcashRequest>) -> impl IntoResponse {
    tracing::info!("Received Ecash Verification Request");

    match ecash::inspect_cashu_token(&payload.token) {
        Ok(info) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "success",
                "amount_satoshis": info.amount_satoshis,
                "token_hash": info.token_hash
            })),
        ),
        Err(e) => {
            tracing::error!("Ecash Verification Failed: {}", e);
            error_json(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": e.to_string() }),
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
