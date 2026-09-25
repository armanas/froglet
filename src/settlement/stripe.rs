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
use std::time::Duration;

use super::{
    PaymentError, PaymentReceipt, PaymentReservation, PaymentReservationState,
    PreparePaymentRequest, SettlementDriver, SettlementDriverDescriptor, WalletBalanceSnapshot,
    new_request_id,
};

// ─── Driver ───────────────────────────────────────────────────────────────────

const STRIPE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const STRIPE_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct StripeDriver {
    api_key: String,
    api_version: String,
    api_base_url: String,
    http_client: reqwest::Client,
}

impl StripeDriver {
    pub(crate) fn new(config: StripeConfig, api_key: String) -> Result<Self, String> {
        Self::with_base_url(config, api_key, "https://api.stripe.com")
    }

    pub(crate) fn with_base_url(
        config: StripeConfig,
        api_key: String,
        api_base_url: &str,
    ) -> Result<Self, String> {
        let http_client = crate::tls::reqwest_client_builder()
            .connect_timeout(STRIPE_CONNECT_TIMEOUT)
            .timeout(STRIPE_REQUEST_TIMEOUT)
            .build()
            .map_err(|error| format!("failed to build Stripe HTTP client: {error}"))?;
        Ok(Self {
            api_key,
            api_version: config.api_version,
            api_base_url: api_base_url.trim_end_matches('/').to_string(),
            http_client,
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.api_base_url, path)
    }

    fn redacted_path(path: &str) -> String {
        const SPT_PREFIX: &str = "/v1/shared_payment/granted_tokens/";
        if path.starts_with(SPT_PREFIX) {
            return format!("{SPT_PREFIX}<redacted>");
        }

        const PAYMENT_INTENT_PREFIX: &str = "/v1/payment_intents/";
        if let Some(resource) = path.strip_prefix(PAYMENT_INTENT_PREFIX) {
            let action = resource
                .split_once('/')
                .map(|(_, action)| format!("/{action}"))
                .unwrap_or_default();
            return format!("{PAYMENT_INTENT_PREFIX}<redacted>{action}");
        }

        path.to_string()
    }

    /// Perform an authenticated GET against the Stripe API and return the
    /// parsed JSON body.
    async fn stripe_get(&self, path: &str) -> Result<serde_json::Value, String> {
        let redacted_path = Self::redacted_path(path);
        let response = self
            .http_client
            .get(self.api_url(path))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Stripe-Version", &self.api_version)
            .send()
            .await
            .map_err(|_| format!("Stripe GET {redacted_path} request failed"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "Stripe GET {redacted_path} returned HTTP {}",
                status.as_u16()
            ));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| format!("Stripe GET {redacted_path} response decode failed"))?;

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
        self.stripe_post_form_with_idempotency(path, params, None)
            .await
    }

    async fn stripe_post_form_with_idempotency(
        &self,
        path: &str,
        params: &[(&str, &str)],
        idempotency_key: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let body = encode_form_params(params);
        let redacted_path = Self::redacted_path(path);

        let mut request = self
            .http_client
            .post(self.api_url(path))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Stripe-Version", &self.api_version)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        let response = request
            .send()
            .await
            .map_err(|_| format!("Stripe POST {redacted_path} request failed"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "Stripe POST {redacted_path} returned HTTP {}",
                status.as_u16()
            ));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| format!("Stripe POST {redacted_path} response decode failed"))?;

        Ok(body)
    }

    /// Cancel a PaymentIntent exactly once for a Froglet reservation.
    ///
    /// Stripe can complete a cancellation even when the response is lost. In
    /// that case the idempotency key prevents a different mutation on retry,
    /// and the status lookup lets Froglet distinguish an already-canceled
    /// intent from a hold that may still be live.
    async fn cancel_payment_intent(&self, pi_id: &str, request_id: &str) -> Result<(), String> {
        let cancel_path = format!("/v1/payment_intents/{pi_id}/cancel");
        let idempotency_key = format!("froglet-stripe-cancel-{request_id}");

        match self
            .stripe_post_form_with_idempotency(&cancel_path, &[], Some(&idempotency_key))
            .await
        {
            Ok(intent)
                if intent.get("status").and_then(serde_json::Value::as_str) == Some("canceled") =>
            {
                Ok(())
            }
            Ok(_) => {
                tracing::error!("Stripe PaymentIntent cancellation returned an unexpected state");
                Err("Stripe PaymentIntent cancellation did not reach canceled state".to_string())
            }
            Err(cancel_error) => {
                let intent_path = format!("/v1/payment_intents/{pi_id}");
                match self.stripe_get(&intent_path).await {
                    Ok(intent)
                        if intent.get("status").and_then(serde_json::Value::as_str)
                            == Some("canceled") =>
                    {
                        tracing::warn!(
                            "Stripe PaymentIntent cancellation reconciled as already canceled"
                        );
                        Ok(())
                    }
                    Ok(_) => {
                        tracing::error!(
                            details = %cancel_error,
                            "Stripe PaymentIntent cancellation could not be reconciled"
                        );
                        Err("Stripe PaymentIntent cancellation could not be confirmed".to_string())
                    }
                    Err(lookup_error) => {
                        tracing::error!(
                            cancel_details = %cancel_error,
                            lookup_details = %lookup_error,
                            "Stripe PaymentIntent cancellation and status lookup failed"
                        );
                        Err("Stripe PaymentIntent cancellation could not be confirmed".to_string())
                    }
                }
            }
        }
    }

    /// Create a manual-capture PaymentIntent with one immediate replay of the
    /// exact idempotent request when the first response is ambiguous. Stripe
    /// associates both attempts with the same mutation, so a response lost
    /// after creation can still be materialized without creating a second
    /// authorization.
    async fn create_payment_intent(
        &self,
        params: &[(&str, &str)],
        idempotency_key: &str,
    ) -> Result<serde_json::Value, String> {
        match self
            .stripe_post_form_with_idempotency("/v1/payment_intents", params, Some(idempotency_key))
            .await
        {
            Ok(intent) => Ok(intent),
            Err(first_error) => {
                tracing::warn!(
                    details = %first_error,
                    "Stripe PaymentIntent creation response was ambiguous; replaying the exact idempotent request"
                );
                self.stripe_post_form_with_idempotency(
                    "/v1/payment_intents",
                    params,
                    Some(idempotency_key),
                )
                .await
            }
        }
    }

    /// Capture succeeds only after Stripe reports the terminal `succeeded`
    /// state. A successful HTTP response is not itself settlement evidence:
    /// non-terminal or malformed bodies are reconciled through the canonical
    /// PaymentIntent lookup and otherwise remain retryable by the caller's
    /// durable deal/materialization state machine.
    async fn capture_payment_intent(&self, pi_id: &str, request_id: &str) -> Result<(), String> {
        let capture_path = format!("/v1/payment_intents/{pi_id}/capture");
        let idempotency_key = format!("froglet-stripe-capture-{request_id}");
        let capture = self
            .stripe_post_form_with_idempotency(&capture_path, &[], Some(&idempotency_key))
            .await;

        if capture
            .as_ref()
            .ok()
            .and_then(|intent| intent.get("status"))
            .and_then(serde_json::Value::as_str)
            == Some("succeeded")
        {
            return Ok(());
        }

        let capture_error = capture.err();
        let intent_path = format!("/v1/payment_intents/{pi_id}");
        match self.stripe_get(&intent_path).await {
            Ok(intent)
                if intent.get("status").and_then(serde_json::Value::as_str)
                    == Some("succeeded") =>
            {
                tracing::warn!("Stripe PaymentIntent capture reconciled as already succeeded");
                Ok(())
            }
            Ok(_) => {
                if let Some(error) = capture_error {
                    tracing::error!(
                        details = %error,
                        "Stripe PaymentIntent capture did not reach succeeded state"
                    );
                } else {
                    tracing::error!(
                        "Stripe PaymentIntent capture returned a non-terminal or malformed state"
                    );
                }
                Err("Stripe PaymentIntent capture could not be confirmed".to_string())
            }
            Err(lookup_error) => {
                if let Some(error) = capture_error {
                    tracing::error!(
                        capture_details = %error,
                        lookup_details = %lookup_error,
                        "Stripe PaymentIntent capture and status lookup failed"
                    );
                } else {
                    tracing::error!(
                        lookup_details = %lookup_error,
                        "Stripe PaymentIntent capture status lookup failed"
                    );
                }
                Err("Stripe PaymentIntent capture could not be confirmed".to_string())
            }
        }
    }

    async fn payment_intent_state(&self, pi_id: &str) -> Result<PaymentReservationState, String> {
        let intent_path = format!("/v1/payment_intents/{pi_id}");
        let intent = self.stripe_get(&intent_path).await?;
        let status = intent
            .get("status")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Stripe PaymentIntent response is missing status".to_string())?;
        Ok(match status {
            "succeeded" => PaymentReservationState::Committed,
            "canceled" => PaymentReservationState::Released,
            other => PaymentReservationState::Pending(other.to_string()),
        })
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
            if !is_valid_spt_id(spt_id) {
                tracing::warn!("Stripe SPT ID has an invalid shape");
                return Err(PaymentError::InvalidPayment {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    kind: "stripe_mpp".to_string(),
                    reason: "Stripe Shared Payment Token ID has an invalid shape",
                });
            }

            // Step 1: Validate the SPT by fetching its details from the API.
            let spt_path = format!("/v1/shared_payment/granted_tokens/{spt_id}");
            let spt_data = self.stripe_get(&spt_path).await.map_err(|err| {
                tracing::error!(details = %err, "Stripe SPT validation failed");
                PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "stripe".to_string(),
                }
            })?;

            let usage_limits = spt_data.get("usage_limits");

            // ── FAIL-CLOSED: all three SPT fields are REQUIRED ─────────────
            //
            // The prior code used `if let Some(...)` / `.is_some_and(...)` for
            // all three checks, meaning an SPT that omitted `expires_at`,
            // `currency`, or `maximum_amount` silently passed. An attacker
            // could craft a token without these fields to bypass payment.
            // Every check below now rejects on field-absent.

            // (1) Expiry — MUST be present and in the future.
            let expires_at = spt_data
                .get("expires_at")
                .or_else(|| usage_limits.and_then(|limits| limits.get("expires_at")))
                .and_then(|v| v.as_i64())
                .ok_or_else(|| {
                    tracing::warn!("Stripe SPT missing expires_at field");
                    PaymentError::BackendUnavailable {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        backend: "stripe".to_string(),
                    }
                })?;
            let now = super::current_unix_timestamp();
            if expires_at <= now {
                tracing::warn!(
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

            // (2) Currency — MUST be present and MUST be "usd".
            let currency = spt_data
                .get("currency")
                .or_else(|| usage_limits.and_then(|limits| limits.get("currency")))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    tracing::warn!("Stripe SPT missing currency field");
                    PaymentError::BackendUnavailable {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        backend: "stripe".to_string(),
                    }
                })?;
            if !currency.eq_ignore_ascii_case("usd") {
                tracing::warn!(%currency, "Stripe SPT currency is not USD");
                return Err(PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "stripe".to_string(),
                });
            }

            // (3) Amount — MUST be present and MUST cover the requested price.
            let max_amount = spt_data
                .get("maximum_amount")
                .or_else(|| spt_data.get("max_amount"))
                .or_else(|| usage_limits.and_then(|limits| limits.get("max_amount")))
                .and_then(|v| v.as_u64())
                .ok_or_else(|| {
                    tracing::warn!("Stripe SPT missing maximum_amount / max_amount field");
                    PaymentError::BackendUnavailable {
                        service_id: request.service_id.as_str().to_string(),
                        price_sats: request.price_sats,
                        backend: "stripe".to_string(),
                    }
                })?;
            if max_amount < request.price_sats {
                tracing::warn!(
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
                ("shared_payment_granted_token", spt_id),
                ("confirm", "true"),
                ("capture_method", "manual"),
            ];

            let request_id = request.request_id.clone().unwrap_or_else(new_request_id);
            let idempotency_key = format!("froglet-stripe-reserve-{request_id}");
            let pi_data = self
                .create_payment_intent(params, &idempotency_key)
                .await
                .map_err(|err| {
                    tracing::error!(details = %err, "Stripe PaymentIntent creation failed");
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

            // FIX 2: Verify the PaymentIntent status is "requires_capture".
            // Any other status (requires_payment_method, canceled, etc.) means
            // the authorisation was not successful; reject to avoid recording a
            // reservation that can never be captured.
            let pi_status = pi_data
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            if pi_status != "requires_capture" {
                tracing::error!(
                    "Stripe PaymentIntent status is not requires_capture after creation"
                );
                if let Err(cleanup_error) = self.cancel_payment_intent(&pi_id, &request_id).await {
                    tracing::error!(
                        details = %cleanup_error,
                        "Stripe PaymentIntent cleanup after rejected preparation failed"
                    );
                }
                return Err(PaymentError::BackendUnavailable {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    backend: "stripe".to_string(),
                });
            }

            // Store the PaymentIntent ID in token_hash so commit/release can
            // reference it. The field name is inherited from the shared struct;
            // for this driver it holds an opaque Stripe resource ID, not a hash.
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
            self.capture_payment_intent(pi_id, &reservation.request_id)
                .await
                .map_err(|_| PaymentError::BackendUnavailable {
                    service_id: reservation.service_id.as_str().to_string(),
                    price_sats: reservation.amount_sats,
                    backend: "stripe".to_string(),
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
            self.cancel_payment_intent(pi_id, &reservation.request_id)
                .await
                .map_err(|err| {
                    tracing::error!(details = %err, "Stripe PaymentIntent release failed");
                    "Stripe PaymentIntent release failed".to_string()
                })
        })
    }

    fn reservation_state<'a>(
        &'a self,
        _state: &'a AppState,
        reservation: &'a PaymentReservation,
    ) -> BoxFuture<'a, Result<PaymentReservationState, String>> {
        Box::pin(async move { self.payment_intent_state(&reservation.token_hash).await })
    }
}

// ─── Buyer-side SPT minting ───────────────────────────────────────────────────

/// A minted Stripe Shared Payment Token returned by [`StripeDriver::mint_spt`].
pub struct MintedSpt {
    /// The SPT ID (e.g. `spt_…`) to hand to the seller as
    /// `ProvidedPayment { kind: "stripe_mpp", token: spt_id }`.
    pub spt_id: String,
}

impl StripeDriver {
    /// Use Stripe's seller-side test helper to simulate receiving a Shared
    /// Payment Token (SPT) from an agent.
    ///
    /// The caller provides:
    /// - `amount_cents` — the maximum amount in cents (≥ the quoted price).
    /// - `expires_at` — Unix timestamp at which the token expires (must be in
    ///   the future; the seller will reject an expired token).
    /// - `payment_method` — a Stripe test payment-method ID (`pm_…`).
    /// - `seller_network_id` / `seller_external_id` — the seller scope to
    ///   which the simulated token is constrained.
    ///
    /// # Assumed Stripe preview API shape
    ///
    /// NOTE: Stripe shared-payment is a preview API; confirm exact field
    /// names/endpoint against Stripe preview docs before use. The
    /// parameters below are modelled on documented preview behaviour and the
    /// seller-side validation in `prepare()` (which reads
    /// `usage_limits.{currency, max_amount, expires_at}`).
    ///
    /// TEST-ONLY mint via the Stripe Agentic Commerce test helper:
    ///   `POST /v1/test_helpers/shared_payment/granted_tokens`
    /// Params (per Stripe docs): `payment_method`, `usage_limits[currency]`,
    /// `usage_limits[max_amount]`, `usage_limits[expires_at]`,
    /// `seller_details[network_id]`, and optional
    /// `seller_details[external_id]`. Requires `Stripe-Version:
    /// 2026-04-22.preview` (sent from the driver's api_version).
    ///
    /// NOTE: the `test_helpers` endpoint only works with TEST keys. In
    /// production the Shared Payment Token is granted by the buyer's
    /// agentic-commerce platform (an ACP agent), not minted by a Froglet node;
    /// production buyers supply the SPT in the deal request's `payment` field.
    /// This mint is a test/demo convenience for validating the seller flow.
    ///
    /// The seller validates the token via
    /// `GET /v1/shared_payment/granted_tokens/{spt_id}` and checks:
    /// - `usage_limits.expires_at` > now
    /// - `usage_limits.currency` == "usd"
    /// - `usage_limits.max_amount` >= price_sats
    pub async fn mint_spt(
        &self,
        amount_cents: u64,
        expires_at: i64,
        payment_method: &str,
        seller_network_id: &str,
        seller_external_id: Option<&str>,
    ) -> Result<MintedSpt, String> {
        if !self.api_key.starts_with("sk_test_") {
            return Err(
                "Stripe's SPT test helper requires an sk_test_ key; live keys are refused"
                    .to_string(),
            );
        }
        if !payment_method.starts_with("pm_") {
            return Err("Stripe's SPT test helper requires a pm_ payment method".to_string());
        }
        if amount_cents == 0 {
            return Err("Stripe's SPT test helper requires a positive amount".to_string());
        }
        if expires_at <= super::current_unix_timestamp() {
            return Err("Stripe's SPT test helper expiry must be in the future".to_string());
        }
        validate_seller_scope("seller network ID", seller_network_id)?;
        if let Some(external_id) = seller_external_id {
            validate_seller_scope("seller external ID", external_id)?;
        }

        // Param shapes match Stripe's Agentic Commerce (Shared Payment Token)
        // test-helper grant endpoint. Stripe-Version is sent from the driver's
        // api_version (must be "2026-04-22.preview").
        let amount_str = amount_cents.to_string();
        let expires_str = expires_at.to_string();

        // SPT test-helper create params, per Stripe docs: no top-level
        // currency; usage limits and seller scope are nested parameters.
        let mut params = vec![
            ("payment_method", payment_method),
            ("usage_limits[currency]", "usd"),
            ("usage_limits[max_amount]", &amount_str),
            ("usage_limits[expires_at]", &expires_str),
            ("seller_details[network_id]", seller_network_id),
        ];
        if let Some(external_id) = seller_external_id {
            params.push(("seller_details[external_id]", external_id));
        }

        let response = self
            .stripe_post_form("/v1/test_helpers/shared_payment/granted_tokens", &params)
            .await
            .map_err(|err| {
                tracing::error!(details = %err, "Stripe SPT mint failed");
                "failed to mint Stripe Shared Payment Token".to_string()
            })?;

        let spt_id = response
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Stripe SPT create response missing 'id' field".to_string())?
            .to_string();
        if !is_valid_spt_id(&spt_id) {
            return Err("Stripe SPT create response returned an invalid token ID".to_string());
        }

        Ok(MintedSpt { spt_id })
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

fn validate_seller_scope(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("Stripe SPT {label} must not be empty"));
    }
    if value.len() > 512 || value.chars().any(char::is_control) {
        return Err(format!(
            "Stripe SPT {label} must be at most 512 bytes and contain no control characters"
        ));
    }
    Ok(())
}

pub(crate) fn is_valid_spt_id(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("spt_") else {
        return false;
    };
    !suffix.is_empty()
        && value.len() <= 255
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
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
        http::HeaderMap,
        routing::{get, post},
    };
    use std::{
        collections::HashMap,
        io::Write,
        sync::{
            Arc, Mutex as StdMutex,
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
                webhook_secret: None,
            },
            "stripe_test_secret_placeholder".to_string(),
        )
        .expect("Stripe driver")
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
                "usage_limits": {
                    "currency": "usd",
                    "expires_at": super::super::current_unix_timestamp() + 600,
                    "max_amount": 50_000
                }
            }))
        }

        async fn create_payment_intent(
            State(state): State<Arc<MockStripeState>>,
            headers: HeaderMap,
            body: String,
        ) -> Json<serde_json::Value> {
            let idempotency = headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>");
            state
                .calls
                .lock()
                .await
                .push(format!("POST:/v1/payment_intents:{idempotency}:{body}"));
            Json(serde_json::json!({
                "id": "pi_test_123",
                "status": "requires_capture"
            }))
        }

        async fn capture_payment_intent(
            State(state): State<Arc<MockStripeState>>,
            headers: HeaderMap,
            Path(intent_id): Path<String>,
        ) -> Json<serde_json::Value> {
            let idempotency = headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>");
            state.calls.lock().await.push(format!(
                "POST:/v1/payment_intents/{intent_id}/capture:{idempotency}"
            ));
            Json(serde_json::json!({
                "id": intent_id,
                "status": "succeeded"
            }))
        }

        async fn cancel_payment_intent(
            State(state): State<Arc<MockStripeState>>,
            headers: HeaderMap,
            Path(intent_id): Path<String>,
        ) -> Json<serde_json::Value> {
            let idempotency = headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>");
            state.calls.lock().await.push(format!(
                "POST:/v1/payment_intents/{intent_id}/cancel:{idempotency}"
            ));
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

    #[test]
    fn stripe_spt_ids_are_strict_path_segments() {
        assert!(is_valid_spt_id("spt_1RgaZcFPC5QUO6ZCDVZuVA8q"));
        assert!(is_valid_spt_id("spt_platform_supplied_test"));
        for invalid in [
            "",
            "spt_",
            "pi_not_an_spt",
            "spt_../payment_intents",
            "spt_with space",
            "spt_with?query",
        ] {
            assert!(!is_valid_spt_id(invalid), "accepted invalid ID: {invalid}");
        }
    }

    #[test]
    fn stripe_diagnostic_paths_redact_bearer_and_resource_ids() {
        assert_eq!(
            StripeDriver::redacted_path("/v1/shared_payment/granted_tokens/spt_bearer_credential"),
            "/v1/shared_payment/granted_tokens/<redacted>"
        );
        assert_eq!(
            StripeDriver::redacted_path("/v1/payment_intents/pi_private/capture"),
            "/v1/payment_intents/<redacted>/capture"
        );
        assert_eq!(
            StripeDriver::redacted_path("/v1/payment_intents"),
            "/v1/payment_intents"
        );
    }

    #[test]
    fn provided_payment_debug_redacts_bearer_credential() {
        let token = "spt_bearer_never_debug";
        let debug = format!(
            "{:?}",
            ProvidedPayment {
                kind: "stripe_mpp".to_string(),
                token: token.to_string(),
            }
        );
        assert!(
            !debug.contains(token),
            "credential leaked via Debug: {debug}"
        );
        assert!(debug.contains("[REDACTED]"));
    }

    #[derive(Clone)]
    struct CapturedWriter(Arc<StdMutex<Vec<u8>>>);

    impl Write for CapturedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("capture log lock").extend(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stripe_failure_diagnostics_never_echo_credentials() {
        const API_KEY: &str = "sk_test_never_log_this";
        const SPT: &str = "spt_never_log_this";
        const PAYMENT_METHOD: &str = "pm_never_log_this";
        const WEBHOOK_SECRET: &str = "whsec_never_log_this";
        const CLIENT_SECRET: &str = "pi_example_secret_never_log_this";

        async fn stripe_failure(
            State(message): State<String>,
        ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": {"message": message}})),
            )
        }

        let upstream_message =
            format!("{API_KEY} {SPT} {PAYMENT_METHOD} {WEBHOOK_SECRET} {CLIENT_SECRET}");
        let app = Router::new()
            .route(
                "/v1/shared_payment/granted_tokens/:token_id",
                get(stripe_failure),
            )
            .route(
                "/v1/test_helpers/shared_payment/granted_tokens",
                post(stripe_failure),
            )
            .with_state(upstream_message);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind redaction mock");
        let address = listener.local_addr().expect("redaction mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve redaction mock");
        });

        let captured = Arc::new(StdMutex::new(Vec::new()));
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || CapturedWriter(writer.clone()))
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);

        let driver = StripeDriver::with_base_url(
            StripeConfig {
                api_version: "2026-04-22.preview".to_string(),
                webhook_secret: None,
            },
            API_KEY.to_string(),
            &format!("http://{address}"),
        )
        .expect("Stripe driver");
        let state = make_state();
        let prepare = driver
            .prepare(&state, priced_request(100, stripe_payment(SPT)))
            .await;
        assert!(
            matches!(prepare, Err(PaymentError::BackendUnavailable { .. })),
            "unexpected prepare result: {prepare:?}"
        );
        let mint_error = match driver
            .mint_spt(
                100,
                super::super::current_unix_timestamp() + 60,
                PAYMENT_METHOD,
                "seller-network",
                None,
            )
            .await
        {
            Ok(_) => panic!("mock Stripe unexpectedly minted an SPT"),
            Err(error) => error,
        };
        assert_eq!(mint_error, "failed to mint Stripe Shared Payment Token");

        let logs = String::from_utf8(captured.lock().expect("captured diagnostics lock").clone())
            .expect("captured diagnostics UTF-8");
        assert!(logs.contains("Stripe SPT validation failed"));
        assert!(logs.contains("Stripe SPT mint failed"));
        assert!(logs.contains("/v1/shared_payment/granted_tokens/<redacted>"));
        for secret in [API_KEY, SPT, PAYMENT_METHOD, WEBHOOK_SECRET, CLIENT_SECRET] {
            assert!(
                !logs.contains(secret),
                "Stripe diagnostic leaked {secret:?}: {logs}"
            );
        }
        server.abort();
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
    async fn stripe_driver_rejects_malformed_spt_without_echoing_it() {
        let driver = make_driver();
        let state = make_state();
        let malformed = "spt_secret with spaces";
        let error = driver
            .prepare(&state, priced_request(100, stripe_payment(malformed)))
            .await
            .expect_err("malformed SPT must fail before network access");
        assert!(matches!(&error, PaymentError::InvalidPayment { .. }));
        let diagnostic = format!("{error:?} {}", error.details());
        assert!(
            !diagnostic.contains(malformed),
            "malformed bearer credential leaked via error: {diagnostic}"
        );
    }

    #[tokio::test]
    async fn stripe_driver_prepare_and_commit_uses_payment_intents() {
        let (base_url, mock_state, handle) = start_mock_stripe().await;
        let driver = StripeDriver::with_base_url(
            StripeConfig {
                api_version: "2024-06-20".to_string(),
                webhook_secret: None,
            },
            "stripe_test_secret_placeholder".to_string(),
            &base_url,
        )
        .expect("Stripe driver");
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
                call.contains(
                    "POST:/v1/payment_intents:froglet-stripe-reserve-stripe-prepare:amount=100",
                ) && call.contains("capture_method=manual")
                    && call.contains("shared_payment_granted_token=spt_test_123")
                    && !call.contains("payment_method_data")
            }),
            "prepare should create a manual-capture payment intent with a stable idempotency key"
        );
        assert!(
            calls
                .iter()
                .any(|call| call == "POST:/v1/payment_intents/pi_test_123/capture:froglet-stripe-capture-stripe-prepare"),
            "commit should capture the payment intent with a stable idempotency key"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn stripe_driver_commit_treats_already_succeeded_intent_as_committed() {
        #[derive(Debug, Default)]
        struct CaptureRetryState {
            calls: TokioMutex<Vec<String>>,
        }

        async fn capture_payment_intent(
            State(state): State<Arc<CaptureRetryState>>,
            headers: HeaderMap,
            Path(intent_id): Path<String>,
        ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
            let idempotency = headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>");
            state
                .calls
                .lock()
                .await
                .push(format!("capture:{intent_id}:{idempotency}"));
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": { "message": "intent already captured" }
                })),
            )
        }

        async fn get_payment_intent(
            State(state): State<Arc<CaptureRetryState>>,
            Path(intent_id): Path<String>,
        ) -> Json<serde_json::Value> {
            state.calls.lock().await.push(format!("get:{intent_id}"));
            Json(serde_json::json!({
                "id": intent_id,
                "status": "succeeded"
            }))
        }

        let state = Arc::new(CaptureRetryState::default());
        let app = Router::new()
            .route(
                "/v1/payment_intents/:intent_id/capture",
                post(capture_payment_intent),
            )
            .route("/v1/payment_intents/:intent_id", get(get_payment_intent))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock stripe");
        let address = listener.local_addr().expect("listener address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve mock stripe");
        });

        let driver = StripeDriver::with_base_url(
            StripeConfig {
                api_version: "2024-06-20".to_string(),
                webhook_secret: None,
            },
            "stripe_test_secret_placeholder".to_string(),
            &format!("http://{address}"),
        )
        .expect("Stripe driver");
        let app_state = make_state();
        let receipt = driver
            .commit(
                &app_state,
                PaymentReservation {
                    request_id: "deal-123".to_string(),
                    method: "stripe_mpp".to_string(),
                    service_id: ServiceId::ExecuteWasm,
                    amount_sats: 42,
                    token_hash: "pi_retry_123".to_string(),
                },
            )
            .await
            .expect("already captured intent should be treated as committed");

        assert_eq!(
            receipt.settlement_status,
            crate::protocol::SettlementStatus::Committed
        );
        assert_eq!(
            receipt.settlement_reference.as_deref(),
            Some("pi_retry_123")
        );
        let calls = state.calls.lock().await.clone();
        assert_eq!(
            calls,
            vec![
                "capture:pi_retry_123:froglet-stripe-capture-deal-123",
                "get:pi_retry_123"
            ]
        );
        handle.abort();
    }

    #[tokio::test]
    async fn stripe_driver_commit_rejects_2xx_nonterminal_capture() {
        #[derive(Debug, Default)]
        struct CaptureState {
            calls: TokioMutex<Vec<String>>,
        }

        async fn capture(
            State(state): State<Arc<CaptureState>>,
            Path(intent_id): Path<String>,
        ) -> Json<serde_json::Value> {
            state
                .calls
                .lock()
                .await
                .push(format!("capture:{intent_id}"));
            Json(serde_json::json!({
                "id": intent_id,
                "status": "processing"
            }))
        }

        async fn get_intent(
            State(state): State<Arc<CaptureState>>,
            Path(intent_id): Path<String>,
        ) -> Json<serde_json::Value> {
            state.calls.lock().await.push(format!("get:{intent_id}"));
            Json(serde_json::json!({
                "id": intent_id,
                "status": "processing"
            }))
        }

        let state = Arc::new(CaptureState::default());
        let app = Router::new()
            .route("/v1/payment_intents/:intent_id/capture", post(capture))
            .route("/v1/payment_intents/:intent_id", get(get_intent))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind capture mock");
        let address = listener.local_addr().expect("listener address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve capture mock");
        });
        let driver = make_driver_with_base(&format!("http://{address}"));

        let error = driver
            .commit(
                &make_state(),
                PaymentReservation {
                    request_id: "nonterminal-capture".to_string(),
                    method: "stripe_mpp".to_string(),
                    service_id: ServiceId::ExecuteWasm,
                    amount_sats: 42,
                    token_hash: "pi_processing".to_string(),
                },
            )
            .await
            .expect_err("a nonterminal capture must not produce a committed receipt");

        assert!(matches!(error, PaymentError::BackendUnavailable { .. }));
        assert_eq!(
            state.calls.lock().await.as_slice(),
            ["capture:pi_processing", "get:pi_processing"]
        );
        handle.abort();
    }

    #[tokio::test]
    async fn stripe_driver_prepare_replays_ambiguous_create_with_same_idempotency_key() {
        #[derive(Debug, Default)]
        struct CreateState {
            calls: TokioMutex<Vec<String>>,
        }

        async fn get_token(Path(token_id): Path<String>) -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "id": token_id,
                "usage_limits": {
                    "currency": "usd",
                    "expires_at": super::super::current_unix_timestamp() + 600,
                    "max_amount": 500
                }
            }))
        }

        async fn create(
            State(state): State<Arc<CreateState>>,
            headers: HeaderMap,
            body: String,
        ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
            let idempotency = headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>")
                .to_string();
            let mut calls = state.calls.lock().await;
            calls.push(format!("{idempotency}:{body}"));
            if calls.len() == 1 {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": {"message": "response lost"}})),
                );
            }
            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({
                    "id": "pi_reconciled_create",
                    "status": "requires_capture"
                })),
            )
        }

        let state = Arc::new(CreateState::default());
        let app = Router::new()
            .route(
                "/v1/shared_payment/granted_tokens/:token_id",
                get(get_token),
            )
            .route("/v1/payment_intents", post(create))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind create mock");
        let address = listener.local_addr().expect("listener address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve create mock");
        });
        let driver = make_driver_with_base(&format!("http://{address}"));

        let reservation = driver
            .prepare(
                &make_state(),
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: stripe_payment("spt_ambiguous_create"),
                    request_id: Some("ambiguous-create".to_string()),
                },
            )
            .await
            .expect("same-key replay should materialize the created intent")
            .expect("priced request reservation");

        assert_eq!(reservation.token_hash, "pi_reconciled_create");
        let calls = state.calls.lock().await;
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0], calls[1],
            "the replay must be byte-for-byte idempotent"
        );
        assert!(calls[0].starts_with("froglet-stripe-reserve-ambiguous-create:"));
        handle.abort();
    }

    #[tokio::test]
    async fn stripe_driver_release_cancels_payment_intent() {
        let (base_url, mock_state, handle) = start_mock_stripe().await;
        let driver = StripeDriver::with_base_url(
            StripeConfig {
                api_version: "2024-06-20".to_string(),
                webhook_secret: None,
            },
            "stripe_test_secret_placeholder".to_string(),
            &base_url,
        )
        .expect("Stripe driver");
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
            calls.iter().any(|call| {
                call
                    == "POST:/v1/payment_intents/pi_test_123/cancel:froglet-stripe-cancel-stripe-release"
            }),
            "release should cancel the payment intent with a stable idempotency key"
        );
        handle.abort();
    }

    #[derive(Debug)]
    struct CancelReconcileState {
        calls: TokioMutex<Vec<String>>,
        reconciled_status: &'static str,
    }

    async fn start_cancel_reconcile_mock(
        reconciled_status: &'static str,
    ) -> (
        String,
        Arc<CancelReconcileState>,
        tokio::task::JoinHandle<()>,
    ) {
        async fn cancel_payment_intent(
            State(state): State<Arc<CancelReconcileState>>,
            headers: HeaderMap,
            Path(intent_id): Path<String>,
        ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
            let idempotency = headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>");
            state
                .calls
                .lock()
                .await
                .push(format!("cancel:{intent_id}:{idempotency}"));
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": {"message": "response lost"}})),
            )
        }

        async fn get_payment_intent(
            State(state): State<Arc<CancelReconcileState>>,
            Path(intent_id): Path<String>,
        ) -> Json<serde_json::Value> {
            state.calls.lock().await.push(format!("get:{intent_id}"));
            Json(serde_json::json!({
                "id": intent_id,
                "status": state.reconciled_status
            }))
        }

        let state = Arc::new(CancelReconcileState {
            calls: TokioMutex::new(Vec::new()),
            reconciled_status,
        });
        let app = Router::new()
            .route(
                "/v1/payment_intents/:intent_id/cancel",
                post(cancel_payment_intent),
            )
            .route("/v1/payment_intents/:intent_id", get(get_payment_intent))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind cancel reconciliation mock");
        let address = listener.local_addr().expect("cancel mock address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve cancel reconciliation mock");
        });
        (format!("http://{address}"), state, handle)
    }

    #[tokio::test]
    async fn stripe_driver_release_reconciles_a_lost_cancel_response() {
        let (base_url, mock_state, handle) = start_cancel_reconcile_mock("canceled").await;
        let driver = make_driver_with_base(&base_url);
        let state = make_state();
        let reservation = PaymentReservation {
            request_id: "release-retry".to_string(),
            method: "stripe_mpp".to_string(),
            service_id: ServiceId::EventsQuery,
            amount_sats: 100,
            token_hash: "pi_cancel_retry".to_string(),
        };

        driver
            .release(&state, &reservation)
            .await
            .expect("a canceled intent should reconcile as released");

        assert_eq!(
            mock_state.calls.lock().await.as_slice(),
            [
                "cancel:pi_cancel_retry:froglet-stripe-cancel-release-retry",
                "get:pi_cancel_retry",
            ]
        );
        handle.abort();
    }

    #[tokio::test]
    async fn stripe_driver_release_fails_closed_when_cancel_cannot_be_confirmed() {
        let (base_url, mock_state, handle) = start_cancel_reconcile_mock("requires_capture").await;
        let driver = make_driver_with_base(&base_url);
        let state = make_state();
        let pi_id = "pi_cancel_unconfirmed";
        let reservation = PaymentReservation {
            request_id: "release-unconfirmed".to_string(),
            method: "stripe_mpp".to_string(),
            service_id: ServiceId::EventsQuery,
            amount_sats: 100,
            token_hash: pi_id.to_string(),
        };

        let error = driver
            .release(&state, &reservation)
            .await
            .expect_err("a live hold must not be reported as released");

        assert_eq!(error, "Stripe PaymentIntent release failed");
        assert!(
            !error.contains(pi_id),
            "release error exposed PaymentIntent ID"
        );
        assert_eq!(
            mock_state.calls.lock().await.as_slice(),
            [
                "cancel:pi_cancel_unconfirmed:froglet-stripe-cancel-release-unconfirmed",
                "get:pi_cancel_unconfirmed",
            ]
        );
        handle.abort();
    }

    // ── Fix 1: fail-closed SPT field validation ──────────────────────────

    /// Spin up a mock Stripe server that returns a custom SPT body and always
    /// returns a valid PI body with status "requires_capture".
    async fn start_mock_stripe_with_custom_spt(
        spt_body: serde_json::Value,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::extract::State as AxumState;

        #[derive(Clone)]
        struct CustomSptState {
            spt_body: serde_json::Value,
        }

        async fn get_token(AxumState(s): AxumState<CustomSptState>) -> Json<serde_json::Value> {
            Json(s.spt_body.clone())
        }

        async fn create_pi() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "id": "pi_custom_test",
                "status": "requires_capture"
            }))
        }

        let shared = CustomSptState { spt_body };
        let app = Router::new()
            .route(
                "/v1/shared_payment/granted_tokens/:token_id",
                axum::routing::get(get_token),
            )
            .route("/v1/payment_intents", axum::routing::post(create_pi))
            .with_state(shared);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind custom spt mock");
        let address = listener.local_addr().expect("listener address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve custom spt mock");
        });
        (format!("http://{address}"), handle)
    }

    fn make_driver_with_base(base_url: &str) -> StripeDriver {
        StripeDriver::with_base_url(
            StripeConfig {
                api_version: "2024-06-20".to_string(),
                webhook_secret: None,
            },
            "sk_test_placeholder".to_string(),
            base_url,
        )
        .expect("Stripe driver")
    }

    fn stripe_payment(token: &str) -> Option<ProvidedPayment> {
        Some(ProvidedPayment {
            kind: "stripe_mpp".to_string(),
            token: token.to_string(),
        })
    }

    fn priced_request(price_sats: u64, payment: Option<ProvidedPayment>) -> PreparePaymentRequest {
        PreparePaymentRequest {
            service_id: ServiceId::EventsQuery,
            price_sats,
            payment,
            request_id: None,
        }
    }

    #[tokio::test]
    async fn spt_missing_expires_at_is_rejected() {
        // SPT body has currency + max_amount but no expires_at anywhere.
        let spt_body = serde_json::json!({
            "id": "spt_no_expiry",
            "usage_limits": {
                "currency": "usd",
                "max_amount": 50_000
                // expires_at deliberately absent
            }
        });
        let (base_url, handle) = start_mock_stripe_with_custom_spt(spt_body).await;
        let driver = make_driver_with_base(&base_url);
        let state = make_state();
        let result = driver
            .prepare(&state, priced_request(100, stripe_payment("spt_no_expiry")))
            .await;
        assert!(
            result.is_err(),
            "SPT missing expires_at must be rejected (fail-closed)"
        );
        assert!(
            matches!(result.unwrap_err(), PaymentError::BackendUnavailable { .. }),
            "rejection must be BackendUnavailable"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn spt_missing_currency_is_rejected() {
        // SPT body has expires_at + max_amount but no currency.
        let spt_body = serde_json::json!({
            "id": "spt_no_currency",
            "usage_limits": {
                "expires_at": super::super::current_unix_timestamp() + 600,
                "max_amount": 50_000
                // currency deliberately absent
            }
        });
        let (base_url, handle) = start_mock_stripe_with_custom_spt(spt_body).await;
        let driver = make_driver_with_base(&base_url);
        let state = make_state();
        let result = driver
            .prepare(
                &state,
                priced_request(100, stripe_payment("spt_no_currency")),
            )
            .await;
        assert!(
            result.is_err(),
            "SPT missing currency must be rejected (fail-closed)"
        );
        assert!(
            matches!(result.unwrap_err(), PaymentError::BackendUnavailable { .. }),
            "rejection must be BackendUnavailable"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn spt_missing_amount_is_rejected() {
        // SPT body has expires_at + currency but no amount field.
        let spt_body = serde_json::json!({
            "id": "spt_no_amount",
            "usage_limits": {
                "currency": "usd",
                "expires_at": super::super::current_unix_timestamp() + 600
                // max_amount / maximum_amount deliberately absent
            }
        });
        let (base_url, handle) = start_mock_stripe_with_custom_spt(spt_body).await;
        let driver = make_driver_with_base(&base_url);
        let state = make_state();
        let result = driver
            .prepare(&state, priced_request(100, stripe_payment("spt_no_amount")))
            .await;
        assert!(
            result.is_err(),
            "SPT missing amount must be rejected (fail-closed)"
        );
        assert!(
            matches!(result.unwrap_err(), PaymentError::BackendUnavailable { .. }),
            "rejection must be BackendUnavailable"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn spt_amount_less_than_price_is_rejected() {
        // SPT has max_amount=50 but requested price is 100.
        let spt_body = serde_json::json!({
            "id": "spt_low_amount",
            "usage_limits": {
                "currency": "usd",
                "expires_at": super::super::current_unix_timestamp() + 600,
                "max_amount": 50u64
            }
        });
        let (base_url, handle) = start_mock_stripe_with_custom_spt(spt_body).await;
        let driver = make_driver_with_base(&base_url);
        let state = make_state();
        let result = driver
            .prepare(
                &state,
                priced_request(100, stripe_payment("spt_low_amount")),
            )
            .await;
        assert!(result.is_err(), "SPT with amount < price must be rejected");
        assert!(
            matches!(result.unwrap_err(), PaymentError::BackendUnavailable { .. }),
            "rejection must be BackendUnavailable"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn fully_valid_spt_is_accepted() {
        // All three fields present; amount >= price; expiry in future.
        let spt_body = serde_json::json!({
            "id": "spt_fully_valid",
            "usage_limits": {
                "currency": "usd",
                "expires_at": super::super::current_unix_timestamp() + 3600,
                "max_amount": 10_000u64
            }
        });
        let (base_url, handle) = start_mock_stripe_with_custom_spt(spt_body).await;
        let driver = make_driver_with_base(&base_url);
        let state = make_state();
        let result = driver
            .prepare(
                &state,
                priced_request(100, stripe_payment("spt_fully_valid")),
            )
            .await;
        assert!(result.is_ok(), "fully-valid SPT should be accepted");
        assert!(result.unwrap().is_some(), "should return a reservation");
        handle.abort();
    }

    // ── Fix 2: PaymentIntent status check ────────────────────────────────

    /// Spin up a mock that returns a good SPT but a PI with a non-requires_capture status.
    async fn start_mock_stripe_with_pi_status(
        pi_status: &'static str,
    ) -> (
        String,
        Arc<CancelReconcileState>,
        tokio::task::JoinHandle<()>,
    ) {
        async fn get_good_token(Path(token_id): Path<String>) -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "id": token_id,
                "usage_limits": {
                    "currency": "usd",
                    "expires_at": super::super::current_unix_timestamp() + 3600,
                    "max_amount": 50_000u64
                }
            }))
        }

        async fn create_pi_with_status(
            State(state): State<Arc<CancelReconcileState>>,
        ) -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "id": "pi_bad_status",
                "status": state.reconciled_status
            }))
        }

        async fn cancel_payment_intent(
            State(state): State<Arc<CancelReconcileState>>,
            headers: HeaderMap,
            Path(intent_id): Path<String>,
        ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
            let idempotency = headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>");
            state
                .calls
                .lock()
                .await
                .push(format!("cancel:{intent_id}:{idempotency}"));
            if state.reconciled_status == "canceled" {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {"message": "intent already canceled"}
                    })),
                );
            }
            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({
                    "id": intent_id,
                    "status": "canceled"
                })),
            )
        }

        async fn get_payment_intent(
            State(state): State<Arc<CancelReconcileState>>,
            Path(intent_id): Path<String>,
        ) -> Json<serde_json::Value> {
            state.calls.lock().await.push(format!("get:{intent_id}"));
            Json(serde_json::json!({
                "id": intent_id,
                "status": state.reconciled_status
            }))
        }

        let state = Arc::new(CancelReconcileState {
            calls: TokioMutex::new(Vec::new()),
            reconciled_status: pi_status,
        });
        let app = Router::new()
            .route(
                "/v1/shared_payment/granted_tokens/:token_id",
                axum::routing::get(get_good_token),
            )
            .route(
                "/v1/payment_intents",
                axum::routing::post(create_pi_with_status),
            )
            .route(
                "/v1/payment_intents/:intent_id/cancel",
                post(cancel_payment_intent),
            )
            .route("/v1/payment_intents/:intent_id", get(get_payment_intent))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind pi status mock");
        let address = listener.local_addr().expect("listener address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve pi status mock");
        });
        (format!("http://{address}"), state, handle)
    }

    #[tokio::test]
    async fn pi_status_requires_payment_method_is_rejected() {
        let (base_url, mock_state, handle) =
            start_mock_stripe_with_pi_status("requires_payment_method").await;
        let driver = make_driver_with_base(&base_url);
        let state = make_state();
        let result = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: stripe_payment("spt_good_token"),
                    request_id: Some("cleanup-unexpected-state".to_string()),
                },
            )
            .await;
        assert!(
            result.is_err(),
            "PI with status requires_payment_method must be rejected"
        );
        assert!(
            matches!(result.unwrap_err(), PaymentError::BackendUnavailable { .. }),
            "rejection must be BackendUnavailable"
        );
        assert_eq!(
            mock_state.calls.lock().await.as_slice(),
            ["cancel:pi_bad_status:froglet-stripe-cancel-cleanup-unexpected-state"]
        );
        handle.abort();
    }

    #[tokio::test]
    async fn pi_status_canceled_is_rejected() {
        let (base_url, mock_state, handle) = start_mock_stripe_with_pi_status("canceled").await;
        let driver = make_driver_with_base(&base_url);
        let state = make_state();
        let result = driver
            .prepare(
                &state,
                PreparePaymentRequest {
                    service_id: ServiceId::EventsQuery,
                    price_sats: 100,
                    payment: stripe_payment("spt_good"),
                    request_id: Some("cleanup-canceled-state".to_string()),
                },
            )
            .await;
        assert!(result.is_err(), "PI with status canceled must be rejected");
        assert_eq!(
            mock_state.calls.lock().await.as_slice(),
            [
                "cancel:pi_bad_status:froglet-stripe-cancel-cleanup-canceled-state",
                "get:pi_bad_status",
            ]
        );
        handle.abort();
    }
}
