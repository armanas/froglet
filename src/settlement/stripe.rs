//! Stripe Machine Payments Protocol (MPP) settlement driver.
//!
//! Implements Stripe's Machine Payments Protocol for server-side payment
//! collection using Shared Payment Tokens (SPTs). The protocol flow is:
//!
//! 1. The client obtains a Stripe Shared Payment Token (SPT) and provides it
//!    in `ProvidedPayment.token`.
//! 2. `prepare()` validates the SPT via `GET /v1/shared_payment/granted_tokens/{spt_id}`
//!    and then creates a PaymentIntent in `manual` capture mode so the funds
//!    are held but not yet captured.
//! 3. `commit()` captures the PaymentIntent, causing the actual charge.
//! 4. `release()` cancels the PaymentIntent, releasing the held funds.
//!
//! The PaymentIntent ID is stored in `PaymentReservation.token_hash` so it
//! survives the prepare→commit/release hand-off within a single request.

use crate::{config::StripeConfig, state::AppState};
use futures::future::BoxFuture;

use super::{
    PaymentError, PaymentReceipt, PaymentReservation, PreparePaymentRequest, SettlementDriver,
    SettlementDriverDescriptor, WalletBalanceSnapshot, new_request_id,
};

// ─── Driver ───────────────────────────────────────────────────────────────────

pub(crate) struct StripeDriver {
    api_key: String,
    api_version: String,
    api_base_url: String,
    http_client: reqwest::Client,
}

impl StripeDriver {
    pub(crate) fn new(config: StripeConfig, api_key: String) -> Self {
        Self::with_base_url(config, api_key, "https://api.stripe.com")
    }

    fn with_base_url(config: StripeConfig, api_key: String, api_base_url: &str) -> Self {
        Self {
            api_key,
            api_version: config.api_version,
            api_base_url: api_base_url.trim_end_matches('/').to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.api_base_url, path)
    }

    /// Perform an authenticated GET against the Stripe API and return the
    /// parsed JSON body.
    async fn stripe_get(&self, path: &str) -> Result<serde_json::Value, String> {
        let response = self
            .http_client
            .get(self.api_url(path))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Stripe-Version", &self.api_version)
            .send()
            .await
            .map_err(|e| format!("Stripe GET request failed: {e}"))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Stripe GET response decode failed: {e}"))?;

        if !status.is_success() {
            let message = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(format!("Stripe GET {path} returned {status}: {message}"));
        }

        Ok(body)
    }

    /// Perform an authenticated form-encoded POST against the Stripe API and
    /// return the parsed JSON body.
    ///
    /// We encode `params` as `application/x-www-form-urlencoded` manually
    /// rather than relying on reqwest's `form()` helper (which requires the
    /// `multipart` / `form` feature flag). This keeps the reqwest feature set
    /// minimal while still speaking the Stripe API's native wire format.
    async fn stripe_post_form(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<serde_json::Value, String> {
        let body = encode_form_params(params);

        let response = self
            .http_client
            .post(self.api_url(path))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Stripe-Version", &self.api_version)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Stripe POST request failed: {e}"))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Stripe POST response decode failed: {e}"))?;

        if !status.is_success() {
            let message = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(format!("Stripe POST {path} returned {status}: {message}"));
        }

        Ok(body)
    }
}

// ─── SettlementDriver impl ────────────────────────────────────────────────────

impl SettlementDriver for StripeDriver {
    fn descriptor(&self, _state: &AppState) -> SettlementDriverDescriptor {
        SettlementDriverDescriptor {
            backend: "stripe".to_string(),
            mode: "mpp".to_string(),
            accepted_payment_methods: vec!["stripe_mpp".to_string()],
            capabilities: vec![
                "shared_payment_tokens".to_string(),
                "payment_intents".to_string(),
            ],
            reservations: true,
            receipts: true,
        }
    }

    fn wallet_balance<'a>(
        &'a self,
        state: &'a AppState,
    ) -> BoxFuture<'a, Result<WalletBalanceSnapshot, PaymentError>> {
        Box::pin(async move {
            // Stripe MPP does not expose a server-side wallet balance; funds
            // flow directly from the client's payment method through Stripe.
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
                        accepted_payment_methods: vec!["stripe_mpp".to_string()],
                    });
                }
            };

            if payment.kind != "stripe_mpp" {
                return Err(PaymentError::UnsupportedKind {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    kind: payment.kind,
                    accepted_payment_methods: vec!["stripe_mpp".to_string()],
                });
            }

            // The token field holds the raw Shared Payment Token ID,
            // e.g. "spt_1RgaZc...".
            let spt_id = &payment.token;

            // Step 1: Validate the SPT by fetching its details from the API.
            let spt_path = format!("/v1/shared_payment/granted_tokens/{spt_id}");
            let spt_data = self.stripe_get(&spt_path).await.map_err(|err| {
                tracing::error!("Stripe SPT validation failed for {spt_id}: {err}");
                PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "stripe".to_string(),
                }
            })?;

            // Verify the SPT is not expired.
            if let Some(expires_at) = spt_data.get("expires_at").and_then(|v| v.as_i64()) {
                let now = super::current_unix_timestamp();
                if expires_at < now {
                    tracing::warn!(
                        spt_id = %spt_id,
                        expires_at = %expires_at,
                        now = %now,
                        "Stripe SPT has expired"
                    );
                    return Err(PaymentError::BackendUnavailable {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        backend: "stripe".to_string(),
                    });
                }
            }

            // Verify the SPT covers at least the requested amount. The amount
            // on the SPT is in cents (same unit as price_sats for now).
            if spt_data
                .get("maximum_amount")
                .and_then(|v| v.as_u64())
                .is_some_and(|max_amount| max_amount < request.price_sats)
            {
                let max_amount = spt_data
                    .get("maximum_amount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                tracing::warn!(
                    spt_id = %spt_id,
                    spt_max = %max_amount,
                    required = %request.price_sats,
                    "Stripe SPT maximum_amount is less than required price"
                );
                return Err(PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "stripe".to_string(),
                });
            }

            // Step 2: Create a PaymentIntent in manual-capture mode so funds
            // are authorised but not yet captured. We capture on commit().
            let amount_str = request.price_sats.to_string();
            let params: &[(&str, &str)] = &[
                ("amount", &amount_str),
                ("currency", "usd"),
                ("payment_method_data[type]", "stripe_payment_token"),
                ("payment_method_data[stripe_payment_token]", spt_id),
                ("confirm", "true"),
                ("capture_method", "manual"),
            ];

            let pi_data = self
                .stripe_post_form("/v1/payment_intents", params)
                .await
                .map_err(|err| {
                    tracing::error!(
                        spt_id = %spt_id,
                        "Stripe PaymentIntent creation failed: {err}"
                    );
                    PaymentError::BackendUnavailable {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        backend: "stripe".to_string(),
                    }
                })?;

            let pi_id = pi_data
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    tracing::error!("Stripe PaymentIntent response missing 'id' field");
                    PaymentError::BackendUnavailable {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        backend: "stripe".to_string(),
                    }
                })?
                .to_string();

            // Store the PaymentIntent ID in token_hash so commit/release can
            // reference it. The field name is inherited from the shared struct;
            // for this driver it holds an opaque Stripe resource ID, not a hash.
            let request_id = request.request_id.unwrap_or_else(new_request_id);
            Ok(Some(PaymentReservation {
                request_id,
                method: "stripe_mpp".to_string(),
                service_id: request.service_id,
                amount_sats: request.price_sats,
                token_hash: pi_id,
            }))
        })
    }

    fn commit<'a>(
        &'a self,
        _state: &'a AppState,
        reservation: PaymentReservation,
    ) -> BoxFuture<'a, Result<PaymentReceipt, PaymentError>> {
        Box::pin(async move {
            // token_hash holds the PaymentIntent ID for this driver.
            let pi_id = &reservation.token_hash;
            let capture_path = format!("/v1/payment_intents/{pi_id}/capture");

            self.stripe_post_form(&capture_path, &[])
                .await
                .map_err(|err| {
                    tracing::error!(pi_id = %pi_id, "Stripe PaymentIntent capture failed: {err}");
                    PaymentError::BackendUnavailable {
                        service_id: reservation.service_id.as_str().to_string(),
                        price_sats: reservation.amount_sats,
                        backend: "stripe".to_string(),
                    }
                })?;

            Ok(reservation.receipt(
                crate::protocol::SettlementStatus::Committed,
                reservation.amount_sats,
                Some(pi_id.clone()),
            ))
        })
    }

    fn release<'a>(
        &'a self,
        _state: &'a AppState,
        reservation: &'a PaymentReservation,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let pi_id = &reservation.token_hash;
            let cancel_path = format!("/v1/payment_intents/{pi_id}/cancel");

            self.stripe_post_form(&cancel_path, &[])
                .await
                .map_err(|err| {
                    tracing::error!(pi_id = %pi_id, "Stripe PaymentIntent cancel failed: {err}");
                    format!("Stripe release failed for {pi_id}: {err}")
                })?;

            Ok(())
        })
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Encode key-value pairs as `application/x-www-form-urlencoded`.
///
/// Both keys and values are percent-encoded using RFC 3986 unreserved
/// characters, then `+` is substituted for encoded spaces to match the
/// HTML form-encoding convention expected by the Stripe API.
fn encode_form_params(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        confidential::ConfidentialConfig,
        config::{
            IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
            PaymentBackend, PricingConfig, StorageConfig, StripeConfig, TorSidecarConfig,
            WasmConfig,
        },
        db::DbPool,
        pricing::ServiceId,
        settlement::{PreparePaymentRequest, ProvidedPayment, SettlementRegistry},
        state::{AppState, TransportStatus},
    };
    use axum::{
        Json, Router,
        extract::{Path, State},
        routing::{get, post},
    };
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

    fn make_driver() -> StripeDriver {
        StripeDriver::new(
            StripeConfig {
                api_version: "2024-06-20".to_string(),
            },
            "stripe_test_secret_placeholder".to_string(),
        )
    }

    #[derive(Debug, Default)]
    struct MockStripeState {
        calls: TokioMutex<Vec<String>>,
    }

    async fn start_mock_stripe() -> (String, Arc<MockStripeState>, tokio::task::JoinHandle<()>) {
        async fn get_granted_token(
            State(state): State<Arc<MockStripeState>>,
            Path(token_id): Path<String>,
        ) -> Json<serde_json::Value> {
            state
                .calls
                .lock()
                .await
                .push(format!("GET:/v1/shared_payment/granted_tokens/{token_id}"));
            Json(serde_json::json!({
                "id": token_id,
                "expires_at": super::super::current_unix_timestamp() + 600,
                "maximum_amount": 50_000
            }))
        }

        async fn create_payment_intent(
            State(state): State<Arc<MockStripeState>>,
            body: String,
        ) -> Json<serde_json::Value> {
            state
                .calls
                .lock()
                .await
                .push(format!("POST:/v1/payment_intents:{body}"));
            Json(serde_json::json!({
                "id": "pi_test_123",
                "status": "requires_capture"
            }))
        }

        async fn capture_payment_intent(
            State(state): State<Arc<MockStripeState>>,
            Path(intent_id): Path<String>,
        ) -> Json<serde_json::Value> {
            state
                .calls
                .lock()
                .await
                .push(format!("POST:/v1/payment_intents/{intent_id}/capture"));
            Json(serde_json::json!({
                "id": intent_id,
                "status": "succeeded"
            }))
        }

        async fn cancel_payment_intent(
            State(state): State<Arc<MockStripeState>>,
            Path(intent_id): Path<String>,
        ) -> Json<serde_json::Value> {
            state
                .calls
                .lock()
                .await
                .push(format!("POST:/v1/payment_intents/{intent_id}/cancel"));
            Json(serde_json::json!({
                "id": intent_id,
                "status": "canceled"
            }))
        }

        let state = Arc::new(MockStripeState::default());
        let app = Router::new()
            .route(
                "/v1/shared_payment/granted_tokens/:token_id",
                get(get_granted_token),
            )
            .route("/v1/payment_intents", post(create_payment_intent))
            .route(
                "/v1/payment_intents/:intent_id/capture",
                post(capture_payment_intent),
            )
            .route(
                "/v1/payment_intents/:intent_id/cancel",
                post(cancel_payment_intent),
            )
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock stripe");
        let address = listener.local_addr().expect("listener address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve mock stripe");
        });
        (format!("http://{address}"), state, handle)
    }

    /// Build a minimal in-memory `AppState` suitable for unit tests.
    ///
    /// This mirrors the pattern used in `tests/payments_and_discovery.rs`.
    /// The `StripeDriver` ignores `&AppState` in all method bodies, so we only
    /// need a structurally valid instance — not a fully-operational node.
    fn make_state() -> AppState {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_dir = std::env::temp_dir().join(format!(
            "froglet-stripe-test-{}-{unique}-{counter}",
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
            hosted_trial_origin_secret: None,
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

    #[test]
    fn stripe_driver_descriptor_reports_correct_backend() {
        let driver = make_driver();
        let state = make_state();
        let desc = driver.descriptor(&state);
        assert_eq!(desc.backend, "stripe");
        assert_eq!(desc.mode, "mpp");
        assert_eq!(desc.accepted_payment_methods, vec!["stripe_mpp"]);
        assert!(
            desc.capabilities
                .contains(&"shared_payment_tokens".to_string()),
            "capabilities should include shared_payment_tokens"
        );
        assert!(
            desc.capabilities.contains(&"payment_intents".to_string()),
            "capabilities should include payment_intents"
        );
        assert!(
            desc.reservations,
            "stripe driver should support reservations"
        );
        assert!(desc.receipts, "stripe driver should support receipts");
    }

    #[tokio::test]
    async fn stripe_driver_prepare_returns_none_for_free_service() {
        let driver = make_driver();
        let state = make_state();
        let request = PreparePaymentRequest {
            service_id: ServiceId::EventsQuery,
            price_sats: 0,
            payment: None,
            request_id: None,
        };
        let result = driver.prepare(&state, request).await;
        assert!(result.is_ok(), "free-service prepare should succeed");
        assert!(
            result.unwrap().is_none(),
            "free-service prepare should return None (no reservation)"
        );
    }

    #[tokio::test]
    async fn stripe_driver_prepare_requires_payment_for_priced_service() {
        let driver = make_driver();
        let state = make_state();
        let request = PreparePaymentRequest {
            service_id: ServiceId::EventsQuery,
            price_sats: 100,
            payment: None,
            request_id: None,
        };
        let result = driver.prepare(&state, request).await;
        assert!(
            result.is_err(),
            "priced service without payment should fail"
        );
        assert!(
            matches!(result.unwrap_err(), PaymentError::PaymentRequired { .. }),
            "error should be PaymentRequired"
        );
    }

    #[tokio::test]
    async fn stripe_driver_prepare_rejects_unsupported_payment_kind() {
        let driver = make_driver();
        let state = make_state();
        let request = PreparePaymentRequest {
            service_id: ServiceId::EventsQuery,
            price_sats: 100,
            payment: Some(ProvidedPayment {
                kind: "lightning".to_string(),
                token: "lnbc100...".to_string(),
            }),
            request_id: None,
        };
        let result = driver.prepare(&state, request).await;
        assert!(result.is_err(), "wrong payment kind should be rejected");
        assert!(
            matches!(result.unwrap_err(), PaymentError::UnsupportedKind { .. }),
            "error should be UnsupportedKind"
        );
    }

    #[tokio::test]
    async fn stripe_driver_prepare_and_commit_uses_payment_intents() {
        let (base_url, mock_state, handle) = start_mock_stripe().await;
        let driver = StripeDriver::with_base_url(
            StripeConfig {
                api_version: "2024-06-20".to_string(),
            },
            "stripe_test_secret_placeholder".to_string(),
            &base_url,
        );
        let state = make_state();
        let reservation = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "stripe_mpp".to_string(),
                        token: "spt_test_123".to_string(),
                    }),
                    request_id: Some("stripe-prepare".to_string()),
                },
            )
            .await
            .expect("prepare should succeed")
            .expect("priced flow should reserve");

        assert_eq!(reservation.method, "stripe_mpp");
        assert_eq!(reservation.token_hash, "pi_test_123");

        let receipt = driver
            .commit(&state, reservation)
            .await
            .expect("commit should succeed");
        assert_eq!(receipt.method, "stripe_mpp");
        assert_eq!(
            receipt.settlement_status,
            crate::protocol::SettlementStatus::Committed
        );
        assert_eq!(receipt.settlement_reference.as_deref(), Some("pi_test_123"));

        let calls = mock_state.calls.lock().await.clone();
        assert!(
            calls
                .iter()
                .any(|call| { call == "GET:/v1/shared_payment/granted_tokens/spt_test_123" }),
            "prepare should validate the shared payment token"
        );
        assert!(
            calls.iter().any(|call| {
                call.contains("POST:/v1/payment_intents:amount=100")
                    && call.contains("capture_method=manual")
            }),
            "prepare should create a manual-capture payment intent"
        );
        assert!(
            calls
                .iter()
                .any(|call| call == "POST:/v1/payment_intents/pi_test_123/capture"),
            "commit should capture the payment intent"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn stripe_driver_release_cancels_payment_intent() {
        let (base_url, mock_state, handle) = start_mock_stripe().await;
        let driver = StripeDriver::with_base_url(
            StripeConfig {
                api_version: "2024-06-20".to_string(),
            },
            "stripe_test_secret_placeholder".to_string(),
            &base_url,
        );
        let state = make_state();
        let reservation = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "stripe_mpp".to_string(),
                        token: "spt_release_456".to_string(),
                    }),
                    request_id: Some("stripe-release".to_string()),
                },
            )
            .await
            .expect("prepare should succeed")
            .expect("priced flow should reserve");

        driver
            .release(&state, &reservation)
            .await
            .expect("release should succeed");

        let calls = mock_state.calls.lock().await.clone();
        assert!(
            calls
                .iter()
                .any(|call| call == "POST:/v1/payment_intents/pi_test_123/cancel"),
            "release should cancel the payment intent"
        );
        handle.abort();
    }
}
