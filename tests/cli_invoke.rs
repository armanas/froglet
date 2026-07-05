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
    let temp_dir = unique_temp_dir("state");
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
    };

    let pool = DbPool::open(&node_config.storage.db_path).expect("init db");
    let events_query_capacity = pool.read_connection_count().max(1);
    let identity =
        froglet::identity::NodeIdentity::load_or_create(&node_config).expect("test identity");
    let pricing = froglet::pricing::PricingTable::from_config(node_config.pricing);
    let settlement_registry = SettlementRegistry::new(&node_config);

    AppState {
        db: pool,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: reqwest::Client::builder()
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
        hosted_trial_deal_quota: None,
        hosted_trial_session_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            Duration::from_secs(60),
        )),
        event_publish_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            Duration::from_secs(60),
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
        service_id: service_id.to_string(),
        input,
        daemon_url: node.provider.base_url.clone(),
        runtime_url: node.runtime.base_url.clone(),
        runtime_token: "test-runtime-token".to_string(),
        provider_id_override: None,
        wait_timeout: Duration::from_secs(30),
        poll_interval: Duration::from_millis(100),
    }
}

/// Publish an inline-source Python service through the provider-control
/// API, mirroring what `froglet-node publish --host local` sends.
async fn publish_python_service(node: &DualNode, service_id: &str, price_sats: u64) {
    let client = reqwest::Client::new();
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
