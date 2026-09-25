use crate::{
    config::{LightningMode, PaymentBackend},
    crypto,
    db::{self, LightningInvoiceBundleRecord},
    deals,
    protocol::{
        DealPayload, InvoiceBundleLeg, InvoiceBundleLegState, InvoiceBundlePayload, QuotePayload,
        QuoteSettlementTerms, SignedArtifact, TRANSPORT_KIND_INVOICE_BUNDLE, sign_artifact,
        validate_quote_artifact, verify_artifact,
    },
    settlement::wallet::{LightningWallet, WalletError},
    state::AppState,
};
use futures::future::BoxFuture;
use lightning_invoice::{Bolt11Invoice, Currency};
use serde::{Deserialize, Serialize};

use super::{
    PaymentError, PaymentReceipt, PaymentReservation, PreparePaymentRequest, SettlementDriver,
    SettlementDriverDescriptor, WalletBalanceSnapshot, current_unix_timestamp, new_request_id,
};

pub const LIGHTNING_MOCK_MODE: &str = "mock_hold_invoice";
pub const LIGHTNING_LND_REST_MODE: &str = "lnd_rest";
pub const LIGHTNING_PHOENIXD_MODE: &str = "phoenixd_prepaid";

/// Error returned whenever the hold-invoice escrow machinery is reached while
/// the active Lightning backend is phoenixd.  phoenixd has no hold invoices, so
/// it drives the prepaid (`lightning.prepaid.v1`) path instead and must never
/// enter the escrow functions below.
pub const PHOENIXD_NO_ESCROW: &str =
    "phoenixd backend does not support hold-invoice escrow; it uses lightning.prepaid.v1";
const LND_INVOICE_EXPIRY_GUARD_SECS: u64 = 5;

// ─── Public lightning-specific types ─────────────────────────────────────────

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

/// Inputs required to bind a provider-returned prepaid invoice to the signed
/// quote before the requester gives the invoice to its wallet.
pub(crate) struct PrepaidLightningInvoiceValidation<'a> {
    pub invoice_bolt11: &'a str,
    pub expected_payment_hash: &'a str,
    pub expected_amount_sat: u64,
    /// Enforce the invoice payee when the requester has a trusted destination.
    /// Production prepaid callers must pass the destination signed in the quote;
    /// `None` exists only for payment-free mock validation.
    pub expected_destination_identity: Option<&'a str>,
    /// Enforce the BOLT11 network when the paying wallet's network is known.
    /// Phoenixd production callers should pass [`Currency::Bitcoin`].
    pub expected_network: Option<Currency>,
    pub now: i64,
    pub deal_admission_deadline: i64,
    pub quote: &'a SignedArtifact<QuotePayload>,
}

// ─── Private internal types ───────────────────────────────────────────────────

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
    expiry_secs: Option<u64>,
    destination_identity: String,
    network: Option<Currency>,
    min_final_cltv_expiry: u32,
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

// ─── Helper functions ─────────────────────────────────────────────────────────

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

fn lightning_wallet(state: &AppState) -> Result<&dyn LightningWallet, String> {
    state
        .lightning_wallet
        .as_deref()
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

fn mock_bolt11(prefix: &str, amount_msat: u64, payment_hash: &str, expires_at: i64) -> String {
    format!("lnmock-{prefix}-{amount_msat}-{payment_hash}-{expires_at}")
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
            expiry_secs: None,
            destination_identity: String::new(),
            network: None,
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
        .as_secs()
        .try_into()
        .map_err(|_| "invoice expiry exceeds the supported timestamp range".to_string())?;
    let destination_identity = hex::encode(invoice.get_payee_pub_key().serialize());

    Ok(DecodedLightningInvoice {
        amount_msat,
        payment_hash: invoice.payment_hash().to_string(),
        expires_at,
        expiry_secs: Some(invoice.expiry_time().as_secs()),
        destination_identity,
        network: Some(invoice.currency()),
        min_final_cltv_expiry: invoice.min_final_cltv_expiry_delta() as u32,
    })
}

pub(crate) fn validate_prepaid_lightning_invoice(
    request: PrepaidLightningInvoiceValidation<'_>,
) -> Result<(), String> {
    let PrepaidLightningInvoiceValidation {
        invoice_bolt11,
        expected_payment_hash,
        expected_amount_sat,
        expected_destination_identity,
        expected_network,
        now,
        deal_admission_deadline,
        quote,
    } = request;
    if quote.artifact_type != crate::protocol::ARTIFACT_KIND_QUOTE {
        return Err("prepaid settlement terms must come from a quote artifact".to_string());
    }
    if !verify_artifact(quote) {
        return Err("prepaid quote signature is invalid".to_string());
    }
    validate_quote_artifact(quote).map_err(|error| format!("prepaid quote is invalid: {error}"))?;
    if quote.payload.settlement_terms.method != "lightning.prepaid.v1" {
        return Err("signed quote does not use lightning.prepaid.v1".to_string());
    }
    let decoded = decode_lightning_invoice(invoice_bolt11)?;
    let expected_amount_msat = expected_amount_sat
        .checked_mul(1_000)
        .ok_or_else(|| "prepaid invoice amount overflowed millisatoshis".to_string())?;
    if quote.payload.settlement_terms.base_fee_msat != expected_amount_msat {
        return Err("provider prepaid amount does not match the signed quote amount".to_string());
    }
    if decoded.amount_msat != expected_amount_msat {
        return Err("prepaid invoice amount does not match the expected amount".to_string());
    }
    let expected_payment_hash_bytes = hex::decode(expected_payment_hash)
        .map_err(|_| "expected prepaid payment hash is not valid hex".to_string())?;
    if expected_payment_hash_bytes.len() != 32 {
        return Err("expected prepaid payment hash must be 32 bytes".to_string());
    }
    if decoded.payment_hash != hex::encode(expected_payment_hash_bytes) {
        return Err(
            "prepaid invoice payment hash does not match the expected payment hash".to_string(),
        );
    }
    if let Some(expected_network) = expected_network
        && decoded.network != Some(expected_network)
    {
        return Err("prepaid invoice network does not match the paying wallet network".to_string());
    }
    if let Some(expected_destination) = expected_destination_identity {
        let quoted_destination = normalize_lightning_destination_identity(
            &quote.payload.settlement_terms.destination_identity,
        )?;
        let expected_destination = normalize_lightning_destination_identity(expected_destination)?;
        if expected_destination != quoted_destination {
            return Err(
                "expected prepaid destination does not match the signed quote destination"
                    .to_string(),
            );
        }
        if decoded.destination_identity != expected_destination {
            return Err(
                "prepaid invoice destination does not match the expected destination".to_string(),
            );
        }
    }
    if request_time_has_reached_invoice_expiry(now, decoded.expires_at) {
        return Err("prepaid invoice is expired".to_string());
    }
    let quoted_expiry_secs: i64 = quote
        .payload
        .settlement_terms
        .max_base_invoice_expiry_secs
        .try_into()
        .map_err(|_| "quoted prepaid invoice expiry is out of range".to_string())?;
    if decoded.expiry_secs.is_some_and(|expiry_secs| {
        expiry_secs > quote.payload.settlement_terms.max_base_invoice_expiry_secs
    }) {
        return Err("prepaid invoice exceeds the quoted expiry window".to_string());
    }
    let latest_expiry_from_now = now
        .checked_add(quoted_expiry_secs)
        .ok_or_else(|| "quoted prepaid invoice expiry overflowed".to_string())?;
    if decoded.expires_at > latest_expiry_from_now {
        return Err("prepaid invoice exceeds the quoted expiry window".to_string());
    }
    if now >= quote.payload.expires_at {
        return Err("signed prepaid quote is expired".to_string());
    }
    if decoded.expires_at > quote.payload.expires_at {
        return Err("prepaid invoice expires after the signed quote deadline".to_string());
    }
    if now >= deal_admission_deadline {
        return Err("prepaid deal admission deadline has elapsed".to_string());
    }
    if decoded.expires_at > deal_admission_deadline {
        return Err("prepaid invoice expires after the deal admission deadline".to_string());
    }
    Ok(())
}

fn request_time_has_reached_invoice_expiry(now: i64, expires_at: i64) -> bool {
    now >= expires_at
}

fn normalize_lightning_destination_identity(value: &str) -> Result<String, String> {
    let bytes = hex::decode(value)
        .map_err(|_| "expected prepaid destination is not valid hex".to_string())?;
    let public_key = k256::PublicKey::from_sec1_bytes(&bytes)
        .map_err(|_| "expected prepaid destination is not a secp256k1 public key".to_string())?;
    Ok(hex::encode(public_key.to_sec1_bytes()))
}

fn map_invoice_state(state: crate::lnd::InvoiceState) -> InvoiceBundleLegState {
    match state {
        crate::lnd::InvoiceState::Open => InvoiceBundleLegState::Open,
        crate::lnd::InvoiceState::Accepted => InvoiceBundleLegState::Accepted,
        crate::lnd::InvoiceState::Settled => InvoiceBundleLegState::Settled,
        crate::lnd::InvoiceState::Canceled => InvoiceBundleLegState::Canceled,
    }
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

async fn cleanup_failed_lnd_bundle_issue(
    client: &dyn LightningWallet,
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
    client: &dyn LightningWallet,
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
    client: &dyn LightningWallet,
    payment_hashes: &[String],
) -> Result<(), String> {
    let mut failures = Vec::new();
    for payment_hash in payment_hashes {
        match client.cancel_invoice(payment_hash).await {
            Ok(()) | Err(WalletError::NotFound) => {}
            Err(error) => failures.push(format!("{payment_hash}: {error}")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

// ─── Public lightning functions ───────────────────────────────────────────────

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

pub async fn issue_lightning_invoice_bundle(
    state: &AppState,
    request: BuildLightningInvoiceBundleRequest,
) -> Result<LightningInvoiceBundleSession, String> {
    match state.config.lightning.mode {
        LightningMode::Mock => build_lightning_invoice_bundle(state, request),
        LightningMode::LndRest => issue_lnd_rest_invoice_bundle(state, request).await,
        LightningMode::Phoenixd => Err(PHOENIXD_NO_ESCROW.to_string()),
    }
}

pub async fn sync_lightning_invoice_bundle_session(
    state: &AppState,
    session: LightningInvoiceBundleSession,
) -> Result<LightningInvoiceBundleSession, String> {
    let session = match state.config.lightning.mode {
        LightningMode::Mock => session,
        LightningMode::Phoenixd => return Err(PHOENIXD_NO_ESCROW.to_string()),
        LightningMode::LndRest => {
            let client = lightning_wallet(state)?;
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
                    Err(WalletError::AlreadySettled) => {
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
        LightningMode::Phoenixd => Err(PHOENIXD_NO_ESCROW.to_string()),
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
            let client = lightning_wallet(state)?;
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
        LightningMode::Phoenixd => Err(PHOENIXD_NO_ESCROW.to_string()),
        LightningMode::LndRest => {
            let client = lightning_wallet(state)?;
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
            cancel_lnd_invoices(client, &payment_hashes).await
        }
    }
}

pub async fn cancel_pending_lightning_materialization_request(
    state: &AppState,
    request: &BuildLightningInvoiceBundleRequest,
) -> Result<(), String> {
    match state.config.lightning.mode {
        LightningMode::Mock => Ok(()),
        LightningMode::Phoenixd => Err(PHOENIXD_NO_ESCROW.to_string()),
        LightningMode::LndRest => {
            let client = lightning_wallet(state)?;
            let mut payment_hashes = Vec::new();
            if request.success_fee_msat > 0 {
                payment_hashes.push(request.success_payment_hash.clone());
            }
            if request.base_fee_msat > 0
                && let Some(session_id) = request.session_id.as_deref()
            {
                payment_hashes.push(deterministic_base_fee_payment_hash(state, session_id)?);
            }
            cancel_lnd_invoices_allowing_missing(client, &payment_hashes).await
        }
    }
}

pub async fn resolve_lightning_destination_identity(state: &AppState) -> Result<String, String> {
    if let Some(destination_identity) = state.config.lightning.destination_identity.clone() {
        return Ok(destination_identity);
    }

    match state.config.lightning.mode {
        LightningMode::Mock => Ok(configured_lightning_destination_identity(state)),
        // Both real backends resolve the destination identity from the node's
        // own pubkey via get_info (phoenixd: nodeId, LND: identity_pubkey).
        LightningMode::LndRest | LightningMode::Phoenixd => state
            .lightning_destination_identity
            .get_or_try_init(|| async {
                let client = lightning_wallet(state)?;
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
        LightningMode::Phoenixd => Err(PHOENIXD_NO_ESCROW.to_string()),
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

pub async fn quoted_lightning_settlement_terms(
    state: &AppState,
    price_sats: u64,
) -> Result<Option<QuoteSettlementTerms>, String> {
    if !state
        .config
        .payment_backends
        .contains(&PaymentBackend::Lightning)
        || price_sats == 0
    {
        return Ok(None);
    }

    // phoenixd backend → prepaid (non-escrow) terms.  The full price is a single
    // upfront base fee; there is no success-fee hold, so the hold/CLTV fields are
    // zeroed (as for Stripe).  destination_identity is still the node's pubkey.
    if state.config.lightning.mode == LightningMode::Phoenixd {
        return Ok(Some(QuoteSettlementTerms {
            method: "lightning.prepaid.v1".to_string(),
            destination_identity: resolve_lightning_destination_identity(state).await?,
            base_fee_msat: price_sats.saturating_mul(1_000),
            success_fee_msat: 0,
            max_base_invoice_expiry_secs: state.config.lightning.base_invoice_expiry_secs,
            max_success_hold_expiry_secs: 0,
            min_final_cltv_expiry: 0,
        }));
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
    if state
        .config
        .payment_backends
        .contains(&PaymentBackend::Lightning)
        && price_sats > 0
    {
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
            LightningMode::Phoenixd => LIGHTNING_PHOENIXD_MODE.to_string(),
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

pub fn validate_lightning_invoice_bundle(
    bundle: &SignedArtifact<InvoiceBundlePayload>,
    quote: &SignedArtifact<QuotePayload>,
    deal: &SignedArtifact<DealPayload>,
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
                    match parse_mock_bolt11(&leg.invoice_bolt11) {
                        Ok(mock) if mock.prefix != expected_prefix => push_bundle_issue(
                            &mut issues,
                            "invoice_prefix_mismatch",
                            format!(
                                "{leg_name} invoice prefix does not match the expected leg type"
                            ),
                        ),
                        Ok(_) => {}
                        Err(error) => push_bundle_issue(
                            &mut issues,
                            "invalid_invoice_encoding",
                            format!("{leg_name}: {error}"),
                        ),
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
    let client = lightning_wallet(state)?;
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
                    client,
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
                client,
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
                    client,
                    &issued_payment_hashes,
                    error.to_string(),
                )
                .await);
            }
        };
        if decoded_base.amount_msat != request.base_fee_msat {
            return Err(cleanup_failed_lnd_bundle_issue(
                client,
                &issued_payment_hashes,
                "LND base invoice amount did not match the requested amount".to_string(),
            )
            .await);
        }
        if decoded_base.payment_hash != deterministic_base_payment_hash {
            return Err(cleanup_failed_lnd_bundle_issue(
                client,
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
                client,
                &issued_payment_hashes,
                "LND base invoice expiry exceeded the deal admission_deadline".to_string(),
            )
            .await);
        }
        if decoded_base.destination_identity != destination_identity {
            return Err(cleanup_failed_lnd_bundle_issue(
                client,
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
                client,
                &issued_payment_hashes,
                error.to_string(),
            )
            .await);
        }
    };
    if decoded_success.amount_msat != request.success_fee_msat {
        return Err(cleanup_failed_lnd_bundle_issue(
            client,
            &issued_payment_hashes,
            "LND success hold invoice amount did not match the requested amount".to_string(),
        )
        .await);
    }
    if let Some(admission_deadline) = request.admission_deadline
        && decoded_success.expires_at > admission_deadline
    {
        return Err(cleanup_failed_lnd_bundle_issue(
            client,
            &issued_payment_hashes,
            "LND success hold invoice expiry exceeded the deal admission_deadline".to_string(),
        )
        .await);
    }
    if decoded_success.payment_hash != request.success_payment_hash {
        return Err(cleanup_failed_lnd_bundle_issue(
            client,
            &issued_payment_hashes,
            "LND success hold invoice payment hash did not match the deal payment lock".to_string(),
        )
        .await);
    }
    if decoded_success.destination_identity != destination_identity {
        return Err(cleanup_failed_lnd_bundle_issue(
            client,
            &issued_payment_hashes,
            "LND success hold invoice destination did not match the provider identity".to_string(),
        )
        .await);
    }
    if decoded_success.min_final_cltv_expiry < state.config.lightning.min_final_cltv_expiry {
        return Err(cleanup_failed_lnd_bundle_issue(
            client,
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

// ─── LightningDriver implementation ──────────────────────────────────────────

pub(crate) struct LightningDriver;

impl LightningDriver {
    fn descriptor_inner(&self, state: &AppState) -> SettlementDriverDescriptor {
        let mode = match state.config.lightning.mode {
            LightningMode::Mock => LIGHTNING_MOCK_MODE,
            LightningMode::LndRest => LIGHTNING_LND_REST_MODE,
            LightningMode::Phoenixd => LIGHTNING_PHOENIXD_MODE,
        };
        // phoenixd is prepaid (no hold invoices / no escrow bundles); the other
        // backends are hold-invoice escrow.  Advertise capabilities accordingly.
        let mut capabilities = match state.config.lightning.mode {
            LightningMode::Phoenixd => {
                vec!["prepaid".to_string(), "preimage_proof".to_string()]
            }
            LightningMode::Mock | LightningMode::LndRest => {
                vec!["invoice_bundles".to_string(), "hold_invoices".to_string()]
            }
        };
        match state.config.lightning.mode {
            LightningMode::Mock => capabilities.push("mock_mode".to_string()),
            LightningMode::LndRest => {
                capabilities.push("lnd_rest".to_string());
                capabilities.push("node_getinfo".to_string());
            }
            LightningMode::Phoenixd => {
                capabilities.push("phoenixd".to_string());
                capabilities.push("self_custodial".to_string());
                capabilities.push("auto_liquidity".to_string());
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

#[cfg(test)]
mod prepaid_invoice_validation_tests {
    use super::*;
    use crate::protocol::{ARTIFACT_KIND_QUOTE, ExecutionLimits};
    use bitcoin::{
        hashes::{Hash as _, sha256},
        secp256k1::{PublicKey, Secp256k1, SecretKey},
    };
    use lightning_invoice::{InvoiceBuilder, PaymentSecret};
    use std::time::Duration;

    const NOW: i64 = 1_700_000_010;
    const QUOTE_CREATED_AT: i64 = 1_700_000_000;
    const QUOTE_EXPIRES_AT: i64 = 1_700_000_180;
    const DEAL_ADMISSION_DEADLINE: i64 = 1_700_000_150;
    const MOCK_QUOTE_DESTINATION: &str =
        "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    fn signed_prepaid_quote(
        amount_msat: u64,
        destination_identity: impl Into<String>,
        max_invoice_expiry_secs: u64,
    ) -> SignedArtifact<QuotePayload> {
        let provider_key = crypto::generate_signing_key();
        let provider_id = crypto::public_key_hex(&provider_key);
        sign_artifact(
            &provider_id,
            |message| crypto::sign_message_hex(&provider_key, message),
            ARTIFACT_KIND_QUOTE,
            QUOTE_CREATED_AT,
            QuotePayload {
                provider_id: provider_id.clone(),
                requester_id: "requester".to_string(),
                descriptor_hash: "aa".repeat(32),
                offer_hash: "bb".repeat(32),
                expires_at: QUOTE_EXPIRES_AT,
                workload_kind: "compute.wasm.v1".to_string(),
                workload_hash: "cc".repeat(32),
                confidential_session_hash: None,
                capabilities_granted: Vec::new(),
                extension_refs: Vec::new(),
                quote_use: None,
                settlement_terms: QuoteSettlementTerms {
                    method: "lightning.prepaid.v1".to_string(),
                    destination_identity: destination_identity.into(),
                    base_fee_msat: amount_msat,
                    success_fee_msat: 0,
                    max_base_invoice_expiry_secs: max_invoice_expiry_secs,
                    max_success_hold_expiry_secs: 0,
                    min_final_cltv_expiry: 0,
                },
                execution_limits: ExecutionLimits {
                    max_input_bytes: 1,
                    max_runtime_ms: 1,
                    max_memory_bytes: 1,
                    max_output_bytes: 1,
                    fuel_limit: 1,
                },
            },
        )
        .expect("sign prepaid quote")
    }

    fn real_bolt11_invoice(
        currency: Currency,
        payee_secret_byte: u8,
        payment_hash_hex: &str,
        amount_msat: Option<u64>,
        expiry_secs: u64,
    ) -> (String, String) {
        let secp = Secp256k1::new();
        let payee_secret = SecretKey::from_slice(&[payee_secret_byte; 32]).expect("payee secret");
        let payee = PublicKey::from_secret_key(&secp, &payee_secret);
        let payment_hash = sha256::Hash::from_slice(
            &hex::decode(payment_hash_hex).expect("valid payment hash hex"),
        )
        .expect("32-byte payment hash");
        let payment_secret =
            PaymentSecret(sha256::Hash::hash(payment_hash_hex.as_bytes()).to_byte_array());
        let invoice = InvoiceBuilder::new(currency)
            .description("froglet prepaid test".to_string())
            .payment_hash(payment_hash)
            .payment_secret(payment_secret)
            .duration_since_epoch(Duration::from_secs(QUOTE_CREATED_AT as u64))
            .expiry_time(Duration::from_secs(expiry_secs))
            .min_final_cltv_expiry_delta(18)
            .payee_pub_key(payee);
        let invoice = match amount_msat {
            Some(amount_msat) => invoice.amount_milli_satoshis(amount_msat),
            None => invoice,
        }
        .build_signed(|hash| secp.sign_ecdsa_recoverable(hash, &payee_secret))
        .expect("build signed BOLT11 invoice")
        .to_string();
        (invoice, hex::encode(payee.serialize()))
    }

    #[test]
    fn prepaid_validator_accepts_a_mock_invoice_bound_to_the_quote() {
        let payment_hash = "11".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 120);
        let invoice = mock_bolt11("prepaid", 7_000, &payment_hash, NOW + 90);

        validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect("valid prepaid invoice");
    }

    #[test]
    fn prepaid_validator_accepts_a_real_mainnet_invoice_with_the_expected_payee() {
        let payment_hash = "12".repeat(32);
        let (invoice, payee) =
            real_bolt11_invoice(Currency::Bitcoin, 7, &payment_hash, Some(7_000), 120);
        let quote = signed_prepaid_quote(7_000, &payee, 120);

        validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: Some(&payee),
            expected_network: Some(Currency::Bitcoin),
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect("valid real prepaid invoice");
    }

    #[test]
    fn prepaid_validator_rejects_an_invoice_with_the_wrong_amount() {
        let payment_hash = "22".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 120);
        let invoice = mock_bolt11("prepaid", 8_000, &payment_hash, NOW + 90);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("wrong invoice amount must be rejected");

        assert!(error.contains("amount"), "unexpected error: {error}");
    }

    #[test]
    fn prepaid_validator_rejects_a_provider_amount_above_the_signed_quote() {
        let payment_hash = "23".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 120);
        let invoice = mock_bolt11("prepaid", 8_000, &payment_hash, NOW + 90);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 8,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("provider amount above the signed quote must be rejected");

        assert!(
            error.contains("signed quote amount"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn prepaid_validator_rejects_an_invoice_with_the_wrong_payment_hash() {
        let quoted_hash = "33".repeat(32);
        let invoice_hash = "44".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 120);
        let invoice = mock_bolt11("prepaid", 7_000, &invoice_hash, NOW + 90);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &quoted_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("wrong invoice payment hash must be rejected");

        assert!(error.contains("payment hash"), "unexpected error: {error}");
    }

    #[test]
    fn prepaid_validator_rejects_a_real_invoice_for_the_wrong_payee() {
        let payment_hash = "45".repeat(32);
        let (invoice, _) =
            real_bolt11_invoice(Currency::Bitcoin, 7, &payment_hash, Some(7_000), 120);
        let (_, expected_payee) =
            real_bolt11_invoice(Currency::Bitcoin, 8, &payment_hash, Some(7_000), 120);
        let quote = signed_prepaid_quote(7_000, &expected_payee, 120);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: Some(&expected_payee),
            expected_network: Some(Currency::Bitcoin),
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("wrong invoice payee must be rejected");

        assert!(error.contains("destination"), "unexpected error: {error}");
    }

    #[test]
    fn prepaid_validator_rejects_a_destination_not_authorized_by_the_signed_quote() {
        let payment_hash = "4a".repeat(32);
        let (invoice, invoice_payee) =
            real_bolt11_invoice(Currency::Bitcoin, 7, &payment_hash, Some(7_000), 120);
        let (_, quoted_payee) =
            real_bolt11_invoice(Currency::Bitcoin, 8, &payment_hash, Some(7_000), 120);
        let quote = signed_prepaid_quote(7_000, &quoted_payee, 120);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: Some(&invoice_payee),
            expected_network: Some(Currency::Bitcoin),
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("expected payee must be authorized by the signed quote");

        assert!(error.contains("destination"), "unexpected error: {error}");
    }

    #[test]
    fn prepaid_validator_rejects_a_real_invoice_for_the_wrong_network() {
        let payment_hash = "46".repeat(32);
        let (invoice, payee) =
            real_bolt11_invoice(Currency::Regtest, 7, &payment_hash, Some(7_000), 120);
        let quote = signed_prepaid_quote(7_000, &payee, 120);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: Some(&payee),
            expected_network: Some(Currency::Bitcoin),
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("wrong BOLT11 network must be rejected");

        assert!(error.contains("network"), "unexpected error: {error}");
    }

    #[test]
    fn prepaid_validator_rejects_an_amountless_real_invoice() {
        let payment_hash = "47".repeat(32);
        let (invoice, payee) = real_bolt11_invoice(Currency::Bitcoin, 7, &payment_hash, None, 120);
        let quote = signed_prepaid_quote(7_000, &payee, 120);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: Some(&payee),
            expected_network: Some(Currency::Bitcoin),
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("amountless BOLT11 must be rejected");

        assert!(error.contains("amount"), "unexpected error: {error}");
    }

    #[test]
    fn prepaid_validator_rejects_a_missing_invoice() {
        let payment_hash = "48".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 120);

        validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: "",
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("missing BOLT11 must be rejected");
    }

    #[test]
    fn prepaid_validator_rejects_a_malformed_mock_invoice() {
        let payment_hash = "49".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 120);

        validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: "lnmock-prepaid-not-an-amount",
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("malformed mock BOLT11 must be rejected");
    }

    #[test]
    fn prepaid_validator_rejects_terms_from_a_tampered_quote() {
        let payment_hash = "55".repeat(32);
        let mut quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 120);
        quote.payload.workload_hash = "ff".repeat(32);
        let invoice = mock_bolt11("prepaid", 7_000, &payment_hash, NOW + 90);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("unsigned quote terms must be rejected");

        assert!(
            error.contains("quote signature"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn prepaid_validator_rejects_an_expired_invoice() {
        let payment_hash = "66".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 120);
        let invoice = mock_bolt11("prepaid", 7_000, &payment_hash, NOW);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("expired invoice must be rejected");

        assert!(error.contains("expired"), "unexpected error: {error}");
    }

    #[test]
    fn prepaid_validator_rejects_an_invoice_beyond_the_quoted_expiry_window() {
        let payment_hash = "77".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 60);
        let invoice = mock_bolt11("prepaid", 7_000, &payment_hash, NOW + 61);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("overlong invoice must be rejected");

        assert!(error.contains("quoted expiry"), "unexpected error: {error}");
    }

    #[test]
    fn prepaid_validator_rejects_a_real_invoice_with_an_overlong_declared_expiry() {
        let payment_hash = "78".repeat(32);
        let (invoice, payee) =
            real_bolt11_invoice(Currency::Bitcoin, 7, &payment_hash, Some(7_000), 121);
        let quote = signed_prepaid_quote(7_000, &payee, 120);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: Some(&payee),
            expected_network: Some(Currency::Bitcoin),
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("overlong declared BOLT11 expiry must be rejected");

        assert!(error.contains("quoted expiry"), "unexpected error: {error}");
    }

    #[test]
    fn prepaid_validator_rejects_an_invoice_expiring_after_the_quote() {
        let payment_hash = "88".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 300);
        let invoice = mock_bolt11("prepaid", 7_000, &payment_hash, QUOTE_EXPIRES_AT + 1);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: QUOTE_EXPIRES_AT + 20,
            quote: &quote,
        })
        .expect_err("invoice beyond the quote deadline must be rejected");

        assert!(
            error.contains("quote deadline"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn prepaid_validator_rejects_an_invoice_expiring_after_deal_admission() {
        let payment_hash = "99".repeat(32);
        let quote = signed_prepaid_quote(7_000, MOCK_QUOTE_DESTINATION, 300);
        let invoice = mock_bolt11("prepaid", 7_000, &payment_hash, DEAL_ADMISSION_DEADLINE + 1);

        let error = validate_prepaid_lightning_invoice(PrepaidLightningInvoiceValidation {
            invoice_bolt11: &invoice,
            expected_payment_hash: &payment_hash,
            expected_amount_sat: 7,
            expected_destination_identity: None,
            expected_network: None,
            now: NOW,
            deal_admission_deadline: DEAL_ADMISSION_DEADLINE,
            quote: &quote,
        })
        .expect_err("invoice beyond admission must be rejected");

        assert!(
            error.contains("admission deadline"),
            "unexpected error: {error}"
        );
    }
}
