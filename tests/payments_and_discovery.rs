use froglet::{
    confidential::ConfidentialConfig,
    config::{
        DiscoveryMode, IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
        PaymentBackend, PricingConfig, ReferenceDiscoveryConfig, StorageConfig, WasmConfig,
    },
    db::{self, DbPool},
    discovery::{
        descriptor_digest_hex, heartbeat_signing_payload, reclaim_signing_payload,
        register_signing_payload,
    },
    pricing::ServiceId,
    protocol::{
        self, DealPayload, ExecutionLimits, InvoiceBundleLegState, QuotePayload, verify_artifact,
    },
    settlement,
    state::{AppState, ReferenceDiscoveryStatus, TransportStatus},
};
use rand::{RngCore, SeedableRng, rngs::StdRng};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
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
        discovery_mode: DiscoveryMode::Reference,
        identity: IdentityConfig {
            auto_generate: true,
        },
        reference_discovery: Some(ReferenceDiscoveryConfig {
            url: "http://localhost".to_string(),
            publish: true,
            required: false,
            heartbeat_interval_secs: 30,
        }),
        pricing: PricingConfig {
            events_query: 10,
            execute_wasm: 30,
        },
        payment_backend: PaymentBackend::Lightning,
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
        storage: StorageConfig {
            data_dir: temp_dir.clone(),
            db_path: db_path.clone(),
            identity_dir: temp_dir.join("identity"),
            identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
            nostr_publication_seed_path: temp_dir.join("identity/nostr-publication.secp256k1.seed"),
            runtime_dir: temp_dir.join("runtime"),
            runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
            tor_dir: temp_dir.join("tor"),
        },
        wasm: WasmConfig {
            policy_path: None,
            policy: None,
        },
        confidential: ConfidentialConfig {
            policy_path: None,
            policy: None,
            session_ttl_secs: 300,
        },
    };

    let pool = DbPool::open(&node_config.storage.db_path).expect("init db");
    let events_query_capacity = pool.read_connection_count().max(1);

    let pricing = froglet::pricing::PricingTable::from_config(node_config.pricing);
    let identity = froglet::identity::NodeIdentity::load_or_create(&node_config).expect("identity");

    AppState {
        db: pool,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        reference_discovery_status: Arc::new(tokio::sync::Mutex::new(
            ReferenceDiscoveryStatus::from_config(&node_config),
        )),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: reqwest::Client::new(),
        wasm_host: None,
        confidential_policy: None,
        runtime_auth_token: "test-runtime-token".to_string(),
        runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
        events_query_semaphore: Arc::new(tokio::sync::Semaphore::new(events_query_capacity)),
        lnd_rest_client: None,
        lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
    }
}

#[test]
fn discovery_signing_payloads_are_stable() {
    let state = in_memory_state();
    let rt = Runtime::new().unwrap();

    let descriptor1 = rt
        .block_on(froglet::discovery_client::build_descriptor(&state))
        .expect("descriptor 1");
    let descriptor2 = rt
        .block_on(froglet::discovery_client::build_descriptor(&state))
        .expect("descriptor 2");

    let digest1 = descriptor_digest_hex(&descriptor1).expect("digest 1");
    let digest2 = descriptor_digest_hex(&descriptor2).expect("digest 2");
    assert_eq!(
        digest1, digest2,
        "descriptor digest must remain stable across rebuilds"
    );
    assert_eq!(descriptor1.updated_at, None);
    assert_eq!(descriptor2.updated_at, None);

    let ts = 1234567890_i64;
    let register_msg = register_signing_payload(&descriptor1, ts).expect("register payload");
    let heartbeat_msg = heartbeat_signing_payload(state.identity.node_id(), ts);
    let reclaim_msg = reclaim_signing_payload(state.identity.node_id(), "challenge", "nonce", ts);

    assert!(
        register_msg.starts_with(b"froglet-register\n"),
        "register payload prefix changed"
    );
    assert!(
        heartbeat_msg.starts_with(b"froglet-heartbeat\n"),
        "heartbeat payload prefix changed"
    );
    assert!(
        reclaim_msg.starts_with(b"froglet-reclaim\n"),
        "reclaim payload prefix changed"
    );
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

    // Backend unavailable when backend is None and price > 0.
    state.config.payment_backend = PaymentBackend::None;
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
        settlement::PaymentError::BackendUnavailable { .. }
    ));

    // Lightning-priced legacy helpers are also unavailable through inline payments.
    state.config.payment_backend = PaymentBackend::Lightning;
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
        settlement::PaymentError::BackendUnavailable { .. }
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

    state.config.payment_backend = PaymentBackend::None;
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
    state.config.payment_backend = PaymentBackend::Lightning;

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
    state.config.payment_backend = PaymentBackend::Lightning;

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
    state.config.payment_backend = PaymentBackend::Lightning;
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

#[test]
fn discovery_initial_sync_returns_after_http_timeout() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let mut state = in_memory_state();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind hung discovery");
        let addr = listener.local_addr().expect("listener addr");
        state
            .config
            .reference_discovery
            .as_mut()
            .expect("reference discovery")
            .url = format!("http://{addr}");
        state.http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(100))
            .timeout(Duration::from_millis(100))
            .build()
            .expect("http client");

        tokio::spawn(async move {
            let _ = listener.accept().await;
            tokio::time::sleep(Duration::from_millis(250)).await;
        });

        let started_at = tokio::time::Instant::now();
        let error = froglet::discovery_client::perform_initial_sync(Arc::new(state))
            .await
            .expect_err("initial sync should time out");
        assert!(
            started_at.elapsed() < Duration::from_secs(1),
            "initial sync should fail quickly, elapsed {:?}",
            started_at.elapsed()
        );
        assert!(
            error.contains("register request failed")
                || error.contains("challenge request failed")
                || error.contains("timed out")
                || error.contains("deadline"),
            "unexpected error: {error}"
        );
    });
}
