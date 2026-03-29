use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{HeaderValue, Request, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use froglet::{
    api::{
        CreateDealRequest, CreateQuoteRequest, ReleaseDealPreimageRequest,
        RuntimeAcceptDealRequest, RuntimeCreateDealRequest, RuntimeProviderRef,
        RuntimeSearchRequest, public_router, runtime_router,
    },
    canonical_json,
    confidential::ConfidentialConfig,
    config::{
        DiscoveryMode, IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
        PaymentBackend, PricingConfig, ReferenceDiscoveryConfig, StorageConfig, WasmConfig,
    },
    crypto,
    db::DbPool,
    deals::DealRecord,
    discovery::{
        DiscoveryNodeRecord, DiscoverySearchResponse, NodeDescriptor, TransportDescriptor,
    },
    execution::ExecutionRuntime,
    jobs::FaaSDescriptor,
    pricing::ServicePriceInfo,
    protocol::{
        self, DescriptorCapabilities, DescriptorPayload, ExecutionLimits, OfferExecutionProfile,
        OfferPayload, OfferPriceSchedule, QuotePayload, QuoteSettlementTerms, ReceiptLegState,
        ReceiptPayload, ReceiptSettlementLeg, ReceiptSettlementRefs, SignedArtifact, WorkloadSpec,
    },
    requester_deals::RequesterDealRecord,
    state::{AppState, ReferenceDiscoveryStatus, TransportStatus},
    wasm::{
        ComputeWasmWorkload, FROGLET_SCHEMA_V1, JCS_JSON_FORMAT, WASM_MODULE_FORMAT,
        WASM_RUN_JSON_ABI_V1, WASM_SUBMISSION_TYPE_V1, WORKLOAD_KIND_COMPUTE_WASM_V1,
        WasmSubmission,
    },
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};
use tower::ServiceExt;

static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(1);

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "froglet-{prefix}-{}-{unique}-{counter}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn fixed_wasm_bytes() -> Vec<u8> {
    wat::parse_str("(module)").expect("valid wasm module")
}

fn fixed_wasm_submission() -> WasmSubmission {
    let module_bytes = fixed_wasm_bytes();
    let input = json!({"hello": "world"});
    let input_hash =
        crypto::sha256_hex(canonical_json::to_vec(&input).expect("canonical request input"));

    WasmSubmission {
        schema_version: FROGLET_SCHEMA_V1.to_string(),
        submission_type: WASM_SUBMISSION_TYPE_V1.to_string(),
        workload: ComputeWasmWorkload {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            workload_kind: WORKLOAD_KIND_COMPUTE_WASM_V1.to_string(),
            abi_version: WASM_RUN_JSON_ABI_V1.to_string(),
            module_format: WASM_MODULE_FORMAT.to_string(),
            module_hash: crypto::sha256_hex(&module_bytes),
            input_format: JCS_JSON_FORMAT.to_string(),
            input_hash,
            requested_capabilities: Vec::new(),
        },
        module_bytes_hex: hex::encode(module_bytes),
        input,
    }
}

fn build_runtime_request(provider: RuntimeProviderRef) -> RuntimeCreateDealRequest {
    RuntimeCreateDealRequest {
        provider,
        offer_id: "execute.compute".to_string(),
        spec: WorkloadSpec::Wasm {
            submission: Box::new(fixed_wasm_submission()),
        },
        max_price_sats: None,
        idempotency_key: Some("runtime-routes-test".to_string()),
        payment: None,
    }
}

fn create_test_state_with_identity_seed(
    reference_discovery_url: Option<String>,
    identity_seed: Option<[u8; 32]>,
) -> AppState {
    let temp_dir = unique_temp_dir("runtime-routes");
    let db_path = temp_dir.join("node.db");
    let node_config = NodeConfig {
        network_mode: NetworkMode::Clearnet,
        listen_addr: "127.0.0.1:0".to_string(),
        public_base_url: None,
        runtime_listen_addr: "127.0.0.1:0".to_string(),
        runtime_allow_non_loopback: false,
        provider_control_listen_addr: "127.0.0.1:0".to_string(),
        provider_control_allow_non_loopback: false,
        http_ca_cert_path: None,
        tor: froglet::config::TorSidecarConfig {
            binary_path: "tor".to_string(),
            backend_listen_addr: "127.0.0.1:0".to_string(),
            startup_timeout_secs: 90,
        },
        discovery_mode: DiscoveryMode::Reference,
        identity: IdentityConfig {
            auto_generate: true,
        },
        reference_discovery: reference_discovery_url.map(|url| ReferenceDiscoveryConfig {
            url,
            publish: true,
            required: false,
            heartbeat_interval_secs: 30,
        }),
        pricing: PricingConfig {
            events_query: 0,
            execute_wasm: 0,
        },
        payment_backend: PaymentBackend::None,
        execution_timeout_secs: 10,
        lightning: LightningConfig {
            mode: LightningMode::Mock,
            destination_identity: None,
            base_invoice_expiry_secs: 300,
            success_hold_expiry_secs: 300,
            min_final_cltv_expiry: 18,
            sync_interval_ms: 1_000,
            lnd_rest: None,
        },
        storage: StorageConfig {
            data_dir: temp_dir.clone(),
            db_path: db_path.clone(),
            identity_dir: temp_dir.join("identity"),
            identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
            nostr_publication_seed_path: temp_dir.join("identity/nostr-publication.secp256k1.seed"),
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
        confidential: ConfidentialConfig {
            policy_path: None,
            policy: None,
            session_ttl_secs: 300,
        },
    };

    if let Some(seed) = identity_seed {
        std::fs::create_dir_all(&node_config.storage.identity_dir).expect("identity dir");
        std::fs::write(&node_config.storage.identity_seed_path, hex::encode(seed))
            .expect("identity seed");
    }

    let pool = DbPool::open(&node_config.storage.db_path).expect("init db");
    let events_query_capacity = pool.read_connection_count().max(1);
    let identity = froglet::identity::NodeIdentity::load_or_create(&node_config)
        .expect("create test identity");
    let pricing = froglet::pricing::PricingTable::from_config(node_config.pricing);

    AppState {
        db: pool,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        reference_discovery_status: Arc::new(tokio::sync::Mutex::new(
            ReferenceDiscoveryStatus::from_config(&node_config),
        )),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client"),
        wasm_host: None,
        confidential_policy: None,
        runtime_auth_token: "test-runtime-token".to_string(),
        runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
        consumer_control_auth_token: "test-consumer-token".to_string(),
        consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
        provider_control_auth_token: "test-provider-token".to_string(),
        provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
        events_query_semaphore: Arc::new(tokio::sync::Semaphore::new(events_query_capacity)),
        lnd_rest_client: None,
        lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
    }
}

fn create_test_state(reference_discovery_url: Option<String>) -> AppState {
    create_test_state_with_identity_seed(reference_discovery_url, None)
}

fn provider_signing_seed(provider_state: &ProviderState) -> [u8; 32] {
    crypto::signing_key_seed_bytes(provider_state.provider_key.as_ref())
}

fn runtime_request(
    method: axum::http::Method,
    uri: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(auth) = auth {
        builder = builder.header(header::AUTHORIZATION, HeaderValue::from_str(auth).unwrap());
    }
    let body = if let Some(value) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&value).expect("serialize request"))
    } else {
        Body::empty()
    };
    builder.body(body).expect("build request")
}

async fn call_json<T: DeserializeOwned>(app: Router, request: Request<Body>) -> (StatusCode, T) {
    let response = app.oneshot(request).await.expect("request to complete");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    let payload = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "parse JSON response failed with status {}: {}; body={}",
            status,
            error,
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, payload)
}

#[derive(Clone)]
struct ProviderState {
    provider_id: String,
    provider_url: String,
    provider_key: Arc<crypto::NodeSigningKey>,
    descriptor: SignedArtifact<DescriptorPayload>,
    offers: Vec<SignedArtifact<OfferPayload>>,
    current_deal: Arc<Mutex<Option<DealRecord>>>,
    deal_delay: Arc<Mutex<Option<Duration>>>,
    tamper_descriptor: bool,
    tamper_quote: bool,
    tamper_quote_workload_hash: bool,
    tamper_receipt: bool,
    tamper_accept_receipt: bool,
    tamper_accept_receipt_semantics: bool,
}

#[derive(Clone, Copy, Default)]
struct ProviderTamperConfig {
    descriptor: bool,
    quote_signature: bool,
    quote_workload_hash: bool,
    deal_receipt: bool,
    accept_receipt: bool,
    accept_receipt_semantics: bool,
}

impl ProviderState {
    fn new(provider_url: String, tamper: ProviderTamperConfig) -> Self {
        let provider_key = Arc::new(crypto::generate_signing_key());
        let provider_id = crypto::public_key_hex(provider_key.as_ref());
        let created_at = 1_700_000_000_i64;
        let descriptor = sign_descriptor(
            provider_key.as_ref(),
            &provider_id,
            &provider_url,
            created_at,
        );
        let offers = vec![sign_offer(
            provider_key.as_ref(),
            &provider_id,
            &descriptor.hash,
            created_at,
        )];

        Self {
            provider_id,
            provider_url,
            provider_key,
            descriptor,
            offers,
            current_deal: Arc::new(Mutex::new(None)),
            deal_delay: Arc::new(Mutex::new(None)),
            tamper_descriptor: tamper.descriptor,
            tamper_quote: tamper.quote_signature,
            tamper_quote_workload_hash: tamper.quote_workload_hash,
            tamper_receipt: tamper.deal_receipt,
            tamper_accept_receipt: tamper.accept_receipt,
            tamper_accept_receipt_semantics: tamper.accept_receipt_semantics,
        }
    }
}

#[derive(Clone)]
struct DiscoveryState {
    record: DiscoveryNodeRecord,
}

#[derive(Debug, Deserialize)]
struct RuntimeCreateDealResponseView {
    provider_id: String,
    provider_url: String,
    quote: SignedArtifact<QuotePayload>,
    deal: RequesterDealRecord,
    payment_intent_path: Option<String>,
    payment_intent: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct RuntimeDealResponseView {
    deal: RequesterDealRecord,
}

#[derive(Debug, Deserialize)]
struct RuntimeAcceptDealResponseView {
    deal: RequesterDealRecord,
}

#[derive(Debug, Deserialize)]
struct RuntimeProviderDetailsResponseView {
    discovery: DiscoveryNodeRecord,
    descriptor: SignedArtifact<DescriptorPayload>,
    offers: Vec<SignedArtifact<OfferPayload>>,
}

fn sign_descriptor(
    provider_key: &crypto::NodeSigningKey,
    provider_id: &str,
    provider_url: &str,
    created_at: i64,
) -> SignedArtifact<DescriptorPayload> {
    protocol::sign_artifact(
        provider_id,
        |message| crypto::sign_message_hex(provider_key, message),
        protocol::ARTIFACT_TYPE_DESCRIPTOR,
        created_at,
        DescriptorPayload {
            provider_id: provider_id.to_string(),
            descriptor_seq: 1,
            protocol_version: FROGLET_SCHEMA_V1.to_string(),
            expires_at: None,
            linked_identities: Vec::new(),
            transport_endpoints: vec![froglet::protocol::TransportEndpoint {
                transport: "https".to_string(),
                uri: provider_url.to_string(),
                created_at: None,
                expires_at: None,
                priority: 1,
                features: Vec::new(),
            }],
            capabilities: DescriptorCapabilities {
                service_kinds: vec![WORKLOAD_KIND_COMPUTE_WASM_V1.to_string()],
                execution_runtimes: vec!["wasm".to_string()],
                max_concurrent_deals: Some(4),
            },
        },
    )
    .expect("sign descriptor")
}

fn sign_offer(
    provider_key: &crypto::NodeSigningKey,
    provider_id: &str,
    descriptor_hash: &str,
    created_at: i64,
) -> SignedArtifact<OfferPayload> {
    protocol::sign_artifact(
        provider_id,
        |message| crypto::sign_message_hex(provider_key, message),
        protocol::ARTIFACT_TYPE_OFFER,
        created_at,
        OfferPayload {
            provider_id: provider_id.to_string(),
            offer_id: "execute.compute".to_string(),
            descriptor_hash: descriptor_hash.to_string(),
            expires_at: None,
            offer_kind: WORKLOAD_KIND_COMPUTE_WASM_V1.to_string(),
            settlement_method: "none".to_string(),
            quote_ttl_secs: 300,
            execution_profile: OfferExecutionProfile {
                runtime: ExecutionRuntime::Wasm,
                package_kind: "inline_module".to_string(),
                contract_version: WASM_RUN_JSON_ABI_V1.to_string(),
                access_handles: Vec::new(),
                abi_version: WASM_RUN_JSON_ABI_V1.to_string(),
                capabilities: Vec::new(),
                max_input_bytes: 128 * 1024,
                max_runtime_ms: 10_000,
                max_memory_bytes: 64 * 1024 * 1024,
                max_output_bytes: 128 * 1024,
                fuel_limit: 10_000_000,
            },
            price_schedule: OfferPriceSchedule {
                base_fee_msat: 0,
                success_fee_msat: 0,
            },
            terms_hash: None,
            confidential_profile_hash: None,
        },
    )
    .expect("sign offer")
}

fn sign_quote(
    provider_key: &crypto::NodeSigningKey,
    provider_id: &str,
    requester_id: &str,
    descriptor_hash: &str,
    offer_hash: &str,
    workload_hash: &str,
    expires_at: i64,
) -> SignedArtifact<QuotePayload> {
    protocol::sign_artifact(
        provider_id,
        |message| crypto::sign_message_hex(provider_key, message),
        protocol::ARTIFACT_TYPE_QUOTE,
        1_700_000_100,
        QuotePayload {
            provider_id: provider_id.to_string(),
            requester_id: requester_id.to_string(),
            descriptor_hash: descriptor_hash.to_string(),
            offer_hash: offer_hash.to_string(),
            expires_at,
            workload_kind: WORKLOAD_KIND_COMPUTE_WASM_V1.to_string(),
            workload_hash: workload_hash.to_string(),
            confidential_session_hash: None,
            capabilities_granted: Vec::new(),
            extension_refs: Vec::new(),
            quote_use: None,
            settlement_terms: QuoteSettlementTerms {
                method: "none".to_string(),
                destination_identity: "".to_string(),
                base_fee_msat: 0,
                success_fee_msat: 0,
                max_base_invoice_expiry_secs: 30,
                max_success_hold_expiry_secs: 30,
                min_final_cltv_expiry: 18,
            },
            execution_limits: ExecutionLimits {
                max_input_bytes: 1024 * 1024,
                max_runtime_ms: 5_000,
                max_memory_bytes: 64 * 1024 * 1024,
                max_output_bytes: 1024 * 1024,
                fuel_limit: 10_000_000,
            },
        },
    )
    .expect("sign quote")
}

#[allow(clippy::too_many_arguments)]
fn sign_receipt(
    provider_key: &crypto::NodeSigningKey,
    provider_id: &str,
    requester_id: &str,
    quote_hash: &str,
    deal_hash: &str,
    result_hash: Option<String>,
    result: Option<Value>,
    _success_payment_hash: &str,
) -> SignedArtifact<ReceiptPayload> {
    let created_at = 1_700_000_300;
    protocol::sign_artifact(
        provider_id,
        |message| crypto::sign_message_hex(provider_key, message),
        protocol::ARTIFACT_TYPE_RECEIPT,
        created_at,
        ReceiptPayload {
            provider_id: provider_id.to_string(),
            requester_id: requester_id.to_string(),
            deal_hash: deal_hash.to_string(),
            quote_hash: quote_hash.to_string(),
            extension_refs: Vec::new(),
            acceptance_ref: None,
            started_at: Some(created_at - 10),
            finished_at: created_at,
            deal_state: "succeeded".to_string(),
            execution_state: "succeeded".to_string(),
            settlement_state: "none".to_string(),
            result_hash,
            confidential_session_hash: None,
            result_envelope_hash: None,
            result_format: Some("application/json+jcs".to_string()),
            executor: froglet::protocol::ReceiptExecutor {
                runtime: "wasm".to_string(),
                runtime_version: "test".to_string(),
                execution_mode: Some("host".to_string()),
                attestation_platform: None,
                measurement: None,
                abi_version: Some(WASM_RUN_JSON_ABI_V1.to_string()),
                module_hash: None,
                capabilities_granted: Vec::new(),
            },
            limits_applied: ExecutionLimits {
                max_input_bytes: 1024 * 1024,
                max_runtime_ms: 5_000,
                max_memory_bytes: 64 * 1024 * 1024,
                max_output_bytes: 1024 * 1024,
                fuel_limit: 10_000_000,
            },
            settlement_refs: ReceiptSettlementRefs {
                method: "none".to_string(),
                bundle_hash: None,
                destination_identity: "".to_string(),
                base_fee: ReceiptSettlementLeg {
                    amount_msat: 0,
                    invoice_hash: "".to_string(),
                    payment_hash: "".to_string(),
                    state: ReceiptLegState::Canceled,
                },
                success_fee: ReceiptSettlementLeg {
                    amount_msat: 0,
                    invoice_hash: "".to_string(),
                    payment_hash: "".to_string(),
                    state: ReceiptLegState::Canceled,
                },
            },
            failure_code: None,
            failure_message: None,
            result_ref: result.map(|_| "result://inline".to_string()),
        },
    )
    .expect("sign receipt")
}

fn resign_receipt(
    provider_key: &crypto::NodeSigningKey,
    provider_id: &str,
    created_at: i64,
    payload: ReceiptPayload,
) -> SignedArtifact<ReceiptPayload> {
    protocol::sign_artifact(
        provider_id,
        |message| crypto::sign_message_hex(provider_key, message),
        protocol::ARTIFACT_TYPE_RECEIPT,
        created_at,
        payload,
    )
    .expect("re-sign receipt")
}

fn tamper_signature<T>(artifact: &mut SignedArtifact<T>) {
    artifact.signature.push('0');
}

fn provider_router(state: Arc<ProviderState>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/provider/descriptor", get(provider_descriptor))
        .route("/v1/provider/offers", get(provider_offers))
        .route("/v1/provider/quotes", post(provider_quote))
        .route("/v1/provider/deals", post(provider_deals))
        .route("/v1/provider/deals/:deal_id", get(provider_deal))
        .route("/v1/provider/deals/:deal_id/accept", post(provider_accept))
        .with_state(state)
}

fn discovery_router(state: Arc<DiscoveryState>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/discovery/search", post(discovery_search))
        .route("/v1/discovery/providers/:node_id", get(discovery_provider))
        .with_state(state)
}

async fn provider_descriptor(State(state): State<Arc<ProviderState>>) -> impl IntoResponse {
    let mut descriptor = state.descriptor.clone();
    if state.tamper_descriptor {
        tamper_signature(&mut descriptor);
    }
    (StatusCode::OK, Json(descriptor))
}

async fn provider_offers(State(state): State<Arc<ProviderState>>) -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "offers": state.offers })))
}

async fn provider_quote(
    State(state): State<Arc<ProviderState>>,
    Json(payload): Json<CreateQuoteRequest>,
) -> impl IntoResponse {
    assert_eq!(payload.offer_id, "execute.compute");
    let submission_hash = payload.spec.request_hash().expect("quote request hash");
    let workload_hash = if state.tamper_quote_workload_hash {
        crypto::sha256_hex(b"tampered-request")
    } else {
        submission_hash
    };
    let mut quote = sign_quote(
        &state.provider_key,
        &state.provider_id,
        &payload.requester_id,
        &state.descriptor.hash,
        &state.offers[0].hash,
        &workload_hash,
        1_700_001_000,
    );
    if state.tamper_quote {
        tamper_signature(&mut quote);
    }

    (StatusCode::OK, Json(quote))
}

async fn provider_deals(
    State(state): State<Arc<ProviderState>>,
    Json(payload): Json<CreateDealRequest>,
) -> impl IntoResponse {
    let deal_id = payload.deal.hash.clone();
    let record = DealRecord {
        deal_id: deal_id.clone(),
        idempotency_key: payload.idempotency_key.clone(),
        status: "accepted".to_string(),
        workload_kind: payload.spec.workload_kind().to_string(),
        deal: payload.deal.clone(),
        quote: payload.quote.clone(),
        result: None,
        result_hash: None,
        error: None,
        receipt: None,
        created_at: 1_700_000_250,
        updated_at: 1_700_000_250,
    };
    *state.current_deal.lock().await = Some(record.clone());
    (StatusCode::OK, Json(record))
}

async fn provider_deal(
    State(state): State<Arc<ProviderState>>,
    Path(deal_id): Path<String>,
) -> impl IntoResponse {
    if let Some(delay) = *state.deal_delay.lock().await {
        tokio::time::sleep(delay).await;
    }
    let current = state.current_deal.lock().await;
    let record = current
        .as_ref()
        .expect("deal should have been created")
        .clone();
    assert_eq!(record.deal_id, deal_id);
    let mut record = record;
    if state.tamper_receipt {
        let result = json!({"ok": true});
        let result_hash = Some(crypto::sha256_hex(
            canonical_json::to_vec(&result).expect("canonical result"),
        ));
        let mut bad_receipt = sign_receipt(
            &state.provider_key,
            &state.provider_id,
            &record.quote.payload.requester_id,
            &record.quote.hash,
            &record.deal.hash,
            result_hash,
            Some(result),
            &record.deal.payload.success_payment_hash,
        );
        tamper_signature(&mut bad_receipt);
        record.receipt = Some(bad_receipt);
    }
    (StatusCode::OK, Json(record))
}

async fn provider_accept(
    State(state): State<Arc<ProviderState>>,
    Path(deal_id): Path<String>,
    Json(payload): Json<ReleaseDealPreimageRequest>,
) -> impl IntoResponse {
    let mut record = {
        let current = state.current_deal.lock().await;
        current
            .as_ref()
            .expect("deal should have been created")
            .clone()
    };
    assert_eq!(record.deal_id, deal_id);
    let decrypted = hex::decode(payload.success_preimage).expect("success preimage hex");
    assert_eq!(
        crypto::sha256_hex(decrypted),
        record.deal.payload.success_payment_hash
    );
    let result = json!({"accepted": true});
    let result_hash = Some(crypto::sha256_hex(
        canonical_json::to_vec(&result).expect("canonical result"),
    ));
    if let Some(expected_result_hash) = payload.expected_result_hash.as_deref()
        && Some(expected_result_hash) != result_hash.as_deref()
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "expected_result_hash does not match the persisted deal result",
                "expected_result_hash": expected_result_hash,
                "result_hash": result_hash,
            })),
        )
            .into_response();
    }
    let receipt = sign_receipt(
        &state.provider_key,
        &state.provider_id,
        &record.quote.payload.requester_id,
        &record.quote.hash,
        &record.deal.hash,
        result_hash.clone(),
        Some(result.clone()),
        &record.deal.payload.success_payment_hash,
    );
    let receipt = if state.tamper_accept_receipt_semantics {
        let mut invalid_payload = receipt.payload.clone();
        invalid_payload.settlement_state = "settled".to_string();
        resign_receipt(
            &state.provider_key,
            &state.provider_id,
            receipt.created_at,
            invalid_payload,
        )
    } else if state.tamper_accept_receipt {
        let mut tampered = receipt;
        tamper_signature(&mut tampered);
        tampered
    } else {
        receipt
    };
    record.status = "succeeded".to_string();
    record.result = Some(result);
    record.result_hash = result_hash;
    record.receipt = Some(receipt);
    record.updated_at = 1_700_000_350;
    *state.current_deal.lock().await = Some(record.clone());

    (StatusCode::OK, Json(record)).into_response()
}

async fn discovery_search(State(state): State<Arc<DiscoveryState>>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(DiscoverySearchResponse {
            nodes: vec![state.record.clone()],
        }),
    )
}

async fn discovery_provider(
    State(state): State<Arc<DiscoveryState>>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    if state.record.descriptor.node_id != node_id {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "node not found" })),
        )
            .into_response();
    }
    (StatusCode::OK, Json(state.record.clone())).into_response()
}

struct TestServer {
    base_url: String,
    join_handle: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn spawn_server(app: Router) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("server addr");
    let base_url = format!("http://{}", addr);
    let join_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    TestServer {
        base_url,
        join_handle,
    }
}

async fn build_provider_fixture(
    tamper_descriptor: bool,
    tamper_quote: bool,
    tamper_receipt: bool,
) -> (TestServer, Arc<ProviderState>) {
    build_provider_fixture_with_tamper(ProviderTamperConfig {
        descriptor: tamper_descriptor,
        quote_signature: tamper_quote,
        deal_receipt: tamper_receipt,
        ..ProviderTamperConfig::default()
    })
    .await
}

async fn build_provider_fixture_with_tamper(
    tamper: ProviderTamperConfig,
) -> (TestServer, Arc<ProviderState>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind provider listener");
    let addr = listener.local_addr().expect("provider listener addr");
    let provider_url = format!("http://{}", addr);
    let state = Arc::new(ProviderState::new(provider_url.clone(), tamper));
    let app = provider_router(state.clone());
    let join_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (
        TestServer {
            base_url: provider_url,
            join_handle,
        },
        state,
    )
}

async fn build_provider_fixture_with_delay(
    tamper_descriptor: bool,
    tamper_quote: bool,
    tamper_receipt: bool,
    deal_delay: Option<Duration>,
) -> (TestServer, Arc<ProviderState>) {
    let (server, state) =
        build_provider_fixture(tamper_descriptor, tamper_quote, tamper_receipt).await;
    *state.deal_delay.lock().await = deal_delay;
    (server, state)
}

async fn build_discovery_fixture(provider_state: &ProviderState) -> TestServer {
    build_discovery_fixture_with_provider_id(provider_state, &provider_state.provider_id).await
}

async fn build_discovery_fixture_with_transports(
    _provider_state: &ProviderState,
    discovery_provider_id: &str,
    clearnet_url: Option<String>,
    onion_url: Option<String>,
    tor_status: &str,
) -> TestServer {
    let record = DiscoveryNodeRecord {
        descriptor: NodeDescriptor {
            node_id: discovery_provider_id.to_string(),
            pubkey: discovery_provider_id.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            discovery_mode: "reference".to_string(),
            transports: TransportDescriptor {
                clearnet_url,
                onion_url,
                tor_status: tor_status.to_string(),
            },
            services: vec![ServicePriceInfo {
                service_id: "execute.compute".to_string(),
                price_sats: 0,
                payment_required: false,
            }],
            faas: FaaSDescriptor::standard(),
            updated_at: None,
        },
        status: "active".to_string(),
        registered_at: 1_700_000_000,
        updated_at: 1_700_000_000,
        last_seen_at: 1_700_000_000,
    };
    spawn_server(discovery_router(Arc::new(DiscoveryState { record }))).await
}

async fn build_discovery_fixture_with_provider_id(
    provider_state: &ProviderState,
    discovery_provider_id: &str,
) -> TestServer {
    build_discovery_fixture_with_transports(
        provider_state,
        discovery_provider_id,
        Some(provider_state.provider_url.clone()),
        None,
        "disabled",
    )
    .await
}

#[tokio::test]
async fn runtime_auth_rejection_blocks_unauthenticated_requests() {
    let state = Arc::new(create_test_state(None));
    let app = runtime_router(state);
    let request = runtime_request(
        axum::http::Method::GET,
        "/v1/runtime/wallet/balance",
        None,
        None,
    );
    let (status, response): (StatusCode, Value) = call_json(app, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        response["error"],
        Value::String("missing runtime authorization".to_string())
    );
}

#[tokio::test]
async fn runtime_auth_rejection_blocks_invalid_bearer_tokens() {
    let state = Arc::new(create_test_state(None));
    let app = runtime_router(state);
    let request = runtime_request(
        axum::http::Method::GET,
        "/v1/runtime/wallet/balance",
        Some("Bearer wrong-runtime-token"),
        None,
    );
    let (status, response): (StatusCode, Value) = call_json(app, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        response["error"],
        Value::String("invalid runtime authorization token".to_string())
    );
}

#[tokio::test]
async fn direct_provider_runtime_roundtrip_succeeds() {
    let (provider_server, provider_state) = build_provider_fixture(false, false, false).await;
    let state = Arc::new(create_test_state_with_identity_seed(
        None,
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state.clone());

    let request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: Some(provider_server.base_url.clone()),
            }))
            .expect("serialize request"),
        ),
    );
    let (status, response): (StatusCode, RuntimeCreateDealResponseView) =
        call_json(app.clone(), request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response.provider_id, provider_state.provider_id);
    assert_eq!(response.provider_url, provider_server.base_url);
    assert_eq!(
        response.quote.payload.provider_id,
        provider_state.provider_id
    );
    assert_eq!(response.deal.status, "accepted");
    assert!(response.payment_intent_path.is_none());
    assert!(response.payment_intent.is_none());

    let get_request = runtime_request(
        axum::http::Method::GET,
        &format!("/v1/runtime/deals/{}", response.deal.deal_id),
        Some("Bearer test-runtime-token"),
        None,
    );
    let (get_status, get_response): (StatusCode, RuntimeDealResponseView) =
        call_json(app.clone(), get_request).await;
    assert_eq!(get_status, StatusCode::OK);
    assert_eq!(get_response.deal.deal_id, response.deal.deal_id);
    assert_eq!(get_response.deal.status, "accepted");
    assert!(get_response.deal.receipt.is_none());

    let accept_request = runtime_request(
        axum::http::Method::POST,
        &format!("/v1/runtime/deals/{}/accept", response.deal.deal_id),
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(RuntimeAcceptDealRequest {
                expected_result_hash: None,
            })
            .expect("serialize request"),
        ),
    );
    let (accept_status, accept_response): (StatusCode, RuntimeAcceptDealResponseView) =
        call_json(app.clone(), accept_request).await;
    assert_eq!(accept_status, StatusCode::OK);
    assert_eq!(accept_response.deal.status, "succeeded");
    assert!(accept_response.deal.receipt.is_some());

    let final_get_request = runtime_request(
        axum::http::Method::GET,
        &format!("/v1/runtime/deals/{}", response.deal.deal_id),
        Some("Bearer test-runtime-token"),
        None,
    );
    let (final_status, final_response): (StatusCode, RuntimeDealResponseView) =
        call_json(app, final_get_request).await;
    assert_eq!(final_status, StatusCode::OK);
    assert_eq!(final_response.deal.status, "succeeded");
    assert!(final_response.deal.receipt.is_some());
}

#[tokio::test]
async fn reference_discovery_runtime_search_and_details_succeed() {
    let (provider_server, provider_state) = build_provider_fixture(false, false, false).await;
    let discovery_server = build_discovery_fixture(&provider_state).await;

    let state = Arc::new(create_test_state_with_identity_seed(
        Some(discovery_server.base_url.clone()),
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let search_request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/search",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(RuntimeSearchRequest {
                limit: Some(10),
                include_inactive: Some(false),
            })
            .expect("serialize request"),
        ),
    );
    let (search_status, search_response): (StatusCode, DiscoverySearchResponse) =
        call_json(app.clone(), search_request).await;
    assert_eq!(search_status, StatusCode::OK);
    assert_eq!(search_response.nodes.len(), 1);
    assert_eq!(
        search_response.nodes[0].descriptor.node_id,
        provider_state.provider_id
    );

    let details_request = runtime_request(
        axum::http::Method::GET,
        &format!("/v1/runtime/providers/{}", provider_state.provider_id),
        Some("Bearer test-runtime-token"),
        None,
    );
    let (details_status, details_response): (StatusCode, RuntimeProviderDetailsResponseView) =
        call_json(app.clone(), details_request).await;
    assert_eq!(details_status, StatusCode::OK);
    assert_eq!(
        details_response.discovery.descriptor.node_id,
        provider_state.provider_id
    );
    assert_eq!(
        details_response.descriptor.payload.provider_id,
        provider_state.provider_id
    );
    assert_eq!(details_response.offers.len(), 1);
    assert_eq!(
        details_response.offers[0].payload.offer_id,
        "execute.compute"
    );

    let create_request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: None,
            }))
            .expect("serialize request"),
        ),
    );
    let (create_status, create_response): (StatusCode, RuntimeCreateDealResponseView) =
        call_json(app, create_request).await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(create_response.provider_id, provider_state.provider_id);
    assert_eq!(create_response.provider_url, provider_server.base_url);
    assert_eq!(
        create_response.quote.payload.provider_id,
        provider_state.provider_id
    );
}

#[tokio::test]
async fn reference_discovery_runtime_details_and_buy_fall_back_to_onion_url() {
    let (provider_server, provider_state) = build_provider_fixture(false, false, false).await;
    let discovery_server = build_discovery_fixture_with_transports(
        &provider_state,
        &provider_state.provider_id,
        None,
        Some(provider_server.base_url.clone()),
        "up",
    )
    .await;

    let state = Arc::new(create_test_state_with_identity_seed(
        Some(discovery_server.base_url.clone()),
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let search_request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/search",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(RuntimeSearchRequest {
                limit: Some(10),
                include_inactive: Some(false),
            })
            .expect("serialize request"),
        ),
    );
    let (search_status, search_response): (StatusCode, DiscoverySearchResponse) =
        call_json(app.clone(), search_request).await;
    assert_eq!(search_status, StatusCode::OK);
    assert_eq!(search_response.nodes.len(), 1);
    assert_eq!(
        search_response.nodes[0].descriptor.transports.clearnet_url,
        None
    );
    assert_eq!(
        search_response.nodes[0]
            .descriptor
            .transports
            .onion_url
            .as_deref(),
        Some(provider_server.base_url.as_str())
    );

    let details_request = runtime_request(
        axum::http::Method::GET,
        &format!("/v1/runtime/providers/{}", provider_state.provider_id),
        Some("Bearer test-runtime-token"),
        None,
    );
    let (details_status, details_response): (StatusCode, RuntimeProviderDetailsResponseView) =
        call_json(app.clone(), details_request).await;
    assert_eq!(details_status, StatusCode::OK);
    assert_eq!(
        details_response
            .discovery
            .descriptor
            .transports
            .clearnet_url,
        None
    );
    assert_eq!(
        details_response
            .discovery
            .descriptor
            .transports
            .onion_url
            .as_deref(),
        Some(provider_server.base_url.as_str())
    );

    let create_request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: None,
            }))
            .expect("serialize request"),
        ),
    );
    let (create_status, create_response): (StatusCode, RuntimeCreateDealResponseView) =
        call_json(app, create_request).await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(create_response.provider_id, provider_state.provider_id);
    assert_eq!(create_response.provider_url, provider_server.base_url);
}

#[tokio::test]
async fn runtime_create_deal_rejects_tampered_provider_descriptor() {
    let (provider_server, provider_state) = build_provider_fixture(true, false, false).await;
    let state = Arc::new(create_test_state_with_identity_seed(
        None,
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: Some(provider_server.base_url.clone()),
            }))
            .expect("serialize request"),
        ),
    );
    let (status, response): (StatusCode, Value) = call_json(app, request).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        response["error"],
        Value::String("provider descriptor signature verification failed".to_string())
    );
}

#[tokio::test]
async fn runtime_provider_details_should_reject_tampered_provider_descriptor() {
    let (_provider_server, provider_state) = build_provider_fixture(true, false, false).await;
    let discovery_server = build_discovery_fixture(&provider_state).await;
    let state = Arc::new(create_test_state_with_identity_seed(
        Some(discovery_server.base_url.clone()),
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let request = runtime_request(
        axum::http::Method::GET,
        &format!("/v1/runtime/providers/{}", provider_state.provider_id),
        Some("Bearer test-runtime-token"),
        None,
    );
    let (status, _response): (StatusCode, Value) = call_json(app, request).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn runtime_provider_details_returns_not_found_for_missing_provider() {
    let (_provider_server, provider_state) = build_provider_fixture(false, false, false).await;
    let discovery_server = build_discovery_fixture(&provider_state).await;
    let state = Arc::new(create_test_state_with_identity_seed(
        Some(discovery_server.base_url.clone()),
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);
    let missing_provider_id = "00".repeat(32);

    let request = runtime_request(
        axum::http::Method::GET,
        &format!("/v1/runtime/providers/{missing_provider_id}"),
        Some("Bearer test-runtime-token"),
        None,
    );
    let (status, response): (StatusCode, Value) = call_json(app, request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        response["error"],
        Value::String("node not found".to_string())
    );
}

#[tokio::test]
async fn runtime_get_deal_should_reject_tampered_provider_receipt() {
    let (provider_server, provider_state) = build_provider_fixture(false, false, true).await;
    let state = Arc::new(create_test_state_with_identity_seed(
        None,
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state.clone());

    let create_request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: Some(provider_server.base_url.clone()),
            }))
            .expect("serialize request"),
        ),
    );
    let (_status, response): (StatusCode, RuntimeCreateDealResponseView) =
        call_json(app.clone(), create_request).await;
    assert_eq!(response.provider_id, provider_state.provider_id);

    let get_request = runtime_request(
        axum::http::Method::GET,
        &format!("/v1/runtime/deals/{}", response.deal.deal_id),
        Some("Bearer test-runtime-token"),
        None,
    );
    let (status, _response): (StatusCode, Value) = call_json(app, get_request).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn runtime_get_deal_allows_provider_sync_beyond_default_timeout() {
    let (provider_server, provider_state) =
        build_provider_fixture_with_delay(false, false, false, None).await;
    let mut state =
        create_test_state_with_identity_seed(None, Some(provider_signing_seed(&provider_state)));
    state.http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client");
    let app = runtime_router(Arc::new(state));

    let create_request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: Some(provider_server.base_url.clone()),
            }))
            .expect("serialize request"),
        ),
    );
    let (create_status, response): (StatusCode, RuntimeCreateDealResponseView) =
        call_json(app.clone(), create_request).await;
    assert_eq!(create_status, StatusCode::OK);

    *provider_state.deal_delay.lock().await = Some(Duration::from_secs(11));

    let get_request = runtime_request(
        axum::http::Method::GET,
        &format!("/v1/runtime/deals/{}", response.deal.deal_id),
        Some("Bearer test-runtime-token"),
        None,
    );
    let (status, response): (StatusCode, RuntimeDealResponseView) =
        call_json(app, get_request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response.deal.deal_id,
        provider_state
            .current_deal
            .lock()
            .await
            .as_ref()
            .unwrap()
            .deal_id
    );
}

#[tokio::test]
async fn runtime_create_deal_rejects_tampered_provider_quote() {
    let (provider_server, provider_state) = build_provider_fixture(false, true, false).await;
    let state = Arc::new(create_test_state_with_identity_seed(
        None,
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: Some(provider_server.base_url.clone()),
            }))
            .expect("serialize request"),
        ),
    );
    let (status, response): (StatusCode, Value) = call_json(app, request).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        response["error"],
        Value::String("provider quote signature verification failed".to_string())
    );
}

#[tokio::test]
async fn runtime_create_deal_rejects_provider_quote_with_mismatched_workload_hash() {
    let (provider_server, provider_state) =
        build_provider_fixture_with_tamper(ProviderTamperConfig {
            quote_workload_hash: true,
            ..ProviderTamperConfig::default()
        })
        .await;
    let state = Arc::new(create_test_state_with_identity_seed(
        None,
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: Some(provider_server.base_url.clone()),
            }))
            .expect("serialize request"),
        ),
    );
    let (status, response): (StatusCode, Value) = call_json(app, request).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        response["error"],
        Value::String("provider quote workload_hash does not match requested workload".to_string())
    );
}

#[tokio::test]
async fn runtime_accept_deal_rejects_tampered_provider_receipt() {
    let (provider_server, provider_state) =
        build_provider_fixture_with_tamper(ProviderTamperConfig {
            accept_receipt: true,
            ..ProviderTamperConfig::default()
        })
        .await;
    let state = Arc::new(create_test_state_with_identity_seed(
        None,
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let create_request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: Some(provider_server.base_url.clone()),
            }))
            .expect("serialize request"),
        ),
    );
    let (create_status, create_response): (StatusCode, RuntimeCreateDealResponseView) =
        call_json(app.clone(), create_request).await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(create_response.provider_id, provider_state.provider_id);

    let accept_request = runtime_request(
        axum::http::Method::POST,
        &format!("/v1/runtime/deals/{}/accept", create_response.deal.deal_id),
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(RuntimeAcceptDealRequest {
                expected_result_hash: None,
            })
            .expect("serialize request"),
        ),
    );
    let (accept_status, accept_response): (StatusCode, Value) =
        call_json(app, accept_request).await;
    assert_eq!(accept_status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        accept_response["error"],
        Value::String("provider receipt signature verification failed".to_string())
    );
}

#[tokio::test]
async fn verify_receipt_rejects_signed_receipt_with_invalid_semantics() {
    let state = Arc::new(create_test_state(None));
    let app = public_router(state);
    let provider_key = crypto::generate_signing_key();
    let provider_id = crypto::public_key_hex(&provider_key);
    let mut receipt = sign_receipt(
        &provider_key,
        &provider_id,
        &"11".repeat(32),
        &"22".repeat(32),
        &"33".repeat(32),
        Some("44".repeat(32)),
        Some(json!({"ok": true})),
        &"55".repeat(32),
    );
    receipt.payload.settlement_state = "settled".to_string();
    let receipt = resign_receipt(
        &provider_key,
        &provider_id,
        receipt.created_at,
        receipt.payload,
    );

    let request = runtime_request(
        axum::http::Method::POST,
        "/v1/receipts/verify",
        None,
        Some(json!({ "receipt": receipt })),
    );
    let (status, response): (StatusCode, Value) = call_json(app, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["valid"], Value::Bool(false));
}

#[tokio::test]
async fn runtime_accept_deal_rejects_provider_receipt_with_invalid_semantics() {
    let (provider_server, provider_state) =
        build_provider_fixture_with_tamper(ProviderTamperConfig {
            accept_receipt_semantics: true,
            ..ProviderTamperConfig::default()
        })
        .await;
    let state = Arc::new(create_test_state_with_identity_seed(
        None,
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let create_request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: Some(provider_server.base_url.clone()),
            }))
            .expect("serialize request"),
        ),
    );
    let (create_status, create_response): (StatusCode, RuntimeCreateDealResponseView) =
        call_json(app.clone(), create_request).await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(create_response.provider_id, provider_state.provider_id);

    let accept_request = runtime_request(
        axum::http::Method::POST,
        &format!("/v1/runtime/deals/{}/accept", create_response.deal.deal_id),
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(RuntimeAcceptDealRequest {
                expected_result_hash: None,
            })
            .expect("serialize request"),
        ),
    );
    let (accept_status, accept_response): (StatusCode, Value) =
        call_json(app, accept_request).await;
    assert_eq!(accept_status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        accept_response["error"],
        Value::String(
            "provider receipt semantic validation failed: free receipt settlement_state must be none"
                .to_string()
        )
    );
}

#[tokio::test]
async fn runtime_accept_deal_preserves_provider_conflict_for_expected_result_hash_mismatch() {
    let (provider_server, provider_state) = build_provider_fixture(false, false, false).await;
    let state = Arc::new(create_test_state_with_identity_seed(
        None,
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let create_request = runtime_request(
        axum::http::Method::POST,
        "/v1/runtime/deals",
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(build_runtime_request(RuntimeProviderRef {
                provider_id: Some(provider_state.provider_id.clone()),
                provider_url: Some(provider_server.base_url.clone()),
            }))
            .expect("serialize request"),
        ),
    );
    let (create_status, create_response): (StatusCode, RuntimeCreateDealResponseView) =
        call_json(app.clone(), create_request).await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(create_response.provider_id, provider_state.provider_id);

    let accept_request = runtime_request(
        axum::http::Method::POST,
        &format!("/v1/runtime/deals/{}/accept", create_response.deal.deal_id),
        Some("Bearer test-runtime-token"),
        Some(
            serde_json::to_value(RuntimeAcceptDealRequest {
                expected_result_hash: Some("00".repeat(32)),
            })
            .expect("serialize request"),
        ),
    );
    let (accept_status, accept_response): (StatusCode, Value) =
        call_json(app, accept_request).await;
    assert_eq!(accept_status, StatusCode::CONFLICT);
    assert_eq!(
        accept_response["error"],
        Value::String("expected_result_hash does not match the persisted deal result".to_string())
    );
    assert_eq!(
        accept_response["expected_result_hash"],
        Value::String("00".repeat(32))
    );
    assert!(accept_response["result_hash"].as_str().is_some());
}

#[tokio::test]
async fn runtime_provider_details_rejects_discovery_provider_mismatch() {
    let (_provider_server, provider_state) = build_provider_fixture(false, false, false).await;
    let mismatched_provider_id = "ff".repeat(32);
    let discovery_server =
        build_discovery_fixture_with_provider_id(&provider_state, &mismatched_provider_id).await;
    let state = Arc::new(create_test_state_with_identity_seed(
        Some(discovery_server.base_url.clone()),
        Some(provider_signing_seed(&provider_state)),
    ));
    let app = runtime_router(state);

    let request = runtime_request(
        axum::http::Method::GET,
        &format!("/v1/runtime/providers/{mismatched_provider_id}"),
        Some("Bearer test-runtime-token"),
        None,
    );
    let (status, response): (StatusCode, Value) = call_json(app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        response["error"],
        Value::String(
            "provider URL targets a local or private-network address and is only allowed for the local node via FROGLET_RUNTIME_PROVIDER_BASE_URL"
                .to_string()
        )
    );
}
