//! Real native-MCP computation through independent requester/provider nodes.
//! A permitted synthetic onion URL is routed to the local provider fixture by
//! the requester test HTTP client only. This verifies the real protocol and
//! executor without claiming that a Tor network has been exercised.
use froglet::{
    api::{public_router, runtime_router},
    cli::invoke::{InvokeOptions, get_task, run_inline_wasm},
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
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncWriteExt, net::TcpListener, task::JoinHandle};
static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(1);
const VALID_WASM_HEX: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";
fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "froglet-cli-mcp-compute-{prefix}-{}-{unique}-{counter}",
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

impl Drop for TestServer {
    fn drop(&mut self) {
        self._handle.abort();
    }
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

async fn start_node(state: AppState) -> DualNode {
    let state = Arc::new(state);
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

fn requester_state(provider: &DualNode, directory: std::path::PathBuf) -> AppState {
    let mut state = create_dual_state_at(vec![PaymentBackend::None], directory);
    let host = format!("{}.onion", "a".repeat(56));
    state.http_client = froglet::tls::reqwest_client_builder()
        .no_proxy()
        .resolve(&host, provider.provider.addr)
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    state
}

async fn pair(price_sats: u64) -> (DualNode, DualNode, String) {
    let backends = if price_sats == 0 {
        vec![PaymentBackend::None]
    } else {
        vec![PaymentBackend::Lightning]
    };
    let mut provider = create_dual_state(backends);
    provider.config.pricing.execute_wasm = price_sats;
    provider.pricing = froglet::pricing::PricingTable::from_config(provider.config.pricing);
    let bob = start_node(provider).await;
    let alice = start_node(requester_state(&bob, unique_temp_dir("requester"))).await;
    assert_ne!(bob.state.identity.node_id(), alice.state.identity.node_id());
    let origin = format!(
        "http://{}.onion:{}",
        "a".repeat(56),
        bob.provider.addr.port()
    );
    (bob, alice, origin)
}

fn compute_options(alice: &DualNode, bob: &DualNode, key: &str) -> InvokeOptions {
    InvokeOptions {
        service_id: String::new(),
        input: json!({"sample": 1}),
        daemon_url: alice.provider.base_url.clone(),
        runtime_url: alice.runtime.base_url.clone(),
        runtime_token: "test-runtime-token".into(),
        access_token_file: None,
        provider_id_override: Some(bob.state.identity.node_id().into()),
        idempotency_key: Some(key.into()),
        max_price_sats: None,
        wait_timeout: Duration::from_secs(10),
        poll_interval: Duration::from_millis(25),
    }
}

async fn native_call(alice: &DualNode, arguments: Value) -> Value {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_froglet-node"))
        .arg("mcp")
        .env("FROGLET_DAEMON_URL", &alice.provider.base_url)
        .env("FROGLET_RUNTIME_URL", &alice.runtime.base_url)
        .env("FROGLET_RUNTIME_AUTH_TOKEN", "test-runtime-token")
        .env("FROGLET_DATA_DIR", &alice.state.config.storage.data_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"froglet","arguments":arguments}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("test-runtime-token"));
    assert!(
        !stdout.contains(VALID_WASM_HEX),
        "native reports must not echo the program bytes"
    );
    let response: Value = serde_json::from_str(&stdout).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

async fn deal_count(node: &DualNode) -> i64 {
    node.state
        .db
        .with_read_conn(|conn| conn.query_row("SELECT COUNT(*) FROM deals", [], |row| row.get(0)))
        .await
        .unwrap()
}

#[tokio::test]
async fn native_stdio_computes_and_reads_verified_evidence_between_independent_nodes() {
    let (bob, alice, origin) = pair(0).await;
    let request = json!({"action":"run_compute","wasm_module_hex":VALID_WASM_HEX,
        "input":{"sample":1},"provider_id":bob.state.identity.node_id(),"provider_url":origin,
        "idempotency_key":"stdio-compute-1","response_format":"compact"}); // gitleaks:allow -- public retry label, not an authentication credential
    let response = native_call(&alice, request.clone()).await;
    assert_eq!(response["isError"], false, "{response}");
    let report = &response["structuredContent"];
    assert_eq!(report["status"], "succeeded", "{report}");
    assert_eq!(report["result"], 42);
    assert_eq!(report["receipt_verification"]["verified"], true);
    assert_eq!(report["workload_kind"], "compute.wasm.v1");
    assert!(
        report.get("service_id").is_none(),
        "generic computation is not a named service"
    );
    assert!(report["execution_limits"]["fuel_limit"].as_u64().unwrap() > 0);
    assert!(
        report["execution_limits"]["max_runtime_ms"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        report["execution_limits"]["max_memory_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    for hash in ["workload_hash", "quote_hash", "deal_hash", "result_hash"] {
        assert_eq!(report[hash].as_str().unwrap().len(), 64);
    }
    let deal_id = report["deal_id"].as_str().unwrap();
    let recovered = native_call(
        &alice,
        json!({"action":"get_task","task_id":deal_id,
        "provider_id":bob.state.identity.node_id(),"response_format":"compact"}),
    )
    .await;
    assert_eq!(recovered["isError"], false, "{recovered}");
    assert_eq!(
        recovered["structuredContent"]["deal_hash"],
        report["deal_hash"]
    );
    assert_eq!(
        recovered["structuredContent"]["receipt_verification"],
        report["receipt_verification"]
    );
    let replayed = native_call(&alice, request).await;
    assert_eq!(replayed["structuredContent"]["deal_id"], report["deal_id"]);
    assert_eq!(
        deal_count(&bob).await,
        1,
        "replay must not create another provider deal"
    );
}

fn load_a2a_config(directory: &std::path::Path, config: Value) -> froglet::a2a_config::A2aConfig {
    let path = directory.join("a2a-test.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    froglet::a2a_config::A2aConfig::load(&path).unwrap()
}

#[tokio::test]
async fn native_stdio_compute_uses_configured_a2a_between_independent_nodes() {
    let bob_directory = unique_temp_dir("a2a-bob");
    let alice_directory = unique_temp_dir("a2a-alice");
    let alice_state = create_dual_state_at(vec![PaymentBackend::None], alice_directory.clone());
    let requester_id = alice_state.identity.node_id().to_string();
    let bootstrap = start_node(create_dual_state_at(
        vec![PaymentBackend::None],
        bob_directory.clone(),
    ))
    .await;
    let offers: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/provider/offers",
            bootstrap.provider.base_url
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let offer_hash = offers["offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| {
            offer["payload"]["offer_id"] == "execute.compute"
                && offer["payload"]["offer_kind"] == "compute.wasm.v1"
                && offer["payload"]["settlement_method"] == "none"
                && offer["payload"]["price_schedule"]["base_fee_msat"] == 0
                && offer["payload"]["price_schedule"]["success_fee_msat"] == 0
        })
        .expect("free pure-Wasm Offer")["hash"]
        .as_str()
        .unwrap()
        .to_string();
    let provider_id = bootstrap.state.identity.node_id().to_string();
    drop(bootstrap);

    let token = "a2a-stdio-test-separate-provider-bearer";
    let mut bob_state = create_dual_state_at(vec![PaymentBackend::None], bob_directory.clone());
    bob_state.config.a2a = load_a2a_config(
        &bob_directory,
        json!({"clients":[{"requester_id":requester_id,"token":token,
            "offer_hashes":[offer_hash]}]}),
    );
    assert_eq!(bob_state.identity.node_id(), provider_id);
    let mut bob = start_node(bob_state).await;
    let observed = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let captured = observed.clone();
    // During the combined test every provider operation must traverse A2A.
    // Rejecting ordinary provider routes rules out a silent adapter fallback.
    let a2a_only = public_router(bob.state.clone()).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let captured = captured.clone();
            async move {
                use axum::response::IntoResponse;
                let path = request.uri().path().to_string();
                if !path.starts_with("/a2a/v1/") {
                    return axum::http::StatusCode::FORBIDDEN.into_response();
                }
                assert_eq!(
                    request.headers()["authorization"],
                    format!("Bearer {token}")
                );
                assert_eq!(request.headers()["a2a-version"], "1.0");
                if request.method() == axum::http::Method::POST {
                    let (parts, body) = request.into_parts();
                    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
                    let body: Value = serde_json::from_slice(&bytes).unwrap();
                    let operation = body["message"]["parts"][0]["data"]["operation"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    captured.lock().unwrap().push(operation);
                    next.run(axum::extract::Request::from_parts(
                        parts,
                        axum::body::Body::from(bytes),
                    ))
                    .await
                } else {
                    captured.lock().unwrap().push("get_task".into());
                    next.run(request).await
                }
            }
        },
    ));
    bob.provider = spawn_server(a2a_only).await;
    let mut alice_state = alice_state;
    alice_state.config.a2a = load_a2a_config(
        &alice_directory,
        json!({"providers":[{"provider_url":bob.provider.base_url,
            "token":token,"allow_loopback":true}]}),
    );
    let alice = start_node(alice_state).await;
    assert_ne!(alice.state.identity.node_id(), bob.state.identity.node_id());
    let arguments = json!({"action":"run_compute","wasm_module_hex":VALID_WASM_HEX,
        "input":{"sample":1},"provider_id":provider_id,"provider_url":bob.provider.base_url,
        "idempotency_key":"stdio-a2a-compute","response_format":"compact"}); // gitleaks:allow -- public retry label, not an authentication credential
    let response = native_call(&alice, arguments.clone()).await;
    assert_eq!(response["isError"], false, "{response}");
    let report = &response["structuredContent"];
    assert_eq!(report["status"], "succeeded", "{report}");
    assert_eq!(report["result"], 42);
    assert_eq!(report["provider_id"], provider_id);
    assert_eq!(report["receipt_verification"]["verified"], true);
    assert_eq!(report["deal_id"], report["deal_hash"]);
    assert!(report["execution_limits"]["fuel_limit"].as_u64().unwrap() > 0);
    assert!(!response.to_string().contains(token));
    let recovered = native_call(
        &alice,
        json!({"action":"get_task","task_id":report["deal_id"],
            "provider_id":provider_id,"response_format":"compact"}),
    )
    .await;
    assert_eq!(recovered["isError"], false, "{recovered}");
    assert_eq!(
        recovered["structuredContent"]["result_hash"],
        report["result_hash"]
    );
    assert_eq!(
        recovered["structuredContent"]["receipt_verification"],
        report["receipt_verification"]
    );
    let module_file = alice_directory.join("requester-program.wasm");
    std::fs::write(&module_file, hex::decode(VALID_WASM_HEX).unwrap()).unwrap();
    let mut path_request = arguments.clone();
    path_request
        .as_object_mut()
        .unwrap()
        .remove("wasm_module_hex");
    path_request["wasm_module_path"] = json!(module_file);
    let path_replay = native_call(&alice, path_request.clone()).await;
    assert_eq!(path_replay["isError"], false, "{path_replay}");
    for field in [
        "workload_hash",
        "deal_id",
        "result_hash",
        "receipt_verification",
    ] {
        assert_eq!(
            path_replay["structuredContent"][field], report[field],
            "{field}"
        );
    }
    // Mutating the same local filename is new work, even though the path and
    // retry key have not changed. The canonical module bytes remain binding.
    let mut changed_module = hex::decode(VALID_WASM_HEX).unwrap();
    *changed_module.last_mut().unwrap() = b'3';
    std::fs::write(&module_file, changed_module).unwrap();
    let changed = native_call(&alice, path_request).await;
    assert_eq!(changed["isError"], true, "{changed}");
    assert!(
        changed["structuredContent"]["error"]
            .as_str()
            .unwrap()
            .contains("idempotency key reused")
    );
    let replay = native_call(&alice, arguments).await;
    assert_eq!(replay["isError"], false, "{replay}");
    assert_eq!(replay["structuredContent"]["deal_id"], report["deal_id"]);
    assert_eq!(deal_count(&bob).await, 1);
    let operations = observed.lock().unwrap();
    for operation in ["catalog", "quote", "submit", "get_task"] {
        assert!(
            operations.iter().any(|seen| seen == operation),
            "missing A2A {operation}: {operations:?}"
        );
    }
}

#[tokio::test]
async fn changed_program_or_input_cannot_reuse_the_compute_key() {
    let (bob, alice, origin) = pair(0).await;
    let mut options = compute_options(&alice, &bob, "compute-replay");
    let first = run_inline_wasm(&options, Some(&origin), VALID_WASM_HEX)
        .await
        .unwrap();
    let uppercase = run_inline_wasm(&options, Some(&origin), &VALID_WASM_HEX.to_uppercase())
        .await
        .unwrap();
    assert_eq!(
        first.deal_id, uppercase.deal_id,
        "equivalent encoding is normalized"
    );
    options.input = json!({"sample":2});
    assert!(
        run_inline_wasm(&options, Some(&origin), VALID_WASM_HEX)
            .await
            .unwrap_err()
            .to_string()
            .contains("idempotency key reused")
    );
    options.input = json!({"sample":1});
    let changed = format!("{}3433", VALID_WASM_HEX.strip_suffix("3432").unwrap());
    assert!(
        run_inline_wasm(&options, Some(&origin), &changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("idempotency key reused")
    );
    assert_eq!(deal_count(&bob).await, 1);
}

#[tokio::test]
async fn pending_reference_survives_requester_restart_and_get_task_does_not_resubmit() {
    let (bob, alice, origin) = pair(0).await;
    let directory = alice.state.config.storage.data_dir.clone();
    let mut options = compute_options(&alice, &bob, "durable-compute");
    options.wait_timeout = Duration::ZERO;
    let submitted = run_inline_wasm(&options, Some(&origin), VALID_WASM_HEX)
        .await
        .unwrap();
    let requester_id = alice.state.identity.node_id().to_string();
    drop(alice);
    let restarted = start_node(requester_state(&bob, directory)).await;
    assert_eq!(restarted.state.identity.node_id(), requester_id);
    let options = compute_options(&restarted, &bob, "durable-compute");
    let mut report = get_task(&options, &submitted.deal_id).await.unwrap();
    for _ in 0..100 {
        if report.terminal {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        report = get_task(&options, &submitted.deal_id).await.unwrap();
    }
    assert_eq!(report.status, "succeeded", "{report:?}");
    assert_eq!(report.result, Some(json!(42)));
    assert_eq!(report.receipt_verification["verified"], true);
    assert_eq!(report.idempotency_key, "durable-compute");
    assert_eq!(deal_count(&bob).await, 1);
}

#[tokio::test]
async fn lost_caller_response_reconciles_persisted_compute_after_restart_without_new_execution() {
    let (bob, mut alice, origin) = pair(0).await;
    let directory = alice.state.config.storage.data_dir.clone();
    let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let injected = lost.clone();
    let lossy = runtime_router(alice.state.clone()).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let inject = request.method() == axum::http::Method::POST
                && request.uri().path() == "/v1/runtime/deals"
                && !injected.swap(true, Ordering::SeqCst);
            async move {
                let response = next.run(request).await;
                if inject && response.status().is_success() {
                    use axum::response::IntoResponse;
                    return (axum::http::StatusCode::BAD_GATEWAY,
                        axum::Json(json!({"error":"test discarded caller response after requester persistence"})))
                        .into_response();
                }
                response
            }
        }
    ));
    alice.runtime = spawn_server(lossy).await;
    let arguments = json!({"action":"run_compute","wasm_module_hex":VALID_WASM_HEX,
        "input":{"sample":1},"provider_id":bob.state.identity.node_id(),"provider_url":origin,
        "idempotency_key":"lost-caller-response","response_format":"compact"});
    let uncertain = native_call(&alice, arguments.clone()).await;
    assert_eq!(uncertain["isError"], true, "{uncertain}");
    assert_eq!(
        uncertain["structuredContent"]["idempotency_key"],
        "lost-caller-response"
    );
    assert!(lost.load(Ordering::SeqCst));
    drop(alice);
    let restarted = start_node(requester_state(&bob, directory)).await;
    let reconciled = native_call(&restarted, arguments).await;
    assert_eq!(reconciled["isError"], false, "{reconciled}");
    assert_eq!(reconciled["structuredContent"]["result"], 42);
    assert_eq!(
        reconciled["structuredContent"]["receipt_verification"]["verified"],
        true
    );
    assert_eq!(deal_count(&bob).await, 1);
}

#[tokio::test]
async fn pending_transport_error_and_unavailable_read_keep_the_durable_task_reference() {
    let reference = "a".repeat(64);
    let returned = reference.clone();
    let runtime = spawn_server(axum::Router::new().route("/v1/runtime/deals", axum::routing::post(move || {
        let reference = returned.clone();
        async move {
            (axum::http::StatusCode::BAD_GATEWAY, axum::Json(json!({
                "error":"provider response unavailable", "deal_id":reference,"task_id":reference,
                "status":"submission_pending", "idempotency_key":"pending-key"
            })))
        }
    }))).await;
    let dummy = create_dual_state(vec![PaymentBackend::None]);
    let alice = start_node(dummy).await;
    let mut options = compute_options(&alice, &alice, "pending-key");
    options.runtime_url = runtime.base_url.clone();
    let error = run_inline_wasm(&options, None, VALID_WASM_HEX)
        .await
        .unwrap_err();
    let froglet::cli::CliError::Structured { report, .. } = error else {
        panic!("structured error expected")
    };
    assert_eq!(report["deal_id"], reference);
    assert_eq!(report["task_id"], reference);
    assert_eq!(report["status"], "submission_pending");
    assert_eq!(report["receipt_verification"]["verified"], false);
    assert!(report["next_action"].as_str().unwrap().contains("get_task"));
    drop(runtime);
    let error = get_task(&options, &reference).await.unwrap_err();
    let froglet::cli::CliError::Structured { report, .. } = error else {
        panic!("structured error expected")
    };
    assert_eq!(report["task_id"], reference);
    assert_eq!(report["receipt_verification"]["status"], "not_checked");
}

#[tokio::test]
async fn free_default_and_existing_cumulative_budget_refuse_paid_compute_before_execution() {
    let (bob, alice, origin) = pair(25).await;
    let mut options = compute_options(&alice, &bob, "free-only");
    let message = run_inline_wasm(&options, Some(&origin), VALID_WASM_HEX)
        .await
        .unwrap_err()
        .to_string();
    assert!(message.contains("max_price_sats"), "{message}");
    options.max_price_sats = Some(25);
    options.idempotency_key = Some("paid-no-budget".into());
    let message = run_inline_wasm(&options, Some(&origin), VALID_WASM_HEX)
        .await
        .unwrap_err()
        .to_string();
    assert!(message.contains("spend_budget_unconfigured"), "{message}");
    assert_eq!(deal_count(&bob).await, 0);
}

#[tokio::test]
async fn nonterminating_compute_is_stopped_with_a_verified_failure_receipt() {
    let (bob, alice, origin) = pair(0).await;
    let module = wat::parse_str(
        r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) i32.const 16)
        (func (export "run") (param i32 i32) (result i64)
          (loop $forever br $forever) i64.const 0))"#,
    )
    .unwrap();
    let response = native_call(
        &alice,
        json!({"action":"run_compute","wasm_module_hex":hex::encode(module),
        "input":null,"provider_id":bob.state.identity.node_id(),"provider_url":origin,
        "idempotency_key":"fuel-stop","response_format":"compact"}),
    )
    .await;
    assert_eq!(response["isError"], true, "{response}");
    let report = &response["structuredContent"];
    assert_eq!(report["status"], "failed", "{report}");
    assert_eq!(report["receipt_verification"]["verified"], true, "{report}");
    assert_eq!(
        report["receipt_verification"]["failure_code"], "execution_limit_exceeded",
        "{report}"
    );
    assert_eq!(
        report["receipt_verification"]["limits_applied"],
        report["execution_limits"]
    );
}

#[tokio::test]
async fn task_reads_authenticate_and_reject_forged_evidence_and_identity() {
    let (bob, alice, origin) = pair(0).await;
    let mut options = compute_options(&alice, &bob, "evidence-check");
    let completed = run_inline_wasm(&options, Some(&origin), VALID_WASM_HEX)
        .await
        .unwrap();
    let payload: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/runtime/deals/{}",
            alice.runtime.base_url, completed.deal_id
        ))
        .bearer_auth("test-runtime-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    options.runtime_token = "wrong-runtime-token".into();
    assert!(get_task(&options, &completed.deal_id).await.is_err());
    options.runtime_token = "test-runtime-token".into();
    for (field, value) in [
        ("/deal/deal_id", json!("other-deal")),
        ("/deal/quote/payload/workload_hash", json!("f".repeat(64))),
        ("/deal/receipt/payload/provider_id", json!("f".repeat(64))),
        ("/deal/result", json!(43)),
        ("/deal/status", json!("failed")),
    ] {
        let mut forged = payload.clone();
        *forged.pointer_mut(field).unwrap() = value;
        let server = spawn_server(axum::Router::new().route(
            "/v1/runtime/deals/:id",
            axum::routing::get(move || {
                let response = forged.clone();
                async move { axum::Json(response) }
            }),
        ))
        .await;
        options.runtime_url = server.base_url.clone();
        let error = match get_task(&options, &completed.deal_id).await {
            Err(error) => error,
            Ok(report) => panic!("forgery {field} accepted: {report:?}"),
        };
        let froglet::cli::CliError::Structured { report, .. } = error else {
            panic!("verification error must retain its requested task reference")
        };
        assert_eq!(report["deal_id"], completed.deal_id);
        assert_eq!(report["task_id"], completed.deal_id);
        assert_eq!(report["receipt_verification"]["status"], "failed");
        assert_eq!(report["receipt_verification"]["verified"], false);
        assert!(report["next_action"].as_str().unwrap().contains("get_task"));
    }
    for (field, value) in [
        ("/deal/result", json!(43)),
        ("/deal/quote/payload/workload_hash", json!("f".repeat(64))),
    ] {
        let mut forged = json!({
            "provider_id":bob.state.identity.node_id(),
            "provider_url":origin,
            "quote":payload["deal"]["quote"],
            "deal":payload["deal"],
        });
        *forged.pointer_mut(field).unwrap() = value;
        let server = spawn_server(axum::Router::new().route(
            "/v1/runtime/deals",
            axum::routing::post(move || {
                let response = forged.clone();
                async move { axum::Json(response) }
            }),
        ))
        .await;
        options.runtime_url = server.base_url.clone();
        let error = match run_inline_wasm(&options, Some(&origin), VALID_WASM_HEX).await {
            Err(error) => error,
            Ok(report) => panic!("admission-response forgery {field} accepted: {report:?}"),
        };
        let froglet::cli::CliError::Structured { report, .. } = error else {
            panic!("post-admission verification error must retain its task reference")
        };
        assert_eq!(report["deal_id"], completed.deal_id);
        assert_eq!(report["task_id"], completed.deal_id);
        assert_eq!(report["idempotency_key"], "evidence-check");
        assert_eq!(report["receipt_verification"]["status"], "failed");
        assert_eq!(report["receipt_verification"]["verified"], false);
        assert!(report["next_action"].as_str().unwrap().contains("get_task"));
    }
    assert_eq!(
        deal_count(&bob).await,
        1,
        "task reads must not submit new work"
    );
}
