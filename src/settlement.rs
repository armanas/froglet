use crate::{
    config::{LightningMode, PaymentBackend},
    crypto,
    db::{self, LightningInvoiceBundleRecord, ReservePaymentTokenOutcome},
    ecash,
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
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, str::FromStr, time::Duration};
use thiserror::Error;

pub const CASHU_VERIFIER_MODE: &str = "format_and_replay_guard";
pub const LIGHTNING_MOCK_MODE: &str = "mock_hold_invoice";

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
        PaymentBackend::Lightning => &LIGHTNING_MOCK_DRIVER,
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
    let session = build_lightning_invoice_bundle(state, request)?;
    let session_id_for_db = session.session_id.clone();
    let bundle_for_db = session.bundle.clone();
    let created_at = session.created_at;
    state
        .db
        .with_conn(move |conn| {
            db::insert_lightning_invoice_bundle(
                conn,
                &session_id_for_db,
                &bundle_for_db,
                InvoiceBundleLegState::Open,
                InvoiceBundleLegState::Open,
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
    let session_id = session_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            db::get_lightning_invoice_bundle(conn, &session_id)
                .map(|record| record.map(map_lightning_bundle_record))
        })
        .await
}

pub async fn get_lightning_invoice_bundle_by_deal_hash(
    state: &AppState,
    deal_hash: &str,
) -> Result<Option<LightningInvoiceBundleSession>, String> {
    let deal_hash = deal_hash.to_string();
    state
        .db
        .with_conn(move |conn| {
            db::get_lightning_invoice_bundle_by_deal_hash(conn, &deal_hash)
                .map(|record| record.map(map_lightning_bundle_record))
        })
        .await
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

pub fn lightning_destination_identity(state: &AppState) -> String {
    state
        .config
        .lightning
        .destination_identity
        .clone()
        .unwrap_or_else(|| state.identity.compressed_public_key_hex().to_string())
}

fn mock_bolt11(prefix: &str, amount_msat: u64, payment_hash: &str, expires_at: i64) -> String {
    format!("lnmock-{prefix}-{amount_msat}-{payment_hash}-{expires_at}")
}

pub fn quoted_lightning_settlement_terms(
    state: &AppState,
    price_sats: u64,
) -> Option<QuoteSettlementTerms> {
    if state.config.payment_backend != PaymentBackend::Lightning || price_sats == 0 {
        return None;
    }

    Some(QuoteSettlementTerms {
        destination_identity: lightning_destination_identity(state),
        base_fee_msat: 0,
        success_fee_msat: price_sats.saturating_mul(1_000),
        max_base_invoice_expiry_secs: state.config.lightning.base_invoice_expiry_secs,
        max_success_hold_expiry_secs: state.config.lightning.success_hold_expiry_secs,
        min_final_cltv_expiry: state.config.lightning.min_final_cltv_expiry,
    })
}

pub fn lightning_quote_expires_at(state: &AppState, created_at: i64, price_sats: u64) -> i64 {
    match quoted_lightning_settlement_terms(state, price_sats) {
        Some(terms) => {
            created_at
                + terms
                    .max_base_invoice_expiry_secs
                    .max(terms.max_success_hold_expiry_secs) as i64
        }
        None => created_at + 60,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MockBolt11Fields {
    prefix: String,
    amount_msat: u64,
    payment_hash: String,
    expires_at: i64,
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

    if bundle.kind != TRANSPORT_KIND_INVOICE_BUNDLE {
        push_bundle_issue(
            &mut issues,
            "bundle_kind_mismatch",
            format!("expected bundle kind '{TRANSPORT_KIND_INVOICE_BUNDLE}'"),
        );
    }
    if bundle.payload.provider_id != quote.actor_id || bundle.actor_id != quote.actor_id {
        push_bundle_issue(
            &mut issues,
            "provider_identity_mismatch",
            "invoice bundle provider identity does not match the quoted provider",
        );
    }
    if deal.actor_id != quote.actor_id {
        push_bundle_issue(
            &mut issues,
            "deal_provider_mismatch",
            "deal was not issued by the same provider as the quote",
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

    if quote.payload.payment_method.as_deref() != Some("lightning") {
        push_bundle_issue(
            &mut issues,
            "quote_payment_method_mismatch",
            "quote does not advertise lightning settlement",
        );
    }

    let Some(settlement_terms) = quote.payload.settlement_terms.as_ref() else {
        push_bundle_issue(
            &mut issues,
            "missing_settlement_terms",
            "quote does not include committed lightning settlement terms",
        );
        return InvoiceBundleValidationReport {
            valid: issues.is_empty(),
            bundle_hash: bundle.hash.clone(),
            quote_hash: quote.hash.clone(),
            deal_hash: deal.hash.clone(),
            expected_requester_id: expected_requester_id.map(str::to_string),
            issues,
        };
    };

    if deal.payload.quote_id != quote.payload.quote_id
        || deal.payload.offer_id != quote.payload.offer_id
        || deal.payload.service_id != quote.payload.service_id
        || deal.payload.workload_hash != quote.payload.workload_hash
    {
        push_bundle_issue(
            &mut issues,
            "deal_quote_binding_mismatch",
            "deal artifact does not match the quoted workload commitment",
        );
    }

    if settlement_terms.base_fee_msat + settlement_terms.success_fee_msat
        != quote.payload.price_sats.saturating_mul(1_000)
    {
        push_bundle_issue(
            &mut issues,
            "quoted_fee_total_mismatch",
            "quote settlement terms do not add up to the quoted price",
        );
    }

    if bundle.payload.destination_identity != settlement_terms.destination_identity {
        push_bundle_issue(
            &mut issues,
            "destination_identity_mismatch",
            "invoice bundle destination identity does not match the quoted settlement destination",
        );
    }
    if bundle.payload.base_invoice.amount_msat != settlement_terms.base_fee_msat {
        push_bundle_issue(
            &mut issues,
            "base_fee_mismatch",
            "invoice bundle base fee does not match the quote settlement terms",
        );
    }
    if bundle.payload.success_hold_invoice.amount_msat != settlement_terms.success_fee_msat {
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

    let Some(payment_lock) = deal.payload.payment_lock.as_ref() else {
        push_bundle_issue(
            &mut issues,
            "missing_payment_lock",
            "deal does not include a payment lock for the success-fee leg",
        );
        return InvoiceBundleValidationReport {
            valid: issues.is_empty(),
            bundle_hash: bundle.hash.clone(),
            quote_hash: quote.hash.clone(),
            deal_hash: deal.hash.clone(),
            expected_requester_id: expected_requester_id.map(str::to_string),
            issues,
        };
    };

    if payment_lock.kind != "lightning" {
        push_bundle_issue(
            &mut issues,
            "payment_lock_kind_mismatch",
            "deal payment lock is not marked as lightning",
        );
    }
    if payment_lock.token_hash != bundle.payload.success_hold_invoice.payment_hash {
        push_bundle_issue(
            &mut issues,
            "success_payment_hash_mismatch",
            "invoice bundle success payment hash does not match the deal payment lock",
        );
    }
    if payment_lock.amount_sats.saturating_mul(1_000) != settlement_terms.success_fee_msat {
        push_bundle_issue(
            &mut issues,
            "payment_lock_amount_mismatch",
            "deal payment lock amount does not match the quoted success-fee leg",
        );
    }

    for (expected_prefix, leg, max_expiry_secs, leg_name) in [
        (
            "base",
            &bundle.payload.base_invoice,
            settlement_terms.max_base_invoice_expiry_secs,
            "base_invoice",
        ),
        (
            "hold",
            &bundle.payload.success_hold_invoice,
            settlement_terms.max_success_hold_expiry_secs,
            "success_hold_invoice",
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

        match parse_mock_bolt11(&leg.invoice_bolt11) {
            Ok(mock) => {
                if mock.prefix != expected_prefix {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_prefix_mismatch",
                        format!("{leg_name} invoice prefix does not match the expected leg type"),
                    );
                }
                if mock.amount_msat != leg.amount_msat {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_amount_mismatch",
                        format!("{leg_name} invoice amount does not match the signed bundle"),
                    );
                }
                if mock.payment_hash != leg.payment_hash {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_payment_hash_mismatch",
                        format!("{leg_name} invoice payment hash does not match the signed bundle"),
                    );
                }
                if mock.expires_at > bundle.payload.created_at + max_expiry_secs as i64 {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_expiry_exceeds_terms",
                        format!(
                            "{leg_name} invoice expiry exceeds the quoted settlement constraints"
                        ),
                    );
                }
                if mock.expires_at > quote.payload.expires_at {
                    push_bundle_issue(
                        &mut issues,
                        "invoice_expiry_exceeds_quote",
                        format!("{leg_name} invoice expiry exceeds the quote deadline"),
                    );
                }
            }
            Err(error) => push_bundle_issue(
                &mut issues,
                "invalid_mock_invoice_encoding",
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
    let session_id = request.session_id.unwrap_or_else(new_request_id);
    let provider_id = state.identity.node_id().to_string();
    let destination_identity = lightning_destination_identity(state);
    let base_payment_hash = crypto::sha256_hex(format!("lightning-base:{session_id}").as_bytes());
    let base_expires_at =
        request.created_at + state.config.lightning.base_invoice_expiry_secs as i64;
    let success_expires_at =
        request.created_at + state.config.lightning.success_hold_expiry_secs as i64;
    let bundle_expires_at = base_expires_at.max(success_expires_at);
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
    let bundle = sign_artifact(
        &provider_id,
        |message| state.identity.sign_message_hex(message),
        TRANSPORT_KIND_INVOICE_BUNDLE,
        request.created_at,
        InvoiceBundlePayload {
            schema_version: "froglet/v1".to_string(),
            bundle_type: "invoice_bundle".to_string(),
            provider_id: provider_id.clone(),
            requester_id: request.requester_id.clone(),
            quote_hash: request.quote_hash.clone(),
            deal_hash: request.deal_hash.clone(),
            created_at: request.created_at,
            expires_at: bundle_expires_at,
            destination_identity,
            base_invoice: InvoiceBundleLeg {
                amount_msat: request.base_fee_msat,
                invoice_bolt11: base_invoice_bolt11.clone(),
                invoice_hash: crypto::sha256_hex(base_invoice_bolt11.as_bytes()),
                payment_hash: base_payment_hash,
                state: InvoiceBundleLegState::Open,
            },
            success_hold_invoice: InvoiceBundleLeg {
                amount_msat: request.success_fee_msat,
                invoice_bolt11: success_hold_invoice_bolt11.clone(),
                invoice_hash: crypto::sha256_hex(success_hold_invoice_bolt11.as_bytes()),
                payment_hash: request.success_payment_hash.clone(),
                state: InvoiceBundleLegState::Open,
            },
            min_final_cltv_expiry: state.config.lightning.min_final_cltv_expiry,
        },
    )?;

    Ok(LightningInvoiceBundleSession {
        session_id,
        bundle,
        base_state: InvoiceBundleLegState::Open,
        success_state: InvoiceBundleLegState::Open,
        created_at: request.created_at,
        updated_at: request.created_at,
    })
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

struct LightningMockDriver;

impl LightningMockDriver {
    fn descriptor_inner(&self, state: &AppState) -> SettlementDriverDescriptor {
        let mode = match state.config.lightning.mode {
            LightningMode::Mock => LIGHTNING_MOCK_MODE,
        };
        SettlementDriverDescriptor {
            backend: PaymentBackend::Lightning.to_string(),
            mode: mode.to_string(),
            accepted_payment_methods: vec!["lightning".to_string()],
            accepted_mints: Vec::new(),
            capabilities: vec![
                "invoice_bundles".to_string(),
                "hold_invoices".to_string(),
                "mock_mode".to_string(),
            ],
            reservations: true,
            receipts: true,
        }
    }
}

impl SettlementDriver for LightningMockDriver {
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
static LIGHTNING_MOCK_DRIVER: LightningMockDriver = LightningMockDriver;
