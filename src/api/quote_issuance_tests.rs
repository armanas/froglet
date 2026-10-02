//! Same-second quote issuance must not hand distinct operations a shared,
//! single-use admission token. Fixed timestamps make the persistence race exact.
use super::*;
use crate::a2a_config::{A2aClient, A2aProvider};
use axum::body::Body;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{net::TcpListener, task::JoinHandle};

const WASM_42: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";
const TOKEN: &str = "quote-collision-regression-separate-bearer";

struct Server(JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn fixed_quote(state: &AppState, requester_id: &str) -> SignedArtifact<QuotePayload> {
    let offer = current_offer_records(state, false)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.offer.payload.offer_id == "execute.compute")
        .unwrap()
        .offer;
    let now = settlement::current_unix_timestamp();
    sign_node_artifact(
        state,
        ARTIFACT_KIND_QUOTE,
        now,
        QuotePayload {
            provider_id: state.identity.node_id().into(),
            requester_id: requester_id.into(),
            descriptor_hash: offer.payload.descriptor_hash,
            offer_hash: offer.hash,
            expires_at: now + 60,
            workload_kind: "compute.wasm.v1".into(),
            workload_hash: "aa".repeat(32),
            confidential_session_hash: None,
            capabilities_granted: Vec::new(),
            extension_refs: Vec::new(),
            quote_use: None,
            settlement_terms: QuoteSettlementTerms {
                method: "none".into(),
                base_fee_msat: 0,
                success_fee_msat: 0,
                destination_identity: String::new(),
                max_base_invoice_expiry_secs: 0,
                max_success_hold_expiry_secs: 0,
                min_final_cltv_expiry: 0,
            },
            execution_limits: ExecutionLimits {
                max_input_bytes: 1024,
                max_runtime_ms: 2000,
                max_memory_bytes: 8_388_608,
                max_output_bytes: 1024,
                fuel_limit: 50_000_000,
            },
        },
    )
    .unwrap()
}

#[tokio::test]
async fn same_second_quote_issuance_rejects_one_of_two_identical_single_use_quotes() {
    let mut state = super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    Arc::get_mut(&mut state)
        .unwrap()
        .config
        .provider_policy
        .max_total_quotes = Some(2);
    let quote = fixed_quote(&state, &"11".repeat(32)).await;
    protocol::validate_quote_artifact(&quote).unwrap();
    let (left, right) = tokio::join!(
        async {
            reserve_provider_quote(&state).await.unwrap();
            persist_created_quote(state.clone(), quote.clone()).await
        },
        async {
            reserve_provider_quote(&state).await.unwrap();
            persist_created_quote(state.clone(), quote.clone()).await
        }
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let failure = left.err().or_else(|| right.err()).unwrap();
    assert_eq!(failure.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure.1["code"], "quote_issuance_collision");
    assert_eq!(failure.1["retry_after_secs"], 1);
    assert_eq!(
        state
            .db
            .with_read_conn(crate::provider_policy::usage)
            .await
            .unwrap()
            .issued_quotes,
        2
    );
    let allowance = reserve_provider_quote(&state).await.unwrap_err();
    assert_eq!(allowance.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(allowance.1["code"], "provider_allowance_unavailable");
    assert_eq!(
        state
            .db
            .with_read_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM quotes", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|error| error.to_string())
            })
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn distinct_requester_or_later_signed_epoch_can_receive_a_new_quote() {
    let state = super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    let first = fixed_quote(&state, &"11".repeat(32)).await;
    let mut next_payload = first.payload.clone();
    next_payload.expires_at += 1;
    let next_second = sign_node_artifact(
        &state,
        ARTIFACT_KIND_QUOTE,
        first.created_at + 1,
        next_payload,
    )
    .unwrap();
    let mut different_requester_payload = first.payload.clone();
    different_requester_payload.requester_id = "22".repeat(32);
    let different_requester = sign_node_artifact(
        &state,
        ARTIFACT_KIND_QUOTE,
        first.created_at,
        different_requester_payload,
    )
    .unwrap();
    for quote in [first, next_second, different_requester] {
        protocol::validate_quote_artifact(&quote).unwrap();
        persist_created_quote(state.clone(), quote).await.unwrap();
    }
    assert_eq!(intent_count(&state, "quotes").await, 3);
}

async fn collision_provider(
    collisions: usize,
    recognized: bool,
) -> (
    Arc<AppState>,
    Arc<AppState>,
    String,
    Arc<AtomicUsize>,
    Server,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut alice = super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    let mut bob = super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    let offer = current_offer_records(&bob, false)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.offer.payload.offer_id == "execute.compute")
        .unwrap();
    let bob_mut = Arc::get_mut(&mut bob).unwrap();
    bob_mut.config.public_base_url = Some(origin.clone());
    bob_mut.config.a2a.clients.push(A2aClient {
        requester_id: alice.identity.node_id().into(),
        token: TOKEN.into(),
        offer_hashes: vec![offer.offer.hash],
    });
    Arc::get_mut(&mut alice)
        .unwrap()
        .config
        .a2a
        .providers
        .push(A2aProvider {
            provider_url: origin.clone(),
            token: TOKEN.into(),
            allow_loopback: true,
        });
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = public_router(bob.clone()).layer(middleware::from_fn({
        let attempts = attempts.clone();
        move |request: Request, next: Next| {
            let attempts = attempts.clone();
            async move {
                if request.method() != axum::http::Method::POST {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, MAX_BODY_BYTES).await.unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                let is_quote = value["message"]["parts"][0]["data"]["operation"] == "quote";
                if is_quote && attempts.fetch_add(1, Ordering::SeqCst) < collisions {
                    return a2a::from_failure(if recognized {
                        quote_issuance_collision()
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, json!({"error":"finite quote allowance exhausted", "code":"provider_allowance_unavailable"}))
                    });
                }
                next.run(Request::from_parts(parts, Body::from(bytes))).await
            }
        }
    }));
    let server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    (alice, bob, origin, attempts, server)
}

async fn invoke(
    alice: Arc<AppState>,
    bob: &AppState,
    origin: &str,
    key: &str,
) -> (StatusCode, HeaderMap, Value) {
    let payload = RuntimeCreateDealRequest {
        provider: RuntimeProviderRef {
            provider_id: Some(bob.identity.node_id().into()),
            provider_url: Some(origin.into()),
        },
        offer_id: "execute.compute".into(),
        spec: WorkloadSpec::Wasm {
            submission: Box::new(
                crate::cli::invoke::build_inline_wasm_submission(WASM_42, json!({"sample":1}))
                    .unwrap(),
            ),
        },
        max_price_sats: Some(0),
        idempotency_key: Some(key.into()),
        payment: None,
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", alice.runtime_auth_token)).unwrap(),
    );
    let response = runtime_create_deal(State(alice), headers, Json(payload)).await;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_UPSTREAM_JSON_BYTES)
        .await
        .unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}

async fn intent_count(state: &AppState, table: &'static str) -> i64 {
    state
        .db
        .with_read_conn(move |conn| {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|error| error.to_string())
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn pre_admission_quote_collision_retries_then_exact_deal_replay_does_not_requote() {
    let (alice, bob, origin, attempts, _server) = collision_provider(1, true).await;
    let (status, _, result) = invoke(alice.clone(), &bob, &origin, "collision-success").await;
    assert!(status.is_success(), "{result}");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(intent_count(&alice, "requester_deals").await, 1);
    assert_eq!(intent_count(&bob, "deals").await, 1);
    let (replay_status, _, replay) =
        invoke(alice.clone(), &bob, &origin, "collision-success").await;
    assert!(replay_status.is_success(), "{replay}");
    assert_eq!(replay["deal"]["deal_id"], result["deal"]["deal_id"]);
    assert_eq!(replay["quote"]["hash"], result["quote"]["hash"]);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(intent_count(&alice, "requester_deals").await, 1);
    assert_eq!(intent_count(&bob, "deals").await, 1);
}

#[tokio::test]
async fn repeated_pre_admission_collision_is_bounded_and_creates_no_pending_intent() {
    let (alice, bob, origin, attempts, _server) = collision_provider(usize::MAX, true).await;
    let (status, headers, result) =
        invoke(alice.clone(), &bob, &origin, "collision-exhausted").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{result}");
    assert_eq!(result["code"], "quote_issuance_collision");
    assert_eq!(headers[header::RETRY_AFTER], "1");
    assert_eq!(attempts.load(Ordering::SeqCst), 4);
    assert_eq!(intent_count(&alice, "requester_deals").await, 0);
    assert_eq!(intent_count(&bob, "deals").await, 0);
}

#[tokio::test]
async fn allowance_refusal_is_not_retried_as_a_quote_collision() {
    let (alice, bob, origin, attempts, _server) = collision_provider(usize::MAX, false).await;
    let (status, _, result) = invoke(alice.clone(), &bob, &origin, "allowance-exhausted").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{result}");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(intent_count(&alice, "requester_deals").await, 0);
    assert_eq!(intent_count(&bob, "deals").await, 0);
}
