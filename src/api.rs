use axum::{
    Json, Router,
    error_handling::HandleErrorLayer,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tower::{BoxError, ServiceBuilder, limit::ConcurrencyLimitLayer, timeout::TimeoutLayer};

use crate::{
    crypto, db, ecash,
    jobs::{self, JobPaymentReceipt, JobSpec, NewJob},
    payments::{self, CASHU_VERIFIER_MODE, PaymentReceipt, ProvidedPayment},
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

const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_EVENT_CONTENT_BYTES: usize = 64 * 1024;
const MAX_LUA_SCRIPT_BYTES: usize = 16 * 1024;
const MAX_WASM_HEX_BYTES: usize = 512 * 1024;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;

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

    Router::new()
        .route("/health", get(health_check))
        .route("/v1/node/capabilities", get(node_capabilities))
        .route("/v1/node/identity", get(node_identity))
        .route("/v1/node/events/query", post(query_events))
        .route("/v1/node/pay/ecash", post(verify_ecash))
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
            backend: state.config.payment_backend.to_string(),
            verifier_mode: matches!(
                state.config.payment_backend,
                crate::config::PaymentBackend::Cashu
            )
            .then(|| CASHU_VERIFIER_MODE.to_string()),
            reservations: true,
            receipts: true,
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

    match run_job_spec_now(JobSpec::Lua {
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

    match run_job_spec_now(JobSpec::Wasm {
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
                "token_hash": info.token_hash
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

async fn run_job_spec_now(spec: JobSpec) -> Result<Value, String> {
    match spec {
        JobSpec::Lua { script, input } => {
            let result = tokio::task::spawn_blocking(move || {
                sandbox::execute_lua_script(&script, input.as_ref())
            })
            .await
            .map_err(|e| format!("execution thread panicked: {e}"))?;
            result.map_err(|e| e.to_string())
        }
        JobSpec::Wasm { wasm_hex } => {
            let wasm_bytes =
                hex::decode(&wasm_hex).map_err(|_| "invalid hex encoding".to_string())?;
            let result =
                tokio::task::spawn_blocking(move || sandbox::execute_wasm_module(&wasm_bytes))
                    .await
                    .map_err(|e| format!("execution thread panicked: {e}"))?;
            result.map(|value| json!(value)).map_err(|e| e.to_string())
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

    match run_job_spec_now(job.spec.clone()).await {
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
                        let _ = db::release_payment_token(conn, &token_hash, &job_id)?;
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
