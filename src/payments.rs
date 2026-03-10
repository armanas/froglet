use crate::{config::PaymentBackend, db, ecash, pricing::ServiceId, state::AppState};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::task;

pub const CASHU_VERIFIER_MODE: &str = "format_and_replay_guard";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidedPayment {
    pub kind: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct PaymentReceipt {
    pub service_id: String,
    pub amount_sats: u64,
    pub token_hash: String,
}

#[derive(Debug, Error)]
pub enum PaymentError {
    #[error("payment required")]
    PaymentRequired { service_id: String, price_sats: u64 },
    #[error("unsupported payment kind")]
    UnsupportedKind {
        service_id: String,
        price_sats: u64,
        kind: String,
    },
    #[error("payment backend unavailable")]
    BackendUnavailable { service_id: String, price_sats: u64 },
    #[error("invalid payment token: {message}")]
    InvalidToken {
        service_id: String,
        price_sats: u64,
        message: String,
    },
    #[error("payment amount is below required price")]
    Underpaid {
        service_id: String,
        price_sats: u64,
        amount_sats: u64,
    },
    #[error("payment token has already been redeemed")]
    Replay {
        service_id: String,
        token_hash: String,
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
            PaymentError::InvalidToken { .. } => StatusCode::BAD_REQUEST,
            PaymentError::Underpaid { .. } => StatusCode::PAYMENT_REQUIRED,
            PaymentError::Replay { .. } => StatusCode::CONFLICT,
            PaymentError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn details(&self) -> serde_json::Value {
        match self {
            PaymentError::PaymentRequired {
                service_id,
                price_sats,
            } => serde_json::json!({
                "error": "payment required",
                "service_id": service_id,
                "price_sats": price_sats,
                "payment_kind": "cashu"
            }),
            PaymentError::UnsupportedKind {
                service_id,
                price_sats,
                kind,
            } => serde_json::json!({
                "error": format!("unsupported payment kind: {kind}"),
                "service_id": service_id,
                "price_sats": price_sats,
                "payment_kind": "cashu"
            }),
            PaymentError::BackendUnavailable {
                service_id,
                price_sats,
            } => serde_json::json!({
                "error": "payment backend unavailable",
                "service_id": service_id,
                "price_sats": price_sats,
                "payment_kind": "cashu"
            }),
            PaymentError::InvalidToken {
                service_id,
                price_sats,
                message,
            } => serde_json::json!({
                "error": message,
                "service_id": service_id,
                "price_sats": price_sats,
                "payment_kind": "cashu"
            }),
            PaymentError::Underpaid {
                service_id,
                price_sats,
                amount_sats,
            } => serde_json::json!({
                "error": "payment amount is below required price",
                "service_id": service_id,
                "price_sats": price_sats,
                "amount_sats": amount_sats,
                "payment_kind": "cashu"
            }),
            PaymentError::Replay {
                service_id,
                token_hash,
            } => serde_json::json!({
                "error": "payment token has already been redeemed",
                "service_id": service_id,
                "token_hash": token_hash,
                "payment_kind": "cashu"
            }),
            PaymentError::Database(message) => serde_json::json!({
                "error": format!("database error: {message}")
            }),
        }
    }
}

pub async fn enforce_payment(
    state: &AppState,
    service_id: ServiceId,
    payment: Option<ProvidedPayment>,
) -> Result<Option<PaymentReceipt>, PaymentError> {
    let price_sats = state.pricing.price_for(service_id);
    if price_sats == 0 {
        return Ok(None);
    }

    if matches!(state.config.payment_backend, PaymentBackend::None) {
        return Err(PaymentError::BackendUnavailable {
            service_id: service_id.as_str().to_string(),
            price_sats,
        });
    }

    let payment = payment.ok_or_else(|| PaymentError::PaymentRequired {
        service_id: service_id.as_str().to_string(),
        price_sats,
    })?;

    if payment.kind.to_lowercase() != "cashu" {
        return Err(PaymentError::UnsupportedKind {
            service_id: service_id.as_str().to_string(),
            price_sats,
            kind: payment.kind,
        });
    }

    let token_info =
        ecash::inspect_cashu_token(&payment.token).map_err(|e| PaymentError::InvalidToken {
            service_id: service_id.as_str().to_string(),
            price_sats,
            message: e.to_string(),
        })?;

    if token_info.amount_satoshis < price_sats {
        return Err(PaymentError::Underpaid {
            service_id: service_id.as_str().to_string(),
            price_sats,
            amount_sats: token_info.amount_satoshis,
        });
    }

    let token_hash = token_info.token_hash.clone();
    let amount_satoshis = token_info.amount_satoshis;
    let inserted = state
        .db
        .with_conn(move |conn| {
            db::try_record_payment_redemption(
                conn,
                &token_hash,
                service_id,
                amount_satoshis,
                current_unix_timestamp(),
            )
        })
        .await
        .map_err(PaymentError::Database)?;

    if !inserted {
        return Err(PaymentError::Replay {
            service_id: service_id.as_str().to_string(),
            token_hash: token_info.token_hash,
        });
    }

    Ok(Some(PaymentReceipt {
        service_id: service_id.as_str().to_string(),
        amount_sats: token_info.amount_satoshis,
        token_hash: token_info.token_hash,
    }))
}

pub fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
