use crate::{
    config::PaymentBackend,
    db::{self, ReservePaymentTokenOutcome},
    ecash,
    pricing::ServiceId,
    state::AppState,
};
use axum::http::StatusCode;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CASHU_VERIFIER_MODE: &str = "format_and_replay_guard";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidedPayment {
    pub kind: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct PaymentReservation {
    pub request_id: String,
    pub service_id: ServiceId,
    pub amount_sats: u64,
    pub token_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentReceipt {
    pub service_id: String,
    pub amount_sats: u64,
    pub token_hash: String,
}

impl PaymentReservation {
    pub fn receipt(&self) -> PaymentReceipt {
        PaymentReceipt {
            service_id: self.service_id.as_str().to_string(),
            amount_sats: self.amount_sats,
            token_hash: self.token_hash.clone(),
        }
    }
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
    #[error("payment token is already reserved by another request")]
    InUse {
        service_id: String,
        token_hash: String,
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
            PaymentError::InUse { .. } => StatusCode::CONFLICT,
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
            PaymentError::InUse {
                service_id,
                token_hash,
            } => serde_json::json!({
                "error": "payment token is already reserved by another request",
                "service_id": service_id,
                "token_hash": token_hash,
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

pub async fn prepare_payment(
    state: &AppState,
    service_id: ServiceId,
    payment: Option<ProvidedPayment>,
    request_id: Option<String>,
) -> Result<Option<PaymentReservation>, PaymentError> {
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

    let request_id = request_id.unwrap_or_else(new_request_id);
    let token_hash = token_info.token_hash.clone();
    let amount_sats = token_info.amount_satoshis;
    let reserve_request_id = request_id.clone();
    let reserve_token_hash = token_hash.clone();
    let outcome = state
        .db
        .with_conn(move |conn| {
            db::reserve_payment_token(
                conn,
                &reserve_token_hash,
                service_id,
                amount_sats,
                &reserve_request_id,
                current_unix_timestamp(),
            )
        })
        .await
        .map_err(PaymentError::Database)?;

    match outcome {
        ReservePaymentTokenOutcome::Reserved => Ok(Some(PaymentReservation {
            request_id,
            service_id,
            amount_sats,
            token_hash,
        })),
        ReservePaymentTokenOutcome::InUse => Err(PaymentError::InUse {
            service_id: service_id.as_str().to_string(),
            token_hash,
        }),
        ReservePaymentTokenOutcome::Replay => Err(PaymentError::Replay {
            service_id: service_id.as_str().to_string(),
            token_hash,
        }),
    }
}

pub async fn commit_payment(
    state: &AppState,
    reservation: PaymentReservation,
) -> Result<PaymentReceipt, PaymentError> {
    let token_hash = reservation.token_hash.clone();
    let request_id = reservation.request_id.clone();
    let committed = state
        .db
        .with_conn(move |conn| {
            db::commit_payment_token(conn, &token_hash, &request_id, current_unix_timestamp())
        })
        .await
        .map_err(PaymentError::Database)?;

    if !committed {
        return Err(PaymentError::Database(
            "payment reservation could not be committed".to_string(),
        ));
    }

    Ok(reservation.receipt())
}

pub async fn release_payment(
    state: &AppState,
    reservation: &PaymentReservation,
) -> Result<(), String> {
    let token_hash = reservation.token_hash.clone();
    let request_id = reservation.request_id.clone();
    state
        .db
        .with_conn(move |conn| db::release_payment_token(conn, &token_hash, &request_id))
        .await?;
    Ok(())
}

pub fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn new_request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
