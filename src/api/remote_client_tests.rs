//! Native requester transport regressions for Lightning recovery and reads.
use super::*;
use crate::a2a_config::A2aClient;
use axum::body::Body;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::{net::TcpListener, task::JoinHandle};

const WASM_42: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";
const TOKEN: &str = "a2a-lightning-replay-separate-provider-bearer";

struct Server(JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn load(bob: &AppState, id: &str) -> deals::StoredDeal {
    let id = id.to_owned();
    bob.db
        .with_read_conn(move |conn| deals::get_deal(conn, &id))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn lightning_acceptance_loss_and_terminal_reads_recover_without_terminal_messages() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut alice = super::super::tests::test_app_state_with_free_pricing(PaymentBackend::None);
    let mut bob = super::super::tests::test_app_state_with_free_pricing(PaymentBackend::Lightning);
    let bob_mut = Arc::get_mut(&mut bob).unwrap();
    bob_mut.config.lightning.base_invoice_expiry_secs = 30;
    bob_mut.config.lightning.success_hold_expiry_secs = 60;
    assert_ne!(alice.identity.node_id(), bob.identity.node_id());
    let now = settlement::current_unix_timestamp();
    let preimage = "77".repeat(32);
    let payment_hash = crypto::sha256_hex(hex::decode(&preimage).unwrap());
    let quote = super::super::tests::signed_lightning_quote_for_state(
        &bob,
        alice.identity.node_id().into(),
        now - 5,
        now + 180,
        30_000,
        30,
        60,
    );
    let deal = build_runtime_requester_deal_artifact(&alice, &quote, &payment_hash, now - 5, true)
        .unwrap();
    let mut bundle = super::super::tests::test_lightning_bundle(
        &bob,
        &quote,
        &deal,
        alice.identity.node_id(),
        now - 5,
    );
    let validation = settlement::validate_lightning_invoice_bundle(
        &bundle.bundle,
        &quote,
        &deal,
        Some(alice.identity.node_id()),
    );
    assert!(validation.valid, "invalid test fixture: {validation:?}");
    let spec = WorkloadSpec::Wasm {
        submission: Box::new(
            crate::cli::invoke::build_inline_wasm_submission(WASM_42, Value::Null).unwrap(),
        ),
    };
    assert_eq!(spec.request_hash().unwrap(), quote.payload.workload_hash);
    let local_id = protocol::new_artifact_id();
    bob.db
        .with_write_conn({
            let id = local_id.clone();
            let quote = quote.clone();
            let deal = deal.clone();
            let bundle = bundle.clone();
            let spec = spec.clone();
            move |conn| -> Result<(), String> {
                deals::insert_or_get_deal(
                    conn,
                    NewDeal {
                        deal_id: id.clone(),
                        idempotency_key: Some("a2a-native-lightning-terminal".into()),
                        quote,
                        spec,
                        artifact: deal.clone(),
                        workload_evidence_hash: None,
                        deal_artifact_hash: deal.hash.clone(),
                        payment_method: Some("lightning".into()),
                        payment_token_hash: Some(deal.payload.success_payment_hash.clone()),
                        payment_amount_sats: Some(10),
                        initial_status: deals::DEAL_STATUS_RUNNING.into(),
                        created_at: now - 5,
                    },
                )?;
                assert!(deals::stage_deal_result_ready(
                    conn,
                    &id,
                    &json!(42),
                    None,
                    None,
                    now - 4
                )?);
                db::insert_lightning_invoice_bundle(
                    conn,
                    &bundle.session_id,
                    &bundle.bundle,
                    InvoiceBundleLegState::Settled,
                    InvoiceBundleLegState::Accepted,
                    now - 5,
                )?;
                Ok(())
            }
        })
        .await
        .unwrap();
    let bob_mut = Arc::get_mut(&mut bob).unwrap();
    bob_mut.config.public_base_url = Some(origin.clone());
    bob_mut.config.a2a.clients.push(A2aClient {
        requester_id: alice.identity.node_id().into(),
        token: TOKEN.into(),
        offer_hashes: vec![quote.payload.offer_hash.clone()],
    });
    bob_mut.public_request_quota = Arc::new(crate::public_quota::IdentityQuota::new(
        16,
        Duration::from_secs(60),
    ));
    for _ in 0..16 {
        assert!(matches!(
            bob.public_request_quota.check_and_increment("public"),
            QuotaDecision::Allowed { .. }
        ));
    }
    assert!(matches!(
        bob.public_request_quota.check_and_increment("public"),
        QuotaDecision::Rejected { .. }
    ));
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
    recover_runtime_state(bob.clone()).await.unwrap();
    let ready = load(&bob, &local_id).await;
    assert_eq!(ready.status, deals::DEAL_STATUS_RESULT_READY);
    assert!(ready.receipt.is_none());
    let hash = ready.result_hash.clone().unwrap();
    let task_id = deal.hash.clone();

    // An execution result with unfinished economic settlement remains an
    // unsigned, provisional WORKING projection, never terminal success.
    let stored = requester_deals::StoredRequesterDeal {
        deal_id: task_id.clone(),
        idempotency_key: None,
        provider_id: bob.identity.node_id().into(),
        provider_url: origin.clone(),
        provider_sync_url: Some(origin.clone()),
        spec,
        quote: quote.clone(),
        deal: deal.clone(),
        status: deals::DEAL_STATUS_RUNNING.into(),
        result: None,
        result_hash: None,
        error: None,
        receipt: None,
        success_preimage: preimage.clone(),
        created_at: now - 5,
        updated_at: now - 5,
    };
    let mut pending = ready.public_record();
    pending.status = deals::DEAL_STATUS_SETTLEMENT_PENDING.into();
    assert!(matches!(
        verify_provider_deal_projection(&stored, &pending).unwrap(),
        ProviderDealProjection::Provisional
    ));
    let projected = a2a::task(&pending).unwrap();
    assert_eq!(projected["status"]["state"], "TASK_STATE_WORKING");
    assert_eq!(
        task_payload(projected, Some(&task_id)).unwrap()["status"],
        "settlement_pending"
    );

    // A separately valid signature cannot make a bundle for another Deal
    // acceptable. The fault changes only the served metadata, not Bob's DB.
    let mut wrong_payload = bundle.bundle.payload.clone();
    wrong_payload.deal_hash = "dd".repeat(32);
    bundle.bundle = protocol::sign_artifact(
        bob.identity.node_id(),
        |bytes| bob.identity.sign_message_hex(bytes),
        &bundle.bundle.artifact_type,
        bundle.bundle.created_at,
        wrong_payload,
    )
    .unwrap();
    assert!(protocol::verify_artifact(&bundle.bundle));
    let wrong_bundle_json = serde_json::to_string(&bundle).unwrap();
    let observed = Arc::new(Mutex::new(Vec::<(axum::http::Method, String)>::new()));
    let lost = Arc::new(AtomicBool::new(false));
    let tamper = Arc::new(AtomicBool::new(false));
    let app = public_router(bob.clone()).layer(middleware::from_fn({
        let observed = observed.clone();
        let lost = lost.clone();
        let tamper = tamper.clone();
        move |request: Request, next: Next| {
            let observed = observed.clone();
            let lost = lost.clone();
            let tamper = tamper.clone();
            let wrong_bundle_json = wrong_bundle_json.clone();
            async move {
                assert_eq!(
                    request.headers()[header::AUTHORIZATION],
                    format!("Bearer {TOKEN}")
                );
                assert_eq!(request.headers()["a2a-version"], "1.0");
                assert_eq!(request.headers()["a2a-extensions"], a2a::EXTENSION);
                let method = request.method().clone();
                let path = request.uri().path().to_owned();
                assert!(
                    path.starts_with("/a2a/v1/"),
                    "ordinary provider transport fallback"
                );
                observed.lock().unwrap().push((method.clone(), path));
                let response = next.run(request).await;
                if !response.status().is_success() {
                    let status = response.status();
                    let body = axum::body::to_bytes(response.into_body(), MAX_UPSTREAM_JSON_BYTES)
                        .await
                        .unwrap();
                    let error: Value = serde_json::from_slice(&body).unwrap();
                    panic!(
                        "native {method} recovery operation failed with {status}: {}",
                        error["error"]
                    );
                }
                if method == axum::http::Method::POST && !lost.swap(true, Ordering::SeqCst) {
                    let (mut parts, body) = response.into_parts();
                    let bytes = axum::body::to_bytes(body, MAX_UPSTREAM_JSON_BYTES)
                        .await
                        .unwrap();
                    let completed: Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(completed["task"]["status"]["state"], "TASK_STATE_COMPLETED");
                    parts.headers.remove(header::TRANSFER_ENCODING);
                    parts.headers.insert(
                        header::CONTENT_LENGTH,
                        HeaderValue::from_str(&bytes.len().to_string()).unwrap(),
                    );
                    let partial = bytes.slice(..bytes.len() / 2);
                    let stream =
                        futures::stream::once(async move { Ok::<_, std::io::Error>(partial) })
                            .chain(futures::stream::once(async {
                                tokio::time::sleep(Duration::from_millis(25)).await;
                                Err(std::io::Error::new(
                                    std::io::ErrorKind::ConnectionReset,
                                    "injected settled acceptance response loss",
                                ))
                            }));
                    return Response::from_parts(parts, Body::from_stream(stream));
                }
                if tamper.load(Ordering::SeqCst) {
                    let (mut parts, body) = response.into_parts();
                    let bytes = axum::body::to_bytes(body, MAX_UPSTREAM_JSON_BYTES)
                        .await
                        .unwrap();
                    let mut task: Value = serde_json::from_slice(&bytes).unwrap();
                    task["metadata"]["froglet"]["invoiceBundle"] = json!(wrong_bundle_json);
                    parts.headers.remove(header::CONTENT_LENGTH);
                    return Response::from_parts(
                        parts,
                        Body::from(serde_json::to_vec(&task).unwrap()),
                    );
                }
                response
            }
        }
    }));
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let addresses = [IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)];
    let accept_url = format!("{origin}/v1/provider/deals/{task_id}/accept");
    let acceptance =
        json!({"success_preimage":preimage,"expected_result_hash":hash.to_uppercase()});
    let recovered: deals::DealRecord = a2a_request(
        &alice,
        &reqwest::Method::POST,
        &accept_url,
        Some(&acceptance),
        &addresses,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(lost.load(Ordering::SeqCst));
    assert_eq!(recovered.deal_id, task_id);
    assert_eq!(recovered.status, deals::DEAL_STATUS_SUCCEEDED);
    assert_eq!(recovered.result, Some(json!(42)));
    assert_eq!(recovered.result_hash.as_deref(), Some(hash.as_str()));
    let receipt = recovered.receipt.as_ref().unwrap();
    assert!(
        protocol::validate_quote_deal_receipt(&recovered.quote, &recovered.deal, receipt, None)
            .valid
    );
    assert_eq!(
        observed
            .lock()
            .unwrap()
            .iter()
            .map(|(method, _)| method.clone())
            .collect::<Vec<_>>(),
        vec![
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::GET
        ]
    );

    // All terminal retries and invoice reads now use GetTask only. This is
    // necessary because A2A forbids sending a message to a terminal Task.
    let baseline = observed.lock().unwrap().len();
    let replay: deals::DealRecord = a2a_request(
        &alice,
        &reqwest::Method::POST,
        &accept_url,
        Some(&acceptance),
        &addresses,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(replay.receipt.unwrap().hash, receipt.hash);
    let wrong_hash =
        json!({"success_preimage":"77".repeat(32),"expected_result_hash":"ee".repeat(32)});
    let rejection = a2a_request::<deals::DealRecord, _>(
        &alice,
        &reqwest::Method::POST,
        &accept_url,
        Some(&wrong_hash),
        &addresses,
        None,
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(rejection.0, StatusCode::BAD_GATEWAY);
    assert!(
        rejection.1["error"]
            .as_str()
            .unwrap()
            .contains("result hash conflicts")
    );
    let invoice_url = format!("{origin}/v1/provider/deals/{task_id}/invoice-bundle");
    let valid_bundle: LightningInvoiceBundleSession = a2a_request::<_, ()>(
        &alice,
        &reqwest::Method::GET,
        &invoice_url,
        None,
        &addresses,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(valid_bundle.bundle.payload.deal_hash, task_id);
    assert_eq!(valid_bundle.bundle.payload.quote_hash, quote.hash);
    assert_eq!(valid_bundle.base_state, InvoiceBundleLegState::Settled);
    assert_eq!(valid_bundle.success_state, InvoiceBundleLegState::Settled);
    assert!(
        settlement::validate_lightning_invoice_bundle(&valid_bundle.bundle, &quote, &deal, None)
            .valid
    );
    tamper.store(true, Ordering::SeqCst);
    let invalid = a2a_request::<LightningInvoiceBundleSession, ()>(
        &alice,
        &reqwest::Method::GET,
        &invoice_url,
        None,
        &addresses,
        None,
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(invalid.0, StatusCode::BAD_GATEWAY);
    assert!(
        invalid.1["error"]
            .as_str()
            .unwrap()
            .contains("conflicts with the signed Deal")
    );
    let operations = observed.lock().unwrap().clone();
    assert!(
        operations[baseline..]
            .iter()
            .all(|(method, path)| *method == axum::http::Method::GET
                && path == &format!("/a2a/v1/tasks/{task_id}")),
        "terminal recovery used a mutation: {operations:?}"
    );
    assert_eq!(
        load(&bob, &local_id).await.receipt.unwrap().hash,
        receipt.hash
    );
}
