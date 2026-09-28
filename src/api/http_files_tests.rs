use crate::{
    file_download::FileLimits,
    provider_policy::{self, AccessMode},
};
use froglet_protocol::file_download::{self, FileMetadata};

async fn file_fixture(mode: AccessMode, count: u64) -> (Arc<AppState>, FileMetadata, String) {
    file_fixture_bytes(mode, count, b"<p/>").await
}
async fn file_fixture_bytes(
    mode: AccessMode,
    count: u64,
    bytes: &[u8],
) -> (Arc<AppState>, FileMetadata, String) {
    let mut state = test_app_state(PaymentBackend::None);
    let policy = &mut Arc::get_mut(&mut state).unwrap().config.provider_policy;
    policy.access_mode = mode;
    policy.file_download =
        Some(FileLimits::new(count, count * bytes.len().max(1) as u64, 16 * 1024 * 1024).unwrap());
    let metadata = FileMetadata {
        filename: "sample.html".into(),
        media_type: "text/html".into(),
        size_bytes: bytes.len() as u64,
        sha256: crypto::sha256_hex(bytes),
        expires_at: settlement::current_unix_timestamp() + 600,
        max_downloads: count,
        max_transfer_bytes: count * bytes.len().max(1) as u64,
    };
    let package = file_download::encode(&metadata, bytes).unwrap();
    let mut payload = native_data_publication_request("download", &package);
    payload["data_source"]["format"] = json!("file");
    payload["verification"] = json!({"input":{"action":"describe"},"expected_output":metadata});
    let response = public_router(state.clone())
        .oneshot(runtime_request(
            Method::POST,
            "/v1/provider/artifacts/publish",
            Some("test-provider-control-token"),
            Some(payload),
        ))
        .await
        .unwrap();
    let (status, body): (StatusCode, Value) = response_json(response).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let revision = body["evidence"]["publication_revision"]["revision_hash"]
        .as_str()
        .unwrap();
    let path = format!("/v1/provider/services/download/files/{revision}/download");
    (state, metadata, path)
}
fn file_request(method: Method, path: &str, token: Option<&str>) -> Request<Body> {
    let mut r = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "https://froglet.dev");
    if let Some(token) = token {
        r = r.header("x-froglet-access-token", token);
    }
    r.body(Body::empty()).unwrap()
}
async fn used(state: &Arc<AppState>) -> crate::file_download::TransferUsage {
    state
        .db
        .with_read_conn(|c| crate::file_download::usage(c, "provider"))
        .await
        .unwrap()
}
#[tokio::test]
async fn file_download_exact_bytes_head_cors_and_persistent_limit() {
    let (state, metadata, path) = file_fixture(AccessMode::Open, 1).await;
    let app = public_router(state.clone());
    let preflight = app
        .clone()
        .oneshot(file_request(Method::OPTIONS, &path, None))
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
    let head = app
        .clone()
        .oneshot(file_request(Method::HEAD, &path, None))
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_LENGTH], "4");
    assert!(to_bytes(head.into_body(), 1024).await.unwrap().is_empty());
    assert_eq!(used(&state).await.downloads, 0);
    let response = app
        .clone()
        .oneshot(file_request(Method::GET, &path, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"sample.html\""
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://froglet.dev"
    );
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    assert_eq!(crypto::sha256_hex(&bytes), metadata.sha256);
    assert_eq!(bytes.as_ref(), b"<p/>");
    assert_eq!(used(&state).await.bytes, 4);
    let retry = app
        .oneshot(file_request(Method::GET, &path, None))
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(used(&state).await.downloads, 1);
}
#[tokio::test]
async fn file_download_invitation_revocation_pause_range_and_foreign_origin() {
    let (state, _, path) = file_fixture(AccessMode::Invite, 10).await;
    let app = public_router(state.clone());
    let now = settlement::current_unix_timestamp();
    let (id, token) = state
        .db
        .with_write_conn(move |c| provider_policy::create_invite(c, "recipient", now + 60, 10, now))
        .await
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(file_request(Method::GET, &path, None))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut foreign = file_request(Method::GET, &path, Some(&token));
    foreign.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://evil.example"),
    );
    assert_eq!(
        app.clone().oneshot(foreign).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut range = file_request(Method::GET, &path, Some(&token));
    range
        .headers_mut()
        .insert(header::RANGE, HeaderValue::from_static("bytes=0-1"));
    assert_eq!(
        app.clone().oneshot(range).await.unwrap().status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );
    assert_eq!(used(&state).await.downloads, 0);
    let response = app
        .clone()
        .oneshot(file_request(Method::GET, &path, Some(&token)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    state
        .db
        .with_write_conn(|c| provider_policy::set_pause(c, Some("test pause")))
        .await
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(file_request(Method::GET, &path, Some(&token)))
            .await
            .unwrap()
            .status(),
        StatusCode::GONE
    );
    state
        .db
        .with_write_conn(move |c| {
            provider_policy::set_pause(c, None)?;
            provider_policy::revoke_invite(c, &id)
        })
        .await
        .unwrap();
    assert_eq!(
        app.oneshot(file_request(Method::GET, &path, Some(&token)))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(used(&state).await.downloads, 1);
}
#[tokio::test]
async fn file_download_rejects_changed_snapshot_and_old_revision_without_refund() {
    let (state, _, path) = file_fixture(AccessMode::Open, 2).await;
    let old = path.replace("/files/", "/files/00");
    assert_eq!(
        public_router(state.clone())
            .oneshot(file_request(Method::GET, &old, None))
            .await
            .unwrap()
            .status(),
        StatusCode::GONE
    );
    let record = provider_service_record(&state, "download", false, true)
        .await
        .unwrap()
        .unwrap();
    let snapshot = state
        .config
        .storage
        .data_dir
        .join("publication-data")
        .join(format!("{}.file", record.module_hash.unwrap()));
    std::fs::write(snapshot, b"changed").unwrap();
    let r = public_router(state.clone())
        .oneshot(file_request(Method::GET, &path, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(used(&state).await.downloads, 1);
}
#[tokio::test]
async fn file_download_concurrency_and_operator_abort_are_bounded() {
    let (state, _, path) = file_fixture(AccessMode::Open, 3).await;
    let app = public_router(state.clone());
    let a = app
        .clone()
        .oneshot(file_request(Method::GET, &path, None))
        .await
        .unwrap();
    assert_eq!(a.status(), StatusCode::OK);
    assert_eq!(
        app.clone()
            .oneshot(file_request(Method::GET, &path, None))
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(used(&state).await.downloads, 1);
    let denied = app
        .clone()
        .oneshot(runtime_request(
            Method::POST,
            "/v1/provider/files/abort",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_ne!(denied.status(), StatusCode::OK);
    let abort = app
        .oneshot(runtime_request(
            Method::POST,
            "/v1/provider/files/abort",
            Some("test-provider-control-token"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(abort.status(), StatusCode::OK);
    assert!(to_bytes(a.into_body(), 1024).await.is_err());
}

/// Explicit local browser harness; no external service or production allowance.
#[tokio::test]
#[ignore = "local browser harness; set FROGLET_FILE_BROWSER_FIXTURE to a new output JSON path"]
async fn file_download_browser_fixture() {
    let output = std::env::var("FROGLET_FILE_BROWSER_FIXTURE").expect("explicit fixture output");
    let bytes = if std::env::var("FROGLET_FILE_TEST_MAX_SIZE").as_deref() == Ok("1") {
        vec![0x5a; file_download::MAX_FILE_BYTES]
    } else {
        b"<p/>".to_vec()
    };
    let (mut state, metadata, path) = file_fixture_bytes(AccessMode::Open, 10, &bytes).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let relay = std::env::var("FROGLET_FILE_TEST_RELAY").ok();
    let mut relay_origin = None;
    if let Some(relay) = relay {
        let control = format!("ws://{relay}/v1/tunnel");
        let provider_id = state.identity.node_id().to_string();
        let config = &mut Arc::get_mut(&mut state).unwrap().config.relay;
        config.enabled = true;
        config.url = Some(control.clone());
        config.public_suffix = Some("relay.froglet.dev".into());
        let public = config.planned_public_url(&provider_id).unwrap().unwrap();
        state.transport_status.lock().await.relay_url = Some(public.clone());
        let url = public.clone();
        let endpoint = control.clone();
        state
            .db
            .with_write_conn(move |c| {
                let lc = db::get_publication_lifecycle(c, "download")?.unwrap();
                db::persist_publication_transport_grant(
                    c,
                    &db::NewPublicationTransportGrant {
                        transport: "relay",
                        service_id: "download",
                        revision_hash: lc.active_revision_hash.as_deref().unwrap(),
                        activation_token: &lc.activation_token,
                        public_url: &url,
                        relay_control_url: &endpoint,
                        now: settlement::current_unix_timestamp(),
                    },
                )?;
                Ok::<(), String>(())
            })
            .await
            .unwrap();
        let app = state.clone();
        let backend = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = crate::relay_tunnel::run_tunnel_once(app, &control, backend, &public).await;
        });
        relay_origin = Some(format!("http://{relay}"));
    }
    let details = json!({"origin":origin,"relay_origin":relay_origin,"provider_id":state.identity.node_id(),"service_id":"download","path":path,"metadata":metadata});
    std::fs::write(output, serde_json::to_vec(&details).unwrap()).unwrap();
    let _ = tokio::time::timeout(
        Duration::from_secs(1200),
        axum::serve(listener, public_router(state)).into_future(),
    )
    .await;
}

#[tokio::test]
async fn file_download_maximum_size_and_lower_relay_cap() {
    let bytes = vec![0x5a; file_download::MAX_FILE_BYTES];
    let (state, m, path) = file_fixture_bytes(AccessMode::Open, 1, &bytes).await;
    let app = public_router(state.clone());
    let mut too_small = file_request(Method::GET, &path, None);
    too_small.headers_mut().insert(
        "x-froglet-relay-response-limit",
        HeaderValue::from_static("1048576"),
    );
    assert_eq!(
        app.clone().oneshot(too_small).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(used(&state).await.downloads, 0);
    let response = app
        .oneshot(file_request(Method::GET, &path, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = to_bytes(response.into_body(), file_download::MAX_FILE_BYTES)
        .await
        .unwrap();
    assert_eq!(result.as_ref(), bytes);
    assert_eq!(crypto::sha256_hex(&result), m.sha256);
}

#[tokio::test]
async fn file_download_requires_one_exact_service_and_revision_grant() {
    let (state, _, path) = file_fixture(AccessMode::Open, 2).await;
    let revision = path.split('/').nth(6).unwrap().to_string();
    let grant = |service: &str, revision: &str| db::PublicationTransportGrantRecord {
        transport: "relay".into(),
        service_id: service.into(),
        revision_hash: revision.into(),
        offer_id: String::new(),
        offer_hash: String::new(),
        activation_token: String::new(),
        public_url: String::new(),
        relay_control_url: String::new(),
        created_at: 0,
        updated_at: 0,
    };
    let app = super::super::http_files::routes().with_state(state.clone());
    for (grants, expected) in [
        (
            vec![
                grant("download", &"a".repeat(64)),
                grant("another-file", &revision),
            ],
            StatusCode::NOT_FOUND,
        ),
        (vec![grant("download", &revision)], StatusCode::OK),
    ] {
        let scope = RelayGrantScope {
            grants: Arc::new(grants),
            _linearization_guard: Arc::new(
                Arc::new(tokio::sync::RwLock::new(())).read_owned().await,
            ),
        };
        let mut request = file_request(Method::GET, &path, None);
        request.extensions_mut().insert(scope);
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected);
    }
    assert_eq!(used(&state).await.downloads, 1);
}

#[tokio::test]
async fn file_metadata_does_not_expose_snapshot_bytes_or_private_paths() {
    let secret = b"PRIVATE FILE PAYLOAD THAT MUST NOT APPEAR IN CATALOG";
    let (state, _, _) = file_fixture_bytes(AccessMode::Open, 2, secret).await;
    let root = state.config.storage.data_dir.to_string_lossy().to_string();
    let app = public_router(state.clone());
    for path in [
        "/v1/provider/descriptor",
        "/v1/provider/offers",
        "/v1/provider/services/download",
    ] {
        let response = app
            .clone()
            .oneshot(file_request(Method::GET, path, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(
            !text.contains(std::str::from_utf8(secret).unwrap()),
            "{path}"
        );
        assert!(
            !text.contains(&base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                secret
            )),
            "{path}"
        );
        assert!(!text.contains(&root), "{path}");
    }
    assert_eq!(used(&state).await.downloads, 0);
}
