//! Regression coverage for a provider-admitted A2A response lost in transit.
//! The fault is a truncated successful HTTP body, not an invented HTTP error.
use super::*;
use crate::settlement::SettlementDriver;
use crate::{
    a2a_config::{A2aClient, A2aProvider},
    config::PaymentBackend,
    db::DbPool,
    identity::NodeIdentity,
    pricing::PricingTable,
    state::TransportStatus,
};
use axum::body::Body;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::{net::TcpListener, task::JoinHandle};

const WASM_42: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";
const A2A_BEARER: &str = "a2a-recovery-test-separate-provider-credential";

struct Server(JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn serve(listener: TcpListener, app: Router) -> Server {
    Server(tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve Bob");
    }))
}

fn request(origin: &str, provider_id: &str, key: &str) -> RuntimeCreateDealRequest {
    RuntimeCreateDealRequest {
        provider: RuntimeProviderRef {
            provider_id: Some(provider_id.into()),
            provider_url: Some(origin.into()),
        },
        offer_id: "execute.compute".into(),
        spec: WorkloadSpec::Wasm {
            submission: Box::new(
                crate::cli::invoke::build_inline_wasm_submission(WASM_42, json!({"sample":1}))
                    .expect("Wasm fixture"),
            ),
        },
        max_price_sats: Some(0),
        idempotency_key: Some(key.into()),
        payment: None,
    }
}

async fn invoke(state: Arc<AppState>, payload: RuntimeCreateDealRequest) -> (StatusCode, Value) {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", state.runtime_auth_token)).unwrap(),
    );
    let response = runtime_create_deal(State(state), headers, Json(payload)).await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_UPSTREAM_JSON_BYTES)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn intent(state: &AppState, key: &str) -> Option<requester_deals::StoredRequesterDeal> {
    let key = key.to_owned();
    state
        .db
        .with_read_conn(move |conn| {
            requester_deals::find_requester_deal_by_idempotency_key(conn, &key)
        })
        .await
        .unwrap()
}

async fn observe_execution_claims(state: &AppState) {
    // process_deal_with_reserved_permit executes only after the atomic
    // try_mark_deal_running transition succeeds. Count that execution gate,
    // since deduplicated equal result evidence alone can hide repeated runs.
    state
        .db
        .with_write_conn(|conn| {
            conn.execute_batch(
                "CREATE TABLE a2a_test_execution_claims (count INTEGER NOT NULL);
            INSERT INTO a2a_test_execution_claims VALUES (0);
            CREATE TRIGGER a2a_test_execution_started AFTER UPDATE OF status ON deals
            WHEN OLD.status='accepted' AND NEW.status='running'
            BEGIN UPDATE a2a_test_execution_claims SET count=count+1; END;",
            )
        })
        .await
        .unwrap();
}

async fn execution_snapshot(state: &AppState) -> (i64, i64, String, i64) {
    state.db.with_read_conn(|conn| {
        let deals = conn.query_row("SELECT COUNT(*) FROM deals", [], |row| row.get(0)).map_err(|e| e.to_string())?;
        let results = conn.query_row("SELECT COUNT(*) FROM execution_evidence WHERE subject_kind='deal' AND evidence_kind='execution_result'", [], |row| row.get(0)).map_err(|e| e.to_string())?;
        let result_hash = conn.query_row("SELECT result_hash FROM deals", [], |row| row.get(0)).map_err(|e| e.to_string())?;
        let claims = conn.query_row("SELECT count FROM a2a_test_execution_claims", [], |row| row.get(0)).map_err(|e| e.to_string())?;
        Ok::<_, String>((deals, results, result_hash, claims))
    }).await.unwrap()
}

/// Reopen durable requester data using a fresh state and pool, retaining no
/// original AppState, database connection, or in-memory execution tracker.
fn restart_requester(config: crate::config::NodeConfig) -> Arc<AppState> {
    let mut fresh = super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    let state = Arc::get_mut(&mut fresh).unwrap();
    state.db = DbPool::open(&config.storage.db_path).expect("reopen Alice database");
    state.identity =
        Arc::new(NodeIdentity::load_or_create(&config).expect("reload Alice identity"));
    state.pricing = PricingTable::from_config(config.pricing);
    state.settlement_registry = settlement::SettlementRegistry::new(&config).unwrap();
    state.transport_status = Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
        &config,
    )));
    state.config = config;
    fresh
}

#[tokio::test]
async fn accepted_a2a_response_loss_recovers_same_requester_intent_after_restart() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut alice = super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    let mut bob = super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    assert_ne!(alice.identity.node_id(), bob.identity.node_id());
    Arc::get_mut(&mut bob).unwrap().config.public_base_url = Some(origin.clone());
    let offer = current_offer_records(&bob, false)
        .await
        .unwrap()
        .into_iter()
        .find(|offer| {
            offer.offer.payload.offer_id == "execute.compute"
                && offer.offer.payload.offer_kind == "compute.wasm.v1"
                && offer.offer.payload.settlement_method == "none"
        })
        .expect("free Wasm Offer");
    Arc::get_mut(&mut bob)
        .unwrap()
        .config
        .a2a
        .clients
        .push(A2aClient {
            requester_id: alice.identity.node_id().into(),
            token: A2A_BEARER.into(),
            offer_hashes: vec![offer.offer.hash],
        });
    Arc::get_mut(&mut alice)
        .unwrap()
        .config
        .a2a
        .providers
        .push(A2aProvider {
            provider_url: origin.clone(),
            token: A2A_BEARER.into(),
            allow_loopback: true,
        });
    observe_execution_claims(&bob).await;
    let config = alice.config.clone();
    let provider_id = bob.identity.node_id().to_owned();
    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let accepted_response = Arc::new(Mutex::new(None::<Value>));
    let lost = Arc::new(AtomicBool::new(false));
    let app = public_router(bob.clone()).layer(middleware::from_fn({
        let observed = observed.clone();
        let accepted_response = accepted_response.clone();
        let lost = lost.clone();
        move |request: Request, next: Next| {
            let observed = observed.clone();
            let accepted_response = accepted_response.clone();
            let lost = lost.clone();
            async move {
                assert!(
                    request.uri().path().starts_with("/a2a/v1/"),
                    "ordinary provider transport fallback"
                );
                assert_eq!(
                    request.headers()[header::AUTHORIZATION],
                    format!("Bearer {A2A_BEARER}")
                );
                let (request, operation) = if request.method() == axum::http::Method::POST {
                    let (parts, body) = request.into_parts();
                    let bytes = axum::body::to_bytes(body, MAX_BODY_BYTES).await.unwrap();
                    let body: Value = serde_json::from_slice(&bytes).unwrap();
                    let operation = body["message"]["parts"][0]["data"]["operation"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    (Request::from_parts(parts, Body::from(bytes)), operation)
                } else {
                    (request, "get_task".into())
                };
                observed.lock().unwrap().push(operation.clone());
                let response = next.run(request).await;
                if operation != "submit" || lost.swap(true, Ordering::SeqCst) {
                    return response;
                }
                assert!(
                    response.status().is_success(),
                    "must lose an admitted successful response"
                );
                let (mut parts, body) = response.into_parts();
                let bytes = axum::body::to_bytes(body, MAX_UPSTREAM_JSON_BYTES)
                    .await
                    .unwrap();
                *accepted_response.lock().unwrap() = Some(serde_json::from_slice(&bytes).unwrap());
                let partial = bytes.slice(..bytes.len() / 2);
                parts.headers.remove(header::TRANSFER_ENCODING);
                parts.headers.insert(
                    header::CONTENT_LENGTH,
                    HeaderValue::from_str(&bytes.len().to_string()).unwrap(),
                );
                // Emit a genuine partial successful response before an error
                // closes the connection. Alice must not decode it as admission.
                let body =
                    Body::from_stream(futures::stream::unfold(Some(partial), |chunk| async move {
                        match chunk {
                            Some(bytes) => Some((Ok::<_, std::io::Error>(bytes), None)),
                            None => {
                                tokio::time::sleep(Duration::from_millis(25)).await;
                                Some((
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::ConnectionReset,
                                        "injected accepted response loss",
                                    )),
                                    None,
                                ))
                            }
                        }
                    }));
                Response::from_parts(parts, body)
            }
        }
    }));
    let _server = serve(listener, app).await;
    let key = "accepted-a2a-response-loss";
    let (status, uncertain) = invoke(alice.clone(), request(&origin, &provider_id, key)).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{uncertain}");
    assert!(lost.load(Ordering::SeqCst));
    let pending = intent(&alice, key)
        .await
        .expect("durable pre-submit requester intent");
    assert_eq!(pending.status, "submission_pending");
    assert_eq!(pending.idempotency_key.as_deref(), Some(key));
    assert_eq!(pending.deal_id, pending.deal.hash);
    assert_eq!(uncertain["deal_id"], pending.deal.hash);
    assert_eq!(uncertain["task_id"], pending.deal.hash);
    assert_eq!(uncertain["idempotency_key"], key);
    assert_eq!(uncertain["status"], "submission_pending");
    let accepted = accepted_response
        .lock()
        .unwrap()
        .clone()
        .expect("Bob accepted before body loss");
    assert_eq!(accepted["task"]["id"], pending.deal.hash);
    let backend = find_existing_deal_by_artifact_hash(&bob, &pending.deal.hash)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let record = bob
                .db
                .with_read_conn({
                    let id = backend.deal_id.clone();
                    move |conn| deals::get_deal(conn, &id)
                })
                .await
                .unwrap()
                .unwrap();
            if record.status == deals::DEAL_STATUS_SUCCEEDED {
                assert_eq!(record.result, Some(json!(42)));
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Bob finishes its admitted computation");
    let before = execution_snapshot(&bob).await;
    assert_eq!((before.0, before.1), (1, 1));
    assert_eq!(before.3, 1, "Bob claimed execution exactly once");
    let requests_before = observed.lock().unwrap().clone();
    assert_eq!(
        requests_before
            .iter()
            .filter(|operation| *operation == "quote")
            .count(),
        1
    );
    assert_eq!(
        requests_before
            .iter()
            .filter(|operation| *operation == "submit")
            .count(),
        1
    );
    let alice_id = alice.identity.node_id().to_owned();
    drop(alice);
    let restarted = restart_requester(config);
    assert_eq!(restarted.identity.node_id(), alice_id);
    assert_eq!(
        intent(&restarted, key).await.unwrap().status,
        "submission_pending"
    );
    recover_runtime_state(restarted.clone())
        .await
        .expect("startup recovery");
    let (status, recovered) = invoke(restarted.clone(), request(&origin, &provider_id, key)).await;
    assert_eq!(status, StatusCode::OK, "{recovered}");
    assert_eq!(recovered["deal"]["deal_id"], pending.deal.hash);
    assert_eq!(recovered["quote"]["hash"], pending.quote.hash);
    assert_eq!(recovered["deal"]["status"], "succeeded");
    assert_eq!(recovered["deal"]["result"], 42);
    let final_intent = intent(&restarted, key).await.unwrap();
    let report = protocol::validate_quote_deal_receipt(
        &final_intent.quote,
        &final_intent.deal,
        final_intent
            .receipt
            .as_ref()
            .expect("signed terminal Receipt"),
        None,
    );
    assert!(report.valid, "{report:?}");
    assert_eq!(final_intent.result_hash.as_deref(), Some(before.2.as_str()));
    assert_eq!(
        execution_snapshot(&bob).await,
        before,
        "restart/retry re-executed admitted work"
    );
    let operations = observed.lock().unwrap();
    assert!(
        operations[requests_before.len()..]
            .iter()
            .all(|operation| operation == "get_task"),
        "recovery issued new work: {operations:?}"
    );
    assert!(
        operations.len() > requests_before.len(),
        "recovery must query Bob's Task"
    );
}

/// Reuse the existing scripted capture/reconciliation driver, adding only
/// deterministic authorization needed to exercise genuine Deal admission.
#[derive(Clone)]
struct ReservingStripeDriver {
    inner: super::tests::ScriptedStripeDriver,
    authorizations: Arc<std::sync::atomic::AtomicUsize>,
}

impl SettlementDriver for ReservingStripeDriver {
    fn descriptor(&self, state: &AppState) -> settlement::SettlementDriverDescriptor {
        self.inner.descriptor(state)
    }
    fn wallet_balance<'a>(
        &'a self,
        state: &'a AppState,
    ) -> futures::future::BoxFuture<
        'a,
        Result<settlement::WalletBalanceSnapshot, settlement::PaymentError>,
    > {
        self.inner.wallet_balance(state)
    }
    fn prepare<'a>(
        &'a self,
        _state: &'a AppState,
        request: settlement::PreparePaymentRequest,
    ) -> futures::future::BoxFuture<'a, Result<Option<PaymentReservation>, settlement::PaymentError>>
    {
        Box::pin(async move {
            let payment = request.payment.expect("caller-supplied payment");
            assert_eq!(payment.kind, "stripe_mpp");
            assert!(settlement::stripe::is_valid_spt_id(&payment.token));
            self.authorizations.fetch_add(1, Ordering::SeqCst);
            Ok(Some(PaymentReservation {
                request_id: request.request_id.expect("stable payment request identity"),
                method: "stripe_mpp".into(),
                service_id: request.service_id,
                amount_sats: request.price_sats,
                token_hash: "pi_a2a_token_validation_test".into(),
            }))
        })
    }
    fn commit<'a>(
        &'a self,
        state: &'a AppState,
        reservation: PaymentReservation,
    ) -> futures::future::BoxFuture<'a, Result<PaymentReceipt, settlement::PaymentError>> {
        self.inner.commit(state, reservation)
    }
    fn release<'a>(
        &'a self,
        state: &'a AppState,
        reservation: &'a PaymentReservation,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        self.inner.release(state, reservation)
    }
    fn reservation_state<'a>(
        &'a self,
        state: &'a AppState,
        reservation: &'a PaymentReservation,
    ) -> futures::future::BoxFuture<'a, Result<settlement::PaymentReservationState, String>> {
        self.inner.reservation_state(state, reservation)
    }
}

#[tokio::test]
async fn rejected_a2a_stripe_tokens_do_not_poison_requester_idempotency() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut alice = super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    Arc::get_mut(&mut alice)
        .unwrap()
        .config
        .requester_spend
        .spend_budget_msat = Some(100_000);
    let driver = super::tests::ScriptedStripeDriver::with_script([true], [], []);
    let authorizations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut bob = super::tests::scripted_stripe_state(&driver);
    let bob_mut = Arc::get_mut(&mut bob).unwrap();
    bob_mut.config.public_base_url = Some(origin.clone());
    bob_mut.settlement_registry = settlement::SettlementRegistry::with_single_driver(
        "stripe_mpp",
        Arc::new(ReservingStripeDriver {
            inner: driver.clone(),
            authorizations: authorizations.clone(),
        }),
    );
    let offer = current_offer_records(&bob, false)
        .await
        .unwrap()
        .into_iter()
        .find(|offer| {
            offer.offer.payload.offer_id == "execute.compute"
                && offer.offer.payload.offer_kind == "compute.wasm.v1"
                && offer.offer.payload.settlement_method == "stripe_mpp.v1"
        })
        .expect("paid Stripe Wasm Offer");
    Arc::get_mut(&mut bob)
        .unwrap()
        .config
        .a2a
        .clients
        .push(A2aClient {
            requester_id: alice.identity.node_id().into(),
            token: A2A_BEARER.into(),
            offer_hashes: vec![offer.offer.hash],
        });
    Arc::get_mut(&mut alice)
        .unwrap()
        .config
        .a2a
        .providers
        .push(A2aProvider {
            provider_url: origin.clone(),
            token: A2A_BEARER.into(),
            allow_loopback: true,
        });
    observe_execution_claims(&bob).await;
    let _server = serve(listener, public_router(bob.clone())).await;
    let key = "a2a-corrected-stripe-token";
    for (payment, expected, code) in [
        (None, StatusCode::PAYMENT_REQUIRED, "stripe_spt_required"),
        (
            Some(settlement::ProvidedPayment {
                kind: "stripe_mpp".into(),
                token: "invalid-token".into(),
            }),
            StatusCode::BAD_REQUEST,
            "invalid_stripe_spt",
        ),
        (
            Some(settlement::ProvidedPayment {
                kind: "lightning".into(),
                token: "spt_platform_supplied_test".into(),
            }),
            StatusCode::BAD_REQUEST,
            "invalid_stripe_spt",
        ),
    ] {
        let mut payload = request(&origin, bob.identity.node_id(), key);
        payload.max_price_sats = Some(30);
        payload.payment = payment;
        let (status, rejected) = invoke(alice.clone(), payload).await;
        assert_eq!(status, expected, "{rejected}");
        assert_eq!(rejected["code"], code, "{rejected}");
        assert!(
            intent(&alice, key).await.is_none(),
            "deterministic payment rejection persisted an unusable intent"
        );
        assert_eq!(authorizations.load(Ordering::SeqCst), 0);
        assert!(driver.calls().is_empty());
    }
    let mut payload = request(&origin, bob.identity.node_id(), key);
    payload.max_price_sats = Some(30);
    payload.payment = Some(settlement::ProvidedPayment {
        kind: "stripe_mpp".into(),
        token: "spt_platform_supplied_test".into(),
    });
    let (status, admitted) = invoke(alice.clone(), payload).await;
    assert!(
        status.is_success(),
        "corrected same-key submission was poisoned: {admitted}"
    );
    let deal_id = admitted["deal"]["deal_id"].as_str().unwrap().to_owned();
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut retry = request(&origin, bob.identity.node_id(), key);
            retry.max_price_sats = Some(30);
            let (status, response) = invoke(alice.clone(), retry).await;
            assert_eq!(status, StatusCode::OK, "{response}");
            assert_eq!(response["deal"]["deal_id"], deal_id);
            if response["deal"]["status"] == "succeeded" {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("corrected payment admits and completes one Deal");
    assert_eq!(completed["deal"]["result"], 42);
    assert_eq!(authorizations.load(Ordering::SeqCst), 1);
    assert_eq!(
        driver
            .calls()
            .iter()
            .filter(|(operation, _)| operation == "capture")
            .count(),
        1
    );
    let stored = intent(&alice, key).await.unwrap();
    assert_eq!(stored.deal_id, deal_id);
    let verified = protocol::validate_quote_deal_receipt(
        &stored.quote,
        &stored.deal,
        stored.receipt.as_ref().unwrap(),
        None,
    );
    assert!(verified.valid, "{verified:?}");
    let (deals, results, _, claims) = execution_snapshot(&bob).await;
    assert_eq!((deals, results, claims), (1, 1, 1));
    // Model the durable paid crash boundary: external authorization happened,
    // while Alice has not yet persisted confirmation of admission. Recovery
    // must commit this exact counted hold using verified GetTask evidence.
    let calls_before = driver.calls();
    let execution_before = execution_snapshot(&bob).await;
    let hash = stored.deal.hash.clone();
    alice.db.with_write_conn(move |conn| {
        conn.execute("UPDATE requester_spend_ledger SET state='external_pending', deal_id=NULL WHERE deal_hash=?1 AND state='committed'", [&hash])
    }).await.map(|updated| assert_eq!(updated, 1)).unwrap();
    let totals = alice
        .db
        .with_read_conn(crate::requester_budget::spend_totals)
        .await
        .unwrap();
    assert_eq!(totals.reserved_msat, 30_000);
    assert_eq!(totals.committed_msat, 0);
    let mut retry = request(&origin, bob.identity.node_id(), key);
    retry.max_price_sats = Some(30);
    let (status, recovered) = invoke(alice.clone(), retry).await;
    assert_eq!(status, StatusCode::OK, "{recovered}");
    assert_eq!(recovered["deal"]["deal_id"], stored.deal_id);
    assert_eq!(
        recovered["deal"]["receipt"]["hash"],
        stored.receipt.unwrap().hash
    );
    let totals = alice
        .db
        .with_read_conn(crate::requester_budget::spend_totals)
        .await
        .unwrap();
    assert_eq!(totals.reserved_msat, 0);
    assert_eq!(totals.committed_msat, 30_000);
    assert_eq!(authorizations.load(Ordering::SeqCst), 1);
    assert_eq!(driver.calls(), calls_before);
    assert_eq!(execution_snapshot(&bob).await, execution_before);
}
