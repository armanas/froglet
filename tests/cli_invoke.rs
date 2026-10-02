//! Integration tests for `froglet-node invoke` (`src/cli/invoke.rs`).
//!
//! One dual node — `public_router` (provider) and `runtime_router` on real
//! TCP listeners sharing an `AppState` — drives the CLI core
//! (`invoke_local_service`) end-to-end. The deal-creating tests run with
//! `FROGLET_RUNTIME_PROVIDER_BASE_URL` unset: a dual node must resolve its
//! own recorded provider listener (src/server.rs records it at bind time).
//!
//! - builtin service (`demo.add`) invoked to a terminal `succeeded` deal
//!   with the executed result,
//! - python `inline_source` service published through the provider-control
//!   API and invoked service-addressed (create validated, `--no-wait`),
//! - requester spend policy 402 surfaced with its remediation `code`,
//! - remote provider / unknown service error paths pointing at MCP
//!   `invoke_service`.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use froglet::{
    api::{public_router, runtime_router},
    cli::invoke::{InvokeOptions, invoke_local_service},
    confidential::ConfidentialConfig,
    config::{
        IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig, PaymentBackend,
        PricingConfig, StorageConfig, WasmConfig,
    },
    db::DbPool,
    settlement::SettlementRegistry,
    state::{AppState, TransportStatus},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};

static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(1);
static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn test_env_lock() -> &'static Mutex<()> {
    TEST_ENV_LOCK.get_or_init(|| Mutex::new(()))
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<String>,
}

impl ScopedEnvVar {
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

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "froglet-cli-invoke-{prefix}-{}-{unique}-{counter}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Dual-node test state. `payment_backends` selects free-only
/// (`PaymentBackend::None`) or mock-Lightning (paid quotes) behavior.
fn create_dual_state(payment_backends: Vec<PaymentBackend>) -> AppState {
    create_dual_state_at(payment_backends, unique_temp_dir("state"))
}

fn create_dual_state_at(
    payment_backends: Vec<PaymentBackend>,
    temp_dir: std::path::PathBuf,
) -> AppState {
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
        relay: froglet::config::RelayConfig::default(),
        identity: IdentityConfig {
            auto_generate: true,
        },
        pricing: PricingConfig {
            events_query: 0,
            execute_wasm: 0,
        },
        payment_backends,
        execution_timeout_secs: 10,
        process_limits: Default::default(),
        public_quota: Default::default(),
        lightning: LightningConfig {
            mode: LightningMode::Mock,
            destination_identity: None,
            base_invoice_expiry_secs: 300,
            success_hold_expiry_secs: 300,
            min_final_cltv_expiry: 18,
            sync_interval_ms: 1_000,
            lnd_rest: None,
            phoenixd: None,
        },
        x402: None,
        stripe: None,
        buyer_stripe: None,
        buyer_phoenixd: None,
        provider_policy: Default::default(),
        requester_spend: Default::default(),
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
        marketplace_allow_local: false,
        provider_artifact_root: None,
        postgres_mounts: std::collections::BTreeMap::new(),
        session_pool: Default::default(),
        hosted_trial_origin_secret: None,
        a2a: Default::default(),
    };

    let pool = DbPool::open(&node_config.storage.db_path).expect("init db");
    let events_query_capacity = pool.read_connection_count().max(1);
    let identity =
        froglet::identity::NodeIdentity::load_or_create(&node_config).expect("test identity");
    let pricing = froglet::pricing::PricingTable::from_config(node_config.pricing);
    let settlement_registry = SettlementRegistry::new(&node_config).expect("settlement registry");

    AppState {
        db: pool,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: froglet::tls::reqwest_client_builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client"),
        wasm_host: None,
        confidential_policy: None,
        runtime_auth_token: "test-runtime-token".to_string(),
        runtime_auth_token_path: unique_temp_dir("token").join("auth.token"),
        consumer_control_auth_token: "test-consumer-token".to_string(),
        consumer_control_auth_token_path: unique_temp_dir("token").join("consumerctl.token"),
        provider_control_auth_token: "test-provider-token".to_string(),
        provider_control_auth_token_path: unique_temp_dir("token").join("froglet-control.token"),
        events_query_semaphore: Arc::new(tokio::sync::Semaphore::new(events_query_capacity)),
        process_execution_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
        native_data_query_handlers: froglet::builtins::DataQueryHandlerCache::default(),
        native_data_publication_lock: tokio::sync::Mutex::const_new(()),
        hosted_trial_deal_quota: None,
        hosted_trial_session_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            Duration::from_secs(60),
        )),
        event_publish_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            Duration::from_secs(60),
        )),
        public_request_quota: std::sync::Arc::new(froglet::public_quota::IdentityQuota::new(
            6000,
            std::time::Duration::from_secs(900),
        )),
        quote_create_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            Duration::from_secs(60),
        )),
        confidential_session_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            Duration::from_secs(60),
        )),
        lnd_rest_client: None,
        phoenixd_client: None,
        lightning_wallet: None,
        lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
        event_batch_writer: None,
        builtin_services: std::collections::HashMap::new(),
        settlement_registry,
        session_pool: None,
    }
}

struct TestServer {
    base_url: String,
    addr: std::net::SocketAddr,
    _handle: JoinHandle<()>,
}

async fn spawn_server(app: axum::Router) -> TestServer {
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
        addr,
        _handle: handle,
    }
}

struct DualNode {
    state: Arc<AppState>,
    provider: TestServer,
    runtime: TestServer,
}

async fn spawn_dual_node(payment_backends: Vec<PaymentBackend>, demo_builtins: bool) -> DualNode {
    let mut state = create_dual_state(payment_backends);
    if demo_builtins {
        state.builtin_services = froglet::builtins::demo_handlers();
    }
    let state = Arc::new(state);
    if demo_builtins {
        froglet::builtins::register_demo_offers(state.as_ref())
            .await
            .expect("register demo offers");
    }
    let provider = spawn_server(public_router(state.clone())).await;
    let runtime = spawn_server(runtime_router(state.clone())).await;
    // Mirror src/server.rs bind-time behavior: a dual node records its own
    // provider listener so the runtime resolves the local provider without
    // FROGLET_RUNTIME_PROVIDER_BASE_URL.
    state
        .transport_status
        .lock()
        .await
        .local_provider_bound_addr = Some(provider.addr);
    DualNode {
        state,
        provider,
        runtime,
    }
}

fn invoke_options(node: &DualNode, service_id: &str, input: Value) -> InvokeOptions {
    InvokeOptions {
        access_token_file: None,
        service_id: service_id.to_string(),
        input,
        daemon_url: node.provider.base_url.clone(),
        runtime_url: node.runtime.base_url.clone(),
        runtime_token: "test-runtime-token".to_string(),
        provider_id_override: None,
        idempotency_key: None,
        max_price_sats: None,
        wait_timeout: Duration::from_secs(30),
        poll_interval: Duration::from_millis(100),
    }
}

/// Publish an inline-source Python service through the provider-control
/// API, mirroring what `froglet-node publish --host local` sends.
async fn publish_python_service(node: &DualNode, service_id: &str, price_sats: u64) {
    let client = froglet::tls::reqwest_client_builder()
        .build()
        .expect("reqwest client");
    let response = client
        .post(format!(
            "{}/v1/provider/artifacts/publish",
            node.provider.base_url
        ))
        .bearer_auth("test-provider-token")
        .json(&json!({
            "service_id": service_id,
            "runtime": "python",
            "package_kind": "inline_source",
            "entrypoint": "handler",
            "inline_source": "def handler(event, context):\n    return event\n",
            "summary": "echo back the input event",
            "settlement_method": if price_sats == 0 { "none" } else { "lightning" },
            "price_sats": price_sats,
        }))
        .send()
        .await
        .expect("publish request");
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(
        status,
        reqwest::StatusCode::CREATED,
        "publish {service_id} failed: {body}"
    );
}

async fn publish_native_json_service(node: &DualNode, service_id: &str) {
    let client = froglet::tls::reqwest_client_builder()
        .build()
        .expect("reqwest client");
    let source = br#"[{"id":1,"name":"Ada"},{"id":2,"name":"Grace"}]"#;
    let response = client
        .post(format!(
            "{}/v1/provider/artifacts/publish",
            node.provider.base_url
        ))
        .bearer_auth("test-provider-token")
        .json(&json!({
            "service_id": service_id,
            "runtime": "builtin",
            "package_kind": "builtin",
            "data_source": {
                "format": "json",
                "content_base64": STANDARD.encode(source),
            },
            "settlement_method": "none",
            "price_sats": 0,
            "publication_state": "active",
            "verification": {
                "input": {"op": "select", "collection": "rows", "limit": 1},
            },
        }))
        .send()
        .await
        .expect("publish request");
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(
        status,
        reqwest::StatusCode::CREATED,
        "publish {service_id} failed: {body}"
    );
}

#[tokio::test]
async fn invoke_builtin_service_runs_to_succeeded_with_result() {
    let node = spawn_dual_node(vec![PaymentBackend::None], true).await;
    // No FROGLET_RUNTIME_PROVIDER_BASE_URL: the dual node must resolve its
    // own bound provider listener recorded at startup.
    let _env_lock = test_env_lock().lock().await;
    let _env = ScopedEnvVar::unset("FROGLET_RUNTIME_PROVIDER_BASE_URL");

    let report = invoke_local_service(&invoke_options(&node, "demo.add", json!({"a": 7, "b": 5})))
        .await
        .expect("invoke demo.add");

    assert_eq!(report.status, "succeeded", "report: {report:?}");
    assert!(report.terminal);
    assert_eq!(report.service_id, "demo.add");
    assert_eq!(report.provider_id, node.state.identity.node_id());
    assert!(!report.deal_id.is_empty());
    assert_eq!(report.result, Some(json!({"sum": 12})));
    assert!(report.result_hash.is_some());
    assert!(report.error.is_none());
}

#[tokio::test]
async fn invoke_published_python_service_creates_service_addressed_deal() {
    let node = spawn_dual_node(vec![PaymentBackend::None], false).await;
    let _env_lock = test_env_lock().lock().await;
    let _env = ScopedEnvVar::unset("FROGLET_RUNTIME_PROVIDER_BASE_URL");

    publish_python_service(&node, "py.echo", 0).await;

    // `--no-wait`: prove the CLI-built service-addressed workload passes the
    // daemon's quote + deal validation against the published record without
    // depending on a python3 interpreter finishing in CI.
    let mut options = invoke_options(&node, "py.echo", json!({"message": "hi"}));
    options.wait_timeout = Duration::ZERO;
    let report = invoke_local_service(&options).await.expect("create deal");

    assert!(!report.deal_id.is_empty());
    assert!(!report.status.is_empty());
    assert_eq!(report.provider_id, node.state.identity.node_id());
}

#[tokio::test]
async fn invoke_published_native_data_service_preserves_immutable_binding() {
    let node = spawn_dual_node(vec![PaymentBackend::None], false).await;
    let _env_lock = test_env_lock().lock().await;
    let _env = ScopedEnvVar::unset("FROGLET_RUNTIME_PROVIDER_BASE_URL");

    publish_native_json_service(&node, "data.people").await;

    let report = invoke_local_service(&invoke_options(
        &node,
        "data.people",
        json!({
            "op": "select",
            "collection": "rows",
            "columns": ["name"],
            "equals": {"id": 2},
            "limit": 1,
        }),
    ))
    .await
    .expect("invoke native data service");

    assert_eq!(report.status, "succeeded", "report: {report:?}");
    let result = report.result.expect("native data result");
    assert_eq!(
        result["contract_version"],
        froglet::builtins::DATA_QUERY_CONTRACT_V1
    );
    assert_eq!(result["source_kind"], "json");
    assert_eq!(result["collection"], "rows");
    assert_eq!(result["rows"], json!([{"name": "Grace"}]));
    assert_eq!(result["returned"], 1);
    let status: Value = froglet::tls::reqwest_client_builder()
        .build()
        .unwrap()
        .get(format!(
            "{}/v1/provider/publications",
            node.provider.base_url
        ))
        .bearer_auth("test-provider-token")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        status["last_successful_calls"]["data.people"]
            .as_i64()
            .is_some_and(|timestamp| timestamp > 0),
        "{status}"
    );
}

#[tokio::test]
async fn paid_service_surfaces_spend_policy_refusal_code() {
    let node = spawn_dual_node(vec![PaymentBackend::Lightning], false).await;
    let _env_lock = test_env_lock().lock().await;
    let _env = ScopedEnvVar::unset("FROGLET_RUNTIME_PROVIDER_BASE_URL");

    publish_python_service(&node, "py.paid", 25).await;

    // No FROGLET_REQUESTER_SPEND_BUDGET_MSAT configured → the runtime must
    // refuse the paid deal fail-closed with the stable spend code, and the
    // CLI must surface the code + remediation, not a generic HTTP error.
    let error = invoke_local_service(&invoke_options(&node, "py.paid", json!({"message": "hi"})))
        .await
        .expect_err("paid deal must be refused without a spend budget");
    let message = error.to_string();
    assert!(
        message.contains("spend_budget_unconfigured"),
        "expected spend code in error, got: {message}"
    );
    assert!(
        message.contains("FROGLET_REQUESTER_SPEND_BUDGET_MSAT"),
        "expected remediation env var in error, got: {message}"
    );
}

#[tokio::test]
async fn remote_provider_id_is_rejected_with_mcp_pointer() {
    let node = spawn_dual_node(vec![PaymentBackend::None], true).await;

    let mut options = invoke_options(&node, "demo.add", Value::Null);
    options.provider_id_override = Some("ff".repeat(32));
    let error = invoke_local_service(&options)
        .await
        .expect_err("remote provider must be rejected");
    let message = error.to_string();
    assert!(
        message.contains("invoke_service"),
        "expected MCP pointer in error, got: {message}"
    );
}

#[tokio::test]
async fn unknown_service_reports_not_published_with_mcp_pointer() {
    let node = spawn_dual_node(vec![PaymentBackend::None], false).await;

    let error = invoke_local_service(&invoke_options(&node, "no.such.service", Value::Null))
        .await
        .expect_err("unknown service must error");
    let message = error.to_string();
    assert!(
        message.contains("not published on this node"),
        "expected not-published error, got: {message}"
    );
    assert!(
        message.contains("invoke_service"),
        "expected MCP pointer in error, got: {message}"
    );
}

#[tokio::test]
async fn private_provider_owner_invocation_uses_bounded_signed_flow() {
    let _env_lock = test_env_lock().lock().await;
    let _env = ScopedEnvVar::unset("FROGLET_RUNTIME_PROVIDER_BASE_URL");
    let mut state = create_dual_state(vec![PaymentBackend::None]);
    state.config.provider_policy = froglet::provider_policy::ProviderPolicy {
        access_mode: froglet::provider_policy::AccessMode::Private,
        max_total_deals: Some(2),
        max_total_quotes: Some(2),
        max_total_runtime_ms: Some(60_000),
        ..Default::default()
    };
    state.builtin_services = froglet::builtins::demo_handlers();
    let state = Arc::new(state);
    froglet::builtins::register_demo_offers(&state)
        .await
        .unwrap();
    let provider = spawn_server(public_router(state.clone())).await;
    let runtime = spawn_server(runtime_router(state.clone())).await;
    state
        .transport_status
        .lock()
        .await
        .local_provider_bound_addr = Some(provider.addr);
    let node = DualNode {
        state,
        provider,
        runtime,
    };
    let mut options = invoke_options(&node, "demo.add", json!({"a":2,"b":3}));
    options.idempotency_key = Some("private-owner-call".into());
    let first = invoke_local_service(&options).await.unwrap();
    assert_eq!(first.status, "succeeded");
    assert_eq!(first.result, Some(json!({"sum":5})));
    let replay = invoke_local_service(&options).await.unwrap();
    assert_eq!(first.deal_id, replay.deal_id);
    let usage = node
        .state
        .db
        .with_read_conn(froglet::provider_policy::usage)
        .await
        .unwrap();
    assert_eq!(usage.reserved_deals, 1);
    assert_eq!(usage.issued_quotes, 1);
    node.state
        .db
        .with_write_conn(|conn| froglet::provider_policy::set_pause(conn, Some("test")))
        .await
        .unwrap();
    options.idempotency_key = Some("paused-owner-call".into());
    assert!(invoke_local_service(&options).await.is_err());
    options.idempotency_key = Some("private-owner-call".into());
    let recovered = invoke_local_service(&options).await.unwrap();
    assert_eq!(recovered.deal_id, first.deal_id);

    // Lookup must authenticate and must bind a reused key to the exact request.
    let client = reqwest::Client::new();
    let query = froglet::api::RuntimeInvocationQuery {
        idempotency_key: "private-owner-call".into(),
        service_id: "demo.add".into(),
        input_hash: froglet::crypto::sha256_hex(
            froglet::canonical_json::to_vec(&options.input).unwrap(),
        ),
        provider_id: Some(node.state.identity.node_id().to_string()),
        provider_url: None,
        max_price_sats: Some(0),
    };
    let lookup = format!("{}/v1/runtime/deals", node.runtime.base_url);
    assert_eq!(
        client
            .get(&lookup)
            .query(&query)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    for (field, value, status) in [
        ("input_hash", json!("f".repeat(64)), 409),
        ("service_id", json!("other.service"), 409),
        ("provider_id", json!("f".repeat(64)), 409),
        ("provider_url", json!("https://other.example"), 409),
        ("idempotency_key", json!("unknown-key"), 404),
        ("input_hash", json!("invalid"), 400),
    ] {
        let mut bad = serde_json::to_value(&query).unwrap();
        bad.as_object_mut().unwrap().retain(|_, v| !v.is_null());
        bad[field] = value;
        let response = client
            .get(&lookup)
            .bearer_auth("test-runtime-token")
            .query(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status, "{field}");
    }

    // Alice must be able to recover her completed result when Bob is offline.
    node.provider._handle.abort();
    node.provider._handle.await.unwrap_err();
    let recovered = invoke_local_service(&options).await.unwrap();
    assert_eq!(recovered.deal_id, first.deal_id);
    assert_eq!(recovered.receipt_verification, first.receipt_verification);
    assert_eq!(recovered.result, first.result);

    // Reopen the persisted requester database through a new runtime listener.
    node.runtime._handle.abort();
    node.runtime._handle.await.unwrap_err();
    let restarted = Arc::new(create_dual_state_at(
        vec![PaymentBackend::None],
        node.state.config.storage.data_dir.clone(),
    ));
    let runtime = spawn_server(runtime_router(restarted.clone())).await;
    options.runtime_url = runtime.base_url.clone();
    let recovered = invoke_local_service(&options).await.unwrap();
    assert_eq!(recovered.deal_id, first.deal_id);
    assert_eq!(recovered.receipt_verification, first.receipt_verification);

    // Exercise both native entrypoints, not just their shared Rust helper.
    let command = || {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_froglet-node"));
        command
            .env_clear()
            .env("FROGLET_DAEMON_URL", &options.daemon_url)
            .env("FROGLET_RUNTIME_URL", &options.runtime_url)
            .env("FROGLET_RUNTIME_AUTH_TOKEN", "test-runtime-token")
            .env("FROGLET_DATA_DIR", &node.state.config.storage.data_dir)
            .kill_on_drop(true);
        command
    };
    let output = command()
        .args([
            "invoke",
            "demo.add",
            "{\"a\":2,\"b\":3}",
            "--idempotency-key",
            "private-owner-call",
            "--json",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(cli["deal_id"], first.deal_id);
    assert_eq!(cli["receipt_verification"], first.receipt_verification);
    use tokio::io::AsyncWriteExt;
    let mut child = command()
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"froglet","arguments":{"action":"invoke_service","service_id":"demo.add","input":{"a":2,"b":3},"idempotency_key":"private-owner-call"}}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mcp: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(mcp["result"]["isError"], false, "{mcp}");
    assert_eq!(mcp["result"]["structuredContent"]["deal_id"], first.deal_id);
    let usage = restarted
        .db
        .with_read_conn(froglet::provider_policy::usage)
        .await
        .unwrap();
    assert_eq!(usage.reserved_deals, 1);
    assert_eq!(usage.issued_quotes, 1);
    runtime._handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_mcp_issues_private_invite_file_and_revokes_without_secret_output() {
    use tokio::io::AsyncWriteExt;
    let mut state = create_dual_state(vec![PaymentBackend::None]);
    state.config.provider_policy = froglet::provider_policy::ProviderPolicy {
        access_mode: froglet::provider_policy::AccessMode::Invite,
        max_total_deals: Some(2),
        max_total_quotes: Some(4),
        max_total_runtime_ms: Some(60_000),
        ..Default::default()
    };
    let state = Arc::new(state);
    let server = spawn_server(public_router(state.clone())).await;
    let directory = tempfile::tempdir().unwrap();
    let token_path = directory.path().join("recipient.token");
    let call = |arguments: Value| {
        let base = server.base_url.clone();
        let directory = directory.path().to_path_buf();
        async move {
            let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_froglet-node"))
                .arg("mcp")
                .env("FROGLET_DAEMON_URL", base)
                .env("FROGLET_PROVIDER_CONTROL_TOKEN", "test-provider-token")
                .env("FROGLET_DATA_DIR", directory)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"froglet","arguments":arguments}});
            child
                .stdin
                .take()
                .unwrap()
                .write_all(format!("{request}\n").as_bytes())
                .await
                .unwrap();
            let output = child.wait_with_output().await.unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            let value: Value = serde_json::from_str(&stdout).unwrap();
            (
                value["result"].clone(),
                stdout,
                String::from_utf8(output.stderr).unwrap(),
            )
        }
    };
    let arguments = json!({"action":"invite_create","name":"Recipient","expires_at":froglet::settlement::current_unix_timestamp()+600,"max_requests":2,"token_file":token_path});
    let (result, stdout, stderr) = call(arguments.clone()).await;
    assert_eq!(result["isError"], false, "{result}");
    let token = froglet::cli::invoke::read_access_token_file(&token_path).unwrap();
    assert!(!stdout.contains(token.as_str()));
    assert!(!stderr.contains(token.as_str()));
    let id = result["structuredContent"]["id"].as_str().unwrap();
    assert_eq!(id, froglet::crypto::sha256_hex(token.as_bytes()));
    assert_eq!(
        call(arguments).await.0["isError"],
        true,
        "existing file must not be replaced"
    );
    assert_eq!(
        std::fs::read_to_string(&token_path).unwrap(),
        token.as_str()
    );
    assert_eq!(
        state
            .db
            .with_read_conn(froglet::provider_policy::list_invites)
            .await
            .unwrap()
            .len(),
        1
    );
    let listed = call(json!({"action":"invite_list"})).await;
    assert!(!listed.1.contains(token.as_str()));
    assert_eq!(
        call(json!({"action":"invite_revoke","invite_id":id}))
            .await
            .0["isError"],
        false
    );
    assert!(
        state
            .db
            .with_read_conn(froglet::provider_policy::list_invites)
            .await
            .unwrap()[0]
            .revoked
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(froglet::cli::invoke::read_access_token_file(&token_path).is_err());
        let symlink = directory.path().join("alias");
        std::os::unix::fs::symlink(&token_path, &symlink).unwrap();
        assert!(froglet::cli::invoke::read_access_token_file(&symlink).is_err());
    }
    server._handle.abort();
}

#[tokio::test]
async fn native_node_refuses_http_operations_without_protected_admission() {
    let directory = tempfile::tempdir().unwrap();
    let policy = directory.path().join("policy.toml");
    std::fs::write(&policy, format!("[http]\noperations_only = true\noperation_hashes = [\"{}\"]\nallowed_hosts = [\"api.example.com\"]\n", "a".repeat(64))).unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_froglet-node"))
            .env_clear()
            .env("FROGLET_NETWORK_MODE", "clearnet")
            .env("FROGLET_PAYMENT_BACKEND", "none")
            .env("FROGLET_DATA_DIR", directory.path().join("data"))
            .env("FROGLET_PROVIDER_ACCESS_MODE", "open")
            .env("FROGLET_WASM_POLICY_PATH", policy)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!output.status.success());
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostics.contains("approved HTTP operations require"),
        "{diagnostics}"
    );
    assert!(
        !directory.path().join("data").exists(),
        "startup must reject before provisioning state"
    );
}
