use froglet::{
    config::{
        CashuConfig, DiscoveryMode, IdentityConfig, LightningConfig, LightningMode,
        MarketplaceConfig, NetworkMode, NodeConfig, PaymentBackend, PricingConfig, StorageConfig,
    },
    db::{self, DbPool},
    marketplace::{
        descriptor_digest_hex, heartbeat_signing_payload, reclaim_signing_payload,
        register_signing_payload,
    },
    payments::{self, ProvidedPayment},
    pricing::ServiceId,
    protocol::{
        self, DealPayload, InvoiceBundleLegState, PaymentLock, QuotePayload, verify_artifact,
    },
    settlement,
    state::{AppState, MarketplaceStatus, TransportStatus},
};
use rusqlite::params;
use std::sync::Arc;
use tokio::runtime::Runtime;

fn in_memory_state() -> AppState {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir =
        std::env::temp_dir().join(format!("froglet-test-{}-{unique}", std::process::id()));
    let db_path = temp_dir.join("node.db");
    std::fs::create_dir_all(&temp_dir).expect("temp dir");

    let node_config = NodeConfig {
        network_mode: NetworkMode::Clearnet,
        listen_addr: "127.0.0.1:0".to_string(),
        discovery_mode: DiscoveryMode::None,
        identity: IdentityConfig {
            auto_generate: true,
        },
        marketplace: Some(MarketplaceConfig {
            url: "http://localhost".to_string(),
            publish: true,
            required: false,
            heartbeat_interval_secs: 30,
        }),
        pricing: PricingConfig {
            events_query: 10,
            execute_wasm: 30,
        },
        payment_backend: PaymentBackend::Cashu,
        execution_timeout_secs: 10,
        cashu: CashuConfig {
            mint_allowlist: Vec::new(),
            remote_checkstate: false,
            request_timeout_secs: 5,
        },
        lightning: LightningConfig {
            mode: LightningMode::Mock,
            destination_identity: None,
            base_invoice_expiry_secs: 300,
            success_hold_expiry_secs: 300,
            min_final_cltv_expiry: 18,
        },
        storage: StorageConfig {
            data_dir: temp_dir.clone(),
            db_path: db_path.clone(),
            identity_dir: temp_dir.join("identity"),
            identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
            runtime_dir: temp_dir.join("runtime"),
            runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
            tor_dir: temp_dir.join("tor"),
        },
    };

    let conn = db::initialize_db(&node_config.storage.db_path).expect("init db");
    let pool = DbPool::new(conn);

    let pricing = froglet::pricing::PricingTable::from_config(node_config.pricing);
    let identity = froglet::identity::NodeIdentity::load_or_create(&node_config).expect("identity");

    AppState {
        db: pool,
        transport_status: Arc::new(tokio::sync::Mutex::new(TransportStatus::from_config(
            &node_config,
        ))),
        marketplace_status: Arc::new(tokio::sync::Mutex::new(MarketplaceStatus::from_config(
            &node_config,
        ))),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: reqwest::Client::new(),
        runtime_auth_token: "test-runtime-token".to_string(),
        runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
    }
}

#[test]
fn marketplace_signing_payloads_are_stable() {
    let state = in_memory_state();

    let descriptor = Runtime::new()
        .unwrap()
        .block_on(froglet::marketplace_client::build_descriptor(&state))
        .expect("descriptor");

    let digest1 = descriptor_digest_hex(&descriptor).expect("digest");
    let digest2 = descriptor_digest_hex(&descriptor).expect("digest again");
    assert_eq!(digest1, digest2, "descriptor digest must be deterministic");

    let ts = 1234567890_i64;
    let register_msg = register_signing_payload(&descriptor, ts).expect("register payload");
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
        .block_on(payments::prepare_payment(
            &state,
            ServiceId::EventsQuery,
            None,
            Some("req-backend-none".to_string()),
        ))
        .unwrap_err();
    assert!(matches!(
        err,
        payments::PaymentError::BackendUnavailable { .. }
    ));

    // Reset backend for further tests.
    state.config.payment_backend = PaymentBackend::Cashu;

    // Payment required when missing.
    let err = rt
        .block_on(payments::prepare_payment(
            &state,
            ServiceId::EventsQuery,
            None,
            Some("req-missing".to_string()),
        ))
        .unwrap_err();
    assert!(matches!(
        err,
        payments::PaymentError::PaymentRequired { .. }
    ));

    // Unsupported kind.
    let err = rt
        .block_on(payments::prepare_payment(
            &state,
            ServiceId::EventsQuery,
            Some(ProvidedPayment {
                kind: "other".to_string(),
                token: "x".to_string(),
            }),
            Some("req-kind".to_string()),
        ))
        .unwrap_err();
    assert!(matches!(
        err,
        payments::PaymentError::UnsupportedKind { .. }
    ));
}

#[test]
fn settlement_driver_reports_capabilities_consistently() {
    let rt = Runtime::new().unwrap();
    let mut state = in_memory_state();

    let cashu_descriptor = settlement::driver_descriptor(&state);
    assert_eq!(cashu_descriptor.backend, "cashu");
    assert_eq!(cashu_descriptor.mode, settlement::CASHU_VERIFIER_MODE);
    assert_eq!(cashu_descriptor.accepted_payment_methods, vec!["cashu"]);
    assert!(cashu_descriptor.accepted_mints.is_empty());
    assert_eq!(
        cashu_descriptor.capabilities,
        vec!["token_format_verification", "local_replay_guard"]
    );
    assert!(cashu_descriptor.reservations);
    assert!(cashu_descriptor.receipts);

    let cashu_wallet = rt
        .block_on(settlement::wallet_balance_snapshot(&state))
        .expect("wallet snapshot");
    assert_eq!(cashu_wallet.backend, "cashu");
    assert_eq!(cashu_wallet.mode, settlement::CASHU_VERIFIER_MODE);
    assert!(!cashu_wallet.balance_known);
    assert_eq!(cashu_wallet.accepted_payment_methods, vec!["cashu"]);
    assert!(cashu_wallet.accepted_mints.is_empty());
    assert_eq!(
        cashu_wallet.capabilities,
        vec!["token_format_verification", "local_replay_guard"]
    );

    state.config.payment_backend = PaymentBackend::Lightning;
    let lightning_descriptor = settlement::driver_descriptor(&state);
    assert_eq!(lightning_descriptor.backend, "lightning");
    assert_eq!(lightning_descriptor.mode, settlement::LIGHTNING_MOCK_MODE);
    assert_eq!(
        lightning_descriptor.accepted_payment_methods,
        vec!["lightning"]
    );
    assert!(lightning_descriptor.accepted_mints.is_empty());
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

    state.config.payment_backend = PaymentBackend::None;
    let none_descriptor = settlement::driver_descriptor(&state);
    assert_eq!(none_descriptor.backend, "none");
    assert_eq!(none_descriptor.mode, "disabled");
    assert!(none_descriptor.accepted_payment_methods.is_empty());
    assert!(none_descriptor.accepted_mints.is_empty());
    assert!(none_descriptor.capabilities.is_empty());
    assert!(!none_descriptor.reservations);
    assert!(!none_descriptor.receipts);

    let none_wallet = rt
        .block_on(settlement::wallet_balance_snapshot(&state))
        .expect("wallet snapshot");
    assert_eq!(none_wallet.backend, "none");
    assert_eq!(none_wallet.mode, "disabled");
    assert!(none_wallet.accepted_payment_methods.is_empty());
    assert!(none_wallet.accepted_mints.is_empty());
    assert!(none_wallet.capabilities.is_empty());
}

#[test]
fn settlement_descriptor_reports_mint_policy_capabilities() {
    let mut configured = in_memory_state();
    configured.config.cashu = CashuConfig {
        mint_allowlist: vec!["https://mint.example".to_string()],
        remote_checkstate: true,
        request_timeout_secs: 2,
    };

    let descriptor = settlement::driver_descriptor(&configured);
    assert_eq!(descriptor.accepted_mints, vec!["https://mint.example"]);
    assert_eq!(
        descriptor.capabilities,
        vec![
            "token_format_verification",
            "local_replay_guard",
            "mint_allowlist",
            "nut07_checkstate",
        ]
    );
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
                success_payment_hash: "11".repeat(32),
                base_fee_msat: 1_500,
                success_fee_msat: 9_000,
                created_at: 1_700_000_000,
            },
        ))
        .expect("lightning bundle");

    assert_eq!(created.session_id, "ln-session-1");
    assert_eq!(created.base_state, InvoiceBundleLegState::Open);
    assert_eq!(created.success_state, InvoiceBundleLegState::Open);
    assert_eq!(created.bundle.kind, "invoice_bundle");
    assert!(verify_artifact(&created.bundle));
    assert_eq!(created.bundle.payload.destination_identity.len(), 66);
    assert_eq!(created.bundle.payload.base_invoice.amount_msat, 1_500);
    assert_eq!(
        created.bundle.payload.success_hold_invoice.amount_msat,
        9_000
    );
    assert_eq!(
        created.bundle.payload.success_hold_invoice.payment_hash,
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
    let mut state = in_memory_state();
    state.config.payment_backend = PaymentBackend::Lightning;

    let now = 1_700_000_000;
    let settlement_terms = settlement::quoted_lightning_settlement_terms(&state, 9)
        .expect("lightning settlement terms");
    let quote = protocol::sign_artifact(
        state.identity.node_id(),
        |message| state.identity.sign_message_hex(message),
        protocol::ARTIFACT_KIND_QUOTE,
        now,
        QuotePayload {
            quote_id: "quote-1".to_string(),
            offer_id: "execute.wasm".to_string(),
            service_id: "execute.wasm".to_string(),
            workload_kind: "compute.wasm.v1".to_string(),
            workload_hash: "aa".repeat(32),
            price_sats: 9,
            payment_method: Some("lightning".to_string()),
            settlement_terms: Some(settlement_terms.clone()),
            expires_at: settlement::lightning_quote_expires_at(&state, now, 9),
        },
    )
    .expect("quote");
    let deal = protocol::sign_artifact(
        state.identity.node_id(),
        |message| state.identity.sign_message_hex(message),
        protocol::ARTIFACT_KIND_DEAL,
        now,
        DealPayload {
            deal_id: "deal-1".to_string(),
            quote_id: quote.payload.quote_id.clone(),
            offer_id: quote.payload.offer_id.clone(),
            service_id: quote.payload.service_id.clone(),
            workload_hash: quote.payload.workload_hash.clone(),
            payment_lock: Some(PaymentLock {
                kind: "lightning".to_string(),
                token_hash: "11".repeat(32),
                amount_sats: 9,
            }),
            idempotency_key: None,
            deadline: quote.payload.expires_at,
        },
    )
    .expect("deal");

    let valid_bundle = settlement::build_lightning_invoice_bundle(
        &state,
        settlement::BuildLightningInvoiceBundleRequest {
            session_id: Some("valid-session".to_string()),
            requester_id: "22".repeat(32),
            quote_hash: quote.hash.clone(),
            deal_hash: deal.hash.clone(),
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
        Some(&"22".repeat(32)),
    );
    assert!(report.valid, "unexpected issues: {:?}", report.issues);

    let invalid_bundle = settlement::build_lightning_invoice_bundle(
        &state,
        settlement::BuildLightningInvoiceBundleRequest {
            session_id: Some("invalid-session".to_string()),
            requester_id: "22".repeat(32),
            quote_hash: quote.hash.clone(),
            deal_hash: deal.hash.clone(),
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
        Some(&"22".repeat(32)),
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
fn payment_token_storage_tracks_release_and_expiry_without_deletion() {
    let rt = Runtime::new().unwrap();
    let state = in_memory_state();

    let reserve = rt
        .block_on(state.db.with_conn(|conn| {
            db::reserve_payment_token(conn, "token-a", ServiceId::EventsQuery, 10, "req-1", 100)
        }))
        .expect("reserve");
    assert!(matches!(reserve, db::ReservePaymentTokenOutcome::Reserved));

    let released = rt
        .block_on(
            state
                .db
                .with_conn(|conn| db::release_payment_token(conn, "token-a", "req-1", 110)),
        )
        .expect("release");
    assert!(released);

    let released_state = rt
        .block_on(state.db.with_conn(|conn| {
            conn.query_row(
                "SELECT state, request_id FROM payment_tokens WHERE token_hash = ?1",
                params!["token-a"],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|e| e.to_string())
        }))
        .expect("load released state");
    assert_eq!(released_state.0, "released");
    assert_eq!(released_state.1, "req-1");

    let reclaimed = rt
        .block_on(state.db.with_conn(|conn| {
            db::reserve_payment_token(conn, "token-a", ServiceId::EventsQuery, 10, "req-2", 120)
        }))
        .expect("re-reserve released token");
    assert!(matches!(
        reclaimed,
        db::ReservePaymentTokenOutcome::Reserved
    ));

    let _ = rt
        .block_on(
            state
                .db
                .with_conn(|conn| db::expire_reserved_payment_tokens(conn, 130)),
        )
        .expect("expire reserved");

    let expired_state = rt
        .block_on(state.db.with_conn(|conn| {
            conn.query_row(
                "SELECT state, request_id FROM payment_tokens WHERE token_hash = ?1",
                params!["token-a"],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|e| e.to_string())
        }))
        .expect("load expired state");
    assert_eq!(expired_state.0, "expired");
    assert_eq!(expired_state.1, "req-2");

    let reclaimed_after_expiry = rt
        .block_on(state.db.with_conn(|conn| {
            db::reserve_payment_token(conn, "token-a", ServiceId::EventsQuery, 10, "req-3", 140)
        }))
        .expect("re-reserve expired token");
    assert!(matches!(
        reclaimed_after_expiry,
        db::ReservePaymentTokenOutcome::Reserved
    ));

    let committed = rt
        .block_on(
            state
                .db
                .with_conn(|conn| db::commit_payment_token(conn, "token-a", "req-3", 150)),
        )
        .expect("commit");
    assert!(committed);

    let replay = rt
        .block_on(state.db.with_conn(|conn| {
            db::reserve_payment_token(conn, "token-a", ServiceId::EventsQuery, 10, "req-4", 160)
        }))
        .expect("replay check");
    assert!(matches!(replay, db::ReservePaymentTokenOutcome::Replay));
}
