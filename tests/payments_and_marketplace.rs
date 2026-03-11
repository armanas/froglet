use froglet::{
    config::{
        CashuConfig, DiscoveryMode, IdentityConfig, MarketplaceConfig, NetworkMode, NodeConfig,
        PaymentBackend, PricingConfig, StorageConfig,
    },
    db::{self, DbPool},
    marketplace::{
        descriptor_digest_hex, heartbeat_signing_payload, reclaim_signing_payload,
        register_signing_payload,
    },
    payments::{self, ProvidedPayment},
    pricing::ServiceId,
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
            execute_lua: 20,
            execute_wasm: 30,
        },
        payment_backend: PaymentBackend::Cashu,
        execution_timeout_secs: 10,
        cashu: CashuConfig {
            mint_allowlist: Vec::new(),
            remote_checkstate: false,
            request_timeout_secs: 5,
        },
        storage: StorageConfig {
            data_dir: temp_dir.clone(),
            db_path: db_path.clone(),
            identity_dir: temp_dir.join("identity"),
            identity_seed_path: temp_dir.join("identity/ed25519.seed"),
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
        .block_on(state.db.with_conn(|conn| {
            db::release_payment_token(conn, "token-a", "req-1", 110)
        }))
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
    assert!(matches!(reclaimed, db::ReservePaymentTokenOutcome::Reserved));

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
        .block_on(state.db.with_conn(|conn| {
            db::commit_payment_token(conn, "token-a", "req-3", 150)
        }))
        .expect("commit");
    assert!(committed);

    let replay = rt
        .block_on(state.db.with_conn(|conn| {
            db::reserve_payment_token(conn, "token-a", ServiceId::EventsQuery, 10, "req-4", 160)
        }))
        .expect("replay check");
    assert!(matches!(replay, db::ReservePaymentTokenOutcome::Replay));
}
