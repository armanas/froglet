use crate::{config::NodeConfig, pricing::ServiceId, state::AppState};
use axum::http::StatusCode;
use futures::future::BoxFuture;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

pub mod lightning;
pub mod none;
pub mod phoenixd;
pub mod stripe;
pub mod wallet;
pub mod x402;

// ─── Settlement Registry ──────────────────────────────────────────────────────

/// Registry of active settlement drivers, constructed from the node's payment
/// backend configuration at startup.  Each entry pairs a payment-method name
/// (e.g. `"lightning"`) with an `Arc` to the corresponding driver.
///
/// Using `Arc<dyn SettlementDriver>` rather than `&'static` references allows
/// drivers like [`x402::X402Driver`] that require runtime configuration to be
/// constructed from the node's config without leaking memory.
pub struct SettlementRegistry {
    drivers: Vec<(String, Arc<dyn SettlementDriver>)>,
}

impl SettlementRegistry {
    /// Build a registry from the node configuration.
    ///
    /// - `PaymentBackend::Lightning` registers the Lightning hold-invoice
    ///   driver.
    /// - `PaymentBackend::X402` registers the x402 USDC facilitator driver
    ///   when `config.x402` is present; logs a warning and skips otherwise.
    /// - `PaymentBackend::Stripe` registers the Stripe Machine Payments
    ///   driver when the API version and secret key are present.
    /// - `PaymentBackend::None` is silently ignored (no driver is registered).
    pub fn new(config: &NodeConfig) -> Self {
        let mut drivers: Vec<(String, Arc<dyn SettlementDriver>)> = Vec::new();
        for backend in &config.payment_backends {
            match backend {
                crate::config::PaymentBackend::None => {}
                crate::config::PaymentBackend::Lightning => {
                    drivers.push((
                        "lightning".to_string(),
                        Arc::new(lightning::LightningDriver),
                    ));
                }
                crate::config::PaymentBackend::X402 => {
                    if let Some(x402_config) = &config.x402 {
                        drivers.push((
                            "x402_usdc".to_string(),
                            Arc::new(x402::X402Driver::new(x402_config.clone())),
                        ));
                    } else {
                        tracing::warn!(
                            "Payment backend 'x402' is configured but \
                             FROGLET_X402_FACILITATOR_URL / FROGLET_X402_WALLET_ADDRESS \
                             are not set; the x402 driver will be skipped"
                        );
                    }
                }
                crate::config::PaymentBackend::Stripe => {
                    if let Some(stripe_config) = &config.stripe {
                        let api_key =
                            std::env::var("FROGLET_STRIPE_SECRET_KEY").unwrap_or_default();
                        if !api_key.is_empty() {
                            drivers.push((
                                "stripe_mpp".to_string(),
                                Arc::new(stripe::StripeDriver::new(stripe_config.clone(), api_key)),
                            ));
                        } else {
                            tracing::warn!(
                                "Payment backend 'stripe' is configured but \
                                 FROGLET_STRIPE_SECRET_KEY is not set; \
                                 the Stripe MPP driver will be skipped"
                            );
                        }
                    } else {
                        tracing::warn!(
                            "Payment backend 'stripe' is configured but no \
                             StripeConfig is present (check FROGLET_STRIPE_API_VERSION); \
                             the Stripe MPP driver will be skipped"
                        );
                    }
                }
            }
        }
        Self { drivers }
    }

    /// Return the driver responsible for `payment_kind`, or `None` if no
    /// registered driver handles that kind.
    pub fn driver_for(&self, payment_kind: &str) -> Option<&dyn SettlementDriver> {
        self.drivers
            .iter()
            .find(|(name, _)| name == payment_kind)
            .map(|(_, driver)| driver.as_ref())
    }

    /// Return the first registered driver, falling back to the no-op driver
    /// when no backends are configured.
    pub fn primary_driver(&self) -> &dyn SettlementDriver {
        self.drivers
            .first()
            .map(|(_, driver)| driver.as_ref())
            .unwrap_or(&none::NO_SETTLEMENT_DRIVER)
    }

    /// List the payment-method names accepted by this node, in registration
    /// order.
    pub fn accepted_payment_methods(&self) -> Vec<String> {
        self.drivers.iter().map(|(name, _)| name.clone()).collect()
    }

    /// Returns `true` when no payment backends are active.
    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }

    /// Build a registry with a single explicitly-provided driver.
    ///
    /// Used in integration tests to inject a driver pointed at a mock HTTP
    /// server without touching real external infrastructure.  Not gated by
    /// `#[cfg(test)]` so that integration test crates (which compile the
    /// library in non-test mode) can call it.
    ///
    /// Pass the driver via [`Arc`] so that the return type is unambiguously
    /// `'static` even when constructed from a function returning `impl Trait`.
    pub fn with_single_driver(
        payment_method: impl Into<String>,
        driver: Arc<dyn SettlementDriver>,
    ) -> Self {
        Self {
            drivers: vec![(payment_method.into(), driver)],
        }
    }
}

// Re-export wallet trait and its error type.
pub use wallet::{ArcLightningWallet, LightningWallet, WalletError};

// Re-export everything from lightning that was previously accessible as settlement::X
pub use lightning::{
    BuildLightningInvoiceBundleRequest, InvoiceBundleValidationIssue,
    InvoiceBundleValidationReport, LIGHTNING_LND_REST_MODE, LIGHTNING_MOCK_MODE,
    LightningInvoiceBundleSession, LightningWalletIntent, LightningWalletMockAction,
    LightningWalletPaymentRequest, LightningWalletReleaseAction, build_lightning_invoice_bundle,
    build_lightning_wallet_intent, cancel_and_sync_lightning_invoice_bundle,
    cancel_lightning_invoice_bundle, cancel_pending_lightning_materialization_request,
    create_lightning_invoice_bundle, get_lightning_invoice_bundle,
    get_lightning_invoice_bundle_by_deal_hash, issue_lightning_invoice_bundle,
    lightning_bundle_can_settle_success, lightning_bundle_is_funded, lightning_quote_expires_at,
    quoted_lightning_settlement_terms, resolve_lightning_destination_identity,
    settle_lightning_success_hold_invoice, sync_lightning_invoice_bundle_session,
    update_lightning_invoice_bundle_states, validate_lightning_invoice_bundle,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidedPayment {
    pub kind: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct PaymentReservation {
    pub request_id: String,
    pub method: String,
    pub service_id: ServiceId,
    pub amount_sats: u64,
    pub token_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentReceipt {
    pub service_id: String,
    pub method: String,
    pub settlement_status: crate::protocol::SettlementStatus,
    pub reserved_amount_sats: u64,
    pub committed_amount_sats: u64,
    pub token_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_reference: Option<String>,
}

impl PaymentReservation {
    pub fn receipt(
        &self,
        settlement_status: crate::protocol::SettlementStatus,
        committed_amount_sats: u64,
        settlement_reference: Option<String>,
    ) -> PaymentReceipt {
        PaymentReceipt {
            service_id: self.service_id.as_str().to_string(),
            method: self.method.clone(),
            settlement_status,
            reserved_amount_sats: self.amount_sats,
            committed_amount_sats,
            token_hash: self.token_hash.clone(),
            settlement_reference,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SettlementDriverDescriptor {
    pub backend: String,
    pub mode: String,
    pub accepted_payment_methods: Vec<String>,
    pub capabilities: Vec<String>,
    pub reservations: bool,
    pub receipts: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WalletBalanceSnapshot {
    pub backend: String,
    pub mode: String,
    pub balance_known: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_sats: Option<u64>,
    pub accepted_payment_methods: Vec<String>,
    pub capabilities: Vec<String>,
    pub reservations: bool,
    pub receipts: bool,
}

impl WalletBalanceSnapshot {
    pub(crate) fn from_descriptor(descriptor: SettlementDriverDescriptor) -> Self {
        Self {
            backend: descriptor.backend,
            mode: descriptor.mode,
            balance_known: false,
            balance_sats: None,
            accepted_payment_methods: descriptor.accepted_payment_methods,
            capabilities: descriptor.capabilities,
            reservations: descriptor.reservations,
            receipts: descriptor.receipts,
        }
    }
}

#[derive(Debug)]
pub struct PreparePaymentRequest {
    pub service_id: ServiceId,
    pub price_sats: u64,
    pub payment: Option<ProvidedPayment>,
    pub request_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum PaymentError {
    #[error("payment required")]
    PaymentRequired {
        service_id: String,
        price_sats: u64,
        accepted_payment_methods: Vec<String>,
    },
    #[error("unsupported payment kind")]
    UnsupportedKind {
        service_id: String,
        price_sats: u64,
        kind: String,
        accepted_payment_methods: Vec<String>,
    },
    #[error("payment backend unavailable")]
    BackendUnavailable {
        service_id: String,
        price_sats: u64,
        backend: String,
    },
    #[error("database error: {0}")]
    Database(String),
}

impl PaymentError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            PaymentError::PaymentRequired { .. } => StatusCode::PAYMENT_REQUIRED,
            PaymentError::UnsupportedKind { .. } => StatusCode::BAD_REQUEST,
            PaymentError::BackendUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            PaymentError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn details(&self) -> serde_json::Value {
        match self {
            PaymentError::PaymentRequired {
                service_id,
                price_sats,
                accepted_payment_methods,
            } => serde_json::json!({
                "error": "payment required",
                "service_id": service_id,
                "price_sats": price_sats,
                "accepted_payment_methods": accepted_payment_methods
            }),
            PaymentError::UnsupportedKind {
                service_id,
                price_sats,
                kind,
                accepted_payment_methods,
            } => serde_json::json!({
                "error": format!("unsupported payment kind: {kind}"),
                "service_id": service_id,
                "price_sats": price_sats,
                "accepted_payment_methods": accepted_payment_methods
            }),
            PaymentError::BackendUnavailable {
                service_id,
                price_sats,
                backend,
            } => serde_json::json!({
                "error": "payment backend unavailable",
                "service_id": service_id,
                "price_sats": price_sats,
                "backend": backend
            }),
            PaymentError::Database(message) => {
                tracing::error!("Settlement database error: {message}");
                serde_json::json!({
                    "error": "internal settlement database error"
                })
            }
        }
    }
}

pub trait SettlementDriver: Send + Sync {
    fn descriptor(&self, state: &AppState) -> SettlementDriverDescriptor;

    fn wallet_balance<'a>(
        &'a self,
        state: &'a AppState,
    ) -> BoxFuture<'a, Result<WalletBalanceSnapshot, PaymentError>>;

    fn prepare<'a>(
        &'a self,
        state: &'a AppState,
        request: PreparePaymentRequest,
    ) -> BoxFuture<'a, Result<Option<PaymentReservation>, PaymentError>>;

    fn commit<'a>(
        &'a self,
        state: &'a AppState,
        reservation: PaymentReservation,
    ) -> BoxFuture<'a, Result<PaymentReceipt, PaymentError>>;

    fn release<'a>(
        &'a self,
        state: &'a AppState,
        reservation: &'a PaymentReservation,
    ) -> BoxFuture<'a, Result<(), String>>;
}

pub fn driver_descriptor(state: &AppState) -> SettlementDriverDescriptor {
    state.settlement_registry.primary_driver().descriptor(state)
}

pub fn accepted_payment_methods(state: &AppState) -> Vec<String> {
    state.settlement_registry.accepted_payment_methods()
}

pub async fn wallet_balance_snapshot(
    state: &AppState,
) -> Result<WalletBalanceSnapshot, PaymentError> {
    state
        .settlement_registry
        .primary_driver()
        .wallet_balance(state)
        .await
}

pub async fn prepare_payment(
    state: &AppState,
    service_id: ServiceId,
    payment: Option<ProvidedPayment>,
    request_id: Option<String>,
) -> Result<Option<PaymentReservation>, PaymentError> {
    let price_sats = state.pricing.price_for(service_id);
    prepare_payment_for_amount(state, service_id, price_sats, payment, request_id).await
}

pub async fn prepare_payment_for_amount(
    state: &AppState,
    service_id: ServiceId,
    price_sats: u64,
    payment: Option<ProvidedPayment>,
    request_id: Option<String>,
) -> Result<Option<PaymentReservation>, PaymentError> {
    if price_sats == 0 {
        return Ok(None);
    }

    let accepted = state.settlement_registry.accepted_payment_methods();

    let driver = match payment.as_ref().map(|p| p.kind.as_str()) {
        Some(kind) => state.settlement_registry.driver_for(kind).ok_or_else(|| {
            PaymentError::UnsupportedKind {
                service_id: service_id.as_str().to_string(),
                price_sats,
                kind: kind.to_string(),
                accepted_payment_methods: accepted.clone(),
            }
        })?,
        None => {
            return Err(PaymentError::PaymentRequired {
                service_id: service_id.as_str().to_string(),
                price_sats,
                accepted_payment_methods: accepted,
            });
        }
    };

    driver
        .prepare(
            state,
            PreparePaymentRequest {
                service_id,
                price_sats,
                payment,
                request_id,
            },
        )
        .await
}

pub async fn commit_payment(
    state: &AppState,
    reservation: PaymentReservation,
) -> Result<PaymentReceipt, PaymentError> {
    // Dispatch to the driver that matches the reservation's payment method,
    // falling back to the primary driver for backward compatibility.
    let driver = state
        .settlement_registry
        .driver_for(&reservation.method)
        .unwrap_or_else(|| state.settlement_registry.primary_driver());
    driver.commit(state, reservation).await
}

pub async fn release_payment(
    state: &AppState,
    reservation: &PaymentReservation,
) -> Result<(), String> {
    // Same dispatch logic as commit_payment.
    let driver = state
        .settlement_registry
        .driver_for(&reservation.method)
        .unwrap_or_else(|| state.settlement_registry.primary_driver());
    driver.release(state, reservation).await
}

/// Build a [`StripeDriver`] pointed at a custom base URL.
///
/// This constructor is intentionally public so that integration tests can
/// wire the driver to a local mock HTTP server without touching real Stripe
/// infrastructure.  Production code always uses [`StripeDriver::new`] (which
/// defaults to `https://api.stripe.com`).
pub fn stripe_driver_with_base_url(
    config: crate::config::StripeConfig,
    api_key: String,
    api_base_url: &str,
) -> impl SettlementDriver {
    stripe::StripeDriver::with_base_url(config, api_key, api_base_url)
}

/// Build a [`StripeDriver`] and box it as `Box<dyn SettlementDriver + Send + Sync + 'static>`.
///
/// Identical to [`stripe_driver_with_base_url`] but returns an owned, type-erased
/// driver suitable for storage in a `SettlementRegistry` constructed via
/// [`SettlementRegistry::with_single_driver`].  The extra erasure step avoids
/// the opaque-`impl`-return lifetime issue when passing the driver across
/// function boundaries in integration tests.
pub fn stripe_driver_boxed(
    config: crate::config::StripeConfig,
    api_key: String,
    api_base_url: impl Into<String>,
) -> Box<dyn SettlementDriver> {
    Box::new(stripe::StripeDriver::with_base_url(
        config,
        api_key,
        &api_base_url.into(),
    ))
}

/// Mint a Stripe Shared Payment Token (SPT) using the buyer node's Stripe
/// credentials.
///
/// This is the buyer-side complement to the seller's `prepare()` step.  Call
/// this before sending a `CreateDealRequest` to a provider whose quote carries
/// `settlement_terms.method == "stripe_mpp.v1"`.
///
/// # Parameters
/// - `buyer_config` — the buyer's Stripe credentials and funding source.
/// - `amount_cents` — the SPT ceiling in cents; must be ≥ the quoted price.
/// - `expires_at` — Unix timestamp; the seller rejects tokens past this time.
/// - `api_base_url` — override the Stripe API base URL (use `None` for
///   production `https://api.stripe.com`; pass a mock URL in tests).
///
/// # Errors
/// Returns a human-readable error string when the Stripe call fails.
pub async fn mint_buyer_spt(
    buyer_config: &crate::config::BuyerStripeConfig,
    amount_cents: u64,
    expires_at: i64,
    api_base_url: Option<&str>,
) -> Result<String, String> {
    let seller_config = crate::config::StripeConfig {
        api_version: buyer_config.api_version.clone(),
        webhook_secret: None,
    };
    let driver = match api_base_url {
        Some(url) => {
            stripe::StripeDriver::with_base_url(seller_config, buyer_config.secret_key.clone(), url)
        }
        None => stripe::StripeDriver::new(seller_config, buyer_config.secret_key.clone()),
    };

    let funding_source = match (
        buyer_config.payment_method.as_deref(),
        buyer_config.customer.as_deref(),
    ) {
        (Some(pm_id), _) => stripe::BuyerFundingSource::PaymentMethod(pm_id),
        (None, Some(cus_id)) => stripe::BuyerFundingSource::Customer(cus_id),
        (None, None) => {
            return Err(
                "buyer Stripe config has no funding source (payment_method or customer)".into(),
            );
        }
    };

    let minted = driver
        .mint_spt(amount_cents, expires_at, &funding_source)
        .await?;
    Ok(minted.spt_id)
}

/// Pay a prepaid (`lightning.prepaid.v1`) BOLT11 invoice from the buyer's own
/// phoenixd node.  Returns the sent payment (including the preimage the payee
/// revealed) so the caller can locally verify `sha256(preimage) == payment_hash`
/// before trusting it.
pub async fn pay_buyer_prepaid_invoice(
    buyer_config: &crate::config::BuyerPhoenixdConfig,
    bolt11: &str,
) -> Result<phoenixd::SentPayment, String> {
    // `api_base_url` redirects to a mock phoenixd in tests; otherwise use the
    // configured buyer phoenixd URL.
    let url = buyer_config
        .api_base_url
        .clone()
        .unwrap_or_else(|| buyer_config.url.clone());
    let client = phoenixd::PhoenixdClient::from_config(&crate::config::LightningPhoenixdConfig {
        url,
        http_password: buyer_config.http_password.clone(),
        request_timeout_secs: 15,
    })
    .map_err(|error| error.to_string())?;
    client
        .pay_invoice(bolt11, None)
        .await
        .map_err(|error| error.to_string())
}

// Re-export buyer-mint types so tests can use them directly.
pub use stripe::{BuyerFundingSource, MintedSpt};

pub fn current_unix_timestamp() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => {
            tracing::error!("System clock is before the Unix epoch: {error}");
            0
        }
    }
}

pub(crate) fn new_request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
