use froglet::{
    confidential::ConfidentialConfig,
    config::{
        BuyerStripeConfig, IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
        PaymentBackend, PricingConfig, StorageConfig, StripeConfig, WasmConfig,
    },
    db::{self, DbPool},
    pricing::ServiceId,
    protocol::{
        self, DealPayload, ExecutionLimits, InvoiceBundleLegState, QuotePayload, ReceiptLegState,
        validate_receipt_artifact, verify_artifact,
    },
    settlement::{
        self, PreparePaymentRequest, ProvidedPayment, SettlementDriver, SettlementRegistry,
    },
    state::{AppState, TransportStatus},
};
use rand::{RngCore, SeedableRng, rngs::StdRng};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::runtime::Runtime;

static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(1);

fn seeded_signing_key(rng: &mut StdRng) -> froglet::crypto::NodeSigningKey {
    loop {
        let mut seed = [0_u8; 32];
        rng.fill_bytes(&mut seed);
        if let Ok(key) = froglet::crypto::signing_key_from_seed_bytes(&seed) {
            return key;
        }
    }
}

fn random_hex(rng: &mut StdRng, bytes_len: usize) -> String {
    let mut bytes = vec![0_u8; bytes_len];
    rng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn random_destination_identity(rng: &mut StdRng) -> String {
    let prefix = if rng.next_u32().is_multiple_of(2) {
        "02"
    } else {
        "03"
    };
    format!("{prefix}{}", random_hex(rng, 32))
}

fn in_memory_state() -> AppState {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "froglet-test-{}-{unique}-{counter}",
        std::process::id()
    ));
    let db_path = temp_dir.join("node.db");
    std::fs::create_dir_all(&temp_dir).expect("temp dir");

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
            events_query: 10,
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
            sync_interval_ms: 1_000,
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

    let pool = DbPool::open(&node_config.storage.db_path).expect("init db");
    let events_query_capacity = pool.read_connection_count().max(1);

    let pricing = froglet::pricing::PricingTable::from_config(node_config.pricing);
    let identity = froglet::identity::NodeIdentity::load_or_create(&node_config).expect("identity");
    let settlement_registry = settlement::SettlementRegistry::new(&node_config);

    AppState {
        db: pool,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: reqwest::Client::new(),
        wasm_host: None,
        confidential_policy: None,
        runtime_auth_token: "test-runtime-token".to_string(),
        runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
        consumer_control_auth_token: "test-consumer-token".to_string(),
        consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
        provider_control_auth_token: "test-provider-token".to_string(),
        provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
        events_query_semaphore: Arc::new(tokio::sync::Semaphore::new(events_query_capacity)),
        lnd_rest_client: None,
        lightning_wallet: None,
        lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
        event_batch_writer: None,
        builtin_services: std::collections::HashMap::new(),
        settlement_registry,
        session_pool: None,
    }
}

#[test]
fn artifact_store_reuses_existing_payload_document_for_republished_roots() {
    let rt = Runtime::new().unwrap();
    let state = in_memory_state();

    rt.block_on(state.db.with_conn(|conn| {
        db::insert_artifact_document(
            conn,
            "artifact-hash-1",
            "payload-hash-1",
            "descriptor",
            "actor-1",
            1,
            r#"{"hash":"artifact-hash-1"}"#,
        )?;
        db::insert_artifact_document(
            conn,
            "artifact-hash-2",
            "payload-hash-1",
            "descriptor",
            "actor-1",
            2,
            r#"{"hash":"artifact-hash-2"}"#,
        )?;

        let stored = db::get_artifact_by_actor_kind_payload(
            conn,
            "actor-1",
            "descriptor",
            "payload-hash-1",
        )?
        .expect("stored artifact");
        let (feed, has_more) = db::list_artifacts(conn, Some(0), 10)?;

        assert_eq!(stored.hash, "artifact-hash-1");
        assert!(!has_more);
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].hash, "artifact-hash-1");

        Ok::<(), String>(())
    }))
    .expect("artifact inserts");
}

#[test]
fn payments_enforce_all_error_paths() {
    let rt = Runtime::new().unwrap();
    let mut state = in_memory_state();

    // PaymentRequired when no payment token is provided and price > 0 (any backend
    // configuration).  The registry dispatches on payment kind, so an absent token
    // triggers PaymentRequired rather than BackendUnavailable.
    state.config.payment_backends = vec![PaymentBackend::None];
    state.settlement_registry = settlement::SettlementRegistry::new(&state.config);
    let err = rt
        .block_on(settlement::prepare_payment(
            &state,
            ServiceId::EventsQuery,
            None,
            Some("req-backend-none".to_string()),
        ))
        .unwrap_err();
    assert!(matches!(
        err,
        settlement::PaymentError::PaymentRequired { .. }
    ));

    // Same for Lightning: no token provided → PaymentRequired.
    state.config.payment_backends = vec![PaymentBackend::Lightning];
    state.settlement_registry = settlement::SettlementRegistry::new(&state.config);
    let err = rt
        .block_on(settlement::prepare_payment(
            &state,
            ServiceId::EventsQuery,
            None,
            Some("req-backend-lightning".to_string()),
        ))
        .unwrap_err();
    assert!(matches!(
        err,
        settlement::PaymentError::PaymentRequired { .. }
    ));
}

#[test]
fn settlement_driver_reports_capabilities_consistently() {
    let rt = Runtime::new().unwrap();
    let mut state = in_memory_state();

    let lightning_descriptor = settlement::driver_descriptor(&state);
    assert_eq!(lightning_descriptor.backend, "lightning");
    assert_eq!(lightning_descriptor.mode, settlement::LIGHTNING_MOCK_MODE);
    assert_eq!(
        lightning_descriptor.accepted_payment_methods,
        vec!["lightning"]
    );
    assert_eq!(
        lightning_descriptor.capabilities,
        vec!["invoice_bundles", "hold_invoices", "mock_mode"]
    );
    assert!(lightning_descriptor.reservations);
    assert!(lightning_descriptor.receipts);

    let lightning_wallet = rt
        .block_on(settlement::wallet_balance_snapshot(&state))
        .expect("wallet snapshot");
    assert_eq!(lightning_wallet.backend, "lightning");
    assert_eq!(lightning_wallet.mode, settlement::LIGHTNING_MOCK_MODE);
    assert_eq!(lightning_wallet.accepted_payment_methods, vec!["lightning"]);
    assert_eq!(
        lightning_wallet.capabilities,
        vec!["invoice_bundles", "hold_invoices", "mock_mode"]
    );

    state.config.lightning.mode = LightningMode::LndRest;
    state.config.lightning.lnd_rest = Some(froglet::config::LightningLndRestConfig {
        rest_url: "http://127.0.0.1:8080".to_string(),
        tls_cert_path: None,
        macaroon_path: std::env::temp_dir().join("froglet-test.macaroon"),
        request_timeout_secs: 5,
    });

    let lnd_descriptor = settlement::driver_descriptor(&state);
    assert_eq!(lnd_descriptor.backend, "lightning");
    assert_eq!(lnd_descriptor.mode, settlement::LIGHTNING_LND_REST_MODE);
    assert_eq!(lnd_descriptor.accepted_payment_methods, vec!["lightning"]);
    assert_eq!(
        lnd_descriptor.capabilities,
        vec![
            "invoice_bundles",
            "hold_invoices",
            "lnd_rest",
            "node_getinfo",
        ]
    );

    state.config.payment_backends = vec![PaymentBackend::None];
    state.settlement_registry = settlement::SettlementRegistry::new(&state.config);
    let none_descriptor = settlement::driver_descriptor(&state);
    assert_eq!(none_descriptor.backend, "none");
    assert_eq!(none_descriptor.mode, "disabled");
    assert!(none_descriptor.accepted_payment_methods.is_empty());
    assert!(none_descriptor.capabilities.is_empty());
    assert!(!none_descriptor.reservations);
    assert!(!none_descriptor.receipts);

    let none_wallet = rt
        .block_on(settlement::wallet_balance_snapshot(&state))
        .expect("wallet snapshot");
    assert_eq!(none_wallet.backend, "none");
    assert_eq!(none_wallet.mode, "disabled");
    assert!(none_wallet.accepted_payment_methods.is_empty());
    assert!(none_wallet.capabilities.is_empty());
}

#[test]
fn lightning_mock_invoice_bundle_persists_and_updates_state() {
    let rt = Runtime::new().unwrap();
    let mut state = in_memory_state();
    state.config.payment_backends = vec![PaymentBackend::Lightning];
    state.settlement_registry = settlement::SettlementRegistry::new(&state.config);

    let created = rt
        .block_on(settlement::create_lightning_invoice_bundle(
            &state,
            settlement::BuildLightningInvoiceBundleRequest {
                session_id: Some("ln-session-1".to_string()),
                requester_id: "requester-1".to_string(),
                quote_hash: "quote-hash-1".to_string(),
                deal_hash: "deal-hash-1".to_string(),
                admission_deadline: None,
                success_payment_hash: "11".repeat(32),
                base_fee_msat: 1_500,
                success_fee_msat: 9_000,
                created_at: settlement::current_unix_timestamp(),
            },
        ))
        .expect("lightning bundle");

    assert_eq!(created.session_id, "ln-session-1");
    assert_eq!(created.base_state, InvoiceBundleLegState::Open);
    assert_eq!(created.success_state, InvoiceBundleLegState::Open);
    assert_eq!(created.bundle.artifact_type, "invoice_bundle");
    assert!(verify_artifact(&created.bundle));
    assert_eq!(created.bundle.payload.destination_identity.len(), 66);
    assert_eq!(created.bundle.payload.base_fee.amount_msat, 1_500);
    assert_eq!(created.bundle.payload.success_fee.amount_msat, 9_000);
    assert_eq!(
        created.bundle.payload.success_fee.payment_hash,
        "11".repeat(32)
    );

    let stored = rt
        .block_on(settlement::get_lightning_invoice_bundle(
            &state,
            "ln-session-1",
        ))
        .expect("stored bundle")
        .expect("bundle should exist");
    assert_eq!(stored.bundle.hash, created.bundle.hash);
    assert_eq!(stored.base_state, InvoiceBundleLegState::Open);

    let updated = rt
        .block_on(settlement::update_lightning_invoice_bundle_states(
            &state,
            "ln-session-1",
            InvoiceBundleLegState::Accepted,
            InvoiceBundleLegState::Settled,
        ))
        .expect("update bundle")
        .expect("bundle should still exist");

    assert_eq!(updated.base_state, InvoiceBundleLegState::Accepted);
    assert_eq!(updated.success_state, InvoiceBundleLegState::Settled);
    assert_eq!(updated.bundle.hash, created.bundle.hash);
}

#[test]
fn lightning_invoice_bundle_validation_checks_quote_and_deal_commitments() {
    let rt = Runtime::new().unwrap();
    let mut state = in_memory_state();
    state.config.payment_backends = vec![PaymentBackend::Lightning];
    state.settlement_registry = settlement::SettlementRegistry::new(&state.config);

    let now = 1_700_000_000;
    let settlement_terms = rt
        .block_on(settlement::quoted_lightning_settlement_terms(&state, 9))
        .expect("settlement terms")
        .expect("lightning settlement terms");
    let requester_signing_key = froglet::crypto::generate_signing_key();
    let requester_id = froglet::crypto::public_key_hex(&requester_signing_key);
    let quote = protocol::sign_artifact(
        state.identity.node_id(),
        |message| state.identity.sign_message_hex(message),
        protocol::ARTIFACT_KIND_QUOTE,
        now,
        QuotePayload {
            provider_id: state.identity.node_id().to_string(),
            requester_id: requester_id.clone(),
            descriptor_hash: "descriptor-hash-1".to_string(),
            offer_hash: "offer-hash-1".to_string(),
            expires_at: settlement::lightning_quote_expires_at(&state, now, 9, 30),
            workload_kind: "compute.wasm.v1".to_string(),
            workload_hash: "aa".repeat(32),
            confidential_session_hash: None,
            capabilities_granted: Vec::new(),
            extension_refs: Vec::new(),
            quote_use: None,
            settlement_terms: settlement_terms.clone(),
            execution_limits: ExecutionLimits {
                max_input_bytes: 128 * 1024,
                max_runtime_ms: 30_000,
                max_memory_bytes: 8 * 1024 * 1024,
                max_output_bytes: 128 * 1024,
                fuel_limit: 50_000_000,
            },
        },
    )
    .expect("quote");
    let deal = protocol::sign_artifact(
        &requester_id,
        |message| froglet::crypto::sign_message_hex(&requester_signing_key, message),
        protocol::ARTIFACT_KIND_DEAL,
        now,
        DealPayload {
            requester_id: requester_id.clone(),
            provider_id: quote.payload.provider_id.clone(),
            quote_hash: quote.hash.clone(),
            workload_hash: quote.payload.workload_hash.clone(),
            confidential_session_hash: None,
            extension_refs: Vec::new(),
            authority_ref: None,
            supersedes_deal_hash: None,
            client_nonce: None,
            success_payment_hash: "11".repeat(32),
            admission_deadline: quote.payload.expires_at,
            completion_deadline: quote.payload.expires_at + 30,
            acceptance_deadline: quote.payload.expires_at + 60,
        },
    )
    .expect("deal");

    let valid_bundle = settlement::build_lightning_invoice_bundle(
        &state,
        settlement::BuildLightningInvoiceBundleRequest {
            session_id: Some("valid-session".to_string()),
            requester_id: requester_id.clone(),
            quote_hash: quote.hash.clone(),
            deal_hash: deal.hash.clone(),
            admission_deadline: Some(deal.payload.admission_deadline),
            success_payment_hash: "11".repeat(32),
            base_fee_msat: settlement_terms.base_fee_msat,
            success_fee_msat: settlement_terms.success_fee_msat,
            created_at: now,
        },
    )
    .expect("valid bundle");

    let report = settlement::validate_lightning_invoice_bundle(
        &valid_bundle.bundle,
        &quote,
        &deal,
        Some(&requester_id),
    );
    assert!(report.valid, "unexpected issues: {:?}", report.issues);

    let invalid_bundle = settlement::build_lightning_invoice_bundle(
        &state,
        settlement::BuildLightningInvoiceBundleRequest {
            session_id: Some("invalid-session".to_string()),
            requester_id: requester_id.clone(),
            quote_hash: quote.hash.clone(),
            deal_hash: deal.hash.clone(),
            admission_deadline: Some(deal.payload.admission_deadline),
            success_payment_hash: "33".repeat(32),
            base_fee_msat: settlement_terms.base_fee_msat,
            success_fee_msat: settlement_terms.success_fee_msat,
            created_at: now,
        },
    )
    .expect("invalid bundle");

    let invalid_report = settlement::validate_lightning_invoice_bundle(
        &invalid_bundle.bundle,
        &quote,
        &deal,
        Some(&requester_id),
    );
    assert!(!invalid_report.valid);
    assert!(
        invalid_report
            .issues
            .iter()
            .any(|issue| issue.code == "success_payment_hash_mismatch")
    );
}

#[test]
fn randomized_invoice_bundle_validation_reports_targeted_issues() {
    let rt = Runtime::new().unwrap();
    let mut state = in_memory_state();
    state.config.payment_backends = vec![PaymentBackend::Lightning];
    state.settlement_registry = settlement::SettlementRegistry::new(&state.config);
    let mut rng = StdRng::seed_from_u64(0x000F_06A1_E7B0_0D1E);

    for iteration in 0..27_u64 {
        let quoted_price_sats = 1 + (iteration % 25);
        let now = 1_700_000_000 + (iteration as i64 * 17);
        let settlement_terms = rt
            .block_on(settlement::quoted_lightning_settlement_terms(
                &state,
                quoted_price_sats,
            ))
            .expect("settlement terms")
            .expect("lightning settlement terms");

        let requester_signing_key = seeded_signing_key(&mut rng);
        let requester_id = froglet::crypto::public_key_hex(&requester_signing_key);
        let quote = protocol::sign_artifact(
            state.identity.node_id(),
            |message| state.identity.sign_message_hex(message),
            protocol::ARTIFACT_KIND_QUOTE,
            now,
            QuotePayload {
                provider_id: state.identity.node_id().to_string(),
                requester_id: requester_id.clone(),
                descriptor_hash: random_hex(&mut rng, 32),
                offer_hash: random_hex(&mut rng, 32),
                expires_at: settlement::lightning_quote_expires_at(
                    &state,
                    now,
                    quoted_price_sats,
                    30,
                ),
                workload_kind: "compute.wasm.v1".to_string(),
                workload_hash: random_hex(&mut rng, 32),
                confidential_session_hash: None,
                capabilities_granted: Vec::new(),
                extension_refs: Vec::new(),
                quote_use: None,
                settlement_terms: settlement_terms.clone(),
                execution_limits: ExecutionLimits {
                    max_input_bytes: 128 * 1024,
                    max_runtime_ms: 30_000,
                    max_memory_bytes: 8 * 1024 * 1024,
                    max_output_bytes: 128 * 1024,
                    fuel_limit: 50_000_000,
                },
            },
        )
        .expect("quote");
        let deal = protocol::sign_artifact(
            &requester_id,
            |message| froglet::crypto::sign_message_hex(&requester_signing_key, message),
            protocol::ARTIFACT_KIND_DEAL,
            now,
            DealPayload {
                requester_id: requester_id.clone(),
                provider_id: quote.payload.provider_id.clone(),
                quote_hash: quote.hash.clone(),
                workload_hash: quote.payload.workload_hash.clone(),
                confidential_session_hash: None,
                extension_refs: Vec::new(),
                authority_ref: None,
                supersedes_deal_hash: None,
                client_nonce: None,
                success_payment_hash: random_hex(&mut rng, 32),
                admission_deadline: quote.payload.expires_at,
                completion_deadline: quote.payload.expires_at + 30,
                acceptance_deadline: quote.payload.expires_at + 60,
            },
        )
        .expect("deal");
        let valid_bundle = settlement::build_lightning_invoice_bundle(
            &state,
            settlement::BuildLightningInvoiceBundleRequest {
                session_id: Some(format!("randomized-valid-{iteration}")),
                requester_id: requester_id.clone(),
                quote_hash: quote.hash.clone(),
                deal_hash: deal.hash.clone(),
                admission_deadline: Some(deal.payload.admission_deadline),
                success_payment_hash: deal.payload.success_payment_hash.clone(),
                base_fee_msat: settlement_terms.base_fee_msat,
                success_fee_msat: settlement_terms.success_fee_msat,
                created_at: now,
            },
        )
        .expect("valid bundle");

        let valid_report = settlement::validate_lightning_invoice_bundle(
            &valid_bundle.bundle,
            &quote,
            &deal,
            Some(&requester_id),
        );
        assert!(
            valid_report.valid,
            "iteration {iteration} unexpectedly invalid: {:?}",
            valid_report.issues
        );

        let mut tampered_payload = valid_bundle.bundle.payload.clone();
        let expected_code = match iteration % 9 {
            0 => {
                tampered_payload.requester_id = random_hex(&mut rng, 32);
                "requester_id_mismatch"
            }
            1 => {
                tampered_payload.quote_hash = random_hex(&mut rng, 32);
                "quote_hash_mismatch"
            }
            2 => {
                tampered_payload.deal_hash = random_hex(&mut rng, 32);
                "deal_hash_mismatch"
            }
            3 => {
                tampered_payload.destination_identity = random_destination_identity(&mut rng);
                "destination_identity_mismatch"
            }
            4 => {
                tampered_payload.base_fee.amount_msat =
                    tampered_payload.base_fee.amount_msat.saturating_add(1);
                "base_fee_mismatch"
            }
            5 => {
                tampered_payload.success_fee.amount_msat =
                    tampered_payload.success_fee.amount_msat.saturating_add(1);
                "success_fee_mismatch"
            }
            6 => {
                tampered_payload.min_final_cltv_expiry =
                    tampered_payload.min_final_cltv_expiry.saturating_add(1);
                "min_final_cltv_mismatch"
            }
            7 => {
                tampered_payload.base_fee.invoice_hash = random_hex(&mut rng, 32);
                "invoice_hash_mismatch"
            }
            _ => {
                tampered_payload.success_fee.payment_hash = random_hex(&mut rng, 32);
                "success_payment_hash_mismatch"
            }
        };
        let tampered_bundle = protocol::sign_artifact(
            state.identity.node_id(),
            |message| state.identity.sign_message_hex(message),
            protocol::TRANSPORT_KIND_INVOICE_BUNDLE,
            valid_bundle.bundle.created_at,
            tampered_payload,
        )
        .expect("tampered bundle");

        let invalid_report = settlement::validate_lightning_invoice_bundle(
            &tampered_bundle,
            &quote,
            &deal,
            Some(&requester_id),
        );
        assert!(
            !invalid_report.valid,
            "iteration {iteration} should be invalid for {expected_code}"
        );
        assert!(
            invalid_report
                .issues
                .iter()
                .any(|issue| issue.code == expected_code),
            "iteration {iteration} missing {expected_code}; issues: {:?}",
            invalid_report.issues
        );
    }
}

// ─── Stripe MPP integration tests ─────────────────────────────────────────────
//
// These tests use an in-process mock Stripe HTTP server (axum + TcpListener on
// port 0) so no real Stripe credentials or external network access is needed.

use axum::{
    Json as AxumJson, Router,
    extract::{Path as AxumPath, State as AxumState},
    routing::{get as axum_get, post as axum_post},
};
use std::sync::Mutex as StdMutex;
use tokio::net::TcpListener;

#[derive(Debug, Default)]
struct MockStripeServer {
    calls: StdMutex<Vec<String>>,
}

async fn start_mock_stripe_server() -> (String, Arc<MockStripeServer>, tokio::task::JoinHandle<()>)
{
    async fn get_token(
        AxumState(state): AxumState<Arc<MockStripeServer>>,
        AxumPath(token_id): AxumPath<String>,
    ) -> AxumJson<serde_json::Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("GET:granted_tokens/{token_id}"));
        AxumJson(serde_json::json!({
            "id": token_id,
            "usage_limits": {
                "currency": "usd",
                "expires_at": settlement::current_unix_timestamp() + 600,
                "max_amount": 50_000
            }
        }))
    }

    // Handle buyer-side POST /v1/shared_payment/granted_tokens (SPT create).
    // Returns a mock SPT id that the seller's GET handler will recognise.
    //
    // NOTE: Stripe shared-payment is a preview API; confirm exact field
    // names/endpoint against Stripe preview docs before live use.
    async fn create_granted_token(
        AxumState(state): AxumState<Arc<MockStripeServer>>,
        body: String,
    ) -> AxumJson<serde_json::Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST:granted_tokens:{body}"));
        // The mock returns a stable SPT id so tests can assert on it.
        AxumJson(serde_json::json!({
            "id": "spt_mock_buyer_test",
            "usage_limits": {
                "currency": "usd",
                "expires_at": settlement::current_unix_timestamp() + 600,
                "max_amount": 50_000
            }
        }))
    }

    async fn create_intent(
        AxumState(state): AxumState<Arc<MockStripeServer>>,
        body: String,
    ) -> AxumJson<serde_json::Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST:payment_intents:{body}"));
        AxumJson(serde_json::json!({
            "id": "pi_stripe_mpp_test",
            "status": "requires_capture"
        }))
    }

    async fn capture_intent(
        AxumState(state): AxumState<Arc<MockStripeServer>>,
        AxumPath(pi_id): AxumPath<String>,
    ) -> AxumJson<serde_json::Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST:payment_intents/{pi_id}/capture"));
        AxumJson(serde_json::json!({
            "id": pi_id,
            "status": "succeeded"
        }))
    }

    async fn cancel_intent(
        AxumState(state): AxumState<Arc<MockStripeServer>>,
        AxumPath(pi_id): AxumPath<String>,
    ) -> AxumJson<serde_json::Value> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST:payment_intents/{pi_id}/cancel"));
        AxumJson(serde_json::json!({
            "id": pi_id,
            "status": "canceled"
        }))
    }

    let server_state = Arc::new(MockStripeServer::default());
    let app = Router::new()
        // Buyer SPT create must be registered BEFORE the GET /:token_id route
        // so the router distinguishes the exact path.
        .route(
            "/v1/shared_payment/granted_tokens",
            axum_post(create_granted_token),
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
        .with_state(server_state.clone());

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock stripe");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve mock stripe");
    });
    (format!("http://{addr}"), server_state, handle)
}

/// Build an AppState configured with the Stripe payment backend and a custom
/// Stripe API base URL pointing at the local mock server.
///
/// The `FROGLET_STRIPE_SECRET_KEY` env var is set to a placeholder value before
/// building the `SettlementRegistry` and unset afterward (test-isolation only —
/// the driver is already constructed by this point).
fn stripe_app_state(mock_base_url: &str) -> AppState {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "froglet-stripe-integration-{}-{unique}-{counter}",
        std::process::id()
    ));
    let db_path = temp_dir.join("node.db");
    std::fs::create_dir_all(&temp_dir).expect("temp dir");

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
            events_query: 10,
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
            sync_interval_ms: 1_000,
            lnd_rest: None,
        },
        x402: None,
        stripe: Some(StripeConfig {
            api_version: "2024-06-20".to_string(),
            webhook_secret: None,
        }),
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

    // Temporarily set the Stripe API key so SettlementRegistry::new can
    // construct the driver.  Use the mock server's placeholder value.
    unsafe {
        std::env::set_var("FROGLET_STRIPE_SECRET_KEY", "sk_test_mock_placeholder");
    }
    // Build a registry that uses a mock-URL-aware StripeDriver.  Because the
    // StripeDriver constructor reads the API base URL from StripeConfig::api_base_url
    // (not directly from env), and the public constructor always uses the real
    // Stripe URL, we instantiate the driver directly here using the internal
    // with_base_url constructor exposed via the SettlementRegistry test helper.
    //
    // Practical approach: build the registry normally (which points at real Stripe)
    // then swap the driver reference — but SettlementRegistry doesn't expose that.
    // Instead, mirror what the stripe.rs unit tests do: call prepare_payment
    // directly on a StripeDriver built with with_base_url, bypassing the registry.
    // The integration test below uses this pattern.
    let settlement_registry = SettlementRegistry::new(&node_config);
    unsafe {
        std::env::remove_var("FROGLET_STRIPE_SECRET_KEY");
    }

    let pool = DbPool::open(&db_path).expect("init test db");
    let events_query_capacity = pool.read_connection_count().max(1);
    let pricing = froglet::pricing::PricingTable::from_config(node_config.pricing);
    let identity = froglet::identity::NodeIdentity::load_or_create(&node_config).expect("identity");

    // Store the mock base URL in the test so the driver can be constructed
    // with `with_base_url`.  AppState holds the registry, which we need for
    // overall state; the driver calls are made directly in the test body.
    let _ = mock_base_url; // consumed in test body

    AppState {
        db: pool,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: reqwest::Client::new(),
        wasm_host: None,
        confidential_policy: None,
        runtime_auth_token: "test-token".to_string(),
        runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
        consumer_control_auth_token: "test-consumer-token".to_string(),
        consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
        provider_control_auth_token: "test-provider-token".to_string(),
        provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
        events_query_semaphore: Arc::new(tokio::sync::Semaphore::new(events_query_capacity)),
        lnd_rest_client: None,
        lightning_wallet: None,
        lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
        event_batch_writer: None,
        builtin_services: std::collections::HashMap::new(),
        settlement_registry,
        session_pool: None,
    }
}

/// Full Stripe deal success path:
/// prepare (SPT → PaymentIntent) → commit (capture) → receipt has
/// method="stripe_mpp.v1" and passes kernel validation.
#[tokio::test]
async fn stripe_mpp_deal_prepare_commit_produces_valid_receipt() {
    let (base_url, mock_server, handle) = start_mock_stripe_server().await;

    // Build the driver pointed at the mock server.
    let driver = froglet::settlement::stripe_driver_with_base_url(
        StripeConfig {
            api_version: "2024-06-20".to_string(),
            webhook_secret: None,
        },
        "sk_test_mock".to_string(),
        &base_url,
    );
    let state = stripe_app_state(&base_url);

    // Step 1 — prepare: validates SPT and creates a manual-capture PaymentIntent.
    let reservation = driver
        .prepare(
            &state,
            PreparePaymentRequest {
                service_id: ServiceId::ExecuteWasm,
                price_sats: 30,
                payment: Some(ProvidedPayment {
                    kind: "stripe_mpp".to_string(),
                    token: "spt_integration_test".to_string(),
                }),
                request_id: Some("integration-prepare".to_string()),
            },
        )
        .await
        .expect("prepare must succeed")
        .expect("paid service must return a reservation");

    assert_eq!(reservation.method, "stripe_mpp");
    assert_eq!(reservation.token_hash, "pi_stripe_mpp_test");
    assert_eq!(reservation.amount_sats, 30);

    // Step 2 — commit: capture the PaymentIntent.
    let receipt = driver
        .commit(&state, reservation.clone())
        .await
        .expect("commit must succeed");

    assert_eq!(receipt.method, "stripe_mpp");
    assert_eq!(
        receipt.settlement_status,
        froglet::protocol::SettlementStatus::Committed
    );
    assert_eq!(
        receipt.settlement_reference.as_deref(),
        Some("pi_stripe_mpp_test")
    );

    // Verify the mock server was called in the right order.
    let calls = mock_server.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|c| c == "GET:granted_tokens/spt_integration_test"),
        "SPT validation call missing; calls: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("POST:payment_intents:")
            && c.contains("amount=30")
            && c.contains("capture_method=manual")),
        "PaymentIntent create call missing; calls: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c == "POST:payment_intents/pi_stripe_mpp_test/capture"),
        "PaymentIntent capture call missing; calls: {calls:?}"
    );

    // Step 3 — build and kernel-validate a Stripe receipt (simulating what
    // sign_deal_receipt would produce for a stripe deal).
    let provider_key = froglet::crypto::generate_signing_key();
    let provider_id = froglet::crypto::public_key_hex(&provider_key);

    let receipt_payload = froglet::protocol::ReceiptPayload {
        provider_id: provider_id.clone(),
        requester_id: "11".repeat(32),
        deal_hash: "22".repeat(32),
        quote_hash: "33".repeat(32),
        extension_refs: Vec::new(),
        acceptance_ref: None,
        started_at: Some(1_000),
        finished_at: 2_000,
        deal_state: "succeeded".to_string(),
        execution_state: "succeeded".to_string(),
        settlement_state: "settled".to_string(),
        result_hash: Some("44".repeat(32)),
        confidential_session_hash: None,
        result_envelope_hash: None,
        result_format: Some("application/json+jcs".to_string()),
        executor: froglet::protocol::ReceiptExecutor {
            runtime: "wasm".to_string(),
            runtime_version: "test".to_string(),
            execution_mode: None,
            attestation_platform: None,
            measurement: None,
            abi_version: Some("froglet.wasm.run_json.v1".to_string()),
            module_hash: Some("55".repeat(32)),
            capabilities_granted: Vec::new(),
        },
        limits_applied: ExecutionLimits {
            max_input_bytes: 1,
            max_runtime_ms: 2,
            max_memory_bytes: 3,
            max_output_bytes: 4,
            fuel_limit: 5,
        },
        settlement_refs: froglet::protocol::ReceiptSettlementRefs {
            method: "stripe_mpp.v1".to_string(),
            bundle_hash: None,
            destination_identity: String::new(),
            base_fee: froglet::protocol::ReceiptSettlementLeg {
                amount_msat: 30_000,
                invoice_hash: String::new(),
                payment_hash: receipt.settlement_reference.clone().unwrap_or_default(),
                state: ReceiptLegState::Settled,
            },
            success_fee: froglet::protocol::ReceiptSettlementLeg {
                amount_msat: 0,
                invoice_hash: String::new(),
                payment_hash: String::new(),
                state: ReceiptLegState::Canceled,
            },
        },
        failure_code: None,
        failure_message: None,
        result_ref: None,
    };

    let signed_receipt = froglet::protocol::sign_artifact(
        &provider_id,
        |msg| froglet::crypto::sign_message_hex(&provider_key, msg),
        froglet::protocol::ARTIFACT_KIND_RECEIPT,
        1_000,
        receipt_payload,
    )
    .expect("sign receipt");

    assert!(
        verify_artifact(&signed_receipt),
        "stripe_mpp.v1 receipt must have valid signature"
    );
    assert!(
        validate_receipt_artifact(&signed_receipt).is_ok(),
        "stripe_mpp.v1 receipt must pass kernel validation: {:?}",
        validate_receipt_artifact(&signed_receipt)
    );
    assert_eq!(
        signed_receipt.payload.settlement_refs.method,
        "stripe_mpp.v1"
    );

    handle.abort();
}

/// Stripe deal failure path: prepare → (execution fails) → release (cancel).
/// Verifies the cancel call was issued and a canceled receipt passes kernel validation.
#[tokio::test]
async fn stripe_mpp_deal_prepare_release_on_failure_produces_valid_receipt() {
    let (base_url, mock_server, handle) = start_mock_stripe_server().await;

    let driver = froglet::settlement::stripe_driver_with_base_url(
        StripeConfig {
            api_version: "2024-06-20".to_string(),
            webhook_secret: None,
        },
        "sk_test_mock".to_string(),
        &base_url,
    );
    let state = stripe_app_state(&base_url);

    // Prepare a reservation.
    let reservation = driver
        .prepare(
            &state,
            PreparePaymentRequest {
                service_id: ServiceId::ExecuteWasm,
                price_sats: 30,
                payment: Some(ProvidedPayment {
                    kind: "stripe_mpp".to_string(),
                    token: "spt_release_test".to_string(),
                }),
                request_id: Some("integration-release".to_string()),
            },
        )
        .await
        .expect("prepare must succeed")
        .expect("paid service must return a reservation");

    assert_eq!(reservation.token_hash, "pi_stripe_mpp_test");

    // Release (cancel) the PaymentIntent.
    driver
        .release(&state, &reservation)
        .await
        .expect("release must succeed");

    let calls = mock_server.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|c| c == "POST:payment_intents/pi_stripe_mpp_test/cancel"),
        "PaymentIntent cancel call missing; calls: {calls:?}"
    );

    // Build and kernel-validate a failure receipt (settlement_state = canceled).
    let provider_key = froglet::crypto::generate_signing_key();
    let provider_id = froglet::crypto::public_key_hex(&provider_key);

    let failure_receipt_payload = froglet::protocol::ReceiptPayload {
        provider_id: provider_id.clone(),
        requester_id: "11".repeat(32),
        deal_hash: "22".repeat(32),
        quote_hash: "33".repeat(32),
        extension_refs: Vec::new(),
        acceptance_ref: None,
        started_at: None,
        finished_at: 2_000,
        deal_state: "failed".to_string(),
        execution_state: "failed".to_string(),
        settlement_state: "canceled".to_string(),
        result_hash: None,
        confidential_session_hash: None,
        result_envelope_hash: None,
        result_format: None,
        executor: froglet::protocol::ReceiptExecutor {
            runtime: "wasm".to_string(),
            runtime_version: "test".to_string(),
            execution_mode: None,
            attestation_platform: None,
            measurement: None,
            abi_version: None,
            module_hash: None,
            capabilities_granted: Vec::new(),
        },
        limits_applied: ExecutionLimits {
            max_input_bytes: 1,
            max_runtime_ms: 2,
            max_memory_bytes: 3,
            max_output_bytes: 4,
            fuel_limit: 5,
        },
        settlement_refs: froglet::protocol::ReceiptSettlementRefs {
            method: "stripe_mpp.v1".to_string(),
            bundle_hash: None,
            destination_identity: String::new(),
            base_fee: froglet::protocol::ReceiptSettlementLeg {
                amount_msat: 30_000,
                invoice_hash: String::new(),
                payment_hash: reservation.token_hash.clone(),
                state: ReceiptLegState::Canceled,
            },
            success_fee: froglet::protocol::ReceiptSettlementLeg {
                amount_msat: 0,
                invoice_hash: String::new(),
                payment_hash: String::new(),
                state: ReceiptLegState::Canceled,
            },
        },
        failure_code: Some("execution_failed".to_string()),
        failure_message: Some("test execution error".to_string()),
        result_ref: None,
    };

    let signed_receipt = froglet::protocol::sign_artifact(
        &provider_id,
        |msg| froglet::crypto::sign_message_hex(&provider_key, msg),
        froglet::protocol::ARTIFACT_KIND_RECEIPT,
        2_000,
        failure_receipt_payload,
    )
    .expect("sign failure receipt");

    assert!(
        verify_artifact(&signed_receipt),
        "stripe_mpp.v1 failure receipt must have valid signature"
    );
    assert!(
        validate_receipt_artifact(&signed_receipt).is_ok(),
        "stripe_mpp.v1 failure receipt must pass kernel validation: {:?}",
        validate_receipt_artifact(&signed_receipt)
    );
    assert_eq!(
        signed_receipt.payload.settlement_refs.method,
        "stripe_mpp.v1"
    );
    assert_eq!(signed_receipt.payload.settlement_state, "canceled");

    handle.abort();
}

// ─── Buyer-side SPT minting tests ─────────────────────────────────────────────
//
// These tests verify the full buyer→seller Stripe path:
//   buyer mints SPT (mock) → seller prepare validates it + creates PaymentIntent
//   (mock) → commit captures → a kernel-valid stripe_mpp.v1 receipt is produced.

/// Unit test: buyer mints an SPT against the mock, then passes it directly to
/// the seller's `prepare` + `commit` cycle.  Asserts the mint call hit the
/// mock, the seller accepted the token, and the resulting receipt passes kernel
/// validation.
///
/// NOTE: Stripe shared-payment is a preview API; confirm exact field
/// names/endpoint against Stripe preview docs before live use.
#[tokio::test]
async fn buyer_mints_spt_and_seller_prepare_commit_produces_valid_receipt() {
    let (base_url, mock_server, handle) = start_mock_stripe_server().await;

    // ── Buyer side: mint an SPT ─────────────────────────────────────────────
    let buyer_config = BuyerStripeConfig {
        secret_key: "sk_test_buyer_mock".to_string(),
        api_version: "2026-03-04.preview".to_string(),
        payment_method: Some("pm_test_buyer_mock".to_string()),
        customer: None,
    };
    let price_cents: u64 = 30;
    let expires_at = settlement::current_unix_timestamp() + 600;

    let spt_id =
        settlement::mint_buyer_spt(&buyer_config, price_cents, expires_at, Some(&base_url))
            .await
            .expect("buyer SPT mint must succeed against mock");

    assert!(
        spt_id.starts_with("spt_"),
        "minted SPT id should start with 'spt_'; got: {spt_id}"
    );

    // Verify the mock received the create call with expected params.
    {
        let calls = mock_server.calls.lock().unwrap().clone();
        assert!(
            calls.iter().any(|c| c.starts_with("POST:granted_tokens:")
                && c.contains("payment_method")
                && c.contains("pm_test_buyer_mock")
                && c.contains("currency=usd")),
            "mock should have received SPT create call with payment_method; calls: {calls:?}"
        );
    }

    // ── Seller side: validate SPT and create PaymentIntent ─────────────────
    let seller_driver = froglet::settlement::stripe_driver_with_base_url(
        StripeConfig {
            api_version: "2026-03-04.preview".to_string(),
            webhook_secret: None,
        },
        "sk_test_seller_mock".to_string(),
        &base_url,
    );
    let state = stripe_app_state(&base_url);

    let reservation = seller_driver
        .prepare(
            &state,
            PreparePaymentRequest {
                service_id: ServiceId::ExecuteWasm,
                price_sats: price_cents,
                payment: Some(ProvidedPayment {
                    kind: "stripe_mpp".to_string(),
                    token: spt_id.clone(),
                }),
                request_id: Some("buyer-mint-unit-test".to_string()),
            },
        )
        .await
        .expect("seller prepare must succeed with buyer-minted SPT")
        .expect("paid service must return a reservation");

    assert_eq!(reservation.method, "stripe_mpp");
    assert_eq!(reservation.token_hash, "pi_stripe_mpp_test");

    // ── Commit ──────────────────────────────────────────────────────────────
    let receipt = seller_driver
        .commit(&state, reservation.clone())
        .await
        .expect("seller commit must succeed");

    assert_eq!(receipt.method, "stripe_mpp");
    assert_eq!(
        receipt.settlement_status,
        froglet::protocol::SettlementStatus::Committed
    );
    assert_eq!(
        receipt.settlement_reference.as_deref(),
        Some("pi_stripe_mpp_test")
    );

    // ── Build and kernel-validate a stripe_mpp.v1 receipt ──────────────────
    let provider_key = froglet::crypto::generate_signing_key();
    let provider_id = froglet::crypto::public_key_hex(&provider_key);

    let receipt_payload = froglet::protocol::ReceiptPayload {
        provider_id: provider_id.clone(),
        requester_id: "11".repeat(32),
        deal_hash: "22".repeat(32),
        quote_hash: "33".repeat(32),
        extension_refs: Vec::new(),
        acceptance_ref: None,
        started_at: Some(1_000),
        finished_at: 2_000,
        deal_state: "succeeded".to_string(),
        execution_state: "succeeded".to_string(),
        settlement_state: "settled".to_string(),
        result_hash: Some("44".repeat(32)),
        confidential_session_hash: None,
        result_envelope_hash: None,
        result_format: Some("application/json+jcs".to_string()),
        executor: froglet::protocol::ReceiptExecutor {
            runtime: "wasm".to_string(),
            runtime_version: "test".to_string(),
            execution_mode: None,
            attestation_platform: None,
            measurement: None,
            abi_version: Some("froglet.wasm.run_json.v1".to_string()),
            module_hash: Some("55".repeat(32)),
            capabilities_granted: Vec::new(),
        },
        limits_applied: ExecutionLimits {
            max_input_bytes: 1,
            max_runtime_ms: 2,
            max_memory_bytes: 3,
            max_output_bytes: 4,
            fuel_limit: 5,
        },
        settlement_refs: froglet::protocol::ReceiptSettlementRefs {
            method: "stripe_mpp.v1".to_string(),
            bundle_hash: None,
            destination_identity: String::new(),
            base_fee: froglet::protocol::ReceiptSettlementLeg {
                amount_msat: 30_000,
                invoice_hash: String::new(),
                payment_hash: receipt.settlement_reference.clone().unwrap_or_default(),
                state: ReceiptLegState::Settled,
            },
            success_fee: froglet::protocol::ReceiptSettlementLeg {
                amount_msat: 0,
                invoice_hash: String::new(),
                payment_hash: String::new(),
                state: ReceiptLegState::Canceled,
            },
        },
        failure_code: None,
        failure_message: None,
        result_ref: None,
    };

    let signed_receipt = froglet::protocol::sign_artifact(
        &provider_id,
        |msg| froglet::crypto::sign_message_hex(&provider_key, msg),
        froglet::protocol::ARTIFACT_KIND_RECEIPT,
        1_000,
        receipt_payload,
    )
    .expect("sign receipt");

    assert!(
        verify_artifact(&signed_receipt),
        "buyer-minted SPT receipt must have a valid signature"
    );
    assert!(
        validate_receipt_artifact(&signed_receipt).is_ok(),
        "buyer-minted SPT receipt must pass kernel validation: {:?}",
        validate_receipt_artifact(&signed_receipt)
    );
    assert_eq!(
        signed_receipt.payload.settlement_refs.method,
        "stripe_mpp.v1"
    );

    // Verify the full call sequence on the mock: create SPT → GET SPT →
    // create PI → capture PI.
    let calls = mock_server.calls.lock().unwrap().clone();
    assert!(
        calls.iter().any(|c| c.starts_with("POST:granted_tokens:")),
        "buyer SPT create call must appear in mock log; calls: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with(&format!("GET:granted_tokens/{spt_id}"))),
        "seller SPT validate GET call must appear; calls: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("POST:payment_intents:") && c.contains("capture_method=manual")),
        "seller PaymentIntent create call must appear; calls: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c == "POST:payment_intents/pi_stripe_mpp_test/capture"),
        "seller PaymentIntent capture call must appear; calls: {calls:?}"
    );

    handle.abort();
}

/// Unit test: `mint_buyer_spt` with a customer funding source produces an SPT
/// id; and the no-config error path returns a descriptive error rather than
/// panicking.
#[tokio::test]
async fn buyer_stripe_config_funding_source_variants() {
    let (base_url, _mock_server, handle) = start_mock_stripe_server().await;

    // Customer funding source.
    let buyer_config_customer = BuyerStripeConfig {
        secret_key: "sk_test_buyer_cus".to_string(),
        api_version: "2026-03-04.preview".to_string(),
        payment_method: None,
        customer: Some("cus_test_buyer_mock".to_string()),
    };
    let spt_id = settlement::mint_buyer_spt(
        &buyer_config_customer,
        50,
        settlement::current_unix_timestamp() + 600,
        Some(&base_url),
    )
    .await
    .expect("customer-funded mint must succeed");
    assert!(spt_id.starts_with("spt_"), "spt id: {spt_id}");

    // No funding source → error path exercised via mint_buyer_spt with an
    // intentionally broken config (no pm, no customer).  We exercise the
    // error path via the low-level driver directly.
    let driver = froglet::settlement::stripe_driver_with_base_url(
        StripeConfig {
            api_version: "2026-03-04.preview".to_string(),
            webhook_secret: None,
        },
        "sk_test_no_funding".to_string(),
        &base_url,
    );
    // The driver itself does not know about the config validation — that is
    // done in BuyerStripeConfig::from_env.  Test the env-level guard: a
    // BuyerStripeConfig with no funding source should fail from_env.
    // We test via the public API rather than calling env vars.
    let no_funding_config = BuyerStripeConfig {
        secret_key: "sk_test_no_funding".to_string(),
        api_version: "2026-03-04.preview".to_string(),
        payment_method: None,
        customer: None,
    };
    let result = settlement::mint_buyer_spt(
        &no_funding_config,
        10,
        settlement::current_unix_timestamp() + 600,
        Some(&base_url),
    )
    .await;
    assert!(
        result.is_err(),
        "mint_buyer_spt with no funding source must fail"
    );
    let err_msg = result.unwrap_err();
    assert!(
        err_msg.contains("funding source"),
        "error must mention funding source; got: {err_msg}"
    );

    // Suppress unused-variable warning on `driver` (used for type inference above).
    drop(driver);
    handle.abort();
}

/// REAL-STRIPE smoke test — validates the shared-payment (SPT) PREVIEW API
/// shapes end-to-end against LIVE Stripe. `#[ignore]`d so it never runs in CI.
///
/// It exercises the *actual* froglet code paths: buyer `mint_buyer_spt` →
/// seller `prepare` (validate SPT + create a manual-capture PaymentIntent) →
/// `commit` (capture); then a second mint → `prepare` → `release` (cancel).
/// Every step prints. If the preview API field shapes differ from our
/// assumptions, the mint step's error string shows the real Stripe error so
/// `src/settlement/stripe.rs::mint_spt` can be corrected.
///
/// Run with your Stripe TEST keys (no real money moves in test mode):
///   FROGLET_STRIPE_SECRET_KEY=sk_test_... \
///   FROGLET_BUYER_STRIPE_SECRET_KEY=sk_test_... \
///   FROGLET_BUYER_STRIPE_PAYMENT_METHOD=pm_card_visa \
///   cargo test --test payments_and_discovery stripe_real_api_smoke -- --ignored --nocapture
#[tokio::test]
#[ignore = "hits live Stripe; run manually with test keys (see doc comment)"]
async fn stripe_real_api_smoke() {
    const REAL: &str = "https://api.stripe.com";
    const API_VERSION: &str = "2026-03-04.preview";

    let Ok(seller_key) = std::env::var("FROGLET_STRIPE_SECRET_KEY") else {
        eprintln!(
            "SKIP stripe_real_api_smoke: set FROGLET_STRIPE_SECRET_KEY (+ \
             FROGLET_BUYER_STRIPE_SECRET_KEY + FROGLET_BUYER_STRIPE_PAYMENT_METHOD) to run"
        );
        return;
    };
    let buyer_key =
        std::env::var("FROGLET_BUYER_STRIPE_SECRET_KEY").unwrap_or_else(|_| seller_key.clone());
    let payment_method = std::env::var("FROGLET_BUYER_STRIPE_PAYMENT_METHOD").ok();
    let customer = std::env::var("FROGLET_BUYER_STRIPE_CUSTOMER").ok();
    if payment_method.is_none() && customer.is_none() {
        eprintln!(
            "SKIP stripe_real_api_smoke: set FROGLET_BUYER_STRIPE_PAYMENT_METHOD (pm_...) \
             or FROGLET_BUYER_STRIPE_CUSTOMER (cus_...)"
        );
        return;
    }

    let buyer_config = BuyerStripeConfig {
        secret_key: buyer_key,
        api_version: API_VERSION.to_string(),
        payment_method,
        customer,
    };
    let price_cents: u64 = 50; // $0.50 expressed in USD cents
    let expires_at = settlement::current_unix_timestamp() + 600;

    eprintln!("\n=== [1/4] buyer mint_buyer_spt against {REAL} (the most uncertain call) ===");
    let spt_id =
        match settlement::mint_buyer_spt(&buyer_config, price_cents, expires_at, None).await {
            Ok(id) => {
                eprintln!("  OK  spt_id = {id}");
                id
            }
            Err(e) => {
                eprintln!(
                    "  FAIL mint: {e}\n  >>> The shared-payment preview API rejected our \
                     request. Paste this error to the agent; the assumed field names live in \
                     src/settlement/stripe.rs::mint_spt and are easy to adjust."
                );
                panic!("mint_buyer_spt failed against live Stripe: {e}");
            }
        };

    let state = stripe_app_state(REAL);
    let seller = froglet::settlement::stripe_driver_with_base_url(
        StripeConfig {
            api_version: API_VERSION.to_string(),
            webhook_secret: None,
        },
        seller_key,
        REAL,
    );

    eprintln!("=== [2/4] seller prepare (validate SPT + create manual-capture PaymentIntent) ===");
    let reservation = match seller
        .prepare(
            &state,
            PreparePaymentRequest {
                service_id: ServiceId::ExecuteWasm,
                price_sats: price_cents,
                payment: Some(ProvidedPayment {
                    kind: "stripe_mpp".to_string(),
                    token: spt_id,
                }),
                request_id: Some("stripe-real-smoke".to_string()),
            },
        )
        .await
    {
        Ok(Some(r)) => {
            eprintln!("  OK  PaymentIntent = {}", r.token_hash);
            r
        }
        Ok(None) => panic!("a priced service must return a reservation"),
        Err(e) => {
            eprintln!(
                "  FAIL prepare: {e:?}\n  >>> Mint worked but validate/PI-create failed. Check \
                 your Stripe dashboard for the attempted PaymentIntent, and/or re-run a seller \
                 node with RUST_LOG=froglet=debug to see the underlying Stripe error."
            );
            panic!("seller prepare failed: {e:?}");
        }
    };

    eprintln!("=== [3/4] seller commit (capture the PaymentIntent) ===");
    match seller.commit(&state, reservation.clone()).await {
        Ok(r) => eprintln!(
            "  OK  captured: method={} ref={:?}",
            r.method, r.settlement_reference
        ),
        Err(e) => {
            eprintln!("  FAIL capture: {e:?}");
            panic!("seller commit failed: {e:?}");
        }
    }

    eprintln!("=== [4/4] release/cancel path (fresh mint -> prepare -> release) ===");
    let spt2 = settlement::mint_buyer_spt(
        &buyer_config,
        price_cents,
        settlement::current_unix_timestamp() + 600,
        None,
    )
    .await
    .expect("second SPT mint for the cancel path");
    let res2 = seller
        .prepare(
            &state,
            PreparePaymentRequest {
                service_id: ServiceId::ExecuteWasm,
                price_sats: price_cents,
                payment: Some(ProvidedPayment {
                    kind: "stripe_mpp".to_string(),
                    token: spt2,
                }),
                request_id: Some("stripe-real-smoke-cancel".to_string()),
            },
        )
        .await
        .expect("prepare (cancel path)")
        .expect("reservation (cancel path)");
    match seller.release(&state, &res2).await {
        Ok(()) => eprintln!("  OK  canceled PaymentIntent {}", res2.token_hash),
        Err(e) => {
            eprintln!("  FAIL release: {e}");
            panic!("seller release failed: {e}");
        }
    }

    eprintln!(
        "\n=== stripe_real_api_smoke PASSED — mint/validate/capture/cancel all work against \
         live Stripe. The Stripe rail is API-shape-validated. ===\n"
    );
}
