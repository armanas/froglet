use crate::{
    config::{LightningMode, PaymentBackend},
    crypto,
    db::{self, LightningInvoiceBundleRecord, ReservePaymentTokenOutcome},
    deals, ecash,
    lnd::LndRestClient,
    pricing::ServiceId,
    protocol::{
        InvoiceBundleLeg, InvoiceBundleLegState, InvoiceBundlePayload, QuotePayload,
        QuoteSettlementTerms, SettlementStatus, SignedArtifact, TRANSPORT_KIND_INVOICE_BUNDLE,
        sign_artifact, verify_artifact,
    },
    state::AppState,
};
use axum::http::StatusCode;
use cashu::{
    MintUrl,
    nuts::nut07::{CheckStateRequest, CheckStateResponse, State as ProofState},
};
use futures::future::BoxFuture;
use lightning_invoice::Bolt11Invoice;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, str::FromStr, time::Duration};
use thiserror::Error;

pub const CASHU_VERIFIER_MODE: &str = "format_and_replay_guard";
pub const LIGHTNING_MOCK_MODE: &str = "mock_hold_invoice";
pub const LIGHTNING_LND_REST_MODE: &str = "lnd_rest";

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

#[derive(Debug, Clone)]
pub struct BuildLightningInvoiceBundleRequest {
    pub session_id: Option<String>,
    pub requester_id: String,
    pub quote_hash: String,
    pub deal_hash: String,
    pub quote_expires_at: Option<i64>,
    pub success_payment_hash: String,
    pub base_fee_msat: u64,
    pub success_fee_msat: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightningInvoiceBundleSession {
    pub session_id: String,
    pub bundle: SignedArtifact<InvoiceBundlePayload>,
    pub base_state: InvoiceBundleLegState,
    pub success_state: InvoiceBundleLegState,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvoiceBundleValidationIssue {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvoiceBundleValidationReport {
    pub valid: bool,
    pub bundle_hash: String,
    pub quote_hash: String,
    pub deal_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_requester_id: Option<String>,
    pub issues: Vec<InvoiceBundleValidationIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LightningWalletPaymentRequest {
    pub role: String,
    pub invoice: String,
    pub amount_msat: u64,
    pub payment_hash: String,
    pub state: InvoiceBundleLegState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LightningWalletReleaseAction {
    pub endpoint_path: String,
    pub payment_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_result_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LightningWalletIntent {
    pub backend: String,
    pub mode: String,
    pub session_id: String,
    pub bundle_hash: String,
    pub deal_id: String,
    pub deal_status: String,
    pub quote_hash: String,
    pub deal_hash: String,
    pub destination_identity: String,
    pub admission_ready: bool,
    pub result_ready: bool,
    pub can_release_preimage: bool,
    pub payment_requests: Vec<LightningWalletPaymentRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_action: Option<LightningWalletReleaseAction>,
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
        PaymentBackend::Lightning => &LIGHTNING_DRIVER,
    }
}

fn new_request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub async fn create_lightning_invoice_bundle(
    state: &AppState,
    request: BuildLightningInvoiceBundleRequest,
) -> Result<LightningInvoiceBundleSession, String> {
    let session = issue_lightning_invoice_bundle(state, request).await?;
    let session_id_for_db = session.session_id.clone();
    let bundle_for_db = session.bundle.clone();
    let created_at = session.created_at;
    let base_state = session.base_state.clone();
    let success_state = session.success_state.clone();
    state
        .db
        .with_conn(move |conn| {
            db::insert_lightning_invoice_bundle(
                conn,
                &session_id_for_db,
                &bundle_for_db,
                base_state,
                success_state,
                created_at,
            )
        })
        .await?;
    Ok(session)
}

pub async fn get_lightning_invoice_bundle(
    state: &AppState,
    session_id: &str,
) -> Result<Option<LightningInvoiceBundleSession>, String> {
    let session_id_for_db = session_id.to_string();
    let session = state
        .db
        .with_conn(move |conn| {
            db::get_lightning_invoice_bundle(conn, &session_id_for_db)
                .map(|record| record.map(map_lightning_bundle_record))
        })
        .await?;
    match session {
        Some(session) => sync_lightning_invoice_bundle_session(state, session)
            .await
            .map(Some),
        None => Ok(None),
    }
}

pub async fn get_lightning_invoice_bundle_by_deal_hash(
    state: &AppState,
    deal_hash: &str,
) -> Result<Option<LightningInvoiceBundleSession>, String> {
    let deal_hash_for_db = deal_hash.to_string();
    let session = state
        .db
        .with_conn(move |conn| {
            db::get_lightning_invoice_bundle_by_deal_hash(conn, &deal_hash_for_db)
                .map(|record| record.map(map_lightning_bundle_record))
        })
        .await?;
    match session {
        Some(session) => sync_lightning_invoice_bundle_session(state, session)
            .await
            .map(Some),
        None => Ok(None),
    }
}

pub async fn update_lightning_invoice_bundle_states(
    state: &AppState,
    session_id: &str,
    base_state: InvoiceBundleLegState,
    success_state: InvoiceBundleLegState,
) -> Result<Option<LightningInvoiceBundleSession>, String> {
    let session_id_for_update = session_id.to_string();
    let session_id_for_read = session_id.to_string();
    let now = current_unix_timestamp();
    state
        .db
        .with_conn(move |conn| {
            if !db::update_lightning_invoice_bundle_states(
                conn,
                &session_id_for_update,
                base_state.clone(),
                success_state.clone(),
                now,
            )? {
                return Ok(None);
            }

            db::get_lightning_invoice_bundle(conn, &session_id_for_read)
                .map(|record| record.map(map_lightning_bundle_record))
        })
        .await
}

fn map_lightning_bundle_record(
    record: LightningInvoiceBundleRecord,
) -> LightningInvoiceBundleSession {
    LightningInvoiceBundleSession {
        session_id: record.session_id,
        bundle: record.bundle,
        base_state: record.base_state,
        success_state: record.success_state,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn expire_open_invoice_legs_if_due(
    session: LightningInvoiceBundleSession,
    now: i64,
) -> Result<Option<(InvoiceBundleLegState, InvoiceBundleLegState)>, String> {
    let mut base_state = session.base_state.clone();
    let mut success_state = session.success_state.clone();

    if matches!(base_state, InvoiceBundleLegState::Open) {
        let decoded =
            decode_lightning_invoice(&session.bundle.payload.base_fee.invoice_bolt11)?;
        if now >= decoded.expires_at {
            base_state = InvoiceBundleLegState::Expired;
        }
    }

    if matches!(success_state, InvoiceBundleLegState::Open) {
        let decoded =
            decode_lightning_invoice(&session.bundle.payload.success_fee.invoice_bolt11)?;
        if now >= decoded.expires_at {
            success_state = InvoiceBundleLegState::Expired;
        }
    }

    if base_state == session.base_state && success_state == session.success_state {
        Ok(None)
    } else {
        Ok(Some((base_state, success_state)))
    }
}

pub async fn issue_lightning_invoice_bundle(
    state: &AppState,
    request: BuildLightningInvoiceBundleRequest,
) -> Result<LightningInvoiceBundleSession, String> {
    match state.config.lightning.mode {
        LightningMode::Mock => build_lightning_invoice_bundle(state, request),
        LightningMode::LndRest => issue_lnd_rest_invoice_bundle(state, request).await,
    }
}

pub async fn sync_lightning_invoice_bundle_session(
    state: &AppState,
    session: LightningInvoiceBundleSession,
) -> Result<LightningInvoiceBundleSession, String> {
    let session = match state.config.lightning.mode {
        LightningMode::Mock => session,
        LightningMode::LndRest => {
            let client = lnd_rest_client(state)?;
            let base_state = if session.bundle.payload.base_fee.amount_msat == 0 {
                InvoiceBundleLegState::Settled
            } else {
                map_invoice_state(
                    client
                        .lookup_invoice(&session.bundle.payload.base_fee.payment_hash)
                        .await
                        .map_err(|error| error.to_string())?
                        .state,
                )
            };
            let success_state = map_invoice_state(
                client
                    .lookup_invoice(&session.bundle.payload.success_fee.payment_hash)
                    .await
                    .map_err(|error| error.to_string())?
                    .state,
            );

            if base_state == session.base_state && success_state == session.success_state {
                session
            } else {
                let Some(updated) = update_lightning_invoice_bundle_states(
                    state,
                    &session.session_id,
                    base_state,
                    success_state,
                )
                .await?
                else {
                    return Err("lightning invoice bundle disappeared during sync".to_string());
                };

                updated
            }
        }
    };

    let Some((base_state, success_state)) =
        expire_open_invoice_legs_if_due(session.clone(), current_unix_timestamp())?
    else {
        return Ok(session);
    };

    let Some(updated) = update_lightning_invoice_bundle_states(
        state,
        &session.session_id,
        base_state,
        success_state,
    )
    .await?
    else {
        return Err("lightning invoice bundle disappeared during expiry normalization".to_string());
    };

    Ok(updated)
}

pub async fn settle_lightning_success_hold_invoice(
    state: &AppState,
    session: &LightningInvoiceBundleSession,
    success_preimage_hex: &str,
) -> Result<LightningInvoiceBundleSession, String> {
    match state.config.lightning.mode {
        LightningMode::Mock => {
            let Some(updated) = update_lightning_invoice_bundle_states(
                state,
                &session.session_id,
                session.base_state.clone(),
                InvoiceBundleLegState::Settled,
            )
            .await?
            else {
                return Err("lightning invoice bundle not found".to_string());
            };
            Ok(updated)
        }
        LightningMode::LndRest => {
            let client = lnd_rest_client(state)?;
            client
                .settle_invoice(success_preimage_hex)
                .await
                .map_err(|error| error.to_string())?;
            let refreshed = sync_lightning_invoice_bundle_session(state, session.clone()).await?;
            if refreshed.success_state != InvoiceBundleLegState::Settled {
                return Err("success hold invoice did not reach settled state".to_string());
            }
            Ok(refreshed)
        }
    }
}

fn lnd_rest_client(state: &AppState) -> Result<LndRestClient, String> {
    let Some(config) = state.config.lightning.lnd_rest.as_ref() else {
        return Err("missing lnd_rest configuration".to_string());
    };
    LndRestClient::from_config(config).map_err(|error| error.to_string())
}

fn configured_lightning_destination_identity(state: &AppState) -> String {
    state
        .config
        .lightning
        .destination_identity
        .clone()
        .unwrap_or_else(|| state.identity.compressed_public_key_hex().to_string())
}

pub async fn resolve_lightning_destination_identity(state: &AppState) -> Result<String, String> {
    if let Some(destination_identity) = state.config.lightning.destination_identity.clone() {
        return Ok(destination_identity);
    }

    match state.config.lightning.mode {
        LightningMode::Mock => Ok(configured_lightning_destination_identity(state)),
        LightningMode::LndRest => {
            let client = lnd_rest_client(state)?;
            client
                .get_info()
                .await
                .map(|info| info.identity_pubkey)
                .map_err(|error| error.to_string())
        }
    }
}

fn mock_bolt11(prefix: &str, amount_msat: u64, payment_hash: &str, expires_at: i64) -> String {
    format!("lnmock-{prefix}-{amount_msat}-{payment_hash}-{expires_at}")
}

pub async fn quoted_lightning_settlement_terms(
    state: &AppState,
    price_sats: u64,
) -> Result<Option<QuoteSettlementTerms>, String> {
    if state.config.payment_backend != PaymentBackend::Lightning || price_sats == 0 {
        return Ok(None);
    }

    Ok(Some(QuoteSettlementTerms {
        method: "lightning.base_fee_plus_success_fee.v1".to_string(),
        destination_identity: resolve_lightning_destination_identity(state).await?,
        base_fee_msat: 0,
        success_fee_msat: price_sats.saturating_mul(1_000),
        max_base_invoice_expiry_secs: state.config.lightning.base_invoice_expiry_secs,
        max_success_hold_expiry_secs: state.config.lightning.success_hold_expiry_secs,
        min_final_cltv_expiry: state.config.lightning.min_final_cltv_expiry,
    }))
}

pub fn lightning_quote_expires_at(state: &AppState, created_at: i64, price_sats: u64) -> i64 {
    if state.config.payment_backend == PaymentBackend::Lightning && price_sats > 0 {
        created_at
            + state
                .config
                .lightning
                .base_invoice_expiry_secs
                .max(state.config.lightning.success_hold_expiry_secs) as i64
    } else {
        created_at + 60
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MockBolt11Fields {
    prefix: String,
    amount_msat: u64,
    payment_hash: String,
    expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DecodedLightningInvoice {
    amount_msat: u64,
    payment_hash: String,
    expires_at: i64,
    destination_identity: String,
    min_final_cltv_expiry: u32,
}

fn parse_mock_bolt11(invoice: &str) -> Result<MockBolt11Fields, String> {
    let Some(rest) = invoice.strip_prefix("lnmock-") else {
        return Err("invoice is not in Froglet mock Lightning format".to_string());
    };
    let mut parts = rest.splitn(4, '-');
    let prefix = parts
        .next()
        .ok_or_else(|| "missing invoice prefix".to_string())?;
    let amount_msat = parts
        .next()
        .ok_or_else(|| "missing invoice amount".to_string())?
        .parse::<u64>()
        .map_err(|_| "invalid invoice amount".to_string())?;
    let payment_hash = parts
        .next()
        .ok_or_else(|| "missing invoice payment hash".to_string())?
        .to_string();
    let expires_at = parts
        .next()
        .ok_or_else(|| "missing invoice expiry".to_string())?
        .parse::<i64>()
        .map_err(|_| "invalid invoice expiry".to_string())?;

    Ok(MockBolt11Fields {
        prefix: prefix.to_string(),
        amount_msat,
        payment_hash,
        expires_at,
    })
}

fn decode_lightning_invoice(invoice: &str) -> Result<DecodedLightningInvoice, String> {
    if let Ok(mock) = parse_mock_bolt11(invoice) {
        return Ok(DecodedLightningInvoice {
            amount_msat: mock.amount_msat,
            payment_hash: mock.payment_hash,
            expires_at: mock.expires_at,
            destination_identity: String::new(),
            min_final_cltv_expiry: 0,
        });
    }

    let invoice = invoice
        .parse::<Bolt11Invoice>()
        .map_err(|error| error.to_string())?;
    let amount_msat = invoice
        .amount_milli_satoshis()
        .ok_or_else(|| "invoice is missing an amount".to_string())?;
    let expires_at = invoice
        .expires_at()
        .ok_or_else(|| "invoice expiry overflowed".to_string())?
        .as_secs() as i64;
    let destination_identity = hex::encode(invoice.get_payee_pub_key().serialize());

    Ok(DecodedLightningInvoice {
        amount_msat,
        payment_hash: invoice.payment_hash().to_string(),
        expires_at,
        destination_identity,
        min_final_cltv_expiry: invoice.min_final_cltv_expiry_delta() as u32,
    })
}

fn map_invoice_state(state: crate::lnd::InvoiceState) -> InvoiceBundleLegState {
    match state {
        crate::lnd::InvoiceState::Open => InvoiceBundleLegState::Open,
        crate::lnd::InvoiceState::Accepted => InvoiceBundleLegState::Accepted,
        crate::lnd::InvoiceState::Settled => InvoiceBundleLegState::Settled,
        crate::lnd::InvoiceState::Canceled => InvoiceBundleLegState::Canceled,
    }
}

pub fn lightning_bundle_is_funded(session: &LightningInvoiceBundleSession) -> bool {
    matches!(session.base_state, InvoiceBundleLegState::Settled)
        && matches!(
            session.success_state,
            InvoiceBundleLegState::Accepted | InvoiceBundleLegState::Settled
        )
}

pub fn lightning_bundle_can_settle_success(session: &LightningInvoiceBundleSession) -> bool {
    matches!(session.base_state, InvoiceBundleLegState::Settled)
        && matches!(
            session.success_state,
            InvoiceBundleLegState::Accepted | InvoiceBundleLegState::Settled
        )
}

pub fn build_lightning_wallet_intent(
    state: &AppState,
    deal_id: &str,
    deal_status: &str,
    result_hash: Option<&str>,
    session: &LightningInvoiceBundleSession,
) -> LightningWalletIntent {
    let mut payment_requests = Vec::new();

    if session.bundle.payload.base_fee.amount_msat > 0 {
        payment_requests.push(LightningWalletPaymentRequest {
            role: "base_fee".to_string(),
            invoice: session.bundle.payload.base_fee.invoice_bolt11.clone(),
            amount_msat: session.bundle.payload.base_fee.amount_msat,
            payment_hash: session.bundle.payload.base_fee.payment_hash.clone(),
            state: session.base_state.clone(),
        });
    }

    payment_requests.push(LightningWalletPaymentRequest {
        role: "success_fee_hold".to_string(),
        invoice: session
            .bundle
            .payload
            .success_fee
            .invoice_bolt11
            .clone(),
        amount_msat: session.bundle.payload.success_fee.amount_msat,
        payment_hash: session
            .bundle
            .payload
            .success_fee
            .payment_hash
            .clone(),
        state: session.success_state.clone(),
    });

    let result_ready = deal_status == deals::DEAL_STATUS_RESULT_READY;
    let can_release_preimage = result_ready && lightning_bundle_can_settle_success(session);
    let release_action = can_release_preimage.then(|| LightningWalletReleaseAction {
        endpoint_path: format!("/v1/deals/{deal_id}/release-preimage"),
        payment_hash: session
            .bundle
            .payload
            .success_fee
            .payment_hash
            .clone(),
        expected_result_hash: result_hash.map(str::to_string),
    });

    LightningWalletIntent {
        backend: PaymentBackend::Lightning.to_string(),
        mode: match state.config.lightning.mode {
            LightningMode::Mock => LIGHTNING_MOCK_MODE.to_string(),
            LightningMode::LndRest => LIGHTNING_LND_REST_MODE.to_string(),
        },
        session_id: session.session_id.clone(),
        bundle_hash: session.bundle.hash.clone(),
        deal_id: deal_id.to_string(),
        deal_status: deal_status.to_string(),
        quote_hash: session.bundle.payload.quote_hash.clone(),
        deal_hash: session.bundle.payload.deal_hash.clone(),
        destination_identity: session.bundle.payload.destination_identity.clone(),
        admission_ready: lightning_bundle_is_funded(session),
        result_ready,
        can_release_preimage,
        payment_requests,
        release_action,
    }
}

fn sign_lightning_invoice_bundle(
    state: &AppState,
    session_id: String,
    provider_id: String,
    request: BuildLightningInvoiceBundleRequest,
    base_invoice_expiry_secs: u64,
    success_hold_expiry_secs: u64,
    destination_identity: String,
    base_invoice_bolt11: String,
    base_payment_hash: String,
    base_state: InvoiceBundleLegState,
    success_hold_invoice_bolt11: String,
    success_state: InvoiceBundleLegState,
) -> Result<LightningInvoiceBundleSession, String> {
    let base_expires_at = request.created_at + base_invoice_expiry_secs as i64;
    let success_expires_at = request.created_at + success_hold_expiry_secs as i64;
    let bundle_expires_at = base_expires_at.max(success_expires_at);
    let bundle = sign_artifact(
        &provider_id,
        |message| state.identity.sign_message_hex(message),
        TRANSPORT_KIND_INVOICE_BUNDLE,
        request.created_at,
        InvoiceBundlePayload {
            provider_id: provider_id.clone(),
            requester_id: request.requester_id.clone(),
            quote_hash: request.quote_hash.clone(),
            deal_hash: request.deal_hash.clone(),
            expires_at: bundle_expires_at,
            destination_identity,
            base_fee: InvoiceBundleLeg {
                amount_msat: request.base_fee_msat,
                invoice_bolt11: base_invoice_bolt11.clone(),
                invoice_hash: crypto::sha256_hex(base_invoice_bolt11.as_bytes()),
                payment_hash: base_payment_hash,
                state: base_state.clone(),
            },
            success_fee: InvoiceBundleLeg {
                amount_msat: request.success_fee_msat,
                invoice_bolt11: success_hold_invoice_bolt11.clone(),
                invoice_hash: crypto::sha256_hex(success_hold_invoice_bolt11.as_bytes()),
                payment_hash: request.success_payment_hash.clone(),
                state: success_state.clone(),
            },
            min_final_cltv_expiry: state.config.lightning.min_final_cltv_expiry,
        },
    )?;

    Ok(LightningInvoiceBundleSession {
        session_id,
        bundle,
        base_state,
        success_state,
        created_at: request.created_at,
        updated_at: request.created_at,
    })
}

fn effective_bundle_expiry_secs(
    state: &AppState,
    request: &BuildLightningInvoiceBundleRequest,
) -> Result<(u64, u64), String> {
    let mut base_invoice_expiry_secs = state.config.lightning.base_invoice_expiry_secs;
    let mut success_hold_expiry_secs = state.config.lightning.success_hold_expiry_secs;

    if let Some(quote_expires_at) = request.quote_expires_at {
        let remaining_secs = quote_expires_at.saturating_sub(request.created_at);
        if remaining_secs <= 0 {
            return Err("quote expired before lightning invoice bundle issuance".to_string());
        }
        let remaining_secs = remaining_secs as u64;
        base_invoice_expiry_secs = base_invoice_expiry_secs.min(remaining_secs);
        success_hold_expiry_secs = success_hold_expiry_secs.min(remaining_secs);
    }

    Ok((base_invoice_expiry_secs, success_hold_expiry_secs))
}

fn push_bundle_issue(
    issues: &mut Vec<InvoiceBundleValidationIssue>,
    code: &str,
    message: impl Into<String>,
) {
    issues.push(InvoiceBundleValidationIssue {
        code: code.to_string(),
        message: message.into(),
    });
}

pub fn validate_lightning_invoice_bundle(
    bundle: &SignedArtifact<InvoiceBundlePayload>,
    quote: &SignedArtifact<QuotePayload>,
    deal: &SignedArtifact<crate::protocol::DealPayload>,
    expected_requester_id: Option<&str>,
) -> InvoiceBundleValidationReport {
    let mut issues = Vec::new();

    if !verify_artifact(bundle) {
        push_bundle_issue(
            &mut issues,
            "invalid_bundle_signature",
            "invoice bundle signature is invalid",
        );
    }
    if !verify_artifact(quote) {
        push_bundle_issue(
            &mut issues,
            "invalid_quote_signature",
            "quote signature is invalid",
        );
    }
    if !verify_artifact(deal) {
        push_bundle_issue(
            &mut issues,
            "invalid_deal_signature",
            "deal signature is invalid",
        );
    }

    if bundle.artifact_type != TRANSPORT_KIND_INVOICE_BUNDLE {
        push_bundle_issue(
            &mut issues,
            "bundle_kind_mismatch",
            format!("expected bundle kind '{TRANSPORT_KIND_INVOICE_BUNDLE}'"),
        );
    }
    if quote.payload.provider_id != quote.signer
        || bundle.payload.provider_id != quote.payload.provider_id
        || bundle.signer != quote.signer
    {
        push_bundle_issue(
            &mut issues,
            "provider_identity_mismatch",
            "invoice bundle provider identity does not match the quoted provider",
        );
    }
    if deal.payload.provider_id != quote.payload.provider_id {
        push_bundle_issue(
            &mut issues,
            "deal_provider_mismatch",
            "deal provider does not match the quoted provider",
        );
    }
    if bundle.payload.quote_hash != quote.hash {
        push_bundle_issue(
            &mut issues,
            "quote_hash_mismatch",
            "invoice bundle quote_hash does not match the quote artifact hash",
        );
    }
    if bundle.payload.deal_hash != deal.hash {
        push_bundle_issue(
            &mut issues,
            "deal_hash_mismatch",
            "invoice bundle deal_hash does not match the deal artifact hash",
        );
    }
    if let Some(expected_requester_id) = expected_requester_id {
        if bundle.payload.requester_id != expected_requester_id {
            push_bundle_issue(
                &mut issues,
                "requester_id_mismatch",
                "invoice bundle requester_id does not match the expected requester",
            );
        }
    }

    if quote.payload.settlement_terms.method != "lightning.base_fee_plus_success_fee.v1" {
        push_bundle_issue(
            &mut issues,
            "quote_payment_method_mismatch",
            "quote does not advertise lightning settlement",
        );
    }

    let settlement_terms = &quote.payload.settlement_terms;

    if deal.signer != deal.payload.requester_id
        || deal.payload.requester_id != quote.payload.requester_id
        || bundle.payload.requester_id != quote.payload.requester_id
    {
        push_bundle_issue(
            &mut issues,
            "requester_identity_mismatch",
            "deal or bundle requester does not match the quoted requester",
        );
    }

    if deal.payload.quote_hash != quote.hash
        || deal.payload.workload_hash != quote.payload.workload_hash
    {
        push_bundle_issue(
            &mut issues,
            "deal_quote_binding_mismatch",
            "deal artifact does not match the quoted workload commitment",
        );
    }

    if bundle.payload.destination_identity != settlement_terms.destination_identity {
        push_bundle_issue(
            &mut issues,
            "destination_identity_mismatch",
            "invoice bundle destination identity does not match the quoted settlement destination",
        );
    }
    if bundle.payload.base_fee.amount_msat != settlement_terms.base_fee_msat {
        push_bundle_issue(
            &mut issues,
            "base_fee_mismatch",
            "invoice bundle base fee does not match the quote settlement terms",
        );
    }
    if bundle.payload.success_fee.amount_msat != settlement_terms.success_fee_msat {
        push_bundle_issue(
            &mut issues,
            "success_fee_mismatch",
            "invoice bundle success fee does not match the quote settlement terms",
        );
    }
    if bundle.payload.min_final_cltv_expiry != settlement_terms.min_final_cltv_expiry {
        push_bundle_issue(
            &mut issues,
            "min_final_cltv_mismatch",
            "invoice bundle CLTV requirement does not match the quote settlement terms",
        );
    }
    if bundle.payload.expires_at > quote.payload.expires_at {
        push_bundle_issue(
            &mut issues,
            "bundle_expiry_exceeds_quote",
            "invoice bundle expires after the quote deadline",
        );
    }

    if deal.payload.success_payment_hash != bundle.payload.success_fee.payment_hash {
        push_bundle_issue(
            &mut issues,
            "success_payment_hash_mismatch",
            "invoice bundle success payment hash does not match the deal commitment",
        );
    }

    for (expected_prefix, leg, max_expiry_secs, leg_name) in [
        (
            "base",
            &bundle.payload.base_fee,
            settlement_terms.max_base_invoice_expiry_secs,
            "base_fee",
        ),
        (
            "hold",
            &bundle.payload.success_fee,
            settlement_terms.max_success_hold_expiry_secs,
            "success_fee",
        ),
    ] {
        let expected_invoice_hash = crypto::sha256_hex(leg.invoice_bolt11.as_bytes());
        if leg.invoice_hash != expected_invoice_hash {
            push_bundle_issue(
                &mut issues,
                "invoice_hash_mismatch",
                format!("{leg_name} invoice_hash does not match the encoded invoice"),
            );
        }

        match decode_lightning_invoice(&leg.invoice_bolt11) {
            Ok(decoded) => {
                if leg.invoice_bolt11.starts_with("lnmock-") {
                    let mock = parse_mock_bolt11(&leg.invoice_bolt11)
                        .expect("mock invoices were decoded successfully above");
                    if mock.prefix != expected_prefix {
                        push_bundle_issue(
                            &mut issues,
                            "invoice_prefix_mismatch",
                            format!(
                                "{leg_name} invoice prefix does not match the expected leg type"
                            ),
                        );
                    }
                }
                if decoded.amount_msat != leg.amount_msat {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_amount_mismatch",
                        format!("{leg_name} invoice amount does not match the signed bundle"),
                    );
                }
                if decoded.payment_hash != leg.payment_hash {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_payment_hash_mismatch",
                        format!("{leg_name} invoice payment hash does not match the signed bundle"),
                    );
                }
                if !decoded.destination_identity.is_empty()
                    && decoded.destination_identity != bundle.payload.destination_identity
                {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_destination_mismatch",
                        format!(
                            "{leg_name} invoice payee identity does not match the signed bundle"
                        ),
                    );
                }
                if !leg.invoice_bolt11.starts_with("lnmock-")
                    && expected_prefix == "hold"
                    && decoded.min_final_cltv_expiry < settlement_terms.min_final_cltv_expiry
                {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_min_final_cltv_too_small",
                        format!(
                            "{leg_name} invoice min_final_cltv_expiry is below the quoted settlement constraint"
                        ),
                    );
                }
                if decoded.expires_at > bundle.created_at + max_expiry_secs as i64 {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_expiry_exceeds_terms",
                        format!(
                            "{leg_name} invoice expiry exceeds the quoted settlement constraints"
                        ),
                    );
                }
                if decoded.expires_at > quote.payload.expires_at {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_expiry_exceeds_quote",
                        format!("{leg_name} invoice expiry exceeds the quote deadline"),
                    );
                }
            }
            Err(error) => push_bundle_issue(
                &mut issues,
                "invalid_invoice_encoding",
                format!("{leg_name}: {error}"),
            ),
        }
    }

    InvoiceBundleValidationReport {
        valid: issues.is_empty(),
        bundle_hash: bundle.hash.clone(),
        quote_hash: quote.hash.clone(),
        deal_hash: deal.hash.clone(),
        expected_requester_id: expected_requester_id.map(str::to_string),
        issues,
    }
}

pub fn build_lightning_invoice_bundle(
    state: &AppState,
    request: BuildLightningInvoiceBundleRequest,
) -> Result<LightningInvoiceBundleSession, String> {
    let (base_invoice_expiry_secs, success_hold_expiry_secs) =
        effective_bundle_expiry_secs(state, &request)?;
    let session_id = request.session_id.clone().unwrap_or_else(new_request_id);
    let provider_id = state.identity.node_id().to_string();
    let destination_identity = configured_lightning_destination_identity(state);
    let base_payment_hash = crypto::sha256_hex(format!("lightning-base:{session_id}").as_bytes());
    let base_expires_at = request.created_at + base_invoice_expiry_secs as i64;
    let success_expires_at = request.created_at + success_hold_expiry_secs as i64;
    let base_invoice_bolt11 = mock_bolt11(
        "base",
        request.base_fee_msat,
        &base_payment_hash,
        base_expires_at,
    );
    let success_hold_invoice_bolt11 = mock_bolt11(
        "hold",
        request.success_fee_msat,
        &request.success_payment_hash,
        success_expires_at,
    );
    sign_lightning_invoice_bundle(
        state,
        session_id,
        provider_id,
        request,
        base_invoice_expiry_secs,
        success_hold_expiry_secs,
        destination_identity,
        base_invoice_bolt11,
        base_payment_hash,
        InvoiceBundleLegState::Open,
        success_hold_invoice_bolt11,
        InvoiceBundleLegState::Open,
    )
}

async fn issue_lnd_rest_invoice_bundle(
    state: &AppState,
    request: BuildLightningInvoiceBundleRequest,
) -> Result<LightningInvoiceBundleSession, String> {
    let client = lnd_rest_client(state)?;
    let (base_invoice_expiry_secs, success_hold_expiry_secs) =
        effective_bundle_expiry_secs(state, &request)?;
    let session_id = request.session_id.clone().unwrap_or_else(new_request_id);
    let provider_id = state.identity.node_id().to_string();
    let destination_identity = resolve_lightning_destination_identity(state).await?;
    let mut base_state = InvoiceBundleLegState::Open;
    let (base_payment_hash, base_invoice_bolt11) = if request.base_fee_msat == 0 {
        base_state = InvoiceBundleLegState::Settled;
        let payment_hash = crypto::sha256_hex(format!("lightning-base:{session_id}").as_bytes());
        let invoice_bolt11 = mock_bolt11(
            "base",
            request.base_fee_msat,
            &payment_hash,
            request.created_at + base_invoice_expiry_secs as i64,
        );
        (payment_hash, invoice_bolt11)
    } else {
        let base_invoice = client
            .add_invoice(
                request.base_fee_msat,
                base_invoice_expiry_secs,
                &format!("froglet base fee {}", session_id),
                true,
            )
            .await
            .map_err(|error| error.to_string())?;
        (base_invoice.payment_hash_hex, base_invoice.payment_request)
    };
    let success_invoice = client
        .add_hold_invoice(
            &request.success_payment_hash,
            request.success_fee_msat,
            success_hold_expiry_secs,
            state.config.lightning.min_final_cltv_expiry,
            &format!("froglet success fee {}", session_id),
            true,
        )
        .await
        .map_err(|error| error.to_string())?;

    if request.base_fee_msat > 0 {
        let decoded_base = decode_lightning_invoice(&base_invoice_bolt11)?;
        if decoded_base.amount_msat != request.base_fee_msat {
            return Err("LND base invoice amount did not match the requested amount".to_string());
        }
        if decoded_base.destination_identity != destination_identity {
            return Err(
                "LND base invoice destination did not match the provider identity".to_string(),
            );
        }
    }

    let decoded_success = decode_lightning_invoice(&success_invoice.payment_request)?;
    if decoded_success.amount_msat != request.success_fee_msat {
        return Err(
            "LND success hold invoice amount did not match the requested amount".to_string(),
        );
    }
    if decoded_success.payment_hash != request.success_payment_hash {
        return Err(
            "LND success hold invoice payment hash did not match the deal payment lock".to_string(),
        );
    }
    if decoded_success.destination_identity != destination_identity {
        return Err(
            "LND success hold invoice destination did not match the provider identity".to_string(),
        );
    }
    if decoded_success.min_final_cltv_expiry < state.config.lightning.min_final_cltv_expiry {
        return Err(
            "LND success hold invoice min_final_cltv_expiry was below the configured floor"
                .to_string(),
        );
    }

    sign_lightning_invoice_bundle(
        state,
        session_id,
        provider_id,
        request,
        base_invoice_expiry_secs,
        success_hold_expiry_secs,
        destination_identity,
        base_invoice_bolt11,
        base_payment_hash,
        base_state,
        success_invoice.payment_request,
        InvoiceBundleLegState::Open,
    )
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
        .map_err(|e| {
            invalid_cashu_token(request, format!("mint checkstate request failed: {e}"))
        })?;

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
        Box::pin(async move {
            Ok(WalletBalanceSnapshot::from_descriptor(
                self.descriptor(state),
            ))
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

            let payment =
                request
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

struct LightningDriver;

impl LightningDriver {
    fn descriptor_inner(&self, state: &AppState) -> SettlementDriverDescriptor {
        let mode = match state.config.lightning.mode {
            LightningMode::Mock => LIGHTNING_MOCK_MODE,
            LightningMode::LndRest => LIGHTNING_LND_REST_MODE,
        };
        let mut capabilities = vec!["invoice_bundles".to_string(), "hold_invoices".to_string()];
        match state.config.lightning.mode {
            LightningMode::Mock => capabilities.push("mock_mode".to_string()),
            LightningMode::LndRest => {
                capabilities.push("lnd_rest".to_string());
                capabilities.push("node_getinfo".to_string());
            }
        }
        SettlementDriverDescriptor {
            backend: PaymentBackend::Lightning.to_string(),
            mode: mode.to_string(),
            accepted_payment_methods: vec!["lightning".to_string()],
            accepted_mints: Vec::new(),
            capabilities,
            reservations: true,
            receipts: true,
        }
    }
}

impl SettlementDriver for LightningDriver {
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
                backend: PaymentBackend::Lightning.to_string(),
            })
        })
    }

    fn commit<'a>(
        &'a self,
        _state: &'a AppState,
        reservation: PaymentReservation,
    ) -> BoxFuture<'a, Result<PaymentReceipt, PaymentError>> {
        Box::pin(async move {
            Err(PaymentError::BackendUnavailable {
                service_id: reservation.service_id.as_str().to_string(),
                price_sats: reservation.amount_sats,
                backend: PaymentBackend::Lightning.to_string(),
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

static NO_SETTLEMENT_DRIVER: NoSettlementDriver = NoSettlementDriver;
static CASHU_VERIFIER_DRIVER: CashuVerifierDriver = CashuVerifierDriver;
static LIGHTNING_DRIVER: LightningDriver = LightningDriver;
