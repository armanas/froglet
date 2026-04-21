//! X402 USDC settlement driver.
//!
//! Implements the x402 HTTP payment protocol for settling USDC payments via
//! an off-chain facilitator service. The protocol flow is:
//!
//! 1. The client signs an EIP-712 `TransferWithAuthorization` and sends it as
//!    a base64url-encoded payment token in the request.
//! 2. `prepare()` verifies the token against the facilitator's `/verify`
//!    endpoint.
//! 3. `commit()` settles the payment against the facilitator's `/settle`
//!    endpoint and returns a receipt with the on-chain transaction hash.
//!
//! Because x402 payments are atomic (they either settle or they don't),
//! `release()` is a no-op.

use crate::{config::X402Config, crypto, state::AppState};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use super::{
    PaymentError, PaymentReceipt, PaymentReservation, PreparePaymentRequest, SettlementDriver,
    SettlementDriverDescriptor, WalletBalanceSnapshot, new_request_id,
};

// ─── Driver ───────────────────────────────────────────────────────────────────

pub(crate) struct X402Driver {
    config: X402Config,
    http_client: reqwest::Client,
}

impl X402Driver {
    pub(crate) fn new(config: X402Config) -> Self {
        Self {
            config,
            http_client: reqwest::Client::new(),
        }
    }
}

// ─── Facilitator API types ────────────────────────────────────────────────────

/// Body sent to both `/verify` and `/settle` facilitator endpoints.
#[derive(Debug, Serialize)]
struct FacilitatorRequest {
    /// The client's signed x402 payment payload (as received in the request
    /// token field). Forwarded verbatim to the facilitator.
    payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct FacilitatorVerifyResponse {
    valid: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FacilitatorSettleResponse {
    success: bool,
    #[serde(default)]
    transaction_hash: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

// ─── SettlementDriver impl ────────────────────────────────────────────────────

impl SettlementDriver for X402Driver {
    fn descriptor(&self, _state: &AppState) -> SettlementDriverDescriptor {
        SettlementDriverDescriptor {
            backend: "x402".to_string(),
            mode: "facilitator".to_string(),
            accepted_payment_methods: vec!["x402_usdc".to_string()],
            capabilities: vec!["usdc_on_base".to_string()],
            reservations: false,
            receipts: true,
        }
    }

    fn wallet_balance<'a>(
        &'a self,
        state: &'a AppState,
    ) -> BoxFuture<'a, Result<WalletBalanceSnapshot, PaymentError>> {
        Box::pin(async move {
            // x402 does not expose server-side wallet balance; the balance is
            // held by the client who signs the EIP-712 authorization.
            let mut snapshot = WalletBalanceSnapshot::from_descriptor(self.descriptor(state));
            snapshot.balance_known = false;
            snapshot.balance_sats = None;
            Ok(snapshot)
        })
    }

    fn prepare<'a>(
        &'a self,
        _state: &'a AppState,
        request: PreparePaymentRequest,
    ) -> BoxFuture<'a, Result<Option<PaymentReservation>, PaymentError>> {
        Box::pin(async move {
            if request.price_sats == 0 {
                return Ok(None);
            }

            let payment = match request.payment {
                Some(p) => p,
                None => {
                    return Err(PaymentError::PaymentRequired {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        accepted_payment_methods: vec!["x402_usdc".to_string()],
                    });
                }
            };

            if payment.kind != "x402_usdc" {
                return Err(PaymentError::UnsupportedKind {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    kind: payment.kind,
                    accepted_payment_methods: vec!["x402_usdc".to_string()],
                });
            }

            // The token is the base64url-encoded signed x402 PaymentPayload.
            // Parse it into a JSON value so we can forward it to the facilitator.
            let payload: serde_json::Value = parse_x402_token(&payment.token).map_err(|err| {
                tracing::warn!("x402 token parse error: {err}");
                PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "x402".to_string(),
                }
            })?;

            let token_amount = parse_x402_amount(&payload).map_err(|err| {
                tracing::warn!("x402 token amount parse error: {err}");
                PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "x402".to_string(),
                }
            })?;
            if token_amount != request.price_sats {
                tracing::warn!(
                    token_amount = %token_amount,
                    required_amount = %request.price_sats,
                    "x402 token amount does not match the requested price"
                );
                return Err(PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "x402".to_string(),
                });
            }

            if let Some(token_network) = parse_x402_network(&payload)
                && token_network != self.config.network
            {
                tracing::warn!(
                    token_network = %token_network,
                    configured_network = %self.config.network,
                    "x402 token network does not match the configured network"
                );
                return Err(PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "x402".to_string(),
                });
            }

            let verify_url = format!("{}/verify", self.config.facilitator_url);
            let body = FacilitatorRequest {
                payload: payload.clone(),
            };

            let response = self
                .http_client
                .post(&verify_url)
                .json(&body)
                .send()
                .await
                .map_err(|err| {
                    tracing::error!("x402 facilitator /verify request failed: {err}");
                    PaymentError::BackendUnavailable {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        backend: "x402".to_string(),
                    }
                })?;

            if !response.status().is_success() {
                tracing::error!(
                    status = %response.status(),
                    "x402 facilitator /verify returned non-2xx"
                );
                return Err(PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "x402".to_string(),
                });
            }

            let verify_response: FacilitatorVerifyResponse =
                response.json().await.map_err(|err| {
                    tracing::error!("x402 facilitator /verify response decode failed: {err}");
                    PaymentError::BackendUnavailable {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        backend: "x402".to_string(),
                    }
                })?;

            if !verify_response.valid {
                tracing::warn!(
                    error = ?verify_response.error,
                    "x402 facilitator rejected payment token"
                );
                return Err(PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "x402".to_string(),
                });
            }

            // Store the raw token in token_hash. Despite the field name, for
            // x402 this holds the raw payment token string so commit() can
            // forward it to /settle. The prepare→commit path is within a single
            // request handler so no cross-request persistence is needed.
            let request_id = request.request_id.unwrap_or_else(new_request_id);
            Ok(Some(PaymentReservation {
                request_id,
                method: "x402_usdc".to_string(),
                service_id: request.service_id,
                amount_sats: request.price_sats,
                token_hash: payment.token,
            }))
        })
    }

    fn commit<'a>(
        &'a self,
        _state: &'a AppState,
        reservation: PaymentReservation,
    ) -> BoxFuture<'a, Result<PaymentReceipt, PaymentError>> {
        Box::pin(async move {
            // The token_hash field holds the raw x402 payment token for this
            // driver (see comment in prepare()).
            let payload: serde_json::Value =
                parse_x402_token(&reservation.token_hash).map_err(|err| {
                    tracing::error!("x402 token re-parse failed in commit: {err}");
                    PaymentError::BackendUnavailable {
                        service_id: reservation.service_id.as_str().to_string(),
                        price_sats: reservation.amount_sats,
                        backend: "x402".to_string(),
                    }
                })?;

            let settle_url = format!("{}/settle", self.config.facilitator_url);
            let body = FacilitatorRequest { payload };

            let response = self
                .http_client
                .post(&settle_url)
                .json(&body)
                .send()
                .await
                .map_err(|err| {
                    tracing::error!("x402 facilitator /settle request failed: {err}");
                    PaymentError::BackendUnavailable {
                        service_id: reservation.service_id.as_str().to_string(),
                        price_sats: reservation.amount_sats,
                        backend: "x402".to_string(),
                    }
                })?;

            if !response.status().is_success() {
                tracing::error!(
                    status = %response.status(),
                    "x402 facilitator /settle returned non-2xx"
                );
                return Err(PaymentError::BackendUnavailable {
                    service_id: reservation.service_id.as_str().to_string(),
                    price_sats: reservation.amount_sats,
                    backend: "x402".to_string(),
                });
            }

            let settle_response: FacilitatorSettleResponse =
                response.json().await.map_err(|err| {
                    tracing::error!("x402 facilitator /settle response decode failed: {err}");
                    PaymentError::BackendUnavailable {
                        service_id: reservation.service_id.as_str().to_string(),
                        price_sats: reservation.amount_sats,
                        backend: "x402".to_string(),
                    }
                })?;

            if !settle_response.success {
                tracing::error!(
                    error = ?settle_response.error,
                    "x402 facilitator /settle reported failure"
                );
                return Err(PaymentError::BackendUnavailable {
                    service_id: reservation.service_id.as_str().to_string(),
                    price_sats: reservation.amount_sats,
                    backend: "x402".to_string(),
                });
            }

            // Compute the token hash for the receipt now that settlement
            // succeeded. This is the sha256 of the raw token string.
            let token_hash = crypto::sha256_hex(reservation.token_hash.as_bytes());

            Ok(reservation.receipt(
                crate::protocol::SettlementStatus::Committed,
                reservation.amount_sats,
                settle_response.transaction_hash.or(Some(token_hash)),
            ))
        })
    }

    fn release<'a>(
        &'a self,
        _state: &'a AppState,
        _reservation: &'a PaymentReservation,
    ) -> BoxFuture<'a, Result<(), String>> {
        // x402 payments are atomic: the EIP-712 authorization either settles
        // on-chain or it doesn't. There is nothing to release.
        Box::pin(async move { Ok(()) })
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Decode an x402 payment token and return its JSON payload.
///
/// The x402 protocol delivers the signed `PaymentPayload` as a base64url-
/// encoded JSON object. We accept both padded and unpadded base64url, and also
/// tolerate tokens that are already raw JSON (for testing convenience).
fn parse_x402_token(token: &str) -> Result<serde_json::Value, String> {
    // Try raw JSON first (facilitates unit tests and development).
    if token.trim_start().starts_with('{') {
        return serde_json::from_str(token).map_err(|err| format!("JSON parse error: {err}"));
    }

    // Decode base64url (with or without padding).
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        token.trim_end_matches('='),
    )
    .map_err(|err| format!("base64url decode error: {err}"))?;

    serde_json::from_slice(&bytes).map_err(|err| format!("JSON parse error after decode: {err}"))
}

fn parse_x402_amount(payload: &serde_json::Value) -> Result<u64, String> {
    let amount = payload
        .get("amount")
        .ok_or_else(|| "missing amount".to_string())?;

    if let Some(amount) = amount.as_u64() {
        return Ok(amount);
    }

    if let Some(amount) = amount.as_str() {
        return amount
            .parse::<u64>()
            .map_err(|err| format!("invalid amount '{amount}': {err}"));
    }

    Err("amount must be a string or integer".to_string())
}

fn parse_x402_network(payload: &serde_json::Value) -> Option<&str> {
    payload.get("network").and_then(|value| value.as_str())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        confidential::ConfidentialConfig,
        config::{
            IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
            PaymentBackend, PricingConfig, StorageConfig, TorSidecarConfig, WasmConfig, X402Config,
        },
        db::DbPool,
        pricing::ServiceId,
        settlement::{PreparePaymentRequest, ProvidedPayment, SettlementRegistry},
        state::{AppState, TransportStatus},
    };
    use axum::{Json, Router, extract::State, routing::post};
    use std::{
        collections::HashMap,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };
    use tokio::net::TcpListener;
    use tokio::sync::{Mutex as TokioMutex, OnceCell, Semaphore};

    static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn make_state() -> AppState {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_dir = std::env::temp_dir().join(format!(
            "froglet-x402-test-{}-{unique}-{counter}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        let db_path = temp_dir.join("node.db");

        let node_config = NodeConfig {
            network_mode: NetworkMode::Clearnet,
            listen_addr: "127.0.0.1:0".to_string(),
            public_base_url: None,
            runtime_listen_addr: "127.0.0.1:0".to_string(),
            runtime_allow_non_loopback: false,
            http_ca_cert_path: None,
            tor: TorSidecarConfig {
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
            payment_backends: vec![PaymentBackend::None],
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
            storage: StorageConfig {
                data_dir: temp_dir.clone(),
                db_path: db_path.clone(),
                identity_dir: temp_dir.join("identity"),
                identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
                nostr_publication_seed_path: temp_dir
                    .join("identity/nostr-publication.secp256k1.seed"),
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
            confidential: ConfidentialConfig {
                policy_path: None,
                policy: None,
                session_ttl_secs: 300,
            },
            marketplace_url: None,
            postgres_mounts: std::collections::BTreeMap::new(),
            session_pool: Default::default(),
        };

        let pool = DbPool::open(&db_path).expect("init test db");
        let events_query_capacity = pool.read_connection_count().max(1);
        let pricing = crate::pricing::PricingTable::from_config(node_config.pricing);
        let identity =
            crate::identity::NodeIdentity::load_or_create(&node_config).expect("test identity");
        let settlement_registry = SettlementRegistry::new(&node_config);

        AppState {
            db: pool,
            transport_status: Arc::new(TokioMutex::new(TransportStatus::from_config(&node_config))),
            wasm_sandbox: Arc::new(crate::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
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
            events_query_semaphore: Arc::new(Semaphore::new(events_query_capacity)),
            lnd_rest_client: None,
            lightning_destination_identity: Arc::new(OnceCell::new()),
            event_batch_writer: None,
            builtin_services: HashMap::new(),
            settlement_registry,
            session_pool: None,
        }
    }

    #[derive(Debug, Default)]
    struct MockX402State {
        calls: TokioMutex<Vec<String>>,
    }

    async fn start_mock_x402() -> (String, Arc<MockX402State>, tokio::task::JoinHandle<()>) {
        async fn verify(
            State(state): State<Arc<MockX402State>>,
            Json(payload): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            state.calls.lock().await.push(format!("verify:{payload}"));
            Json(serde_json::json!({ "valid": true }))
        }

        async fn settle(
            State(state): State<Arc<MockX402State>>,
            Json(payload): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            state.calls.lock().await.push(format!("settle:{payload}"));
            Json(serde_json::json!({
                "success": true,
                "transaction_hash": "0xsettled"
            }))
        }

        let state = Arc::new(MockX402State::default());
        let app = Router::new()
            .route("/verify", post(verify))
            .route("/settle", post(settle))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock x402");
        let address = listener.local_addr().expect("listener address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve mock x402");
        });
        (format!("http://{address}"), state, handle)
    }

    #[test]
    fn x402_driver_descriptor_reports_backend() {
        let driver = X402Driver::new(X402Config {
            facilitator_url: "https://example.invalid".to_string(),
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        });
        let state = make_state();
        let descriptor = driver.descriptor(&state);
        assert_eq!(descriptor.backend, "x402");
        assert_eq!(descriptor.mode, "facilitator");
        assert_eq!(descriptor.accepted_payment_methods, vec!["x402_usdc"]);
        assert_eq!(descriptor.capabilities, vec!["usdc_on_base"]);
        assert!(!descriptor.reservations);
        assert!(descriptor.receipts);
    }

    #[test]
    fn parse_raw_json_token() {
        let token = r#"{"x":"1","sig":"0xdeadbeef"}"#;
        let value = parse_x402_token(token).expect("should parse raw JSON");
        assert_eq!(value["x"], "1");
    }

    #[test]
    fn parse_base64url_encoded_token() {
        let json = r#"{"amount":"100","network":"base"}"#;
        let encoded = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            json.as_bytes(),
        );
        let value = parse_x402_token(&encoded).expect("should decode and parse");
        assert_eq!(value["amount"], "100");
        assert_eq!(value["network"], "base");
    }

    #[test]
    fn parse_base64url_with_padding_is_tolerated() {
        let json = r#"{"k":"v"}"#;
        let padded =
            base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE, json.as_bytes());
        let value = parse_x402_token(&padded).expect("padded base64url should also work");
        assert_eq!(value["k"], "v");
    }

    #[test]
    fn parse_invalid_token_returns_error() {
        let result = parse_x402_token("not-valid-b64-nor-json!!!");
        assert!(result.is_err(), "garbage token should return an error");
    }

    #[test]
    fn parse_amount_accepts_string_and_integer() {
        let from_string = serde_json::json!({ "amount": "100" });
        let from_integer = serde_json::json!({ "amount": 100 });

        assert_eq!(parse_x402_amount(&from_string).unwrap(), 100);
        assert_eq!(parse_x402_amount(&from_integer).unwrap(), 100);
    }

    #[tokio::test]
    async fn x402_driver_prepare_commit_and_release_follow_facilitator_flow() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        });
        let state = make_state();
        let token = r#"{"amount":"100","network":"base","authorization":"signed"}"#.to_string();

        let reservation = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "x402_usdc".to_string(),
                        token: token.clone(),
                    }),
                    request_id: Some("x402-prepare".to_string()),
                },
            )
            .await
            .expect("prepare should succeed")
            .expect("priced flow should produce a reservation");

        assert_eq!(reservation.method, "x402_usdc");
        assert_eq!(reservation.token_hash, token);

        let receipt = driver
            .commit(&state, reservation.clone())
            .await
            .expect("commit should succeed");
        assert_eq!(receipt.method, "x402_usdc");
        assert_eq!(receipt.settlement_reference.as_deref(), Some("0xsettled"));
        assert_eq!(
            receipt.settlement_status,
            crate::protocol::SettlementStatus::Committed
        );

        driver
            .release(&state, &reservation)
            .await
            .expect("release should be a no-op");

        let calls = mock_state.calls.lock().await.clone();
        assert!(
            calls.iter().any(|call| call.starts_with("verify:")),
            "prepare should call facilitator verify"
        );
        assert!(
            calls.iter().any(|call| call.starts_with("settle:")),
            "commit should call facilitator settle"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_rejects_amount_mismatch() {
        let (base_url, _mock_state, handle) = start_mock_x402().await;
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        });
        let state = make_state();
        let token = r#"{"amount":"99","network":"base","authorization":"signed"}"#.to_string();

        let result = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "x402_usdc".to_string(),
                        token,
                    }),
                    request_id: Some("x402-amount-mismatch".to_string()),
                },
            )
            .await;

        assert!(
            result.is_err(),
            "prepare should reject an underfunded token"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_rejects_network_mismatch() {
        let (base_url, _mock_state, handle) = start_mock_x402().await;
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        });
        let state = make_state();
        let token =
            r#"{"amount":"100","network":"base-sepolia","authorization":"signed"}"#.to_string();

        let result = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "x402_usdc".to_string(),
                        token,
                    }),
                    request_id: Some("x402-network-mismatch".to_string()),
                },
            )
            .await;

        assert!(
            result.is_err(),
            "prepare should reject a mismatched network"
        );
        handle.abort();
    }
}
