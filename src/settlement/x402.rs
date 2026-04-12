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
    new_request_id, PaymentError, PaymentReceipt, PaymentReservation, PreparePaymentRequest,
    SettlementDriver, SettlementDriverDescriptor, WalletBalanceSnapshot,
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
        let padded = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE,
            json.as_bytes(),
        );
        let value = parse_x402_token(&padded).expect("padded base64url should also work");
        assert_eq!(value["k"], "v");
    }

    #[test]
    fn parse_invalid_token_returns_error() {
        let result = parse_x402_token("not-valid-b64-nor-json!!!");
        assert!(result.is_err(), "garbage token should return an error");
    }
}
