/// Full paid-deal lifecycle integration tests.
///
/// Each test drives a complete deal through the real froglet HTTP handlers:
///   Runtime side: `POST /v1/runtime/deals` → deal creation, quote, execution
///   Provider side: `POST /v1/provider/quotes`, `/deals`, `/deals/{id}/accept`
///
/// Test 1 — Lightning mock:
///   quote → deal (+ invoice bundle) → mock-pay (fund base hold) →
///   wait for result_ready → release preimage (accept) →
///   signed receipt with settlement_state="settled".
///
/// Test 2 — Stripe MPP (mock server):
///   buyer mints SPT (mock) → quote → deal + PaymentIntent reservation →
///   execution + capture → signed receipt with settlement_state="settled".
use axum::{
    Json as AxumJson, Router,
    extract::{Path as AxumPath, State as AxumState},
    http::{StatusCode, header},
    routing::{get as axum_get, post as axum_post},
};
use froglet::{
    api::{public_router, runtime_router},
    confidential::ConfidentialConfig,
    config::{
        BuyerStripeConfig, IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
        PaymentBackend, PricingConfig, StorageConfig, StripeConfig, WasmConfig,
    },
    crypto,
    db::DbPool,
    deals,
    protocol::{validate_receipt_artifact, verify_artifact},
    settlement::{self, SettlementRegistry},
    state::{AppState, TransportStatus},
    wasm::{
        ComputeWasmWorkload, FROGLET_SCHEMA_V1, JCS_JSON_FORMAT, WASM_MODULE_FORMAT,
        WASM_RUN_JSON_ABI_V1, WASM_SUBMISSION_TYPE_V1, WORKLOAD_KIND_COMPUTE_WASM_V1,
        WasmSubmission,
    },
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpListener;

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Serialises tests that mutate `FROGLET_RUNTIME_PROVIDER_BASE_URL` so they
/// don't corrupt each other's environment.
static ENV_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    ENV_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<String>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: test-only mutation; documented requirement to hold env_lock.
        unsafe { std::env::set_var(key, value) }
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        match self.previous.as_deref() {
            Some(val) => unsafe { std::env::set_var(self.key, val) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "froglet-full-deal-{prefix}-{}-{unique}-{counter}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn wasm_module_bytes() -> Vec<u8> {
    // Minimal valid WASM module (returns integer 42).
    hex::decode("0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432")
        .expect("valid wasm hex")
}

fn test_wasm_submission() -> WasmSubmission {
    let module_bytes = wasm_module_bytes();
    let input = json!({"hello": "world"});
    let input_hash =
        crypto::sha256_hex(froglet::canonical_json::to_vec(&input).expect("canonical input"));

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

struct TestServer {
    pub base_url: String,
    _handle: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self._handle.abort();
    }
}

async fn spawn_server(app: Router) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("server addr");
    let base_url = format!("http://{addr}");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    TestServer {
        base_url,
        _handle: handle,
    }
}

async fn http_post_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    auth: Option<&str>,
    body: &Value,
) -> (StatusCode, T) {
    let mut req = client
        .post(url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(body);
    if let Some(token) = auth {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let resp = req.send().await.expect("HTTP POST");
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap();
    let bytes = resp.bytes().await.expect("read response bytes");
    let parsed = serde_json::from_slice(&bytes).unwrap_or_else(|err| {
        panic!(
            "Failed to parse response (status {status}): {err}\nbody={}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, parsed)
}

async fn http_post_empty_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    auth: Option<&str>,
) -> (StatusCode, T) {
    http_post_json(client, url, auth, &json!({})).await
}

async fn http_get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    auth: Option<&str>,
) -> (StatusCode, T) {
    let mut req = client.get(url);
    if let Some(token) = auth {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let resp = req.send().await.expect("HTTP GET");
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap();
    let bytes = resp.bytes().await.expect("read response bytes");
    let parsed = serde_json::from_slice(&bytes).unwrap_or_else(|err| {
        panic!(
            "Failed to parse GET response (status {status}): {err}\nbody={}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, parsed)
}

/// Wait for a deal in the provider DB to reach the expected status.
/// Polls at 25ms intervals for up to 5 seconds.
async fn wait_for_deal_status(
    state: &Arc<AppState>,
    deal_id: &str,
    expected: &str,
) -> deals::StoredDeal {
    for _ in 0..200 {
        let id = deal_id.to_string();
        let deal = state
            .db
            .with_read_conn(move |conn| deals::get_deal(conn, &id))
            .await
            .expect("db read")
            .expect("deal exists");
        if deal.status == expected {
            return deal;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let id = deal_id.to_string();
    let deal = state
        .db
        .with_read_conn(move |conn| deals::get_deal(conn, &id))
        .await
        .expect("db read")
        .expect("deal exists");
    panic!(
        "Timed out waiting for deal {deal_id} to reach {expected}; current={}",
        deal.status
    );
}

// ─── AppState builders ────────────────────────────────────────────────────────

fn lightning_app_state() -> Arc<AppState> {
    let temp_dir = unique_temp_dir("lightning");
    let db_path = temp_dir.join("node.db");

    let node_config = NodeConfig {
        network_mode: NetworkMode::Clearnet,
        listen_addr: "127.0.0.1:0".to_string(),
        public_base_url: None,
        runtime_listen_addr: "127.0.0.1:0".to_string(),
        runtime_allow_non_loopback: false,
        http_ca_cert_path: None,
        tor: froglet::config::TorSidecarConfig {
            binary_path: "tor".to_string(),
            backend_listen_addr: "127.0.0.1:0".to_string(),
            startup_timeout_secs: 90,
        },
        identity: IdentityConfig {
            auto_generate: true,
        },
        pricing: PricingConfig {
            events_query: 0,
            // Provider's pricing table is not used for priced offers — the price
            // lives in the offer's price_schedule.  Set to non-zero so the default
            // offer is priced when we publish it below.
            execute_wasm: 30,
        },
        payment_backends: vec![PaymentBackend::Lightning],
        execution_timeout_secs: 10,
        lightning: LightningConfig {
            mode: LightningMode::Mock,
            destination_identity: None,
            base_invoice_expiry_secs: 300,
            success_hold_expiry_secs: 300,
            min_final_cltv_expiry: 18,
            sync_interval_ms: 100,
            lnd_rest: None,
        },
        x402: None,
        stripe: None,
        buyer_stripe: None,
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
        gpu: Default::default(),
        confidential: ConfidentialConfig {
            policy_path: None,
            policy: None,
            session_ttl_secs: 300,
        },
        marketplace_url: None,
        postgres_mounts: std::collections::BTreeMap::new(),
        session_pool: Default::default(),
        hosted_trial_origin_secret: None,
    };

    let db = DbPool::open(&db_path).expect("db pool");
    let events_query_capacity = db.read_connection_count().max(1);
    let identity = froglet::identity::NodeIdentity::load_or_create(&node_config).expect("identity");
    let pricing = froglet::pricing::PricingTable::from_config(node_config.pricing);
    let settlement_registry = SettlementRegistry::new(&node_config);

    Arc::new(AppState {
        db,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::new(4).expect("sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client"),
        wasm_host: None,
        confidential_policy: None,
        runtime_auth_token: "test-runtime-token".to_string(),
        runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
        consumer_control_auth_token: "test-consumer-token".to_string(),
        consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
        provider_control_auth_token: "test-provider-control-token".to_string(),
        provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
        events_query_semaphore: Arc::new(tokio::sync::Semaphore::new(events_query_capacity)),
        lnd_rest_client: None,
        lightning_wallet: None,
        lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
        event_batch_writer: None,
        builtin_services: std::collections::HashMap::new(),
        settlement_registry,
        session_pool: None,
    })
}

fn stripe_app_state_with_mock(
    mock_base_url: &str,
    buyer_config: BuyerStripeConfig,
) -> Arc<AppState> {
    let temp_dir = unique_temp_dir("stripe");
    let db_path = temp_dir.join("node.db");

    let node_config = NodeConfig {
        network_mode: NetworkMode::Clearnet,
        listen_addr: "127.0.0.1:0".to_string(),
        public_base_url: None,
        runtime_listen_addr: "127.0.0.1:0".to_string(),
        runtime_allow_non_loopback: false,
        http_ca_cert_path: None,
        tor: froglet::config::TorSidecarConfig {
            binary_path: "tor".to_string(),
            backend_listen_addr: "127.0.0.1:0".to_string(),
            startup_timeout_secs: 90,
        },
        identity: IdentityConfig {
            auto_generate: true,
        },
        pricing: PricingConfig {
            events_query: 0,
            execute_wasm: 30,
        },
        payment_backends: vec![PaymentBackend::Stripe],
        execution_timeout_secs: 10,
        lightning: LightningConfig {
            mode: LightningMode::Mock,
            destination_identity: None,
            base_invoice_expiry_secs: 300,
            success_hold_expiry_secs: 300,
            min_final_cltv_expiry: 18,
            sync_interval_ms: 100,
            lnd_rest: None,
        },
        x402: None,
        stripe: Some(StripeConfig {
            api_version: "2026-04-22.preview".to_string(),
            webhook_secret: None,
        }),
        buyer_stripe: Some(buyer_config),
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
        gpu: Default::default(),
        confidential: ConfidentialConfig {
            policy_path: None,
            policy: None,
            session_ttl_secs: 300,
        },
        marketplace_url: None,
        postgres_mounts: std::collections::BTreeMap::new(),
        session_pool: Default::default(),
        hosted_trial_origin_secret: None,
    };

    let db = DbPool::open(&db_path).expect("db pool");
    let events_query_capacity = db.read_connection_count().max(1);
    let identity = froglet::identity::NodeIdentity::load_or_create(&node_config).expect("identity");
    let pricing = froglet::pricing::PricingTable::from_config(node_config.pricing);

    // Build a StripeDriver pointed at the mock server and inject it into a
    // custom registry — bypassing the SettlementRegistry::new path which
    // reads the real FROGLET_STRIPE_SECRET_KEY env var and always points at
    // https://api.stripe.com.
    //
    // Use stripe_driver_boxed so the returned driver is `'static` even though
    // stripe_driver_with_base_url's opaque `impl` return captures &str lifetime.
    let stripe_driver: Arc<dyn froglet::settlement::SettlementDriver> =
        Arc::from(froglet::settlement::stripe_driver_boxed(
            StripeConfig {
                api_version: "2026-04-22.preview".to_string(),
                webhook_secret: None,
            },
            "sk_test_mock_full_deal".to_string(),
            mock_base_url.to_string(),
        ));
    let settlement_registry = SettlementRegistry::with_single_driver("stripe_mpp", stripe_driver);

    Arc::new(AppState {
        db,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::new(4).expect("sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client"),
        wasm_host: None,
        confidential_policy: None,
        runtime_auth_token: "test-runtime-token".to_string(),
        runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
        consumer_control_auth_token: "test-consumer-token".to_string(),
        consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
        provider_control_auth_token: "test-provider-control-token".to_string(),
        provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
        events_query_semaphore: Arc::new(tokio::sync::Semaphore::new(events_query_capacity)),
        lnd_rest_client: None,
        lightning_wallet: None,
        lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
        event_batch_writer: None,
        builtin_services: std::collections::HashMap::new(),
        settlement_registry,
        session_pool: None,
    })
}

// ─── Mock Stripe server (mirrored from payments_and_discovery.rs) ─────────────

#[derive(Debug, Default)]
struct MockStripeState {
    pub calls: std::sync::Mutex<Vec<String>>,
}

async fn start_mock_stripe() -> (String, Arc<MockStripeState>, tokio::task::JoinHandle<()>) {
    async fn get_token(
        AxumState(state): AxumState<Arc<MockStripeState>>,
        AxumPath(id): AxumPath<String>,
    ) -> AxumJson<Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("GET:granted_tokens/{id}"));
        AxumJson(json!({
            "id": id,
            "usage_limits": {
                "currency": "usd",
                "expires_at": settlement::current_unix_timestamp() + 600,
                "max_amount": 50_000
            }
        }))
    }

    async fn create_token(
        AxumState(state): AxumState<Arc<MockStripeState>>,
        body: String,
    ) -> AxumJson<Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST:granted_tokens:{body}"));
        AxumJson(json!({
            "id": "spt_mock_full_deal_test",
            "usage_limits": {
                "currency": "usd",
                "expires_at": settlement::current_unix_timestamp() + 600,
                "max_amount": 50_000
            }
        }))
    }

    async fn create_intent(
        AxumState(state): AxumState<Arc<MockStripeState>>,
        body: String,
    ) -> AxumJson<Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST:payment_intents:{body}"));
        AxumJson(json!({
            "id": "pi_full_deal_test",
            "status": "requires_capture"
        }))
    }

    async fn capture_intent(
        AxumState(state): AxumState<Arc<MockStripeState>>,
        AxumPath(pi_id): AxumPath<String>,
    ) -> AxumJson<Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST:payment_intents/{pi_id}/capture"));
        AxumJson(json!({
            "id": pi_id,
            "status": "succeeded"
        }))
    }

    async fn cancel_intent(
        AxumState(state): AxumState<Arc<MockStripeState>>,
        AxumPath(pi_id): AxumPath<String>,
    ) -> AxumJson<Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST:payment_intents/{pi_id}/cancel"));
        AxumJson(json!({
            "id": pi_id,
            "status": "canceled"
        }))
    }

    let mock = Arc::new(MockStripeState::default());
    let app = Router::new()
        .route(
            "/v1/test_helpers/shared_payment/granted_tokens",
            axum_post(create_token),
        )
        .route(
            "/v1/shared_payment/granted_tokens/:token_id",
            axum_get(get_token),
        )
        .route("/v1/payment_intents", axum_post(create_intent))
        .route(
            "/v1/payment_intents/:pi_id/capture",
            axum_post(capture_intent),
        )
        .route(
            "/v1/payment_intents/:pi_id/cancel",
            axum_post(cancel_intent),
        )
        .with_state(mock.clone());

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock stripe");
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock stripe serve");
    });
    (format!("http://{addr}"), mock, handle)
}

// ─── Test 1: Lightning mock full paid deal ─────────────────────────────────────

/// Full Lightning paid deal from deal creation through settlement.
///
/// Architecture: provider and runtime are the same node, served on separate
/// TCP ports so `runtime_create_deal_inner` can reach the provider over real
/// HTTP (required to exercise the full HTTP client + artifact validation path).
///
/// Settlement lifecycle:
///   1. Publish a 30-sat wasm service on the provider.
///   2. Runtime POST /v1/runtime/deals  →  quote + deal + invoice bundle.
///   3. Runtime POST /v1/runtime/deals/{id}/mock-pay  →  fund the base hold.
///   4. Provider executes the wasm workload asynchronously.
///   5. Wait for deal to reach result_ready in the provider DB.
///   6. Runtime POST /v1/runtime/deals/{id}/accept  →  release preimage, settle
///      success hold, provider signs and returns receipt.
///   7. Assert receipt: deal_state="succeeded", settlement_state="settled",
///      settlement_refs.method="lightning.base_fee_plus_success_fee.v1",
///      and validate_receipt_artifact passes.
#[tokio::test]
async fn lightning_mock_full_paid_deal_produces_settled_receipt() {
    let state = lightning_app_state();

    // Spawn the provider (public API) and runtime on separate ports, sharing state.
    // Use the built-in "execute.compute" offer (compute.wasm.v1 kind) which is
    // automatically published at price_sats=30 from the PricingConfig.
    let provider = spawn_server(public_router(state.clone())).await;
    let runtime = spawn_server(runtime_router(state.clone())).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client");

    let provider_id = state.identity.node_id().to_string();

    // ── Step 1: POST /v1/runtime/deals ──────────────────────────────────────
    // Serialise env-var mutation across parallel integration tests.
    let _env_guard = env_lock().lock().await;
    let _env = ScopedEnvVar::set("FROGLET_RUNTIME_PROVIDER_BASE_URL", &provider.base_url);

    let create_body = json!({
        "provider": {
            "provider_id": provider_id,
            "provider_url": provider.base_url,
        },
        "offer_id": "execute.compute",
        "kind": "wasm",
        "submission": test_wasm_submission(),
    });
    let (create_status, create_resp): (StatusCode, Value) = http_post_json(
        &client,
        &format!("{}/v1/runtime/deals", runtime.base_url),
        Some("test-runtime-token"),
        &create_body,
    )
    .await;
    // Release the env lock now that the deal is created (provider URL is recorded
    // in the deal record; subsequent calls don't need the env var).
    drop(_env);
    drop(_env_guard);
    assert_eq!(
        create_status,
        StatusCode::OK,
        "runtime create deal failed: {create_resp}"
    );

    let deal_id = create_resp["deal"]["deal_id"]
        .as_str()
        .expect("deal_id in response")
        .to_string();
    let payment_intent_path = create_resp["payment_intent_path"]
        .as_str()
        .expect("payment_intent_path must be present for lightning paid deal");

    // Sanity-check the payment intent includes the mock-pay action URL.
    assert!(
        payment_intent_path.contains(&deal_id),
        "payment_intent_path {payment_intent_path} should reference deal {deal_id}"
    );

    // ── Step 2: mock-pay the base invoice (fund the hold) ───────────────────
    let mock_pay_url = format!("{}/v1/runtime/deals/{deal_id}/mock-pay", runtime.base_url);
    let (mock_pay_status, mock_pay_resp): (StatusCode, Value) =
        http_post_empty_json(&client, &mock_pay_url, Some("test-runtime-token")).await;
    assert_eq!(
        mock_pay_status,
        StatusCode::OK,
        "runtime mock-pay failed: {mock_pay_resp}"
    );

    // ── Step 3: wait for provider to execute the workload ──────────────────
    wait_for_deal_status(&state, &deal_id, deals::DEAL_STATUS_RESULT_READY).await;

    // ── Step 4: accept the deal (release preimage, settle success hold) ─────
    let accept_body = json!({ "expected_result_hash": null });
    let accept_url = format!("{}/v1/runtime/deals/{deal_id}/accept", runtime.base_url);
    let (accept_status, accept_resp): (StatusCode, Value) = http_post_json(
        &client,
        &accept_url,
        Some("test-runtime-token"),
        &accept_body,
    )
    .await;
    assert_eq!(
        accept_status,
        StatusCode::OK,
        "runtime accept deal failed: {accept_resp}"
    );

    // ── Step 5: verify the receipt ───────────────────────────────────────────
    let receipt_value = &accept_resp["deal"]["receipt"];
    assert!(
        !receipt_value.is_null(),
        "expected a receipt in the accept response; got: {accept_resp}"
    );

    let receipt: froglet::protocol::SignedArtifact<froglet::protocol::ReceiptPayload> =
        serde_json::from_value(receipt_value.clone()).expect("deserialize receipt");

    assert!(
        verify_artifact(&receipt),
        "Lightning receipt signature must verify"
    );
    assert!(
        validate_receipt_artifact(&receipt).is_ok(),
        "Lightning receipt must pass kernel validation: {:?}",
        validate_receipt_artifact(&receipt)
    );

    assert_eq!(
        receipt.payload.deal_state, "succeeded",
        "deal_state must be 'succeeded'"
    );
    assert_eq!(
        receipt.payload.settlement_state, "settled",
        "settlement_state must be 'settled'"
    );
    assert_eq!(
        receipt.payload.settlement_refs.method, "lightning.base_fee_plus_success_fee.v1",
        "settlement method must be lightning"
    );
}

// ─── Test 2: Stripe MPP full paid deal ────────────────────────────────────────

/// Full Stripe MPP paid deal from creation through capture.
///
/// Architecture: provider and runtime are the same node. The buyer_stripe config
/// on the AppState causes `runtime_create_deal_inner` to auto-mint an SPT (via
/// the mock Stripe server), attach it as a ProvidedPayment, and send it to the
/// provider's `POST /v1/provider/deals`. The provider calls `prepare_payment_for_amount`
/// (via `settlement_registry.driver_for("stripe_mpp")` pointed at the mock), which
/// validates the SPT and creates a PaymentIntent. After execution completes,
/// `process_deal_with_reserved_permit` calls `commit_payment` (capture) and signs
/// a receipt with settlement_state="settled".
///
/// This path was previously unexercised end-to-end. The test verifies:
/// - The buyer SPT mint call hits the mock.
/// - The seller SPT validation + PI creation calls hit the mock.
/// - The PI capture call hits the mock.
/// - The final receipt has deal_state="succeeded", settlement_state="settled",
///   settlement_refs.method="stripe_mpp.v1", and passes validate_receipt_artifact.
#[tokio::test]
async fn stripe_mpp_full_paid_deal_produces_settled_receipt() {
    let (mock_base_url, mock_stripe, _mock_handle) = start_mock_stripe().await;

    let buyer_config = BuyerStripeConfig {
        secret_key: "sk_test_buyer_full_deal".to_string(),
        api_version: "2026-04-22.preview".to_string(),
        payment_method: Some("pm_test_full_deal_buyer".to_string()),
        customer: None,
        // Redirect buyer-side SPT minting to the local mock instead of real Stripe.
        api_base_url: Some(mock_base_url.clone()),
    };

    let state = stripe_app_state_with_mock(&mock_base_url, buyer_config);

    // Spawn provider + runtime sharing the same AppState.
    // Use the built-in "execute.compute" offer (compute.wasm.v1 kind) priced
    // at 30 cents/sats from PricingConfig.execute_wasm=30.  With
    // PaymentBackend::Stripe as the only backend, the offer and quote will
    // automatically carry settlement_method="stripe_mpp.v1".
    let provider = spawn_server(public_router(state.clone())).await;
    let runtime = spawn_server(runtime_router(state.clone())).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client");

    let provider_id = state.identity.node_id().to_string();

    // ── Step 1: POST /v1/runtime/deals ──────────────────────────────────────
    // Serialise env-var mutation across parallel integration tests.
    let _env_guard = env_lock().lock().await;
    let _env = ScopedEnvVar::set("FROGLET_RUNTIME_PROVIDER_BASE_URL", &provider.base_url);

    // `runtime_create_deal_inner` detects stripe_mpp.v1 in the quote and
    // auto-mints an SPT via `state.config.buyer_stripe`, then forwards it to
    // `POST /v1/provider/deals` as a ProvidedPayment.
    let create_body = json!({
        "provider": {
            "provider_id": provider_id,
            "provider_url": provider.base_url,
        },
        "offer_id": "execute.compute",
        "kind": "wasm",
        "submission": test_wasm_submission(),
    });
    let (create_status, create_resp): (StatusCode, Value) = http_post_json(
        &client,
        &format!("{}/v1/runtime/deals", runtime.base_url),
        Some("test-runtime-token"),
        &create_body,
    )
    .await;
    drop(_env);
    drop(_env_guard);
    assert_eq!(
        create_status,
        StatusCode::OK,
        "stripe runtime create deal failed: {create_resp}"
    );

    let deal_id = create_resp["deal"]["deal_id"]
        .as_str()
        .expect("deal_id in response")
        .to_string();

    // Stripe deals do NOT use the invoice-bundle flow, so no payment_intent_path.
    assert!(
        create_resp["payment_intent_path"].is_null(),
        "stripe deals must not expose a lightning payment_intent_path"
    );

    // ── Step 2: wait for provider to execute and capture ────────────────────
    // The Stripe deal is accepted synchronously at deal-create time (no mock-pay
    // step). `process_deal_with_reserved_permit` runs in the background: executes
    // the wasm workload, captures the PaymentIntent, and writes the receipt.
    wait_for_deal_status(&state, &deal_id, deals::DEAL_STATUS_SUCCEEDED).await;

    // ── Step 3: GET /v1/runtime/deals/{id} to retrieve the receipt ──────────
    let (get_status, get_resp): (StatusCode, Value) = http_get_json(
        &client,
        &format!("{}/v1/runtime/deals/{deal_id}", runtime.base_url),
        Some("test-runtime-token"),
    )
    .await;
    assert_eq!(get_status, StatusCode::OK, "get deal failed: {get_resp}");

    // ── Step 4: verify mock Stripe received the expected calls ───────────────
    {
        let calls = mock_stripe.calls.lock().unwrap().clone();

        assert!(
            calls.iter().any(|c| c.starts_with("POST:granted_tokens:")),
            "buyer SPT create call must appear; calls: {calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("GET:granted_tokens/spt_mock_full_deal_test")),
            "seller SPT validation GET must appear; calls: {calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("POST:payment_intents:")
                    && c.contains("capture_method=manual")),
            "PaymentIntent create call must appear; calls: {calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c == "POST:payment_intents/pi_full_deal_test/capture"),
            "PaymentIntent capture call must appear; calls: {calls:?}"
        );
    }

    // ── Step 5: verify the receipt in the final GET response ─────────────────
    let receipt_value = &get_resp["deal"]["receipt"];
    assert!(
        !receipt_value.is_null(),
        "expected a receipt in the GET response after succeeded; got: {get_resp}"
    );

    let receipt: froglet::protocol::SignedArtifact<froglet::protocol::ReceiptPayload> =
        serde_json::from_value(receipt_value.clone()).expect("deserialize stripe receipt");

    assert!(
        verify_artifact(&receipt),
        "Stripe receipt signature must verify"
    );
    assert!(
        validate_receipt_artifact(&receipt).is_ok(),
        "Stripe receipt must pass kernel validation: {:?}",
        validate_receipt_artifact(&receipt)
    );

    assert_eq!(
        receipt.payload.deal_state, "succeeded",
        "deal_state must be 'succeeded'"
    );
    assert_eq!(
        receipt.payload.settlement_state, "settled",
        "settlement_state must be 'settled'"
    );
    assert_eq!(
        receipt.payload.settlement_refs.method, "stripe_mpp.v1",
        "settlement method must be stripe_mpp.v1"
    );
}
