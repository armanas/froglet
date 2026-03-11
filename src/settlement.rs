use crate::{
    config::PaymentBackend,
    db::{self, ReservePaymentTokenOutcome},
    ecash,
    pricing::ServiceId,
    protocol::SettlementStatus,
    state::AppState,
};
use axum::http::StatusCode;
use cashu::{
    MintUrl,
    nuts::nut07::{CheckStateRequest, CheckStateResponse, State as ProofState},
};
use futures::future::BoxFuture;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, str::FromStr, time::Duration};
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
    pub method: String,
    pub service_id: ServiceId,
    pub amount_sats: u64,
    pub token_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentReceipt {
    pub service_id: String,
    pub method: String,
    pub settlement_status: SettlementStatus,
    pub reserved_amount_sats: u64,
    pub committed_amount_sats: u64,
    pub token_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_reference: Option<String>,
}

impl PaymentReservation {
    pub fn receipt(
        &self,
        settlement_status: SettlementStatus,
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
    pub accepted_mints: Vec<String>,
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
    pub accepted_mints: Vec<String>,
    pub capabilities: Vec<String>,
    pub reservations: bool,
    pub receipts: bool,
}

impl WalletBalanceSnapshot {
    fn from_descriptor(descriptor: SettlementDriverDescriptor) -> Self {
        Self {
            backend: descriptor.backend,
            mode: descriptor.mode,
            balance_known: false,
            balance_sats: None,
            accepted_payment_methods: descriptor.accepted_payment_methods,
            accepted_mints: descriptor.accepted_mints,
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
    selected_driver(state).descriptor(state)
}

pub fn accepted_payment_methods(state: &AppState) -> Vec<String> {
    driver_descriptor(state).accepted_payment_methods
}

pub async fn wallet_balance_snapshot(
    state: &AppState,
) -> Result<WalletBalanceSnapshot, PaymentError> {
    selected_driver(state).wallet_balance(state).await
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
    selected_driver(state)
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
    selected_driver(state).commit(state, reservation).await
}

pub async fn release_payment(
    state: &AppState,
    reservation: &PaymentReservation,
) -> Result<(), String> {
    selected_driver(state).release(state, reservation).await
}

pub fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn selected_driver(state: &AppState) -> &'static dyn SettlementDriver {
    match state.config.payment_backend {
        PaymentBackend::None => &NO_SETTLEMENT_DRIVER,
        PaymentBackend::Cashu => &CASHU_VERIFIER_DRIVER,
    }
}

fn new_request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn invalid_cashu_token(
    request: &PreparePaymentRequest,
    message: impl Into<String>,
) -> PaymentError {
    PaymentError::InvalidToken {
        service_id: request.service_id.as_str().to_string(),
        price_sats: request.price_sats,
        message: message.into(),
    }
}

fn canonicalize_mint_urlish(value: &str) -> String {
    value.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn mint_allowed(allowlist: &[String], mint_url: &str) -> bool {
    if allowlist.is_empty() {
        return true;
    }

    let normalized_mint = canonicalize_mint_urlish(mint_url);
    allowlist
        .iter()
        .any(|entry| canonicalize_mint_urlish(entry) == normalized_mint)
}

async fn verify_cashu_token_policies(
    state: &AppState,
    request: &PreparePaymentRequest,
    token_info: &ecash::CashuTokenInfo,
) -> Result<(), PaymentError> {
    if !mint_allowed(&state.config.cashu.mint_allowlist, &token_info.mint_url) {
        return Err(invalid_cashu_token(
            request,
            format!("cashu mint is not allowed: {}", token_info.mint_url),
        ));
    }

    if token_info.has_spend_conditions {
        return Err(invalid_cashu_token(
            request,
            "spend-conditioned Cashu tokens are not supported by Froglet's verifier-only settlement mode",
        ));
    }

    if state.config.cashu.remote_checkstate {
        verify_cashu_checkstate(state, request, token_info).await?;
    }

    Ok(())
}

async fn verify_cashu_checkstate(
    state: &AppState,
    request: &PreparePaymentRequest,
    token_info: &ecash::CashuTokenInfo,
) -> Result<(), PaymentError> {
    let mint_url = MintUrl::from_str(&token_info.mint_url)
        .map_err(|e| invalid_cashu_token(request, format!("invalid cashu mint url: {e}")))?;
    let checkstate_url = mint_url.join("v1/checkstate").map_err(|e| {
        invalid_cashu_token(request, format!("failed to build checkstate url: {e}"))
    })?;

    let response = state
        .http_client
        .post(checkstate_url)
        .timeout(Duration::from_secs(state.config.cashu.request_timeout_secs))
        .json(&CheckStateRequest {
            ys: token_info.proof_ys.clone(),
        })
        .send()
        .await
        .map_err(|e| invalid_cashu_token(request, format!("mint checkstate request failed: {e}")))?;

    if !response.status().is_success() {
        return Err(invalid_cashu_token(
            request,
            format!("mint checkstate returned {}", response.status()),
        ));
    }

    let response = response.json::<CheckStateResponse>().await.map_err(|e| {
        invalid_cashu_token(request, format!("invalid mint checkstate response: {e}"))
    })?;

    let requested_ys = token_info
        .proof_ys
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    let returned_ys = response
        .states
        .iter()
        .map(|state| state.y.to_string())
        .collect::<BTreeSet<_>>();

    if requested_ys != returned_ys {
        return Err(invalid_cashu_token(
            request,
            "mint checkstate response did not match all submitted proofs",
        ));
    }

    if let Some(proof_state) = response
        .states
        .iter()
        .find(|state| !matches!(state.state, ProofState::Unspent))
    {
        return Err(invalid_cashu_token(
            request,
            format!(
                "cashu proof is not spendable according to the mint: {} ({})",
                proof_state.y, proof_state.state
            ),
        ));
    }

    Ok(())
}

struct NoSettlementDriver;

impl SettlementDriver for NoSettlementDriver {
    fn descriptor(&self, _state: &AppState) -> SettlementDriverDescriptor {
        SettlementDriverDescriptor {
            backend: PaymentBackend::None.to_string(),
            mode: "disabled".to_string(),
            accepted_payment_methods: Vec::new(),
            accepted_mints: Vec::new(),
            capabilities: Vec::new(),
            reservations: false,
            receipts: false,
        }
    }

    fn wallet_balance<'a>(
        &'a self,
        state: &'a AppState,
    ) -> BoxFuture<'a, Result<WalletBalanceSnapshot, PaymentError>> {
        Box::pin(async move { Ok(WalletBalanceSnapshot::from_descriptor(self.descriptor(state))) })
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

            Err(PaymentError::BackendUnavailable {
                service_id: request.service_id.as_str().to_string(),
                price_sats: request.price_sats,
                backend: PaymentBackend::None.to_string(),
            })
        })
    }

    fn commit<'a>(
        &'a self,
        _state: &'a AppState,
        _reservation: PaymentReservation,
    ) -> BoxFuture<'a, Result<PaymentReceipt, PaymentError>> {
        Box::pin(async move {
            Err(PaymentError::BackendUnavailable {
                service_id: "unknown".to_string(),
                price_sats: 0,
                backend: PaymentBackend::None.to_string(),
            })
        })
    }

    fn release<'a>(
        &'a self,
        _state: &'a AppState,
        _reservation: &'a PaymentReservation,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { Ok(()) })
    }
}

struct CashuVerifierDriver;

impl CashuVerifierDriver {
    fn descriptor_inner(&self, state: &AppState) -> SettlementDriverDescriptor {
        let mut capabilities = vec![
            "token_format_verification".to_string(),
            "local_replay_guard".to_string(),
        ];
        if !state.config.cashu.mint_allowlist.is_empty() {
            capabilities.push("mint_allowlist".to_string());
        }
        if state.config.cashu.remote_checkstate {
            capabilities.push("nut07_checkstate".to_string());
        }
        SettlementDriverDescriptor {
            backend: PaymentBackend::Cashu.to_string(),
            mode: CASHU_VERIFIER_MODE.to_string(),
            accepted_payment_methods: vec!["cashu".to_string()],
            accepted_mints: state.config.cashu.mint_allowlist.clone(),
            capabilities,
            reservations: true,
            receipts: true,
        }
    }
}

impl SettlementDriver for CashuVerifierDriver {
    fn descriptor(&self, state: &AppState) -> SettlementDriverDescriptor {
        self.descriptor_inner(state)
    }

    fn wallet_balance<'a>(
        &'a self,
        state: &'a AppState,
    ) -> BoxFuture<'a, Result<WalletBalanceSnapshot, PaymentError>> {
        let descriptor = self.descriptor_inner(state);
        Box::pin(async move { Ok(WalletBalanceSnapshot::from_descriptor(descriptor)) })
    }

    fn prepare<'a>(
        &'a self,
        state: &'a AppState,
        request: PreparePaymentRequest,
    ) -> BoxFuture<'a, Result<Option<PaymentReservation>, PaymentError>> {
        let accepted_payment_methods = self.descriptor_inner(state).accepted_payment_methods;

        Box::pin(async move {
            if request.price_sats == 0 {
                return Ok(None);
            }

            let payment = request
                .payment
                .as_ref()
                .ok_or_else(|| PaymentError::PaymentRequired {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    accepted_payment_methods: accepted_payment_methods.clone(),
                })?;

            if payment.kind.to_lowercase() != "cashu" {
                return Err(PaymentError::UnsupportedKind {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    kind: payment.kind.clone(),
                    accepted_payment_methods,
                });
            }

            let token_info = ecash::inspect_cashu_token(&payment.token).map_err(|e| {
                PaymentError::InvalidToken {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    message: e.to_string(),
                }
            })?;
            verify_cashu_token_policies(state, &request, &token_info).await?;

            if token_info.amount_satoshis < request.price_sats {
                return Err(PaymentError::Underpaid {
                    service_id: request.service_id.as_str().to_string(),
                    price_sats: request.price_sats,
                    amount_sats: token_info.amount_satoshis,
                });
            }

            let request_id = request.request_id.unwrap_or_else(new_request_id);
            let token_hash = token_info.token_hash.clone();
            let amount_sats = token_info.amount_satoshis;
            let reserve_request_id = request_id.clone();
            let reserve_token_hash = token_hash.clone();
            let service_id = request.service_id;
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
                    method: "cashu".to_string(),
                    service_id: request.service_id,
                    amount_sats,
                    token_hash,
                })),
                ReservePaymentTokenOutcome::InUse => Err(PaymentError::InUse {
                    service_id: request.service_id.as_str().to_string(),
                    token_hash,
                }),
                ReservePaymentTokenOutcome::Replay => Err(PaymentError::Replay {
                    service_id: request.service_id.as_str().to_string(),
                    token_hash,
                }),
            }
        })
    }

    fn commit<'a>(
        &'a self,
        state: &'a AppState,
        reservation: PaymentReservation,
    ) -> BoxFuture<'a, Result<PaymentReceipt, PaymentError>> {
        Box::pin(async move {
            let token_hash = reservation.token_hash.clone();
            let request_id = reservation.request_id.clone();
            let committed = state
                .db
                .with_conn(move |conn| {
                    db::commit_payment_token(
                        conn,
                        &token_hash,
                        &request_id,
                        current_unix_timestamp(),
                    )
                })
                .await
                .map_err(PaymentError::Database)?;

            if !committed {
                return Err(PaymentError::Database(
                    "payment reservation could not be committed".to_string(),
                ));
            }

            Ok(reservation.receipt(SettlementStatus::Committed, reservation.amount_sats, None))
        })
    }

    fn release<'a>(
        &'a self,
        state: &'a AppState,
        reservation: &'a PaymentReservation,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let token_hash = reservation.token_hash.clone();
            let request_id = reservation.request_id.clone();
            state
                .db
                .with_conn(move |conn| {
                    db::release_payment_token(
                        conn,
                        &token_hash,
                        &request_id,
                        current_unix_timestamp(),
                    )
                })
                .await?;
            Ok(())
        })
    }
}

static NO_SETTLEMENT_DRIVER: NoSettlementDriver = NoSettlementDriver;
static CASHU_VERIFIER_DRIVER: CashuVerifierDriver = CashuVerifierDriver;
