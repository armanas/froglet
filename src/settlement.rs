use crate::{
    config::{LightningMode, PaymentBackend},
    crypto,
    db::{self, LightningInvoiceBundleRecord},
    deals,
    lnd::{LndRestClient, LndRestError},
    pricing::ServiceId,
    protocol::{
        InvoiceBundleLeg, InvoiceBundleLegState, InvoiceBundlePayload, QuotePayload,
        QuoteSettlementTerms, SettlementStatus, SignedArtifact, TRANSPORT_KIND_INVOICE_BUNDLE,
        sign_artifact, verify_artifact,
    },
    state::AppState,
};
use axum::http::StatusCode;
use futures::future::BoxFuture;
use lightning_invoice::Bolt11Invoice;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const LIGHTNING_MOCK_MODE: &str = "mock_hold_invoice";
pub const LIGHTNING_LND_REST_MODE: &str = "lnd_rest";
const LND_INVOICE_EXPIRY_GUARD_SECS: u64 = 5;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildLightningInvoiceBundleRequest {
    pub session_id: Option<String>,
    pub requester_id: String,
    pub quote_hash: String,
    pub deal_hash: String,
    pub admission_deadline: Option<i64>,
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
pub struct LightningWalletMockAction {
    pub endpoint_path: String,
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
    pub mock_action: Option<LightningWalletMockAction>,
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
    fn from_descriptor(descriptor: SettlementDriverDescriptor) -> Self {
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
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => {
            tracing::error!("System clock is before the Unix epoch: {error}");
            0
        }
    }
}

fn selected_driver(state: &AppState) -> &'static dyn SettlementDriver {
    match state.config.payment_backend {
        PaymentBackend::None => &NO_SETTLEMENT_DRIVER,
        PaymentBackend::Lightning => &LIGHTNING_DRIVER,
    }
}

fn new_request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn deterministic_base_fee_preimage_hex(state: &AppState, session_id: &str) -> String {
    state
        .identity
        .keyed_hmac_hex(format!("lightning-base-preimage:{session_id}").as_bytes())
}

fn deterministic_base_fee_payment_hash(
    state: &AppState,
    session_id: &str,
) -> Result<String, String> {
    let preimage_bytes = hex::decode(deterministic_base_fee_preimage_hex(state, session_id))
        .map_err(|e| format!("invalid hex preimage: {e}"))?;
    Ok(crypto::sha256_hex(preimage_bytes))
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
    let insert_result = state
        .db
        .with_write_conn(move |conn| {
            db::insert_lightning_invoice_bundle(
                conn,
                &session_id_for_db,
                &bundle_for_db,
                base_state,
                success_state,
                created_at,
            )
        })
        .await;
    if let Err(error) = insert_result {
        if let Err(cancel_error) = cancel_lightning_invoice_bundle(state, &session).await {
            return Err(format!(
                "{error}; additionally failed to cancel issued lightning invoices: {cancel_error}"
            ));
        }
        return Err(error);
    }
    Ok(session)
}

pub async fn get_lightning_invoice_bundle(
    state: &AppState,
    session_id: &str,
) -> Result<Option<LightningInvoiceBundleSession>, String> {
    let session_id_for_db = session_id.to_string();
    let session = state
        .db
        .with_read_conn(move |conn| {
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
        .with_read_conn(move |conn| {
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
        .with_write_conn(move |conn| {
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
        let decoded = decode_lightning_invoice(&session.bundle.payload.base_fee.invoice_bolt11)?;
        if now >= decoded.expires_at {
            base_state = InvoiceBundleLegState::Expired;
        }
    }

    if matches!(success_state, InvoiceBundleLegState::Open) {
        let decoded = decode_lightning_invoice(&session.bundle.payload.success_fee.invoice_bolt11)?;
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
            let mut base_state = if session.bundle.payload.base_fee.amount_msat == 0 {
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
            if matches!(base_state, InvoiceBundleLegState::Accepted) {
                match client
                    .settle_invoice(&deterministic_base_fee_preimage_hex(
                        state,
                        &session.session_id,
                    ))
                    .await
                {
                    Ok(()) => {}
                    Err(LndRestError::Status { status: 409, .. }) => {
                        // Already settled — proceed idempotently.
                    }
                    Err(error) => return Err(error.to_string()),
                }
                base_state = map_invoice_state(
                    client
                        .lookup_invoice(&session.bundle.payload.base_fee.payment_hash)
                        .await
                        .map_err(|error| error.to_string())?
                        .state,
                );
            }
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

pub async fn cancel_lightning_invoice_bundle(
    state: &AppState,
    session: &LightningInvoiceBundleSession,
) -> Result<(), String> {
    match state.config.lightning.mode {
        LightningMode::Mock => Ok(()),
        LightningMode::LndRest => {
            let client = lnd_rest_client(state)?;
            let mut payment_hashes = Vec::new();
            if session.bundle.payload.base_fee.amount_msat > 0
                && matches!(
                    session.base_state,
                    InvoiceBundleLegState::Open | InvoiceBundleLegState::Accepted
                )
            {
                payment_hashes.push(session.bundle.payload.base_fee.payment_hash.clone());
            }
            if matches!(
                session.success_state,
                InvoiceBundleLegState::Open | InvoiceBundleLegState::Accepted
            ) {
                payment_hashes.push(session.bundle.payload.success_fee.payment_hash.clone());
            }
            cancel_lnd_invoices(&client, &payment_hashes).await
        }
    }
}

pub async fn cancel_pending_lightning_materialization_request(
    state: &AppState,
    request: &BuildLightningInvoiceBundleRequest,
) -> Result<(), String> {
    match state.config.lightning.mode {
        LightningMode::Mock => Ok(()),
        LightningMode::LndRest => {
            let client = lnd_rest_client(state)?;
            let mut payment_hashes = Vec::new();
            if request.success_fee_msat > 0 {
                payment_hashes.push(request.success_payment_hash.clone());
            }
            if request.base_fee_msat > 0
                && let Some(session_id) = request.session_id.as_deref()
            {
                payment_hashes.push(deterministic_base_fee_payment_hash(state, session_id)?);
            }
            cancel_lnd_invoices_allowing_missing(&client, &payment_hashes).await
        }
    }
}

fn lnd_rest_client(state: &AppState) -> Result<LndRestClient, String> {
    state
        .lnd_rest_client
        .as_ref()
        .map(|client| client.as_ref().clone())
        .ok_or_else(|| "missing lnd_rest configuration".to_string())
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
        LightningMode::LndRest => state
            .lightning_destination_identity
            .get_or_try_init(|| async {
                let client = lnd_rest_client(state)?;
                client
                    .get_info()
                    .await
                    .map(|info| info.identity_pubkey)
                    .map_err(|error| error.to_string())
            })
            .await
            .cloned(),
    }
}

pub async fn cancel_and_sync_lightning_invoice_bundle(
    state: &AppState,
    session: &LightningInvoiceBundleSession,
) -> Result<LightningInvoiceBundleSession, String> {
    match state.config.lightning.mode {
        LightningMode::Mock => {
            let mut base_state = session.base_state.clone();
            let mut success_state = session.success_state.clone();

            if session.bundle.payload.base_fee.amount_msat > 0
                && matches!(
                    base_state,
                    InvoiceBundleLegState::Open | InvoiceBundleLegState::Accepted
                )
            {
                base_state = InvoiceBundleLegState::Canceled;
            }
            if matches!(
                success_state,
                InvoiceBundleLegState::Open | InvoiceBundleLegState::Accepted
            ) {
                success_state = InvoiceBundleLegState::Canceled;
            }

            if base_state == session.base_state && success_state == session.success_state {
                return Ok(session.clone());
            }

            update_lightning_invoice_bundle_states(
                state,
                &session.session_id,
                base_state,
                success_state,
            )
            .await?
            .ok_or_else(|| "lightning invoice bundle not found".to_string())
        }
        LightningMode::LndRest => {
            cancel_lightning_invoice_bundle(state, session).await?;
            sync_lightning_invoice_bundle_session(state, session.clone()).await
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

pub fn lightning_quote_expires_at(
    state: &AppState,
    created_at: i64,
    price_sats: u64,
    execution_window_secs: u64,
) -> i64 {
    if state.config.payment_backend == PaymentBackend::Lightning && price_sats > 0 {
        let admission_window_secs = state
            .config
            .lightning
            .base_invoice_expiry_secs
            .max(state.config.lightning.success_hold_expiry_secs);
        created_at
            + admission_window_secs as i64
            + execution_window_secs as i64
            + state.config.lightning.success_hold_expiry_secs as i64
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
        invoice: session.bundle.payload.success_fee.invoice_bolt11.clone(),
        amount_msat: session.bundle.payload.success_fee.amount_msat,
        payment_hash: session.bundle.payload.success_fee.payment_hash.clone(),
        state: session.success_state.clone(),
    });

    let result_ready = deal_status == deals::DEAL_STATUS_RESULT_READY;
    let can_release_preimage = result_ready && lightning_bundle_can_settle_success(session);
    let mock_action = matches!(state.config.lightning.mode, LightningMode::Mock)
        .then(|| deal_status == deals::DEAL_STATUS_PAYMENT_PENDING)
        .unwrap_or(false)
        .then(|| LightningWalletMockAction {
            endpoint_path: format!("/v1/provider/deals/{deal_id}/mock-pay"),
        });
    let release_action = can_release_preimage.then(|| LightningWalletReleaseAction {
        endpoint_path: format!("/v1/provider/deals/{deal_id}/accept"),
        payment_hash: session.bundle.payload.success_fee.payment_hash.clone(),
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
        mock_action,
        release_action,
    }
}

struct LightningInvoiceBundleSignature {
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
}

fn sign_lightning_invoice_bundle(
    state: &AppState,
    signature: LightningInvoiceBundleSignature,
) -> Result<LightningInvoiceBundleSession, String> {
    let base_expires_at = signature.request.created_at + signature.base_invoice_expiry_secs as i64;
    let success_expires_at =
        signature.request.created_at + signature.success_hold_expiry_secs as i64;
    let bundle_expires_at = base_expires_at.max(success_expires_at);
    let bundle = sign_artifact(
        &signature.provider_id,
        |message| state.identity.sign_message_hex(message),
        TRANSPORT_KIND_INVOICE_BUNDLE,
        signature.request.created_at,
        InvoiceBundlePayload {
            provider_id: signature.provider_id.clone(),
            requester_id: signature.request.requester_id.clone(),
            quote_hash: signature.request.quote_hash.clone(),
            deal_hash: signature.request.deal_hash.clone(),
            expires_at: bundle_expires_at,
            destination_identity: signature.destination_identity,
            base_fee: InvoiceBundleLeg {
                amount_msat: signature.request.base_fee_msat,
                invoice_bolt11: signature.base_invoice_bolt11.clone(),
                invoice_hash: crypto::sha256_hex(signature.base_invoice_bolt11.as_bytes()),
                payment_hash: signature.base_payment_hash,
                state: signature.base_state.clone(),
            },
            success_fee: InvoiceBundleLeg {
                amount_msat: signature.request.success_fee_msat,
                invoice_bolt11: signature.success_hold_invoice_bolt11.clone(),
                invoice_hash: crypto::sha256_hex(signature.success_hold_invoice_bolt11.as_bytes()),
                payment_hash: signature.request.success_payment_hash.clone(),
                state: signature.success_state.clone(),
            },
            min_final_cltv_expiry: state.config.lightning.min_final_cltv_expiry,
        },
    )?;

    Ok(LightningInvoiceBundleSession {
        session_id: signature.session_id,
        bundle,
        base_state: signature.base_state,
        success_state: signature.success_state,
        created_at: signature.request.created_at,
        updated_at: signature.request.created_at,
    })
}

fn effective_bundle_expiry_secs(
    state: &AppState,
    request: &BuildLightningInvoiceBundleRequest,
) -> Result<(u64, u64), String> {
    let mut base_invoice_expiry_secs = state.config.lightning.base_invoice_expiry_secs;
    let mut success_hold_expiry_secs = state.config.lightning.success_hold_expiry_secs;

    if let Some(admission_deadline) = request.admission_deadline {
        let remaining_secs = admission_deadline.saturating_sub(request.created_at);
        if remaining_secs <= 0 {
            return Err(
                "deal admission_deadline passed before lightning invoice bundle issuance"
                    .to_string(),
            );
        }
        let remaining_secs = remaining_secs as u64;
        base_invoice_expiry_secs = base_invoice_expiry_secs.min(remaining_secs);
        success_hold_expiry_secs = success_hold_expiry_secs.min(remaining_secs);
    }

    Ok((base_invoice_expiry_secs, success_hold_expiry_secs))
}

fn guarded_lnd_invoice_expiry_secs(expiry_secs: u64) -> u64 {
    expiry_secs
        .saturating_sub(LND_INVOICE_EXPIRY_GUARD_SECS)
        .max(1)
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
    if let Some(expected_requester_id) = expected_requester_id
        && bundle.payload.requester_id != expected_requester_id
    {
        push_bundle_issue(
            &mut issues,
            "requester_id_mismatch",
            "invoice bundle requester_id does not match the expected requester",
        );
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
    if bundle.payload.expires_at > deal.payload.admission_deadline {
        push_bundle_issue(
            &mut issues,
            "bundle_expiry_exceeds_admission_deadline",
            "invoice bundle expires after the deal admission_deadline",
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
                if decoded.expires_at > deal.payload.admission_deadline {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_expiry_exceeds_admission_deadline",
                        format!("{leg_name} invoice expiry exceeds the deal admission_deadline"),
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
        LightningInvoiceBundleSignature {
            session_id,
            provider_id,
            request,
            base_invoice_expiry_secs,
            success_hold_expiry_secs,
            destination_identity,
            base_invoice_bolt11,
            base_payment_hash,
            base_state: InvoiceBundleLegState::Open,
            success_hold_invoice_bolt11,
            success_state: InvoiceBundleLegState::Open,
        },
    )
}

async fn issue_lnd_rest_invoice_bundle(
    state: &AppState,
    request: BuildLightningInvoiceBundleRequest,
) -> Result<LightningInvoiceBundleSession, String> {
    let client = lnd_rest_client(state)?;
    let (max_base_invoice_expiry_secs, max_success_hold_expiry_secs) =
        effective_bundle_expiry_secs(state, &request)?;
    let base_invoice_expiry_secs = guarded_lnd_invoice_expiry_secs(max_base_invoice_expiry_secs);
    let success_hold_expiry_secs = guarded_lnd_invoice_expiry_secs(max_success_hold_expiry_secs);
    let session_id = request.session_id.clone().unwrap_or_else(new_request_id);
    let provider_id = state.identity.node_id().to_string();
    let destination_identity = resolve_lightning_destination_identity(state).await?;
    let mut issued_payment_hashes = Vec::new();
    let mut base_state = InvoiceBundleLegState::Open;
    let mut bundle_created_at = request.created_at;
    let deterministic_base_payment_hash = deterministic_base_fee_payment_hash(state, &session_id)?;
    let (base_payment_hash, base_invoice_bolt11) = if request.base_fee_msat == 0 {
        base_state = InvoiceBundleLegState::Settled;
        let invoice_bolt11 = mock_bolt11(
            "base",
            request.base_fee_msat,
            &deterministic_base_payment_hash,
            request.created_at + base_invoice_expiry_secs as i64,
        );
        (deterministic_base_payment_hash.clone(), invoice_bolt11)
    } else {
        let base_invoice = match client
            .add_hold_invoice(
                &deterministic_base_payment_hash,
                request.base_fee_msat,
                base_invoice_expiry_secs,
                state.config.lightning.min_final_cltv_expiry,
                &format!("froglet base fee {}", session_id),
                true,
            )
            .await
        {
            Ok(invoice) => invoice,
            Err(error) => {
                return Err(cleanup_failed_lnd_bundle_issue(
                    &client,
                    &issued_payment_hashes,
                    error.to_string(),
                )
                .await);
            }
        };
        issued_payment_hashes.push(deterministic_base_payment_hash.clone());
        (
            deterministic_base_payment_hash.clone(),
            base_invoice.payment_request,
        )
    };
    let success_invoice = match client
        .add_hold_invoice(
            &request.success_payment_hash,
            request.success_fee_msat,
            success_hold_expiry_secs,
            state.config.lightning.min_final_cltv_expiry,
            &format!("froglet success fee {}", session_id),
            true,
        )
        .await
    {
        Ok(invoice) => invoice,
        Err(error) => {
            return Err(cleanup_failed_lnd_bundle_issue(
                &client,
                &issued_payment_hashes,
                error.to_string(),
            )
            .await);
        }
    };
    issued_payment_hashes.push(request.success_payment_hash.clone());

    if request.base_fee_msat > 0 {
        let decoded_base = match decode_lightning_invoice(&base_invoice_bolt11) {
            Ok(decoded) => decoded,
            Err(error) => {
                return Err(cleanup_failed_lnd_bundle_issue(
                    &client,
                    &issued_payment_hashes,
                    error.to_string(),
                )
                .await);
            }
        };
        if decoded_base.amount_msat != request.base_fee_msat {
            return Err(cleanup_failed_lnd_bundle_issue(
                &client,
                &issued_payment_hashes,
                "LND base invoice amount did not match the requested amount".to_string(),
            )
            .await);
        }
        if decoded_base.payment_hash != deterministic_base_payment_hash {
            return Err(cleanup_failed_lnd_bundle_issue(
                &client,
                &issued_payment_hashes,
                "LND base invoice payment hash did not match the deterministic session payment hash"
                    .to_string(),
            )
            .await);
        }
        if let Some(admission_deadline) = request.admission_deadline
            && decoded_base.expires_at > admission_deadline
        {
            return Err(cleanup_failed_lnd_bundle_issue(
                &client,
                &issued_payment_hashes,
                "LND base invoice expiry exceeded the deal admission_deadline".to_string(),
            )
            .await);
        }
        if decoded_base.destination_identity != destination_identity {
            return Err(cleanup_failed_lnd_bundle_issue(
                &client,
                &issued_payment_hashes,
                "LND base invoice destination did not match the provider identity".to_string(),
            )
            .await);
        }
        bundle_created_at = bundle_created_at.max(
            decoded_base
                .expires_at
                .saturating_sub(base_invoice_expiry_secs as i64),
        );
    }

    let decoded_success = match decode_lightning_invoice(&success_invoice.payment_request) {
        Ok(decoded) => decoded,
        Err(error) => {
            return Err(cleanup_failed_lnd_bundle_issue(
                &client,
                &issued_payment_hashes,
                error.to_string(),
            )
            .await);
        }
    };
    if decoded_success.amount_msat != request.success_fee_msat {
        return Err(cleanup_failed_lnd_bundle_issue(
            &client,
            &issued_payment_hashes,
            "LND success hold invoice amount did not match the requested amount".to_string(),
        )
        .await);
    }
    if let Some(admission_deadline) = request.admission_deadline
        && decoded_success.expires_at > admission_deadline
    {
        return Err(cleanup_failed_lnd_bundle_issue(
            &client,
            &issued_payment_hashes,
            "LND success hold invoice expiry exceeded the deal admission_deadline".to_string(),
        )
        .await);
    }
    if decoded_success.payment_hash != request.success_payment_hash {
        return Err(cleanup_failed_lnd_bundle_issue(
            &client,
            &issued_payment_hashes,
            "LND success hold invoice payment hash did not match the deal payment lock".to_string(),
        )
        .await);
    }
    if decoded_success.destination_identity != destination_identity {
        return Err(cleanup_failed_lnd_bundle_issue(
            &client,
            &issued_payment_hashes,
            "LND success hold invoice destination did not match the provider identity".to_string(),
        )
        .await);
    }
    if decoded_success.min_final_cltv_expiry < state.config.lightning.min_final_cltv_expiry {
        return Err(cleanup_failed_lnd_bundle_issue(
            &client,
            &issued_payment_hashes,
            "LND success hold invoice min_final_cltv_expiry was below the configured floor"
                .to_string(),
        )
        .await);
    }

    bundle_created_at = bundle_created_at.max(
        decoded_success
            .expires_at
            .saturating_sub(success_hold_expiry_secs as i64),
    );

    let mut request = request;
    request.created_at = bundle_created_at;

    sign_lightning_invoice_bundle(
        state,
        LightningInvoiceBundleSignature {
            session_id,
            provider_id,
            request,
            base_invoice_expiry_secs,
            success_hold_expiry_secs,
            destination_identity,
            base_invoice_bolt11,
            base_payment_hash,
            base_state,
            success_hold_invoice_bolt11: success_invoice.payment_request,
            success_state: InvoiceBundleLegState::Open,
        },
    )
}

async fn cleanup_failed_lnd_bundle_issue(
    client: &LndRestClient,
    payment_hashes: &[String],
    issue_error: String,
) -> String {
    match cancel_lnd_invoices(client, payment_hashes).await {
        Ok(()) => issue_error,
        Err(cancel_error) => {
            format!("{issue_error}; additionally failed to cancel issued invoices: {cancel_error}")
        }
    }
}

async fn cancel_lnd_invoices(
    client: &LndRestClient,
    payment_hashes: &[String],
) -> Result<(), String> {
    let mut failures = Vec::new();
    for payment_hash in payment_hashes {
        if let Err(error) = client.cancel_invoice(payment_hash).await {
            failures.push(format!("{payment_hash}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

async fn cancel_lnd_invoices_allowing_missing(
    client: &LndRestClient,
    payment_hashes: &[String],
) -> Result<(), String> {
    let mut failures = Vec::new();
    for payment_hash in payment_hashes {
        match client.cancel_invoice(payment_hash).await {
            Ok(()) | Err(LndRestError::Status { status: 404, .. }) => {}
            Err(error) => failures.push(format!("{payment_hash}: {error}")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

struct NoSettlementDriver;

impl SettlementDriver for NoSettlementDriver {
    fn descriptor(&self, _state: &AppState) -> SettlementDriverDescriptor {
        SettlementDriverDescriptor {
            backend: PaymentBackend::None.to_string(),
            mode: "disabled".to_string(),
            accepted_payment_methods: Vec::new(),
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
static LIGHTNING_DRIVER: LightningDriver = LightningDriver;
