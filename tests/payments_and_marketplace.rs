use froglet::{
    config::{
        DiscoveryMode, IdentityConfig, MarketplaceConfig, NetworkMode, NodeConfig, PaymentBackend,
        PricingConfig, StorageConfig,
    },
    db::{self, DbPool},
    marketplace::{
        descriptor_digest_hex, heartbeat_signing_payload, reclaim_signing_payload,
        register_signing_payload,
    },
    payments::{self, ProvidedPayment},
    pricing::ServiceId,
    state::{AppState, MarketplaceStatus, TransportStatus},
};
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
        storage: StorageConfig {
            data_dir: temp_dir.clone(),
            db_path: db_path.clone(),
            identity_dir: temp_dir.join("identity"),
            identity_seed_path: temp_dir.join("identity/ed25519.seed"),
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
