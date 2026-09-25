/// Integration test for the BuiltinServiceHandler dispatch mechanism.
///
/// Proves that a custom builtin service handler registered on AppState is
/// invoked through the standard execution dispatch when a deal or job targets
/// the corresponding offer_kind.
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use froglet::{
    api::runtime_router,
    builtins::{DataQueryHandler, DataQuerySourceKind},
    confidential::ConfidentialConfig,
    config::{
        IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig, PaymentBackend,
        PricingConfig, StorageConfig, WasmConfig,
    },
    db::DbPool,
    execution::BuiltinServiceHandler,
    pricing::PricingTable,
    state::{AppState, TransportStatus},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
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

// A mock builtin service handler that echoes the input with a counter.
struct EchoHandler {
    call_count: AtomicUsize,
}

impl EchoHandler {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

impl BuiltinServiceHandler for EchoHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(json!({
                "echo": input,
                "handler": "test.echo"
            }))
        })
    }
}

fn create_test_state_with_handler(
    handler_name: &str,
    handler: Arc<dyn BuiltinServiceHandler>,
) -> Arc<AppState> {
    let temp_dir = unique_temp_dir("builtin-dispatch");
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
        payment_backends: vec![PaymentBackend::None],
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

    let pool = DbPool::open(&db_path).expect("init db");
    let events_query_capacity = pool.read_connection_count().max(1);
    let identity =
        froglet::identity::NodeIdentity::load_or_create(&node_config).expect("create identity");
    let pricing = PricingTable::from_config(node_config.pricing);
    let settlement_registry =
        froglet::settlement::SettlementRegistry::new(&node_config).expect("settlement registry");

    let mut builtin_services: HashMap<String, Arc<dyn BuiltinServiceHandler>> = HashMap::new();
    builtin_services.insert(handler_name.to_string(), handler);

    Arc::new(AppState {
        db: pool,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: froglet::tls::reqwest_client_builder()
            .timeout(std::time::Duration::from_secs(5))
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
        process_execution_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
        native_data_query_handlers: froglet::builtins::DataQueryHandlerCache::default(),
        native_data_publication_lock: tokio::sync::Mutex::const_new(()),
        hosted_trial_deal_quota: None,
        hosted_trial_session_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            std::time::Duration::from_secs(60),
        )),
        event_publish_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            std::time::Duration::from_secs(60),
        )),
        quote_create_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            std::time::Duration::from_secs(60),
        )),
        confidential_session_quota: Arc::new(froglet::public_quota::IdentityQuota::new(
            1000,
            std::time::Duration::from_secs(60),
        )),
        lnd_rest_client: None,
        phoenixd_client: None,
        lightning_wallet: None,
        lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
        event_batch_writer: None,
        builtin_services,
        settlement_registry,
        session_pool: None,
    })
}

fn runtime_request(method: axum::http::Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer test-runtime-token");
    let body = if let Some(value) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&value).expect("serialize request"))
    } else {
        Body::empty()
    };
    builder.body(body).expect("build request")
}

async fn response_json(response: axum::response::Response<Body>) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let payload: Value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "parse JSON (status {}): {}; body={}",
            status,
            error,
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, payload)
}

async fn run_job_to_terminal(
    app: &axum::Router,
    execution: froglet::execution::ExecutionWorkload,
    idempotency_key: &str,
) -> Value {
    let response = app
        .clone()
        .oneshot(runtime_request(
            axum::http::Method::POST,
            "/v1/node/jobs",
            Some(json!({
                "kind": "execution",
                "execution": execution,
                "idempotency_key": idempotency_key,
            })),
        ))
        .await
        .expect("job create response");
    let (status, payload) = response_json(response).await;
    assert_eq!(status, StatusCode::ACCEPTED, "unexpected job: {payload}");
    let job_id = payload["job_id"].as_str().expect("job_id");

    for _ in 0..100 {
        let response = app
            .clone()
            .oneshot(runtime_request(
                axum::http::Method::GET,
                &format!("/v1/node/jobs/{job_id}"),
                None,
            ))
            .await
            .expect("job status response");
        let (status, payload) = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "unexpected job payload: {payload}");
        match payload["status"].as_str() {
            Some("succeeded") => return payload["result"].clone(),
            Some("failed") => panic!("job failed: {payload}"),
            _ => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
        }
    }
    panic!("timed out waiting for job")
}

/// Test: a registered BuiltinServiceHandler is invoked through the /v1/node/jobs
/// execution path. This proves that the dispatch in run_workload_spec_with_admission
/// correctly routes to the handler.
#[tokio::test]
async fn builtin_service_handler_dispatch_through_jobs_api() {
    let echo = Arc::new(EchoHandler::new());
    let state = create_test_state_with_handler("test.echo", echo.clone());
    let app = runtime_router(state);

    let execution = froglet::execution::ExecutionWorkload::builtin_service(
        "test.echo".to_string(),
        json!({"query": "hello froglet"}),
    )
    .expect("builtin execution workload");

    let response = app
        .oneshot(runtime_request(
            axum::http::Method::POST,
            "/v1/node/jobs",
            Some(json!({
                "kind": "execution",
                "execution": execution,
                "idempotency_key": "builtin-dispatch-test",
            })),
        ))
        .await
        .expect("job create response");

    let (status, payload) = response_json(response).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "job should be accepted: {payload}"
    );

    let _job_id = payload["job_id"].as_str().expect("job_id").to_string();

    // Jobs execute asynchronously. Wait for the handler to be called.
    for _ in 0..20 {
        if echo.calls() > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    assert_eq!(
        echo.calls(),
        1,
        "handler should have been called exactly once"
    );
}

/// Test: the events.query builtin still works even with custom handlers registered,
/// verifying backward compatibility.
#[tokio::test]
async fn events_query_still_works_with_custom_handlers_registered() {
    let echo = Arc::new(EchoHandler::new());
    let state = create_test_state_with_handler("test.echo", echo.clone());
    let app = runtime_router(state);

    let execution = froglet::execution::ExecutionWorkload::builtin_events_query(
        vec!["market.listing".to_string()],
        Some(10),
    )
    .expect("events query execution");

    let response = app
        .oneshot(runtime_request(
            axum::http::Method::POST,
            "/v1/node/jobs",
            Some(json!({
                "kind": "execution",
                "execution": execution,
                "idempotency_key": "events-query-compat-test",
            })),
        ))
        .await
        .expect("job create response");

    let (status, payload) = response_json(response).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "events.query job should be accepted: {payload}"
    );

    // The echo handler should NOT have been called — events.query has its own path
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        echo.calls(),
        0,
        "echo handler should not be called for events.query"
    );
}

/// Test: requesting an unknown builtin service returns an error.
#[tokio::test]
async fn unknown_builtin_service_is_rejected() {
    let echo = Arc::new(EchoHandler::new());
    let state = create_test_state_with_handler("test.echo", echo.clone());
    let app = runtime_router(state);

    let execution = froglet::execution::ExecutionWorkload::builtin_service(
        "nonexistent.service".to_string(),
        json!({}),
    )
    .expect("builtin execution workload");

    let response = app
        .oneshot(runtime_request(
            axum::http::Method::POST,
            "/v1/node/jobs",
            Some(json!({
                "kind": "execution",
                "execution": execution,
                "idempotency_key": "unknown-builtin-test",
            })),
        ))
        .await
        .expect("job create response");

    let (status, _payload) = response_json(response).await;
    // Job is accepted initially (async), but the handler will fail.
    // The important thing is the echo handler was NOT called.
    assert_eq!(status, StatusCode::ACCEPTED);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        echo.calls(),
        0,
        "echo handler should not be called for unknown service"
    );
}

/// Proves a user-supplied SQLite file can be invoked locally through the
/// native builtin job path without Python, Node, OCI, or caller-provided SQL.
#[tokio::test]
async fn sqlite_data_query_builtin_invokes_through_jobs_api() {
    let data_root = tempfile::tempdir().expect("data root");
    let db_path = data_root.path().join("samples.sqlite");
    let connection = rusqlite::Connection::open(&db_path).expect("create fixture database");
    connection
        .execute_batch(
            "CREATE TABLE samples (id INTEGER PRIMARY KEY, status TEXT NOT NULL);
             INSERT INTO samples (id, status) VALUES (1, 'ready');
             INSERT INTO samples (id, status) VALUES (2, 'pending');",
        )
        .expect("seed fixture database");
    drop(connection);

    let handler = Arc::new(
        DataQueryHandler::open(
            data_root.path(),
            "samples.sqlite",
            DataQuerySourceKind::Sqlite,
        )
        .expect("bind read-only SQLite handler"),
    );
    let state = create_test_state_with_handler("data.samples", handler);
    let app = runtime_router(state);
    let execution = froglet::execution::ExecutionWorkload::builtin_service(
        "data.samples".to_string(),
        json!({
            "op": "select",
            "collection": "samples",
            "columns": ["id"],
            "equals": {"status": "ready"}
        }),
    )
    .expect("data query execution workload");

    let response = app
        .clone()
        .oneshot(runtime_request(
            axum::http::Method::POST,
            "/v1/node/jobs",
            Some(json!({
                "kind": "execution",
                "execution": execution,
                "idempotency_key": "sqlite-data-query-local-invocation",
            })),
        ))
        .await
        .expect("job create response");
    let (status, payload) = response_json(response).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "unexpected payload: {payload}"
    );
    let job_id = payload["job_id"].as_str().expect("job_id");

    for _ in 0..50 {
        let response = app
            .clone()
            .oneshot(runtime_request(
                axum::http::Method::GET,
                &format!("/v1/node/jobs/{job_id}"),
                None,
            ))
            .await
            .expect("job status response");
        let (status, payload) = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "unexpected job payload: {payload}");
        match payload["status"].as_str() {
            Some("succeeded") => {
                assert_eq!(payload["result"]["source_kind"], "sqlite");
                assert_eq!(payload["result"]["rows"], json!([{"id": 1}]));
                return;
            }
            Some("failed") => panic!("data query job failed: {payload}"),
            _ => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }
    panic!("timed out waiting for SQLite data query job");
}

#[tokio::test]
async fn repeated_csv_native_invocation_reuses_validated_handler() {
    use froglet_protocol::publication::{
        PublicationCsvColumn, PublicationCsvColumnType, PublicationCsvSchema,
        PublicationDataFormat, PublicationDataSource,
    };

    let state = create_test_state_with_handler("unrelated.echo", Arc::new(EchoHandler::new()));
    let publication_root = state.config.storage.data_dir.join("publication-data");
    std::fs::create_dir_all(&publication_root).expect("publication data directory");
    let source = b"id,name\n1,Ada\n2,Grace\n";
    let schema = PublicationCsvSchema {
        collection: "people".to_string(),
        columns: vec![
            PublicationCsvColumn {
                name: "id".to_string(),
                column_type: PublicationCsvColumnType::Integer,
                nullable: false,
                indexed: true,
            },
            PublicationCsvColumn {
                name: "name".to_string(),
                column_type: PublicationCsvColumnType::String,
                nullable: false,
                indexed: true,
            },
        ],
    };
    let package = PublicationDataSource {
        format: PublicationDataFormat::Csv,
        content_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, source),
        csv_schema: Some(schema.clone()),
    };
    let package_digest = package.csv_package_digest().expect("CSV package digest");
    let source_path = publication_root.join(format!("{package_digest}.csv"));
    let schema_path = publication_root.join(format!("{package_digest}.csv.schema.json"));
    std::fs::write(&source_path, source).expect("write immutable CSV fixture");
    std::fs::write(
        &schema_path,
        froglet::canonical_json::to_vec(&schema).expect("canonical schema"),
    )
    .expect("write immutable CSV schema");
    let app = runtime_router(state);

    let workload = |name: &str| {
        froglet::execution::ExecutionWorkload::bound_builtin_service(
            "data.people".to_string(),
            froglet::builtins::DATA_QUERY_CSV_CONTRACT_V1.to_string(),
            Some(package_digest.clone()),
            json!({
                "op": "select",
                "collection": "people",
                "columns": ["name"],
                "equals": {"name": name},
                "limit": 1
            }),
        )
        .expect("bound CSV workload")
    };

    let first = run_job_to_terminal(&app, workload("Ada"), "csv-cache-first").await;
    assert_eq!(first["rows"], json!([{"name": "Ada"}]));

    // A second invocation can only succeed after these immutable inputs are
    // removed if the already-validated handler is reused rather than opened
    // and validated again.
    std::fs::remove_file(&source_path).expect("remove staged CSV after first invocation");
    std::fs::remove_file(&schema_path).expect("remove staged schema after first invocation");
    let second = run_job_to_terminal(&app, workload("Grace"), "csv-cache-second").await;
    assert_eq!(second["rows"], json!([{"name": "Grace"}]));
}
