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

const MAX_FACILITATOR_RESPONSE_BYTES: usize = 256 * 1024;
const X402_VERSION: u8 = 2;
const X402_SCHEME: &str = "exact";
const BASE_MAINNET_NETWORK: &str = "eip155:8453";
const BASE_MAINNET_USDC_ASSET: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
const X402_MAX_TIMEOUT_SECONDS: u64 = 60;

// ─── Driver ───────────────────────────────────────────────────────────────────

pub(crate) struct X402Driver {
    config: X402Config,
    http_client: reqwest::Client,
}

impl X402Driver {
    pub(crate) fn new(mut config: X402Config) -> Result<Self, String> {
        canonical_x402_network(&config.network)?;
        let facilitator_url = reqwest::Url::parse(config.facilitator_url.trim())
            .map_err(|error| format!("invalid x402 facilitator URL: {error}"))?;
        if !facilitator_url.username().is_empty() || facilitator_url.password().is_some() {
            return Err("x402 facilitator URL must not contain credentials".to_string());
        }
        let is_loopback_http = facilitator_url.scheme() == "http"
            && facilitator_url
                .host_str()
                .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                .is_some_and(|address| address.is_loopback());
        if facilitator_url.scheme() != "https" && !is_loopback_http {
            return Err(
                "x402 facilitator URL must use HTTPS (loopback HTTP is test-only)".to_string(),
            );
        }
        if facilitator_url.host_str() == Some("api.cdp.coinbase.com") {
            return Err(
                "the CDP x402 facilitator requires CDP API authentication, which Froglet does \
                 not synthesize from an unauthenticated URL; configure an authenticated x402 v2 \
                 facilitator or an operator-managed authenticated proxy"
                    .to_string(),
            );
        }
        config.facilitator_url = facilitator_url.as_str().trim_end_matches('/').to_string();
        let http_client = crate::tls::reqwest_client_builder()
            .build()
            .map_err(|error| format!("failed to build x402 HTTP client: {error}"))?;
        Ok(Self {
            config,
            http_client,
        })
    }
}

// ─── Facilitator API types ────────────────────────────────────────────────────

/// Official x402 v2 body sent to both facilitator endpoints.
#[derive(Debug, Serialize)]
struct FacilitatorRequest {
    #[serde(rename = "x402Version")]
    x402_version: u8,
    #[serde(rename = "paymentPayload")]
    payment_payload: serde_json::Value,
    #[serde(rename = "paymentRequirements")]
    payment_requirements: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct FacilitatorVerifyResponse {
    #[serde(rename = "isValid")]
    is_valid: bool,
    #[serde(rename = "invalidReason", default)]
    invalid_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FacilitatorSettleResponse {
    success: bool,
    transaction: String,
    network: String,
    #[serde(rename = "errorReason", default)]
    error_reason: Option<String>,
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
                Some(ref p) => p,
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
                    kind: payment.kind.clone(),
                    accepted_payment_methods: vec!["x402_usdc".to_string()],
                });
            }

            // The token is the base64url-encoded signed x402 PaymentPayload.
            // Parse it into a JSON value so we can forward it to the facilitator.
            let payload: serde_json::Value = parse_x402_token(&payment.token).map_err(|err| {
                tracing::warn!("x402 token parse error: {err}");
                PaymentError::InvalidPayment {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    kind: "x402_usdc".to_string(),
                    reason: "malformed x402 v2 payment payload",
                }
            })?;

            let payment_requirements =
                validate_x402_payment_binding(&payload, &self.config, &request).map_err(|err| {
                    tracing::warn!("x402 token binding validation failed: {err}");
                    PaymentError::InvalidPayment {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        kind: "x402_usdc".to_string(),
                        reason: "x402 payment does not match requirements",
                    }
                })?;

            let verify_url = format!("{}/verify", self.config.facilitator_url);
            let body = FacilitatorRequest {
                x402_version: X402_VERSION,
                payment_payload: payload.clone(),
                payment_requirements,
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
                let status = response.status();
                tracing::error!(
                    status = %status,
                    "x402 facilitator /verify returned non-2xx"
                );
                if facilitator_status_is_payment_error(status) {
                    return Err(PaymentError::InvalidPayment {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        kind: "x402_usdc".to_string(),
                        reason: "x402 facilitator rejected payment",
                    });
                }
                return Err(PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "x402".to_string(),
                });
            }

            let verify_response: FacilitatorVerifyResponse =
                crate::http_body::read_json_response_limited(
                    response,
                    MAX_FACILITATOR_RESPONSE_BYTES,
                    "x402 facilitator /verify response",
                )
                .await
                .map_err(|err| {
                    tracing::error!("x402 facilitator /verify response decode failed: {err}");
                    PaymentError::BackendUnavailable {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        backend: "x402".to_string(),
                    }
                })?;

            if !verify_response.is_valid {
                tracing::warn!(
                    error = ?verify_response.invalid_reason,
                    "x402 facilitator rejected payment token"
                );
                return Err(PaymentError::InvalidPayment {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    kind: "x402_usdc".to_string(),
                    reason: "x402 facilitator rejected payment",
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
                token_hash: payment.token.clone(),
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

            let payment_requirements = validate_x402_payment_binding(
                &payload,
                &self.config,
                &PreparePaymentRequest {
                    service_id: reservation.service_id,
                    price_sats: reservation.amount_sats,
                    payment: None,
                    request_id: Some(reservation.request_id.clone()),
                },
            )
            .map_err(|err| {
                tracing::error!("x402 reservation binding validation failed in commit: {err}");
                PaymentError::BackendUnavailable {
                    service_id: reservation.service_id.as_str().to_string(),
                    price_sats: reservation.amount_sats,
                    backend: "x402".to_string(),
                }
            })?;

            let settle_url = format!("{}/settle", self.config.facilitator_url);
            let body = FacilitatorRequest {
                x402_version: X402_VERSION,
                payment_payload: payload,
                payment_requirements,
            };

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
                let status = response.status();
                tracing::error!(
                    status = %status,
                    "x402 facilitator /settle returned non-2xx"
                );
                if facilitator_status_is_payment_error(status) {
                    return Err(PaymentError::InvalidPayment {
                        service_id: reservation.service_id.as_str().to_string(),
                        price_sats: reservation.amount_sats,
                        kind: "x402_usdc".to_string(),
                        reason: "x402 facilitator rejected settlement",
                    });
                }
                return Err(PaymentError::BackendUnavailable {
                    service_id: reservation.service_id.as_str().to_string(),
                    price_sats: reservation.amount_sats,
                    backend: "x402".to_string(),
                });
            }

            let settle_response: FacilitatorSettleResponse =
                crate::http_body::read_json_response_limited(
                    response,
                    MAX_FACILITATOR_RESPONSE_BYTES,
                    "x402 facilitator /settle response",
                )
                .await
                .map_err(|err| {
                    tracing::error!("x402 facilitator /settle response decode failed: {err}");
                    PaymentError::BackendUnavailable {
                        service_id: reservation.service_id.as_str().to_string(),
                        price_sats: reservation.amount_sats,
                        backend: "x402".to_string(),
                    }
                })?;

            if !settle_response.success {
                tracing::error!(
                    error = ?settle_response.error_reason,
                    "x402 facilitator /settle reported failure"
                );
                return Err(PaymentError::InvalidPayment {
                    service_id: reservation.service_id.as_str().to_string(),
                    price_sats: reservation.amount_sats,
                    kind: "x402_usdc".to_string(),
                    reason: "x402 facilitator rejected settlement",
                });
            }
            if settle_response.transaction.is_empty() {
                tracing::error!("x402 facilitator /settle reported success without a transaction");
                return Err(PaymentError::BackendUnavailable {
                    service_id: reservation.service_id.as_str().to_string(),
                    price_sats: reservation.amount_sats,
                    backend: "x402".to_string(),
                });
            }

            // Compute the token hash for the receipt now that settlement
            // succeeded. This is the sha256 of the raw token string.
            let token_hash = crypto::sha256_hex(reservation.token_hash.as_bytes());

            let expected_network = canonical_x402_network(&self.config.network).map_err(|err| {
                tracing::error!("x402 configured network became invalid: {err}");
                PaymentError::BackendUnavailable {
                    service_id: reservation.service_id.as_str().to_string(),
                    price_sats: reservation.amount_sats,
                    backend: "x402".to_string(),
                }
            })?;
            if settle_response.network != expected_network {
                tracing::error!(
                    network = %settle_response.network,
                    "x402 facilitator /settle returned the wrong network"
                );
                return Err(PaymentError::BackendUnavailable {
                    service_id: reservation.service_id.as_str().to_string(),
                    price_sats: reservation.amount_sats,
                    backend: "x402".to_string(),
                });
            }

            let mut receipt_reservation = reservation;
            receipt_reservation.token_hash = token_hash.clone();
            Ok(receipt_reservation.receipt(
                crate::protocol::SettlementStatus::Committed,
                receipt_reservation.amount_sats,
                Some(settle_response.transaction),
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

fn facilitator_status_is_payment_error(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 400 | 402 | 409 | 422)
}

fn canonical_x402_network(network: &str) -> Result<&'static str, String> {
    match network {
        "base" | BASE_MAINNET_NETWORK => Ok(BASE_MAINNET_NETWORK),
        _ => Err(format!(
            "unsupported x402 network {network}; only Base mainnet is supported"
        )),
    }
}

fn required_x402_payment_requirements(
    config: &X402Config,
    request: &PreparePaymentRequest,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "scheme": X402_SCHEME,
        "network": canonical_x402_network(&config.network)?,
        "amount": request.price_sats.to_string(),
        "asset": BASE_MAINNET_USDC_ASSET,
        "payTo": config.wallet_address,
        "maxTimeoutSeconds": X402_MAX_TIMEOUT_SECONDS,
        "extra": {
            "name": "USDC",
            "version": "2"
        }
    }))
}

fn validate_x402_payment_binding(
    payload: &serde_json::Value,
    config: &X402Config,
    request: &PreparePaymentRequest,
) -> Result<serde_json::Value, String> {
    if payload
        .get("x402Version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(X402_VERSION))
    {
        return Err("x402Version must be 2".to_string());
    }

    let required = required_x402_payment_requirements(config, request)?;
    let accepted = payload
        .get("accepted")
        .ok_or_else(|| "missing accepted payment requirements".to_string())?;
    if accepted != &required {
        return Err("accepted payment requirements do not match Froglet requirements".to_string());
    }

    let resource = payload
        .pointer("/resource/url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing resource.url".to_string())?;
    if resource != request.service_id.as_str() {
        return Err(format!(
            "resource {resource} does not match service {}",
            request.service_id.as_str()
        ));
    }

    let authorization = payload
        .pointer("/payload/authorization")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "missing payload.authorization".to_string())?;
    let authorized_amount = authorization
        .get("value")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing payload.authorization.value".to_string())?;
    if authorized_amount != request.price_sats.to_string() {
        return Err("signed authorization amount does not match required amount".to_string());
    }

    let authorized_wallet = authorization
        .get("to")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing payload.authorization.to".to_string())?;
    if !authorized_wallet.eq_ignore_ascii_case(&config.wallet_address) {
        return Err("signed authorization recipient does not match configured wallet".to_string());
    }

    if payload
        .pointer("/payload/signature")
        .and_then(serde_json::Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("missing payload.signature".to_string());
    }

    Ok(required)
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
    use axum::{
        Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post,
    };
    use std::{
        collections::HashMap,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
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
            relay: crate::config::RelayConfig::default(),
            identity: IdentityConfig {
                auto_generate: true,
            },
            pricing: PricingConfig {
                events_query: 10,
                execute_wasm: 30,
            },
            payment_backends: vec![PaymentBackend::None],
            execution_timeout_secs: 10,
            process_limits: Default::default(),
            public_quota: Default::default(),
            lightning: LightningConfig {
                mode: LightningMode::Mock,
                destination_identity: None,
                base_invoice_expiry_secs: 300,
                success_hold_expiry_secs: 300,
                min_final_cltv_expiry: 18,
                sync_interval_ms: 1_000,
                lnd_rest: None,
                phoenixd: None,
            },
            x402: None,
            stripe: None,
            buyer_stripe: None,
            buyer_phoenixd: None,
            requester_spend: Default::default(),
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
            gpu: Default::default(),
            confidential: ConfidentialConfig {
                policy_path: None,
                policy: None,
                session_ttl_secs: 300,
            },
            marketplace_url: None,
            marketplace_allow_local: false,
            provider_artifact_root: None,
            postgres_mounts: std::collections::BTreeMap::new(),
            session_pool: Default::default(),
            hosted_trial_origin_secret: None,
        };

        let pool = DbPool::open(&db_path).expect("init test db");
        let events_query_capacity = pool.read_connection_count().max(1);
        let pricing = crate::pricing::PricingTable::from_config(node_config.pricing);
        let identity =
            crate::identity::NodeIdentity::load_or_create(&node_config).expect("test identity");
        let settlement_registry =
            SettlementRegistry::new(&node_config).expect("settlement registry");

        AppState {
            db: pool,
            transport_status: Arc::new(TokioMutex::new(TransportStatus::from_config(&node_config))),
            wasm_sandbox: Arc::new(crate::sandbox::WasmSandbox::from_env().expect("wasm sandbox")),
            config: node_config,
            identity: Arc::new(identity),
            pricing,
            http_client: crate::tls::reqwest_client_builder()
                .build()
                .expect("test HTTP client configuration must be valid"),
            wasm_host: None,
            confidential_policy: None,
            runtime_auth_token: "test-token".to_string(),
            runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
            consumer_control_auth_token: "test-consumer-token".to_string(),
            consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
            provider_control_auth_token: "test-provider-token".to_string(),
            provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
            events_query_semaphore: Arc::new(Semaphore::new(events_query_capacity)),
            process_execution_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
            native_data_query_handlers: crate::builtins::DataQueryHandlerCache::default(),
            native_data_publication_lock: TokioMutex::const_new(()),
            hosted_trial_deal_quota: None,
            hosted_trial_session_quota: Arc::new(crate::public_quota::IdentityQuota::new(
                1000,
                std::time::Duration::from_secs(60),
            )),
            event_publish_quota: Arc::new(crate::public_quota::IdentityQuota::new(
                1000,
                std::time::Duration::from_secs(60),
            )),
            quote_create_quota: Arc::new(crate::public_quota::IdentityQuota::new(
                1000,
                std::time::Duration::from_secs(60),
            )),
            confidential_session_quota: Arc::new(crate::public_quota::IdentityQuota::new(
                1000,
                std::time::Duration::from_secs(60),
            )),
            lnd_rest_client: None,
            phoenixd_client: None,
            lightning_wallet: None,
            lightning_destination_identity: Arc::new(OnceCell::new()),
            event_batch_writer: None,
            builtin_services: HashMap::new(),
            settlement_registry,
            session_pool: None,
        }
    }

    #[derive(Debug, Default)]
    struct MockX402State {
        calls: TokioMutex<Vec<(String, serde_json::Value)>>,
        oversized_verify: AtomicBool,
        reject_verify: AtomicBool,
        invalid_verify_status: AtomicBool,
        reject_settle: AtomicBool,
        empty_settle_transaction: AtomicBool,
    }

    async fn start_mock_x402() -> (String, Arc<MockX402State>, tokio::task::JoinHandle<()>) {
        async fn verify(
            State(state): State<Arc<MockX402State>>,
            Json(payload): Json<serde_json::Value>,
        ) -> axum::response::Response {
            state
                .calls
                .lock()
                .await
                .push(("verify".to_string(), payload));
            if state.oversized_verify.load(Ordering::Relaxed) {
                return "x"
                    .repeat(MAX_FACILITATOR_RESPONSE_BYTES + 1)
                    .into_response();
            }
            if state.reject_verify.load(Ordering::Relaxed) {
                return Json(serde_json::json!({
                    "isValid": false,
                    "invalidReason": "invalid_exact_evm_payload_signature",
                    "payer": "0xpayer"
                }))
                .into_response();
            }
            if state.invalid_verify_status.load(Ordering::Relaxed) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "isValid": false,
                        "invalidReason": "invalid_payload"
                    })),
                )
                    .into_response();
            }
            Json(serde_json::json!({
                "isValid": true,
                "payer": "0xpayer"
            }))
            .into_response()
        }

        async fn settle(
            State(state): State<Arc<MockX402State>>,
            Json(payload): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            state
                .calls
                .lock()
                .await
                .push(("settle".to_string(), payload));
            if state.reject_settle.load(Ordering::Relaxed) {
                return Json(serde_json::json!({
                    "success": false,
                    "errorReason": "insufficient_funds",
                    "transaction": "",
                    "network": "eip155:8453",
                    "payer": "0xpayer"
                }));
            }
            let transaction = if state.empty_settle_transaction.load(Ordering::Relaxed) {
                ""
            } else {
                "0xsettled"
            };
            Json(serde_json::json!({
                "success": true,
                "transaction": transaction,
                "network": "eip155:8453",
                "payer": "0xpayer"
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

    fn bound_token(amount: u64, network: &str, wallet: &str, resource: &str) -> String {
        serde_json::json!({
            "x402Version": 2,
            "resource": {
                "url": resource
            },
            "accepted": {
                "scheme": "exact",
                "network": network,
                "amount": amount.to_string(),
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "payTo": wallet,
                "maxTimeoutSeconds": 60,
                "extra": {
                    "name": "USDC",
                    "version": "2"
                }
            },
            "payload": {
                "signature": "0xsigned",
                "authorization": {
                    "from": "0xpayer",
                    "to": wallet,
                    "value": amount.to_string(),
                    "validAfter": "0",
                    "validBefore": "4102444800",
                    "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
                }
            },
            "extensions": {}
        })
        .to_string()
    }

    #[test]
    fn x402_driver_descriptor_reports_backend() {
        let driver = X402Driver::new(X402Config {
            facilitator_url: "https://example.invalid".to_string(),
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
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
    fn x402_driver_rejects_a_network_outside_its_base_v2_profile() {
        let result = X402Driver::new(X402Config {
            facilitator_url: "https://example.invalid".to_string(),
            wallet_address: "0xabc123".to_string(),
            network: "base-sepolia".to_string(),
        });

        assert!(matches!(result, Err(error) if error.contains("only Base mainnet is supported")));
    }

    #[test]
    fn x402_driver_rejects_unauthenticated_cdp_mainnet_configuration() {
        let result = X402Driver::new(X402Config {
            facilitator_url: "https://api.cdp.coinbase.com/platform/v2/x402".to_string(),
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        });

        assert!(matches!(result, Err(error) if error.contains("requires CDP API authentication")));
    }

    #[test]
    fn compose_files_do_not_restore_an_unauthenticated_cdp_default() {
        for compose in [
            include_str!("../../compose.yaml"),
            include_str!("../../compose.provider.yaml"),
        ] {
            assert!(!compose.contains("api.cdp.coinbase.com/platform/v2/x402"));
            assert!(compose.contains("FROGLET_X402_FACILITATOR_URL"));
        }
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

    #[tokio::test]
    async fn x402_driver_prepare_reports_a_malformed_token_as_invalid_payment() {
        let driver = X402Driver::new(X402Config {
            facilitator_url: "https://example.invalid".to_string(),
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();

        let result = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "x402_usdc".to_string(),
                        token: "not-valid-b64-nor-json!!!".to_string(),
                    }),
                    request_id: Some("x402-malformed-token".to_string()),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(PaymentError::InvalidPayment {
                kind,
                reason: "malformed x402 v2 payment payload",
                ..
            }) if kind == "x402_usdc"
        ));
    }

    #[tokio::test]
    async fn x402_driver_prepare_commit_and_release_follow_facilitator_flow() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let token = bound_token(
            100,
            "eip155:8453",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        );

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
        assert_eq!(receipt.token_hash, crypto::sha256_hex(token.as_bytes()));
        assert_ne!(
            receipt.token_hash, token,
            "receipt must not expose raw token"
        );
        assert_eq!(
            receipt.settlement_status,
            crate::protocol::SettlementStatus::Committed
        );

        driver
            .release(&state, &reservation)
            .await
            .expect("release should be a no-op");

        let calls = mock_state.calls.lock().await.clone();
        let expected_payload: serde_json::Value =
            serde_json::from_str(&token).expect("test payment payload");
        let expected_requirements = expected_payload["accepted"].clone();
        let expected_facilitator_body = serde_json::json!({
            "x402Version": 2,
            "paymentPayload": expected_payload,
            "paymentRequirements": expected_requirements,
        });
        assert_eq!(
            calls,
            vec![
                ("verify".to_string(), expected_facilitator_body.clone()),
                ("settle".to_string(), expected_facilitator_body),
            ]
        );
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_rejects_signed_authorization_amount_mismatch() {
        let (base_url, _mock_state, handle) = start_mock_x402().await;
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let mut payload: serde_json::Value = serde_json::from_str(&bound_token(
            100,
            "eip155:8453",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        ))
        .expect("test payment payload");
        payload["payload"]["authorization"]["value"] = serde_json::json!("99");

        let result = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "x402_usdc".to_string(),
                        token: payload.to_string(),
                    }),
                    request_id: Some("x402-amount-mismatch".to_string()),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(PaymentError::InvalidPayment {
                kind,
                reason: "x402 payment does not match requirements",
                ..
            }) if kind == "x402_usdc"
        ));
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_rejects_unsigned_shadow_fields() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let mut payload: serde_json::Value = serde_json::from_str(&bound_token(
            99,
            "eip155:8453",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        ))
        .expect("test payment payload");
        payload["amount"] = serde_json::json!("100");
        payload["network"] = serde_json::json!("base");
        payload["payTo"] = serde_json::json!("0xabc123");
        payload["resource"] = serde_json::json!(ServiceId::EventsQuery.as_str());

        let result = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "x402_usdc".to_string(),
                        token: payload.to_string(),
                    }),
                    request_id: Some("x402-shadow-fields".to_string()),
                },
            )
            .await;

        assert!(matches!(result, Err(PaymentError::InvalidPayment { .. })));
        assert!(
            mock_state.calls.lock().await.is_empty(),
            "locally invalid payment must not reach the facilitator"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_reports_facilitator_rejection_as_invalid_payment() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        mock_state.reject_verify.store(true, Ordering::Relaxed);
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let token = bound_token(
            100,
            "eip155:8453",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        );

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
                    request_id: Some("x402-facilitator-rejection".to_string()),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(PaymentError::InvalidPayment {
                kind,
                reason: "x402 facilitator rejected payment",
                ..
            }) if kind == "x402_usdc"
        ));
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_reports_facilitator_422_as_invalid_payment() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        mock_state
            .invalid_verify_status
            .store(true, Ordering::Relaxed);
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let token = bound_token(
            100,
            "eip155:8453",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        );

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
                    request_id: Some("x402-facilitator-422".to_string()),
                },
            )
            .await;

        assert!(matches!(result, Err(PaymentError::InvalidPayment { .. })));
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_commit_reports_facilitator_rejection_as_invalid_payment() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        mock_state.reject_settle.store(true, Ordering::Relaxed);
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let token = bound_token(
            100,
            "eip155:8453",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        );
        let reservation = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "x402_usdc".to_string(),
                        token,
                    }),
                    request_id: Some("x402-settle-rejection".to_string()),
                },
            )
            .await
            .expect("verify should succeed")
            .expect("priced flow should reserve");

        let result = driver.commit(&state, reservation).await;

        assert!(matches!(
            result,
            Err(PaymentError::InvalidPayment {
                kind,
                reason: "x402 facilitator rejected settlement",
                ..
            }) if kind == "x402_usdc"
        ));
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_commit_requires_a_transaction_for_success() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        mock_state
            .empty_settle_transaction
            .store(true, Ordering::Relaxed);
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let token = bound_token(
            100,
            "eip155:8453",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        );
        let reservation = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "x402_usdc".to_string(),
                        token,
                    }),
                    request_id: Some("x402-empty-transaction".to_string()),
                },
            )
            .await
            .expect("verify should succeed")
            .expect("priced flow should reserve");

        let result = driver.commit(&state, reservation).await;

        assert!(matches!(
            result,
            Err(PaymentError::BackendUnavailable { .. })
        ));
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_rejects_network_mismatch() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let token = bound_token(
            100,
            "eip155:84532",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        );

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

        assert!(matches!(result, Err(PaymentError::InvalidPayment { .. })));
        assert!(mock_state.calls.lock().await.is_empty());
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_rejects_wallet_mismatch() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let mut payload: serde_json::Value = serde_json::from_str(&bound_token(
            100,
            "eip155:8453",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        ))
        .expect("test payment payload");
        payload["payload"]["authorization"]["to"] = serde_json::json!("0xdeadbeef");

        let result = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: Some(ProvidedPayment {
                        kind: "x402_usdc".to_string(),
                        token: payload.to_string(),
                    }),
                    request_id: Some("x402-wallet-mismatch".to_string()),
                },
            )
            .await;

        assert!(matches!(result, Err(PaymentError::InvalidPayment { .. })));
        assert!(mock_state.calls.lock().await.is_empty());
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_rejects_resource_mismatch() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let token = bound_token(
            100,
            "eip155:8453",
            "0xabc123",
            ServiceId::ExecuteWasm.as_str(),
        );

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
                    request_id: Some("x402-resource-mismatch".to_string()),
                },
            )
            .await;

        assert!(matches!(result, Err(PaymentError::InvalidPayment { .. })));
        assert!(mock_state.calls.lock().await.is_empty());
        handle.abort();
    }

    #[tokio::test]
    async fn x402_driver_prepare_caps_facilitator_verify_response() {
        let (base_url, mock_state, handle) = start_mock_x402().await;
        mock_state.oversized_verify.store(true, Ordering::Relaxed);
        let driver = X402Driver::new(X402Config {
            facilitator_url: base_url,
            wallet_address: "0xabc123".to_string(),
            network: "base".to_string(),
        })
        .expect("x402 driver");
        let state = make_state();
        let token = bound_token(
            100,
            "eip155:8453",
            "0xabc123",
            ServiceId::EventsQuery.as_str(),
        );

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
                    request_id: Some("x402-oversized-facilitator".to_string()),
                },
            )
            .await;

        assert!(
            result.is_err(),
            "prepare should reject an oversized facilitator response"
        );
        handle.abort();
    }
}
