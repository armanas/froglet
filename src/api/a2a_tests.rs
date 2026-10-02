use super::*;
use crate::config::PaymentBackend;
use tower::ServiceExt;

const TOKEN: &str = "a2a-targeted-separate-requester-bearer";
const WASM_42: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";

async fn fixture() -> (Arc<AppState>, Arc<AppState>, WorkloadSpec) {
    let mut bob = super::super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    let alice = super::super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    assert_ne!(alice.identity.node_id(), bob.identity.node_id());
    let offer = current_offer_artifacts(&bob)
        .await
        .unwrap()
        .into_iter()
        .find(|offer| offer.payload.offer_id == "execute.compute")
        .unwrap();
    Arc::get_mut(&mut bob)
        .unwrap()
        .config
        .a2a
        .clients
        .push(A2aClient {
            requester_id: alice.identity.node_id().into(),
            token: TOKEN.into(),
            offer_hashes: vec![offer.hash],
        });
    let spec = WorkloadSpec::Wasm {
        submission: Box::new(
            crate::cli::invoke::build_inline_wasm_submission(WASM_42, json!({})).unwrap(),
        ),
    };
    (bob, alice, spec)
}

fn wire(operation: &str, payload: impl Serialize, task_id: Option<&str>) -> Value {
    let mut value = json!({"message":{"messageId":"targeted-test","role":"ROLE_USER","parts":[data_part(operation,&payload).unwrap()]},"configuration":{"returnImmediately":false,"historyLength":0}});
    if let Some(id) = task_id {
        value["message"]["taskId"] = json!(id);
        value["message"]["contextId"] = json!(context_id(id));
    }
    value
}

async fn call(
    app: Router,
    method: axum::http::Method,
    path: &str,
    token: Option<&str>,
    value: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("a2a-version", "1.0")
        .header("a2a-extensions", EXTENSION)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(
            request
                .body(axum::body::Body::from(
                    value.map(|value| value.to_string()).unwrap_or_default(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_UPSTREAM_JSON_BYTES)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn quote(
    bob: Arc<AppState>,
    alice: &AppState,
    spec: WorkloadSpec,
) -> SignedArtifact<QuotePayload> {
    let (status, value) = call(
        super::super::public_router(bob),
        axum::http::Method::POST,
        "/a2a/v1/message:send",
        Some(TOKEN),
        Some(wire(
            "quote",
            CreateQuoteRequest {
                offer_id: "execute.compute".into(),
                requester_id: alice.identity.node_id().into(),
                spec,
                max_price_sats: Some(0),
            },
            None,
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    serde_json::from_value(remote_client::message_payload(value, "quote").unwrap()).unwrap()
}

fn submission(
    alice: &AppState,
    quote: SignedArtifact<QuotePayload>,
    spec: WorkloadSpec,
    key: &str,
) -> CreateDealRequest {
    let deal = build_runtime_requester_deal_artifact(
        alice,
        &quote,
        &crypto::sha256_hex([7; 32]),
        settlement::current_unix_timestamp(),
        false,
    )
    .unwrap();
    CreateDealRequest {
        quote,
        deal,
        spec,
        idempotency_key: Some(key.into()),
        payment: None,
    }
}

async fn submit(bob: Arc<AppState>, request: &CreateDealRequest) -> Value {
    let (status, value) = call(
        super::super::public_router(bob),
        axum::http::Method::POST,
        "/a2a/v1/message:send",
        Some(TOKEN),
        Some(wire("submit", request, None)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value["task"].clone()
}

#[tokio::test]
async fn a2a_signed_wasm_task_rejects_tampered_projection_and_cross_identity() {
    let (mut bob, alice, spec) = fixture().await;
    let offer_hashes = bob.config.a2a.clients[0].offer_hashes.clone();
    Arc::get_mut(&mut bob)
        .unwrap()
        .config
        .a2a
        .clients
        .push(A2aClient {
            requester_id: "aa".repeat(32),
            token: "other-separate-a2a-credential-00000000".into(),
            offer_hashes,
        });
    let request = submission(
        &alice,
        quote(bob.clone(), &alice, spec.clone()).await,
        spec,
        "success",
    );
    let task = submit(bob.clone(), &request).await;
    assert_eq!(task["status"]["state"], "TASK_STATE_COMPLETED");
    let record: deals::DealRecord =
        serde_json::from_value(remote_client::task_payload(task.clone(), None).unwrap()).unwrap();
    assert_eq!(record.result, Some(json!(42)));
    assert!(protocol::validate_quote_deal(&record.quote, &record.deal, None).valid);
    assert!(record.receipt.is_some());
    let mut tampered = task.clone();
    tampered["status"]["state"] = json!("TASK_STATE_CANCELED");
    assert!(remote_client::task_payload(tampered, None).is_err());
    let mut tampered = task.clone();
    let mut wrong = record.clone();
    wrong.result = Some(json!(43));
    tampered["artifacts"][0]["parts"][0] = data_part("deal", &wrong).unwrap();
    assert!(remote_client::task_payload(tampered, None).is_err());
    let id = task["id"].as_str().unwrap();
    let app = super::super::public_router(bob.clone());
    for token in [
        None,
        Some("test-runtime-token"),
        Some("test-provider-control-token"),
    ] {
        assert_eq!(
            call(
                app.clone(),
                axum::http::Method::GET,
                &format!("/a2a/v1/tasks/{id}"),
                token,
                None
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, error) = call(
        app.clone(),
        axum::http::Method::GET,
        &format!("/a2a/v1/tasks/{id}"),
        Some("other-separate-a2a-credential-00000000"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["details"][0]["reason"], "TASK_NOT_FOUND");
    let (status, error) = call(
        app,
        axum::http::Method::POST,
        &format!("/a2a/v1/tasks/{id}:cancel"),
        Some(TOKEN),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error["error"]["details"][0]["reason"],
        "TASK_NOT_CANCELABLE"
    );
}

#[tokio::test]
async fn a2a_exact_existing_deal_replays_after_quote_expiry_but_new_deal_is_rejected() {
    let (bob, alice, spec) = fixture().await;
    let original = quote(bob.clone(), &alice, spec.clone()).await;
    let now = settlement::current_unix_timestamp();
    let mut payload = original.payload;
    payload.expires_at = now + 2;
    let short = sign_node_artifact(&bob, ARTIFACT_KIND_QUOTE, now, payload.clone()).unwrap();
    payload.expires_at = now + 1;
    let unused = sign_node_artifact(&bob, ARTIFACT_KIND_QUOTE, now, payload).unwrap();
    for artifact in [short.clone(), unused.clone()] {
        bob.db
            .with_write_conn(move |conn| {
                persist_runtime_artifact(
                    conn,
                    &artifact.hash,
                    &artifact.payload_hash,
                    ARTIFACT_KIND_QUOTE,
                    &artifact.signer,
                    artifact.created_at,
                    &serde_json::to_string(&artifact).unwrap(),
                )?;
                deals::insert_quote(conn, &artifact)
            })
            .await
            .unwrap();
    }
    let request = submission(&alice, short, spec.clone(), "expires-but-existing");
    let new_request = submission(&alice, unused, spec, "unused-expired");
    let task = submit(bob.clone(), &request).await;
    while settlement::current_unix_timestamp() <= now + 2 {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let replay = submit(bob.clone(), &request).await;
    assert_eq!(task["id"], replay["id"]);
    assert_eq!(task["artifacts"], replay["artifacts"]);
    let (status, value) = call(
        super::super::public_router(bob.clone()),
        axum::http::Method::POST,
        "/a2a/v1/message:send",
        Some(TOKEN),
        Some(wire("submit", &new_request, None)),
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "{value}");
    let hash = protocol::artifact_hash(&new_request.deal).unwrap();
    let (status, value) = call(
        super::super::public_router(bob.clone()),
        axum::http::Method::POST,
        "/a2a/v1/message:send",
        Some(TOKEN),
        Some(wire("submit", &new_request, Some(&hash))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{value}");
    assert_eq!(value["error"]["details"][0]["reason"], "TASK_NOT_FOUND");
    let count = bob
        .db
        .with_read_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM deals", [], |row| row.get::<_, i64>(0))
                .map_err(|error| error.to_string())
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn a2a_public_quota_preserves_bounded_owned_recovery_and_structured_errors() {
    let (mut bob, alice, spec) = fixture().await;
    let request = submission(
        &alice,
        quote(bob.clone(), &alice, spec.clone()).await,
        spec.clone(),
        "quota",
    );
    let task = submit(bob.clone(), &request).await;
    Arc::get_mut(&mut bob).unwrap().public_request_quota = Arc::new(
        crate::public_quota::IdentityQuota::new(4, Duration::from_secs(60)),
    );
    let app = super::super::public_router(bob.clone());
    let id = task["id"].as_str().unwrap();
    for _ in 0..4 {
        assert_eq!(
            call(
                app.clone(),
                axum::http::Method::POST,
                "/a2a/v1/message:send",
                Some(TOKEN),
                Some(wire("catalog", json!({}), None))
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let (status, error) = call(
        app.clone(),
        axum::http::Method::POST,
        "/a2a/v1/message:send",
        Some(TOKEN),
        Some(wire(
            "quote",
            CreateQuoteRequest {
                offer_id: "execute.compute".into(),
                requester_id: alice.identity.node_id().into(),
                spec,
                max_price_sats: Some(0),
            },
            None,
        )),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error["error"]["code"], 429);
    assert_eq!(
        call(
            app.clone(),
            axum::http::Method::GET,
            &format!("/a2a/v1/tasks/{id}"),
            Some(TOKEN),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    for operation in ["accept", "invoice_bundle"] {
        let (status, error) = call(
            app.clone(),
            axum::http::Method::POST,
            "/a2a/v1/message:send",
            Some(TOKEN),
            Some(wire(operation, json!({}), Some(id))),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(
            error["error"]["details"][0]["reason"],
            "UNSUPPORTED_OPERATION"
        );
    }
    assert_eq!(
        call(
            app.clone(),
            axum::http::Method::POST,
            "/a2a/v1/message:send",
            Some(TOKEN),
            Some(wire("submit", &request, None))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, error) = call(
        app,
        axum::http::Method::GET,
        &format!("/a2a/v1/tasks/{id}"),
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error["error"]["code"], 429);
    assert!(error["error"]["details"].is_array());
}

#[tokio::test]
async fn a2a_pause_preserves_existing_recovery_and_invalid_query_is_structured() {
    let (bob, alice, spec) = fixture().await;
    let request = submission(
        &alice,
        quote(bob.clone(), &alice, spec.clone()).await,
        spec.clone(),
        "paused",
    );
    let task = submit(bob.clone(), &request).await;
    bob.db
        .with_write_conn(|conn| crate::provider_policy::set_pause(conn, Some("paused test")))
        .await
        .unwrap();
    let app = super::super::public_router(bob.clone());
    let (status, value) = call(
        app.clone(),
        axum::http::Method::GET,
        "/a2a/v1/tasks?pageSize=abc",
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], 400);
    assert_eq!(value["error"]["status"], "INVALID_ARGUMENT");
    assert_eq!(value["error"]["details"][0]["reason"], "INVALID_REQUEST");
    let (status, value) = call(
        app.clone(),
        axum::http::Method::POST,
        "/a2a/v1/message:send",
        Some(TOKEN),
        Some(Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["status"], "INVALID_ARGUMENT");
    let (status, value) = call(
        app.clone(),
        axum::http::Method::POST,
        "/a2a/v1/message:send",
        Some(TOKEN),
        Some(wire(
            "quote",
            CreateQuoteRequest {
                offer_id: "execute.compute".into(),
                requester_id: alice.identity.node_id().into(),
                spec,
                max_price_sats: Some(0),
            },
            None,
        )),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{value}");
    assert_eq!(
        call(
            app,
            axum::http::Method::GET,
            &format!("/a2a/v1/tasks/{}", task["id"].as_str().unwrap()),
            Some(TOKEN),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(submit(bob, &request).await["id"], task["id"]);
}

#[tokio::test]
async fn a2a_private_invite_and_storage_guards_reject_new_work_before_admission() {
    for (mode, storage_limit, expected) in [
        (
            crate::provider_policy::AccessMode::Private,
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            crate::provider_policy::AccessMode::Invite,
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            crate::provider_policy::AccessMode::Open,
            Some(1),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let (mut bob, alice, spec) = fixture().await;
        let request = submission(
            &alice,
            quote(bob.clone(), &alice, spec.clone()).await,
            spec.clone(),
            "not-admitted",
        );
        let policy = &mut Arc::get_mut(&mut bob).unwrap().config.provider_policy;
        policy.access_mode = mode;
        policy.max_database_bytes = storage_limit;
        let app = super::super::public_router(bob.clone());
        for payload in [
            wire(
                "quote",
                CreateQuoteRequest {
                    offer_id: "execute.compute".into(),
                    requester_id: alice.identity.node_id().into(),
                    spec: spec.clone(),
                    max_price_sats: Some(0),
                },
                None,
            ),
            wire("submit", &request, None),
        ] {
            let (status, value) = call(
                app.clone(),
                axum::http::Method::POST,
                "/a2a/v1/message:send",
                Some(TOKEN),
                Some(payload),
            )
            .await;
            assert_eq!(status, expected, "{mode:?}: {value}");
            assert_eq!(value["error"]["code"], expected.as_u16());
        }
        let count = bob
            .db
            .with_read_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM deals", [], |row| row.get::<_, i64>(0))
                    .map_err(|error| error.to_string())
            })
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
