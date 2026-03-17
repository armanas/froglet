use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bitcoin::hashes::{Hash as _, sha256};
use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use froglet::{
    api,
    config::{
        DiscoveryMode, IdentityConfig, LightningConfig, LightningLndRestConfig, LightningMode,
        MarketplaceConfig, NetworkMode, NodeConfig, PaymentBackend, PricingConfig, StorageConfig,
        WasmConfig,
    },
    crypto,
    db::{self, DbPool},
    deals::{self, NewDeal},
    identity::NodeIdentity,
    lnd::InvoiceState,
    pricing::PricingTable,
    protocol::{self, DealPayload, ExecutionLimits, QuotePayload, WorkloadSpec, verify_artifact},
    settlement::{self, BuildLightningInvoiceBundleRequest},
    state::{AppState, MarketplaceStatus, TransportStatus},
};
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::Mutex};

static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct FakeLndAppState {
    invoices: Arc<Mutex<HashMap<String, FakeInvoice>>>,
    node_secret: SecretKey,
    node_pubkey_hex: String,
    next_counter: Arc<AtomicU64>,
    issue_delay_secs: u64,
}

#[derive(Debug, Clone)]
struct FakeInvoice {
    payment_request: String,
    payment_hash_hex: String,
    value_msat: u64,
    expiry_secs: u64,
    state: InvoiceState,
}

struct FakeLndHandle {
    base_url: String,
    invoices: Arc<Mutex<HashMap<String, FakeInvoice>>>,
    node_pubkey_hex: String,
    macaroon_path: PathBuf,
    temp_dir: PathBuf,
    server_task: tokio::task::JoinHandle<()>,
}

impl FakeLndHandle {
    fn config(&self) -> LightningLndRestConfig {
        LightningLndRestConfig {
            rest_url: self.base_url.clone(),
            tls_cert_path: None,
            macaroon_path: self.macaroon_path.clone(),
            request_timeout_secs: 5,
        }
    }

    async fn set_invoice_state(&self, payment_hash_hex: &str, state: InvoiceState) {
        let mut invoices = self.invoices.lock().await;
        let invoice = invoices
            .get_mut(payment_hash_hex)
            .unwrap_or_else(|| panic!("missing fake invoice {payment_hash_hex}"));
        invoice.state = state;
    }

    async fn get_invoice_state(&self, payment_hash_hex: &str) -> Option<InvoiceState> {
        let invoices = self.invoices.lock().await;
        invoices
            .get(payment_hash_hex)
            .map(|invoice| invoice.state.clone())
    }
}

impl Drop for FakeLndHandle {
    fn drop(&mut self) {
        self.server_task.abort();
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

#[derive(Debug, Deserialize)]
struct AddInvoiceRequest {
    memo: String,
    value_msat: String,
    expiry: String,
    #[allow(dead_code)]
    private: bool,
}

#[derive(Debug, Deserialize)]
struct AddHoldInvoiceRequest {
    memo: String,
    hash: String,
    value_msat: String,
    expiry: String,
    cltv_expiry: String,
    #[allow(dead_code)]
    private: bool,
}

#[derive(Debug, Deserialize)]
struct SettleInvoiceRequest {
    preimage: String,
}

#[derive(Debug, Deserialize)]
struct CancelInvoiceRequest {
    payment_hash: String,
}

#[derive(Debug, Serialize)]
struct AddInvoiceResponse {
    r_hash: String,
    payment_request: String,
}

#[derive(Debug, Serialize)]
struct AddHoldInvoiceResponse {
    payment_request: String,
}

#[derive(Debug, Serialize)]
struct InvoiceLookupResponse {
    r_hash: String,
    payment_request: String,
    value_msat: u64,
    expiry: u64,
    state: String,
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "froglet-{label}-{}-{unique}-{counter}",
        std::process::id()
    ))
}

fn payment_secret_for_hash(payment_hash_hex: &str) -> PaymentSecret {
    let digest = sha256::Hash::hash(payment_hash_hex.as_bytes());
    PaymentSecret(digest.to_byte_array())
}

fn fake_bolt11_invoice(
    node_secret: &SecretKey,
    payment_hash_hex: &str,
    amount_msat: u64,
    expiry_secs: u64,
    min_final_cltv_expiry: u32,
    memo: &str,
) -> String {
    let secp = Secp256k1::new();
    let payee_pubkey = PublicKey::from_secret_key(&secp, node_secret);
    let payment_hash =
        sha256::Hash::from_slice(&hex::decode(payment_hash_hex).expect("valid payment hash"))
            .expect("32-byte payment hash");
    InvoiceBuilder::new(Currency::Regtest)
        .description(memo.to_string())
        .amount_milli_satoshis(amount_msat)
        .payment_hash(payment_hash)
        .payment_secret(payment_secret_for_hash(payment_hash_hex))
        .current_timestamp()
        .min_final_cltv_expiry_delta(min_final_cltv_expiry as u64)
        .expiry_time(Duration::from_secs(expiry_secs))
        .payee_pub_key(payee_pubkey)
        .build_signed(|hash| secp.sign_ecdsa_recoverable(hash, node_secret))
        .expect("build signed invoice")
        .to_string()
}

fn fake_lnd_state() -> FakeLndAppState {
    fake_lnd_state_with_issue_delay(0)
}

fn fake_lnd_state_with_issue_delay(issue_delay_secs: u64) -> FakeLndAppState {
    let node_secret = SecretKey::from_slice(&[7u8; 32]).expect("node secret");
    let secp = Secp256k1::new();
    let node_pubkey_hex = hex::encode(PublicKey::from_secret_key(&secp, &node_secret).serialize());
    FakeLndAppState {
        invoices: Arc::new(Mutex::new(HashMap::new())),
        node_secret,
        node_pubkey_hex,
        next_counter: Arc::new(AtomicU64::new(1)),
        issue_delay_secs,
    }
}

async fn fake_lnd_get_info(
    headers: HeaderMap,
    State(state): State<FakeLndAppState>,
) -> (StatusCode, Json<Value>) {
    assert!(headers.contains_key("Grpc-Metadata-macaroon"));
    (
        StatusCode::OK,
        Json(json!({
            "identity_pubkey": state.node_pubkey_hex,
            "alias": "froglet-fake-lnd",
            "version": "test"
        })),
    )
}

async fn fake_lnd_add_invoice(
    State(state): State<FakeLndAppState>,
    Json(payload): Json<AddInvoiceRequest>,
) -> (StatusCode, Json<AddInvoiceResponse>) {
    if state.issue_delay_secs > 0 {
        tokio::time::sleep(Duration::from_secs(state.issue_delay_secs)).await;
    }
    let counter = state.next_counter.fetch_add(1, Ordering::Relaxed);
    let payment_hash_hex = crypto::sha256_hex(format!("fake-lnd-base-{counter}").as_bytes());
    let value_msat = payload.value_msat.parse::<u64>().expect("value_msat");
    let expiry_secs = payload.expiry.parse::<u64>().expect("expiry");
    let payment_request = fake_bolt11_invoice(
        &state.node_secret,
        &payment_hash_hex,
        value_msat,
        expiry_secs,
        18,
        &payload.memo,
    );

    state.invoices.lock().await.insert(
        payment_hash_hex.clone(),
        FakeInvoice {
            payment_request: payment_request.clone(),
            payment_hash_hex: payment_hash_hex.clone(),
            value_msat,
            expiry_secs,
            state: InvoiceState::Open,
        },
    );

    (
        StatusCode::OK,
        Json(AddInvoiceResponse {
            r_hash: STANDARD.encode(hex::decode(payment_hash_hex).expect("payment hash bytes")),
            payment_request,
        }),
    )
}

async fn fake_lnd_add_hold_invoice(
    State(state): State<FakeLndAppState>,
    Json(payload): Json<AddHoldInvoiceRequest>,
) -> (StatusCode, Json<AddHoldInvoiceResponse>) {
    if state.issue_delay_secs > 0 {
        tokio::time::sleep(Duration::from_secs(state.issue_delay_secs)).await;
    }
    let payment_hash_hex = hex::encode(STANDARD.decode(payload.hash).expect("payment hash"));
    let value_msat = payload.value_msat.parse::<u64>().expect("value_msat");
    let expiry_secs = payload.expiry.parse::<u64>().expect("expiry");
    let cltv_expiry = payload.cltv_expiry.parse::<u32>().expect("cltv_expiry");
    let payment_request = fake_bolt11_invoice(
        &state.node_secret,
        &payment_hash_hex,
        value_msat,
        expiry_secs,
        cltv_expiry,
        &payload.memo,
    );

    state.invoices.lock().await.insert(
        payment_hash_hex.clone(),
        FakeInvoice {
            payment_request: payment_request.clone(),
            payment_hash_hex,
            value_msat,
            expiry_secs,
            state: InvoiceState::Open,
        },
    );

    (
        StatusCode::OK,
        Json(AddHoldInvoiceResponse { payment_request }),
    )
}

async fn fake_lnd_lookup_invoice(
    Path(payment_hash): Path<String>,
    State(state): State<FakeLndAppState>,
) -> (StatusCode, Json<Value>) {
    let invoices = state.invoices.lock().await;
    let Some(invoice) = invoices.get(&payment_hash) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "invoice not found" })),
        );
    };

    let response = InvoiceLookupResponse {
        r_hash: STANDARD
            .encode(hex::decode(&invoice.payment_hash_hex).expect("invoice payment hash bytes")),
        payment_request: invoice.payment_request.clone(),
        value_msat: invoice.value_msat,
        expiry: invoice.expiry_secs,
        state: match invoice.state {
            InvoiceState::Open => "OPEN",
            InvoiceState::Accepted => "ACCEPTED",
            InvoiceState::Settled => "SETTLED",
            InvoiceState::Canceled => "CANCELED",
        }
        .to_string(),
    };

    (StatusCode::OK, Json(json!(response)))
}

async fn fake_lnd_settle_invoice(
    State(state): State<FakeLndAppState>,
    Json(payload): Json<SettleInvoiceRequest>,
) -> (StatusCode, Json<Value>) {
    let preimage = STANDARD.decode(payload.preimage).expect("preimage");
    let payment_hash_hex = crypto::sha256_hex(&preimage);
    let mut invoices = state.invoices.lock().await;
    let Some(invoice) = invoices.get_mut(&payment_hash_hex) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "invoice not found" })),
        );
    };
    if invoice.state != InvoiceState::Accepted {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "invoice must be accepted before settlement" })),
        );
    }
    invoice.state = InvoiceState::Settled;
    (StatusCode::OK, Json(json!({})))
}

async fn fake_lnd_cancel_invoice(
    State(state): State<FakeLndAppState>,
    Json(payload): Json<CancelInvoiceRequest>,
) -> (StatusCode, Json<Value>) {
    let payment_hash_hex = hex::encode(STANDARD.decode(payload.payment_hash).expect("hash"));
    let mut invoices = state.invoices.lock().await;
    let Some(invoice) = invoices.get_mut(&payment_hash_hex) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "invoice not found" })),
        );
    };
    invoice.state = InvoiceState::Canceled;
    (StatusCode::OK, Json(json!({})))
}

async fn spawn_fake_lnd() -> FakeLndHandle {
    let temp_dir = unique_temp_dir("fake-lnd");
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let macaroon_path = temp_dir.join("admin.macaroon");
    std::fs::write(&macaroon_path, [1u8, 2, 3, 4]).expect("write macaroon");

    let app_state = fake_lnd_state();
    let invoices = app_state.invoices.clone();
    let node_pubkey_hex = app_state.node_pubkey_hex.clone();
    let app = Router::new()
        .route("/v1/getinfo", get(fake_lnd_get_info))
        .route("/v1/invoices", post(fake_lnd_add_invoice))
        .route("/v2/invoices/hodl", post(fake_lnd_add_hold_invoice))
        .route("/v1/invoice/:payment_hash", get(fake_lnd_lookup_invoice))
        .route("/v2/invoices/settle", post(fake_lnd_settle_invoice))
        .route("/v2/invoices/cancel", post(fake_lnd_cancel_invoice))
        .with_state(app_state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake lnd");
    let addr = listener.local_addr().expect("fake lnd addr");
    let server_task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fake lnd");
    });

    FakeLndHandle {
        base_url: format!("http://{}", addr),
        invoices,
        node_pubkey_hex,
        macaroon_path,
        temp_dir,
        server_task,
    }
}

async fn spawn_fake_lnd_with_issue_delay(issue_delay_secs: u64) -> FakeLndHandle {
    let temp_dir = unique_temp_dir("fake-lnd");
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let macaroon_path = temp_dir.join("admin.macaroon");
    std::fs::write(&macaroon_path, [1u8, 2, 3, 4]).expect("write macaroon");

    let app_state = fake_lnd_state_with_issue_delay(issue_delay_secs);
    let invoices = app_state.invoices.clone();
    let node_pubkey_hex = app_state.node_pubkey_hex.clone();
    let app = Router::new()
        .route("/v1/getinfo", get(fake_lnd_get_info))
        .route("/v1/invoices", post(fake_lnd_add_invoice))
        .route("/v2/invoices/hodl", post(fake_lnd_add_hold_invoice))
        .route("/v1/invoice/:payment_hash", get(fake_lnd_lookup_invoice))
        .route("/v2/invoices/settle", post(fake_lnd_settle_invoice))
        .route("/v2/invoices/cancel", post(fake_lnd_cancel_invoice))
        .with_state(app_state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake lnd");
    let addr = listener.local_addr().expect("fake lnd addr");
    let server_task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fake lnd");
    });

    FakeLndHandle {
        base_url: format!("http://{}", addr),
        invoices,
        node_pubkey_hex,
        macaroon_path,
        temp_dir,
        server_task,
    }
}

fn lnd_rest_state(fake_lnd: &FakeLndHandle) -> AppState {
    let temp_dir = unique_temp_dir("lnd-rest-state");
    let db_path = temp_dir.join("node.db");
    std::fs::create_dir_all(&temp_dir).expect("temp dir");

    let node_config = NodeConfig {
        network_mode: NetworkMode::Clearnet,
        listen_addr: "127.0.0.1:0".to_string(),
        public_base_url: None,
        runtime_listen_addr: "127.0.0.1:0".to_string(),
        tor: froglet::config::TorSidecarConfig {
            binary_path: "tor".to_string(),
            backend_listen_addr: "127.0.0.1:0".to_string(),
            startup_timeout_secs: 90,
        },
        discovery_mode: DiscoveryMode::None,
        identity: IdentityConfig {
            auto_generate: true,
        },
        marketplace: Some(MarketplaceConfig {
            url: "http://localhost".to_string(),
            publish: false,
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
            mode: LightningMode::LndRest,
            destination_identity: None,
            base_invoice_expiry_secs: 300,
            success_hold_expiry_secs: 300,
            min_final_cltv_expiry: 18,
            sync_interval_ms: 100,
            lnd_rest: Some(fake_lnd.config()),
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
    };

    let pool = DbPool::open(&node_config.storage.db_path).expect("init db");
    let events_query_capacity = pool.read_connection_count().max(1);
    let pricing = PricingTable::from_config(node_config.pricing);
    let identity = NodeIdentity::load_or_create(&node_config).expect("identity");
    let lnd_rest_client = froglet::lnd::LndRestClient::from_config(
        node_config
            .lightning
            .lnd_rest
            .as_ref()
            .expect("lnd rest config"),
    )
    .expect("cached lnd client");

    AppState {
        db: pool,
        transport_status: Arc::new(Mutex::new(TransportStatus::from_config(&node_config))),
        marketplace_status: Arc::new(Mutex::new(MarketplaceStatus::from_config(&node_config))),
        wasm_sandbox: Arc::new(froglet::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
        config: node_config,
        identity: Arc::new(identity),
        pricing,
        http_client: reqwest::Client::new(),
        wasm_host: None,
        runtime_auth_token: "test-runtime-token".to_string(),
        runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
        events_query_semaphore: Arc::new(tokio::sync::Semaphore::new(events_query_capacity)),
        lnd_rest_client: Some(Arc::new(lnd_rest_client)),
        lightning_destination_identity: Arc::new(tokio::sync::OnceCell::new()),
    }
}

fn sign_quote_and_deal(
    state: &AppState,
    settlement_terms: froglet::protocol::QuoteSettlementTerms,
    now: i64,
    success_payment_hash: &str,
) -> (
    protocol::SignedArtifact<QuotePayload>,
    protocol::SignedArtifact<DealPayload>,
) {
    let price_sats = (settlement_terms.base_fee_msat + settlement_terms.success_fee_msat) / 1_000;
    let requester_signing_key = crypto::generate_signing_key();
    let requester_id = crypto::public_key_hex(&requester_signing_key);
    let quote = protocol::sign_artifact(
        state.identity.node_id(),
        |message| state.identity.sign_message_hex(message),
        protocol::ARTIFACT_KIND_QUOTE,
        now,
        QuotePayload {
            provider_id: state.identity.node_id().to_string(),
            requester_id: requester_id.clone(),
            descriptor_hash: "descriptor-hash-lnd-1".to_string(),
            offer_hash: "offer-hash-lnd-1".to_string(),
            expires_at: settlement::lightning_quote_expires_at(state, now, price_sats, 30),
            workload_kind: "compute.wasm.v1".to_string(),
            workload_hash: "aa".repeat(32),
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
        |message| crypto::sign_message_hex(&requester_signing_key, message),
        protocol::ARTIFACT_KIND_DEAL,
        now,
        DealPayload {
            requester_id: requester_id.clone(),
            provider_id: quote.payload.provider_id.clone(),
            quote_hash: quote.hash.clone(),
            workload_hash: quote.payload.workload_hash.clone(),
            extension_refs: Vec::new(),
            authority_ref: None,
            supersedes_deal_hash: None,
            client_nonce: None,
            success_payment_hash: success_payment_hash.to_string(),
            admission_deadline: quote.payload.expires_at,
            completion_deadline: quote.payload.expires_at + 30,
            acceptance_deadline: quote.payload.expires_at + 60,
        },
    )
    .expect("deal");
    (quote, deal)
}

#[tokio::test(flavor = "current_thread")]
async fn lnd_rest_bundle_uses_real_bolt11_and_syncs_backend_state() {
    let fake_lnd = spawn_fake_lnd().await;
    let state = lnd_rest_state(&fake_lnd);
    let now = settlement::current_unix_timestamp() + 2;
    let success_preimage = vec![0x41; 32];
    let success_payment_hash = crypto::sha256_hex(&success_preimage);

    let mut settlement_terms = settlement::quoted_lightning_settlement_terms(&state, 11)
        .await
        .expect("settlement terms")
        .expect("lightning settlement terms");
    settlement_terms.base_fee_msat = 2_000;
    settlement_terms.success_fee_msat = 9_000;
    assert_eq!(
        settlement_terms.destination_identity,
        fake_lnd.node_pubkey_hex
    );

    let (quote, deal) =
        sign_quote_and_deal(&state, settlement_terms.clone(), now, &success_payment_hash);

    let session = settlement::create_lightning_invoice_bundle(
        &state,
        BuildLightningInvoiceBundleRequest {
            session_id: Some("lnd-rest-session-1".to_string()),
            requester_id: deal.payload.requester_id.clone(),
            quote_hash: quote.hash.clone(),
            deal_hash: deal.hash.clone(),
            admission_deadline: Some(deal.payload.admission_deadline),
            success_payment_hash: success_payment_hash.clone(),
            base_fee_msat: settlement_terms.base_fee_msat,
            success_fee_msat: settlement_terms.success_fee_msat,
            created_at: now,
        },
    )
    .await
    .expect("bundle");

    assert!(verify_artifact(&session.bundle));
    assert!(
        !session
            .bundle
            .payload
            .base_fee
            .invoice_bolt11
            .starts_with("lnmock-")
    );
    assert!(
        !session
            .bundle
            .payload
            .success_fee
            .invoice_bolt11
            .starts_with("lnmock-")
    );

    let report = settlement::validate_lightning_invoice_bundle(
        &session.bundle,
        &quote,
        &deal,
        Some(&deal.payload.requester_id),
    );
    assert!(
        report.valid,
        "unexpected validation issues: {:?}",
        report.issues
    );

    fake_lnd
        .set_invoice_state(
            &session.bundle.payload.base_fee.payment_hash,
            InvoiceState::Accepted,
        )
        .await;
    fake_lnd
        .set_invoice_state(
            &session.bundle.payload.success_fee.payment_hash,
            InvoiceState::Accepted,
        )
        .await;

    let synced = settlement::get_lightning_invoice_bundle(&state, &session.session_id)
        .await
        .expect("synced bundle")
        .expect("bundle");
    assert_eq!(synced.base_state, protocol::InvoiceBundleLegState::Settled);
    assert_eq!(
        synced.success_state,
        protocol::InvoiceBundleLegState::Accepted
    );
    assert_eq!(
        fake_lnd
            .get_invoice_state(&session.bundle.payload.base_fee.payment_hash)
            .await,
        Some(InvoiceState::Settled)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lnd_rest_bundle_stays_valid_when_invoice_issue_is_delayed() {
    let fake_lnd = spawn_fake_lnd_with_issue_delay(2).await;
    let state = lnd_rest_state(&fake_lnd);
    let now = settlement::current_unix_timestamp() + 2;
    let success_preimage = vec![0x51; 32];
    let success_payment_hash = crypto::sha256_hex(&success_preimage);

    let mut settlement_terms = settlement::quoted_lightning_settlement_terms(&state, 11)
        .await
        .expect("settlement terms")
        .expect("lightning settlement terms");
    settlement_terms.base_fee_msat = 2_000;
    settlement_terms.success_fee_msat = 9_000;

    let (quote, deal) =
        sign_quote_and_deal(&state, settlement_terms.clone(), now, &success_payment_hash);

    let session = settlement::create_lightning_invoice_bundle(
        &state,
        BuildLightningInvoiceBundleRequest {
            session_id: Some("lnd-rest-session-delayed".to_string()),
            requester_id: deal.payload.requester_id.clone(),
            quote_hash: quote.hash.clone(),
            deal_hash: deal.hash.clone(),
            admission_deadline: Some(deal.payload.admission_deadline),
            success_payment_hash: success_payment_hash.clone(),
            base_fee_msat: settlement_terms.base_fee_msat,
            success_fee_msat: settlement_terms.success_fee_msat,
            created_at: now,
        },
    )
    .await
    .expect("bundle");

    let report = settlement::validate_lightning_invoice_bundle(
        &session.bundle,
        &quote,
        &deal,
        Some(&deal.payload.requester_id),
    );
    assert!(
        report.valid,
        "unexpected validation issues after delayed LND issuance: {:?}",
        report.issues
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lnd_rest_success_settlement_calls_backend_and_updates_bundle() {
    let fake_lnd = spawn_fake_lnd().await;
    let state = lnd_rest_state(&fake_lnd);
    let now = settlement::current_unix_timestamp() + 2;
    let success_preimage = vec![0x42; 32];
    let success_payment_hash = crypto::sha256_hex(&success_preimage);

    let session = settlement::create_lightning_invoice_bundle(
        &state,
        BuildLightningInvoiceBundleRequest {
            session_id: Some("lnd-rest-session-2".to_string()),
            requester_id: "33".repeat(32),
            quote_hash: "quote-hash-2".to_string(),
            deal_hash: "deal-hash-2".to_string(),
            admission_deadline: None,
            success_payment_hash: success_payment_hash.clone(),
            base_fee_msat: 0,
            success_fee_msat: 9_000,
            created_at: now,
        },
    )
    .await
    .expect("bundle");

    fake_lnd
        .set_invoice_state(
            &session.bundle.payload.success_fee.payment_hash,
            InvoiceState::Accepted,
        )
        .await;

    let settled = settlement::settle_lightning_success_hold_invoice(
        &state,
        &session,
        &hex::encode(success_preimage),
    )
    .await
    .expect("settled bundle");

    assert_eq!(settled.base_state, protocol::InvoiceBundleLegState::Settled);
    assert_eq!(
        settled.success_state,
        protocol::InvoiceBundleLegState::Settled
    );
    assert_eq!(
        fake_lnd
            .get_invoice_state(&session.bundle.payload.success_fee.payment_hash)
            .await,
        Some(InvoiceState::Settled)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lnd_rest_bundle_sync_reflects_backend_cancellation() {
    let fake_lnd = spawn_fake_lnd().await;
    let state = lnd_rest_state(&fake_lnd);
    let now = settlement::current_unix_timestamp() + 2;
    let success_payment_hash = crypto::sha256_hex([0x44; 32]);

    let session = settlement::create_lightning_invoice_bundle(
        &state,
        BuildLightningInvoiceBundleRequest {
            session_id: Some("lnd-rest-session-3".to_string()),
            requester_id: "44".repeat(32),
            quote_hash: "quote-hash-3".to_string(),
            deal_hash: "deal-hash-3".to_string(),
            admission_deadline: None,
            success_payment_hash,
            base_fee_msat: 0,
            success_fee_msat: 9_000,
            created_at: now,
        },
    )
    .await
    .expect("bundle");

    fake_lnd
        .set_invoice_state(
            &session.bundle.payload.success_fee.payment_hash,
            InvoiceState::Canceled,
        )
        .await;

    let synced = settlement::get_lightning_invoice_bundle(&state, &session.session_id)
        .await
        .expect("synced bundle")
        .expect("bundle");
    assert_eq!(
        synced.success_state,
        protocol::InvoiceBundleLegState::Canceled
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lnd_rest_bundle_creation_cancels_issued_invoices_when_local_persistence_fails() {
    let fake_lnd = spawn_fake_lnd().await;
    let mut state = lnd_rest_state(&fake_lnd);
    let conn = rusqlite::Connection::open(&state.config.storage.db_path).expect("open db");
    froglet::db::initialize_db_for_connection(&conn).expect("init db");
    conn.execute_batch("PRAGMA query_only = ON;")
        .expect("set query_only");
    state.db = DbPool::new(conn);

    let now = settlement::current_unix_timestamp() + 2;
    let success_payment_hash = crypto::sha256_hex([0x55; 32]);
    let error = settlement::create_lightning_invoice_bundle(
        &state,
        BuildLightningInvoiceBundleRequest {
            session_id: Some("lnd-rest-session-4".to_string()),
            requester_id: "55".repeat(32),
            quote_hash: "quote-hash-4".to_string(),
            deal_hash: "deal-hash-4".to_string(),
            admission_deadline: None,
            success_payment_hash: success_payment_hash.clone(),
            base_fee_msat: 2_000,
            success_fee_msat: 9_000,
            created_at: now,
        },
    )
    .await
    .expect_err("bundle creation should fail when local persistence is read-only");
    assert!(
        error.contains("attempt to write a readonly database"),
        "unexpected error: {error}"
    );

    let success_state = fake_lnd
        .get_invoice_state(&success_payment_hash)
        .await
        .expect("success invoice state");
    assert_eq!(success_state, InvoiceState::Canceled);

    let invoices = fake_lnd.invoices.lock().await;
    let canceled_base_hashes = invoices
        .values()
        .filter(|invoice| invoice.state == InvoiceState::Canceled)
        .map(|invoice| invoice.payment_hash_hex.clone())
        .collect::<Vec<_>>();
    assert!(
        canceled_base_hashes
            .iter()
            .any(|hash| hash != &success_payment_hash),
        "expected the issued base invoice to be canceled as well"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn remote_recovery_cancels_orphaned_materialization_invoices() {
    let fake_lnd = spawn_fake_lnd().await;
    let state = Arc::new(lnd_rest_state(&fake_lnd));
    let now = settlement::current_unix_timestamp() + 2;
    let success_payment_hash = crypto::sha256_hex([0x66; 32]);
    let deal_id = protocol::new_artifact_id();

    let mut settlement_terms = settlement::quoted_lightning_settlement_terms(state.as_ref(), 11)
        .await
        .expect("settlement terms")
        .expect("lightning settlement terms");
    settlement_terms.base_fee_msat = 2_000;
    settlement_terms.success_fee_msat = 9_000;

    let (quote, deal) = sign_quote_and_deal(
        state.as_ref(),
        settlement_terms.clone(),
        now,
        &success_payment_hash,
    );
    let request = BuildLightningInvoiceBundleRequest {
        session_id: Some(deal_id.clone()),
        requester_id: deal.payload.requester_id.clone(),
        quote_hash: quote.hash.clone(),
        deal_hash: deal.hash.clone(),
        admission_deadline: Some(deal.payload.admission_deadline),
        success_payment_hash: success_payment_hash.clone(),
        base_fee_msat: settlement_terms.base_fee_msat,
        success_fee_msat: settlement_terms.success_fee_msat,
        created_at: now,
    };
    let issued = settlement::issue_lightning_invoice_bundle(state.as_ref(), request.clone())
        .await
        .expect("issue orphaned bundle");

    let materialization_json = serde_json::to_string(&request).expect("materialization json");
    state
        .db
        .with_write_conn({
            let deal_id = deal_id.clone();
            let quote = quote.clone();
            let deal = deal.clone();
            let success_payment_hash = success_payment_hash.clone();
            move |conn| -> Result<(), String> {
                deals::insert_or_get_deal(
                    conn,
                    NewDeal {
                        deal_id: deal_id.clone(),
                        idempotency_key: Some("orphaned-materialization".to_string()),
                        quote,
                        spec: WorkloadSpec::EventsQuery {
                            kinds: vec!["note".to_string()],
                            limit: Some(1),
                        },
                        artifact: deal.clone(),
                        workload_evidence_hash: None,
                        deal_artifact_hash: deal.hash.clone(),
                        payment_method: Some("lightning".to_string()),
                        payment_token_hash: Some(success_payment_hash),
                        payment_amount_sats: Some(
                            (settlement_terms.base_fee_msat + settlement_terms.success_fee_msat)
                                / 1_000,
                        ),
                        initial_status: deals::DEAL_STATUS_PAYMENT_PENDING.to_string(),
                        created_at: now,
                    },
                )?;
                db::insert_deal_settlement_materialization(
                    conn,
                    &deal_id,
                    "lightning_invoice_bundle",
                    &materialization_json,
                    now,
                )?;
                Ok(())
            }
        })
        .await
        .expect("seed orphaned materialization");

    api::recover_runtime_state_remote(state.clone())
        .await
        .expect("remote recovery");

    assert_eq!(
        fake_lnd
            .get_invoice_state(&issued.bundle.payload.base_fee.payment_hash)
            .await,
        Some(InvoiceState::Canceled)
    );
    assert_eq!(
        fake_lnd
            .get_invoice_state(&issued.bundle.payload.success_fee.payment_hash)
            .await,
        Some(InvoiceState::Canceled)
    );

    let recovered_deal = state
        .db
        .with_read_conn({
            let deal_id = deal_id.clone();
            move |conn| deals::get_deal(conn, &deal_id)
        })
        .await
        .expect("load recovered deal")
        .expect("deal");
    assert_eq!(recovered_deal.status, deals::DEAL_STATUS_FAILED);
    assert_eq!(
        recovered_deal
            .receipt
            .as_ref()
            .and_then(|receipt| receipt.payload.failure_code.as_deref()),
        Some("settlement_materialization_interrupted_during_recovery")
    );

    let remaining_materialization = state
        .db
        .with_read_conn({
            let deal_id = deal_id.clone();
            move |conn| db::get_deal_settlement_materialization(conn, &deal_id)
        })
        .await
        .expect("load materialization");
    assert!(remaining_materialization.is_none());
}
