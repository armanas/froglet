use serde::{Deserialize, Serialize};

use super::kernel::{
    ARTIFACT_TYPE_DEAL, ARTIFACT_TYPE_DESCRIPTOR, ARTIFACT_TYPE_OFFER, ARTIFACT_TYPE_QUOTE,
    ARTIFACT_TYPE_RECEIPT, DealPayload, DescriptorPayload, ExecutionLimits, InvoiceBundleLeg,
    InvoiceBundleLegState, InvoiceBundlePayload, OfferPayload, QuotePayload, ReceiptPayload,
    SignedArtifact, TRANSPORT_TYPE_INVOICE_BUNDLE, validate_descriptor_artifact,
    validate_offer_artifact, validate_receipt_artifact, verify_artifact,
};
use crate::crypto;

pub const ISSUE_ARTIFACT_TYPE_MISMATCH: &str = "artifact_type_mismatch";
pub const ISSUE_ARTIFACT_ENVELOPE_INVALID: &str = "artifact_envelope_invalid";
pub const ISSUE_ARTIFACT_SEMANTIC_INVALID: &str = "artifact_semantic_invalid";
pub const ISSUE_ARTIFACT_EXPIRED: &str = "artifact_expired";
pub const ISSUE_PROVIDER_MISMATCH: &str = "provider_mismatch";
pub const ISSUE_REQUESTER_MISMATCH: &str = "requester_mismatch";
pub const ISSUE_DESCRIPTOR_HASH_MISMATCH: &str = "descriptor_hash_mismatch";
pub const ISSUE_OFFER_HASH_MISMATCH: &str = "offer_hash_mismatch";
pub const ISSUE_QUOTE_HASH_MISMATCH: &str = "quote_hash_mismatch";
pub const ISSUE_DEAL_HASH_MISMATCH: &str = "deal_hash_mismatch";
pub const ISSUE_WORKLOAD_KIND_MISMATCH: &str = "workload_kind_mismatch";
pub const ISSUE_WORKLOAD_HASH_MISMATCH: &str = "workload_hash_mismatch";
pub const ISSUE_CONFIDENTIAL_SESSION_HASH_MISMATCH: &str = "confidential_session_hash_mismatch";
pub const ISSUE_QUOTE_EXPIRY_EXCEEDS_OFFER: &str = "quote_expiry_exceeds_offer";
pub const ISSUE_SETTLEMENT_METHOD_MISMATCH: &str = "settlement_method_mismatch";
pub const ISSUE_SETTLEMENT_TERMS_MISMATCH: &str = "settlement_terms_mismatch";
pub const ISSUE_EXECUTION_LIMITS_EXCEED_OFFER: &str = "execution_limits_exceed_offer";
pub const ISSUE_DEADLINE_ORDER_INVALID: &str = "deadline_order_invalid";
pub const ISSUE_DEADLINE_EXCEEDS_QUOTE: &str = "deadline_exceeds_quote";
pub const ISSUE_INVOICE_BUNDLE_FOR_NON_LIGHTNING: &str = "invoice_bundle_for_non_lightning_method";
pub const ISSUE_INVOICE_AMOUNT_MISMATCH: &str = "invoice_amount_mismatch";
pub const ISSUE_INVOICE_DESTINATION_MISMATCH: &str = "invoice_destination_mismatch";
pub const ISSUE_INVOICE_SUCCESS_PAYMENT_HASH_MISMATCH: &str =
    "invoice_success_payment_hash_mismatch";
pub const ISSUE_INVOICE_MIN_CLTV_MISMATCH: &str = "invoice_min_cltv_mismatch";
pub const ISSUE_INVOICE_HASH_MISMATCH: &str = "invoice_hash_mismatch";
pub const ISSUE_INVOICE_EXPIRY_EXCEEDS_DEAL: &str = "invoice_expiry_exceeds_deal";

const SETTLEMENT_METHOD_NONE: &str = "none";
const SETTLEMENT_METHOD_LIGHTNING_ESCROW: &str = "lightning.base_fee_plus_success_fee.v1";
const SETTLEMENT_METHOD_STRIPE_MPP: &str = "stripe_mpp.v1";
const SETTLEMENT_METHOD_LIGHTNING_PREPAID: &str = "lightning.prepaid.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainPath {
    DescriptorOffer,
    OfferQuote,
    QuoteDeal,
    QuoteInvoiceBundleDeal,
    QuoteDealReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainValidationIssue {
    pub code: String,
    pub artifact_type: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainValidationReport {
    pub path: ChainPath,
    pub valid: bool,
    pub issues: Vec<ChainValidationIssue>,
}

impl ChainValidationReport {
    fn new(path: ChainPath) -> Self {
        Self {
            path,
            valid: true,
            issues: Vec::new(),
        }
    }

    fn push_issue(
        &mut self,
        code: &'static str,
        artifact_type: &'static str,
        message: impl Into<String>,
    ) {
        self.valid = false;
        self.issues.push(ChainValidationIssue {
            code: code.to_string(),
            artifact_type: artifact_type.to_string(),
            message: message.into(),
        });
    }
}

pub fn validate_descriptor_offer(
    descriptor: &SignedArtifact<DescriptorPayload>,
    offer: &SignedArtifact<OfferPayload>,
    now: Option<i64>,
) -> ChainValidationReport {
    let mut report = ChainValidationReport::new(ChainPath::DescriptorOffer);
    verify_descriptor_envelope(&mut report, descriptor);
    verify_offer_envelope(&mut report, offer);
    validate_descriptor_semantics(&mut report, descriptor, now);
    validate_offer_semantics(&mut report, offer, now);

    if offer.payload.provider_id != descriptor.payload.provider_id {
        report.push_issue(
            ISSUE_PROVIDER_MISMATCH,
            ARTIFACT_TYPE_OFFER,
            "offer provider_id must match descriptor provider_id",
        );
    }
    if offer.payload.descriptor_hash != descriptor.hash {
        report.push_issue(
            ISSUE_DESCRIPTOR_HASH_MISMATCH,
            ARTIFACT_TYPE_OFFER,
            "offer descriptor_hash must match descriptor hash",
        );
    }

    report
}

pub fn validate_offer_quote(
    offer: &SignedArtifact<OfferPayload>,
    quote: &SignedArtifact<QuotePayload>,
    now: Option<i64>,
) -> ChainValidationReport {
    let mut report = ChainValidationReport::new(ChainPath::OfferQuote);
    verify_offer_envelope(&mut report, offer);
    verify_quote_envelope(&mut report, quote);
    validate_offer_semantics(&mut report, offer, now);
    validate_quote_semantics(&mut report, quote, now);
    validate_offer_quote_links(&mut report, offer, quote);
    report
}

pub fn validate_quote_deal(
    quote: &SignedArtifact<QuotePayload>,
    deal: &SignedArtifact<DealPayload>,
    now: Option<i64>,
) -> ChainValidationReport {
    let mut report = ChainValidationReport::new(ChainPath::QuoteDeal);
    verify_quote_envelope(&mut report, quote);
    verify_deal_envelope(&mut report, deal);
    validate_quote_semantics(&mut report, quote, now);
    validate_deal_semantics(&mut report, deal);
    validate_quote_deal_links(&mut report, quote, deal);
    report
}

pub fn validate_quote_invoice_bundle_deal(
    quote: &SignedArtifact<QuotePayload>,
    invoice_bundle: &SignedArtifact<InvoiceBundlePayload>,
    deal: &SignedArtifact<DealPayload>,
    now: Option<i64>,
) -> ChainValidationReport {
    let mut report = ChainValidationReport::new(ChainPath::QuoteInvoiceBundleDeal);
    verify_quote_envelope(&mut report, quote);
    verify_invoice_bundle_envelope(&mut report, invoice_bundle);
    verify_deal_envelope(&mut report, deal);
    validate_quote_semantics(&mut report, quote, now);
    validate_invoice_bundle_semantics(&mut report, invoice_bundle, now);
    validate_deal_semantics(&mut report, deal);
    validate_quote_deal_links(&mut report, quote, deal);
    validate_invoice_bundle_links(&mut report, quote, invoice_bundle, deal);
    report
}

pub fn validate_quote_deal_receipt(
    quote: &SignedArtifact<QuotePayload>,
    deal: &SignedArtifact<DealPayload>,
    receipt: &SignedArtifact<ReceiptPayload>,
    now: Option<i64>,
) -> ChainValidationReport {
    let mut report = ChainValidationReport::new(ChainPath::QuoteDealReceipt);
    verify_quote_envelope(&mut report, quote);
    verify_deal_envelope(&mut report, deal);
    verify_receipt_envelope(&mut report, receipt);
    validate_quote_semantics(&mut report, quote, now);
    validate_deal_semantics(&mut report, deal);
    validate_receipt_semantics(&mut report, receipt);
    validate_quote_deal_links(&mut report, quote, deal);
    validate_receipt_links(&mut report, quote, deal, receipt);
    report
}

fn verify_descriptor_envelope(
    report: &mut ChainValidationReport,
    descriptor: &SignedArtifact<DescriptorPayload>,
) {
    verify_common_artifact(report, descriptor, ARTIFACT_TYPE_DESCRIPTOR);
}

fn validate_descriptor_semantics(
    report: &mut ChainValidationReport,
    descriptor: &SignedArtifact<DescriptorPayload>,
    now: Option<i64>,
) {
    if let Err(message) = validate_descriptor_artifact(descriptor) {
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            ARTIFACT_TYPE_DESCRIPTOR,
            message,
        );
    }
    if let (Some(now), Some(expires_at)) = (now, descriptor.payload.expires_at)
        && expires_at < now
    {
        report.push_issue(
            ISSUE_ARTIFACT_EXPIRED,
            ARTIFACT_TYPE_DESCRIPTOR,
            "descriptor expires_at is earlier than now",
        );
    }
}

fn verify_offer_envelope(report: &mut ChainValidationReport, offer: &SignedArtifact<OfferPayload>) {
    verify_common_artifact(report, offer, ARTIFACT_TYPE_OFFER);
}

fn validate_offer_semantics(
    report: &mut ChainValidationReport,
    offer: &SignedArtifact<OfferPayload>,
    now: Option<i64>,
) {
    if let Err(message) = validate_offer_artifact(offer) {
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            ARTIFACT_TYPE_OFFER,
            message,
        );
    }
    validate_offer_settlement(report, offer);
    if let (Some(now), Some(expires_at)) = (now, offer.payload.expires_at)
        && expires_at < now
    {
        report.push_issue(
            ISSUE_ARTIFACT_EXPIRED,
            ARTIFACT_TYPE_OFFER,
            "offer expires_at is earlier than now",
        );
    }
}

fn verify_quote_envelope(report: &mut ChainValidationReport, quote: &SignedArtifact<QuotePayload>) {
    verify_common_artifact(report, quote, ARTIFACT_TYPE_QUOTE);
}

fn validate_quote_semantics(
    report: &mut ChainValidationReport,
    quote: &SignedArtifact<QuotePayload>,
    now: Option<i64>,
) {
    if let Err(message) = validate_quote_artifact(quote) {
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            ARTIFACT_TYPE_QUOTE,
            message,
        );
    }
    if let Some(now) = now
        && quote.payload.expires_at < now
    {
        report.push_issue(
            ISSUE_ARTIFACT_EXPIRED,
            ARTIFACT_TYPE_QUOTE,
            "quote expires_at is earlier than now",
        );
    }
}

fn verify_deal_envelope(report: &mut ChainValidationReport, deal: &SignedArtifact<DealPayload>) {
    verify_common_artifact(report, deal, ARTIFACT_TYPE_DEAL);
}

fn validate_deal_semantics(report: &mut ChainValidationReport, deal: &SignedArtifact<DealPayload>) {
    if let Err(message) = validate_deal_artifact(deal) {
        report.push_issue(ISSUE_ARTIFACT_SEMANTIC_INVALID, ARTIFACT_TYPE_DEAL, message);
    }
}

fn verify_invoice_bundle_envelope(
    report: &mut ChainValidationReport,
    invoice_bundle: &SignedArtifact<InvoiceBundlePayload>,
) {
    verify_common_artifact(report, invoice_bundle, TRANSPORT_TYPE_INVOICE_BUNDLE);
}

fn validate_invoice_bundle_semantics(
    report: &mut ChainValidationReport,
    invoice_bundle: &SignedArtifact<InvoiceBundlePayload>,
    now: Option<i64>,
) {
    if let Err(message) = validate_invoice_bundle_artifact(invoice_bundle) {
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            message,
        );
    }
    if let Some(now) = now
        && invoice_bundle.payload.expires_at < now
    {
        report.push_issue(
            ISSUE_ARTIFACT_EXPIRED,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle expires_at is earlier than now",
        );
    }
}

fn verify_receipt_envelope(
    report: &mut ChainValidationReport,
    receipt: &SignedArtifact<ReceiptPayload>,
) {
    verify_common_artifact(report, receipt, ARTIFACT_TYPE_RECEIPT);
}

fn validate_receipt_semantics(
    report: &mut ChainValidationReport,
    receipt: &SignedArtifact<ReceiptPayload>,
) {
    if let Err(message) = validate_receipt_artifact(receipt) {
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            ARTIFACT_TYPE_RECEIPT,
            message,
        );
    }
}

fn verify_common_artifact<T: Serialize>(
    report: &mut ChainValidationReport,
    artifact: &SignedArtifact<T>,
    expected_artifact_type: &'static str,
) {
    if artifact.artifact_type != expected_artifact_type {
        report.push_issue(
            ISSUE_ARTIFACT_TYPE_MISMATCH,
            expected_artifact_type,
            format!(
                "expected artifact_type {expected_artifact_type}, got {}",
                artifact.artifact_type
            ),
        );
    }
    if !verify_artifact(artifact) {
        report.push_issue(
            ISSUE_ARTIFACT_ENVELOPE_INVALID,
            expected_artifact_type,
            "artifact envelope hash, payload hash, schema version, or signature is invalid",
        );
    }
}

fn validate_offer_settlement(
    report: &mut ChainValidationReport,
    offer: &SignedArtifact<OfferPayload>,
) {
    let fees_are_zero = offer.payload.price_schedule.base_fee_msat == 0
        && offer.payload.price_schedule.success_fee_msat == 0;
    if fees_are_zero && offer.payload.settlement_method != SETTLEMENT_METHOD_NONE {
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            ARTIFACT_TYPE_OFFER,
            "zero-fee offers must use settlement_method none",
        );
    } else if !fees_are_zero && !is_known_paid_settlement_method(&offer.payload.settlement_method) {
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            ARTIFACT_TYPE_OFFER,
            "paid offers must use a known paid settlement method",
        );
    }
}

fn validate_quote_artifact(quote: &SignedArtifact<QuotePayload>) -> Result<(), String> {
    let payload = &quote.payload;
    if quote.signer != payload.provider_id {
        return Err("quote signer does not match provider_id".to_string());
    }
    if payload.requester_id.trim().is_empty() {
        return Err("quote requester_id must be non-empty".to_string());
    }
    if payload.descriptor_hash.trim().is_empty() {
        return Err("quote descriptor_hash must be non-empty".to_string());
    }
    if payload.offer_hash.trim().is_empty() {
        return Err("quote offer_hash must be non-empty".to_string());
    }
    if payload.workload_kind.trim().is_empty() {
        return Err("quote workload_kind must be non-empty".to_string());
    }
    if payload.workload_hash.trim().is_empty() {
        return Err("quote workload_hash must be non-empty".to_string());
    }
    validate_quote_settlement_terms(&payload.settlement_terms)?;
    Ok(())
}

fn validate_deal_artifact(deal: &SignedArtifact<DealPayload>) -> Result<(), String> {
    let payload = &deal.payload;
    if deal.signer != payload.requester_id {
        return Err("deal signer does not match requester_id".to_string());
    }
    if payload.provider_id.trim().is_empty() {
        return Err("deal provider_id must be non-empty".to_string());
    }
    if payload.quote_hash.trim().is_empty() {
        return Err("deal quote_hash must be non-empty".to_string());
    }
    if payload.workload_hash.trim().is_empty() {
        return Err("deal workload_hash must be non-empty".to_string());
    }
    if !is_lower_hex_len(&payload.success_payment_hash, 64) {
        return Err("deal success_payment_hash must be lowercase 32-byte hex".to_string());
    }
    if payload.completion_deadline <= payload.admission_deadline {
        return Err("deal completion_deadline must be greater than admission_deadline".to_string());
    }
    if payload.acceptance_deadline < payload.completion_deadline {
        return Err(
            "deal acceptance_deadline must be greater than or equal to completion_deadline"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_invoice_bundle_artifact(
    invoice_bundle: &SignedArtifact<InvoiceBundlePayload>,
) -> Result<(), String> {
    let payload = &invoice_bundle.payload;
    if invoice_bundle.signer != payload.provider_id {
        return Err("invoice_bundle signer does not match provider_id".to_string());
    }
    if payload.requester_id.trim().is_empty() {
        return Err("invoice_bundle requester_id must be non-empty".to_string());
    }
    if payload.quote_hash.trim().is_empty() {
        return Err("invoice_bundle quote_hash must be non-empty".to_string());
    }
    if payload.deal_hash.trim().is_empty() {
        return Err("invoice_bundle deal_hash must be non-empty".to_string());
    }
    if !is_lower_hex_len(&payload.destination_identity, 66) {
        return Err(
            "invoice_bundle destination_identity must be compressed secp256k1 lowercase hex"
                .to_string(),
        );
    }
    validate_invoice_leg("base_fee", &payload.base_fee)?;
    validate_invoice_leg("success_fee", &payload.success_fee)?;
    if payload.success_fee.state != InvoiceBundleLegState::Open {
        return Err("invoice_bundle success_fee.state must be open at issuance".to_string());
    }
    if payload.base_fee.state != InvoiceBundleLegState::Open
        && !(payload.base_fee.amount_msat == 0
            && payload.base_fee.state == InvoiceBundleLegState::Settled)
    {
        return Err(
            "invoice_bundle base_fee.state must be open unless zero-valued and settled".to_string(),
        );
    }
    Ok(())
}

fn validate_invoice_leg(name: &str, leg: &InvoiceBundleLeg) -> Result<(), String> {
    if leg.invoice_bolt11.trim().is_empty() {
        return Err(format!(
            "invoice_bundle {name}.invoice_bolt11 must be non-empty"
        ));
    }
    if !is_lower_hex_len(&leg.invoice_hash, 64) {
        return Err(format!(
            "invoice_bundle {name}.invoice_hash must be lowercase 32-byte hex"
        ));
    }
    if !is_lower_hex_len(&leg.payment_hash, 64) {
        return Err(format!(
            "invoice_bundle {name}.payment_hash must be lowercase 32-byte hex"
        ));
    }
    let invoice_hash = crypto::sha256_hex(leg.invoice_bolt11.as_bytes());
    if leg.invoice_hash != invoice_hash {
        return Err(format!(
            "invoice_bundle {name}.invoice_hash must equal SHA256(invoice_bolt11)"
        ));
    }
    Ok(())
}

fn validate_offer_quote_links(
    report: &mut ChainValidationReport,
    offer: &SignedArtifact<OfferPayload>,
    quote: &SignedArtifact<QuotePayload>,
) {
    if quote.payload.provider_id != offer.payload.provider_id {
        report.push_issue(
            ISSUE_PROVIDER_MISMATCH,
            ARTIFACT_TYPE_QUOTE,
            "quote provider_id must match offer provider_id",
        );
    }
    if quote.payload.descriptor_hash != offer.payload.descriptor_hash {
        report.push_issue(
            ISSUE_DESCRIPTOR_HASH_MISMATCH,
            ARTIFACT_TYPE_QUOTE,
            "quote descriptor_hash must match offer descriptor_hash",
        );
    }
    if quote.payload.offer_hash != offer.hash {
        report.push_issue(
            ISSUE_OFFER_HASH_MISMATCH,
            ARTIFACT_TYPE_QUOTE,
            "quote offer_hash must match offer hash",
        );
    }
    if let Some(offer_expires_at) = offer.payload.expires_at
        && quote.payload.expires_at > offer_expires_at
    {
        report.push_issue(
            ISSUE_QUOTE_EXPIRY_EXCEEDS_OFFER,
            ARTIFACT_TYPE_QUOTE,
            "quote expires_at must not exceed offer expires_at",
        );
    }
    if quote.payload.workload_kind != offer.payload.offer_kind {
        report.push_issue(
            ISSUE_WORKLOAD_KIND_MISMATCH,
            ARTIFACT_TYPE_QUOTE,
            "quote workload_kind must match offer offer_kind",
        );
    }
    if quote.payload.settlement_terms.method != offer.payload.settlement_method {
        report.push_issue(
            ISSUE_SETTLEMENT_METHOD_MISMATCH,
            ARTIFACT_TYPE_QUOTE,
            "quote settlement_terms.method must match offer settlement_method",
        );
    }
    if quote.payload.settlement_terms.base_fee_msat != offer.payload.price_schedule.base_fee_msat
        || quote.payload.settlement_terms.success_fee_msat
            != offer.payload.price_schedule.success_fee_msat
    {
        report.push_issue(
            ISSUE_SETTLEMENT_TERMS_MISMATCH,
            ARTIFACT_TYPE_QUOTE,
            "quote settlement fee amounts must match offer price_schedule",
        );
    }
    if !limits_within_offer(&quote.payload.execution_limits, offer) {
        report.push_issue(
            ISSUE_EXECUTION_LIMITS_EXCEED_OFFER,
            ARTIFACT_TYPE_QUOTE,
            "quote execution_limits must not exceed offer execution_profile maxima",
        );
    }
}

fn validate_quote_deal_links(
    report: &mut ChainValidationReport,
    quote: &SignedArtifact<QuotePayload>,
    deal: &SignedArtifact<DealPayload>,
) {
    if deal.payload.provider_id != quote.payload.provider_id {
        report.push_issue(
            ISSUE_PROVIDER_MISMATCH,
            ARTIFACT_TYPE_DEAL,
            "deal provider_id must match quote provider_id",
        );
    }
    if deal.payload.requester_id != quote.payload.requester_id {
        report.push_issue(
            ISSUE_REQUESTER_MISMATCH,
            ARTIFACT_TYPE_DEAL,
            "deal requester_id must match quote requester_id",
        );
    }
    if deal.payload.quote_hash != quote.hash {
        report.push_issue(
            ISSUE_QUOTE_HASH_MISMATCH,
            ARTIFACT_TYPE_DEAL,
            "deal quote_hash must match quote hash",
        );
    }
    if deal.payload.workload_hash != quote.payload.workload_hash {
        report.push_issue(
            ISSUE_WORKLOAD_HASH_MISMATCH,
            ARTIFACT_TYPE_DEAL,
            "deal workload_hash must match quote workload_hash",
        );
    }
    if deal.payload.confidential_session_hash != quote.payload.confidential_session_hash {
        report.push_issue(
            ISSUE_CONFIDENTIAL_SESSION_HASH_MISMATCH,
            ARTIFACT_TYPE_DEAL,
            "deal confidential_session_hash must match quote confidential_session_hash",
        );
    }
    if deal.payload.admission_deadline > quote.payload.expires_at {
        report.push_issue(
            ISSUE_DEADLINE_EXCEEDS_QUOTE,
            ARTIFACT_TYPE_DEAL,
            "deal admission_deadline must not exceed quote expires_at",
        );
    }
    if deal.payload.completion_deadline <= deal.payload.admission_deadline
        || deal.payload.acceptance_deadline < deal.payload.completion_deadline
    {
        report.push_issue(
            ISSUE_DEADLINE_ORDER_INVALID,
            ARTIFACT_TYPE_DEAL,
            "deal deadlines must satisfy admission < completion <= acceptance",
        );
    }
}

fn validate_invoice_bundle_links(
    report: &mut ChainValidationReport,
    quote: &SignedArtifact<QuotePayload>,
    invoice_bundle: &SignedArtifact<InvoiceBundlePayload>,
    deal: &SignedArtifact<DealPayload>,
) {
    if quote.payload.settlement_terms.method != SETTLEMENT_METHOD_LIGHTNING_ESCROW {
        report.push_issue(
            ISSUE_INVOICE_BUNDLE_FOR_NON_LIGHTNING,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle is only valid for lightning.base_fee_plus_success_fee.v1 quotes",
        );
    }
    if invoice_bundle.payload.provider_id != quote.payload.provider_id
        || invoice_bundle.payload.provider_id != deal.payload.provider_id
    {
        report.push_issue(
            ISSUE_PROVIDER_MISMATCH,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle provider_id must match quote and deal provider_id",
        );
    }
    if invoice_bundle.payload.requester_id != quote.payload.requester_id
        || invoice_bundle.payload.requester_id != deal.payload.requester_id
    {
        report.push_issue(
            ISSUE_REQUESTER_MISMATCH,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle requester_id must match quote and deal requester_id",
        );
    }
    if invoice_bundle.payload.quote_hash != quote.hash {
        report.push_issue(
            ISSUE_QUOTE_HASH_MISMATCH,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle quote_hash must match quote hash",
        );
    }
    if invoice_bundle.payload.deal_hash != deal.hash {
        report.push_issue(
            ISSUE_DEAL_HASH_MISMATCH,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle deal_hash must match deal hash",
        );
    }
    if invoice_bundle.payload.destination_identity
        != quote.payload.settlement_terms.destination_identity
    {
        report.push_issue(
            ISSUE_INVOICE_DESTINATION_MISMATCH,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle destination_identity must match quote settlement terms",
        );
    }
    if invoice_bundle.payload.base_fee.amount_msat != quote.payload.settlement_terms.base_fee_msat
        || invoice_bundle.payload.success_fee.amount_msat
            != quote.payload.settlement_terms.success_fee_msat
    {
        report.push_issue(
            ISSUE_INVOICE_AMOUNT_MISMATCH,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle fee amounts must match quote settlement terms",
        );
    }
    if invoice_bundle.payload.success_fee.payment_hash != deal.payload.success_payment_hash {
        report.push_issue(
            ISSUE_INVOICE_SUCCESS_PAYMENT_HASH_MISMATCH,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle success_fee.payment_hash must match deal success_payment_hash",
        );
    }
    if invoice_bundle.payload.min_final_cltv_expiry
        != quote.payload.settlement_terms.min_final_cltv_expiry
    {
        report.push_issue(
            ISSUE_INVOICE_MIN_CLTV_MISMATCH,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle min_final_cltv_expiry must match quote settlement terms",
        );
    }
    if invoice_bundle.payload.expires_at > quote.payload.expires_at {
        report.push_issue(
            ISSUE_DEADLINE_EXCEEDS_QUOTE,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle expires_at must not exceed quote expires_at",
        );
    }
    if invoice_bundle.payload.expires_at > deal.payload.admission_deadline {
        report.push_issue(
            ISSUE_INVOICE_EXPIRY_EXCEEDS_DEAL,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle expires_at must not exceed deal admission_deadline",
        );
    }
    validate_invoice_leg_hash_link(report, &invoice_bundle.payload.base_fee);
    validate_invoice_leg_hash_link(report, &invoice_bundle.payload.success_fee);
}

fn validate_receipt_links(
    report: &mut ChainValidationReport,
    quote: &SignedArtifact<QuotePayload>,
    deal: &SignedArtifact<DealPayload>,
    receipt: &SignedArtifact<ReceiptPayload>,
) {
    if receipt.payload.provider_id != quote.payload.provider_id
        || receipt.payload.provider_id != deal.payload.provider_id
    {
        report.push_issue(
            ISSUE_PROVIDER_MISMATCH,
            ARTIFACT_TYPE_RECEIPT,
            "receipt provider_id must match quote and deal provider_id",
        );
    }
    if receipt.payload.requester_id != quote.payload.requester_id
        || receipt.payload.requester_id != deal.payload.requester_id
    {
        report.push_issue(
            ISSUE_REQUESTER_MISMATCH,
            ARTIFACT_TYPE_RECEIPT,
            "receipt requester_id must match quote and deal requester_id",
        );
    }
    if receipt.payload.quote_hash != quote.hash
        || receipt.payload.quote_hash != deal.payload.quote_hash
    {
        report.push_issue(
            ISSUE_QUOTE_HASH_MISMATCH,
            ARTIFACT_TYPE_RECEIPT,
            "receipt quote_hash must match quote hash and deal quote_hash",
        );
    }
    if receipt.payload.deal_hash != deal.hash {
        report.push_issue(
            ISSUE_DEAL_HASH_MISMATCH,
            ARTIFACT_TYPE_RECEIPT,
            "receipt deal_hash must match deal hash",
        );
    }
    if receipt.payload.confidential_session_hash != quote.payload.confidential_session_hash
        || receipt.payload.confidential_session_hash != deal.payload.confidential_session_hash
    {
        report.push_issue(
            ISSUE_CONFIDENTIAL_SESSION_HASH_MISMATCH,
            ARTIFACT_TYPE_RECEIPT,
            "receipt confidential_session_hash must match quote and deal confidential_session_hash",
        );
    }
    if receipt.payload.settlement_refs.method != quote.payload.settlement_terms.method {
        report.push_issue(
            ISSUE_SETTLEMENT_METHOD_MISMATCH,
            ARTIFACT_TYPE_RECEIPT,
            "receipt settlement_refs.method must match quote settlement terms",
        );
    }
    if receipt.payload.settlement_refs.base_fee.amount_msat
        != quote.payload.settlement_terms.base_fee_msat
        || receipt.payload.settlement_refs.success_fee.amount_msat
            != quote.payload.settlement_terms.success_fee_msat
    {
        report.push_issue(
            ISSUE_SETTLEMENT_TERMS_MISMATCH,
            ARTIFACT_TYPE_RECEIPT,
            "receipt settlement fee amounts must match quote settlement terms",
        );
    }
    if receipt.payload.settlement_refs.destination_identity
        != quote.payload.settlement_terms.destination_identity
    {
        report.push_issue(
            ISSUE_INVOICE_DESTINATION_MISMATCH,
            ARTIFACT_TYPE_RECEIPT,
            "receipt destination_identity must match quote settlement terms",
        );
    }
    if !limits_within_quote(&receipt.payload.limits_applied, quote) {
        report.push_issue(
            ISSUE_EXECUTION_LIMITS_EXCEED_OFFER,
            ARTIFACT_TYPE_RECEIPT,
            "receipt limits_applied must not exceed quote execution_limits",
        );
    }
}

fn validate_quote_settlement_terms(
    terms: &super::kernel::QuoteSettlementTerms,
) -> Result<(), String> {
    match terms.method.as_str() {
        SETTLEMENT_METHOD_NONE => {
            if !terms.destination_identity.is_empty() {
                return Err("free quote destination_identity must be empty".to_string());
            }
            if terms.base_fee_msat != 0 || terms.success_fee_msat != 0 {
                return Err("free quote fee amounts must be zero".to_string());
            }
        }
        SETTLEMENT_METHOD_LIGHTNING_ESCROW => {
            if !is_lower_hex_len(&terms.destination_identity, 66) {
                return Err(
                    "lightning quote destination_identity must be compressed secp256k1 lowercase hex"
                        .to_string(),
                );
            }
        }
        SETTLEMENT_METHOD_STRIPE_MPP | SETTLEMENT_METHOD_LIGHTNING_PREPAID => {
            if !terms.destination_identity.is_empty() {
                return Err("non-escrow quote destination_identity must be empty".to_string());
            }
            if terms.success_fee_msat != 0 {
                return Err("non-escrow quote success_fee_msat must be zero".to_string());
            }
        }
        _ => return Err("quote settlement_terms.method is invalid".to_string()),
    }
    Ok(())
}

fn limits_within_offer(limits: &ExecutionLimits, offer: &SignedArtifact<OfferPayload>) -> bool {
    limits.max_input_bytes <= offer.payload.execution_profile.max_input_bytes
        && limits.max_runtime_ms <= offer.payload.execution_profile.max_runtime_ms
        && limits.max_memory_bytes <= offer.payload.execution_profile.max_memory_bytes
        && limits.max_output_bytes <= offer.payload.execution_profile.max_output_bytes
        && limits.fuel_limit <= offer.payload.execution_profile.fuel_limit
}

fn limits_within_quote(limits: &ExecutionLimits, quote: &SignedArtifact<QuotePayload>) -> bool {
    limits.max_input_bytes <= quote.payload.execution_limits.max_input_bytes
        && limits.max_runtime_ms <= quote.payload.execution_limits.max_runtime_ms
        && limits.max_memory_bytes <= quote.payload.execution_limits.max_memory_bytes
        && limits.max_output_bytes <= quote.payload.execution_limits.max_output_bytes
        && limits.fuel_limit <= quote.payload.execution_limits.fuel_limit
}

fn is_known_paid_settlement_method(method: &str) -> bool {
    matches!(
        method,
        SETTLEMENT_METHOD_LIGHTNING_ESCROW
            | SETTLEMENT_METHOD_STRIPE_MPP
            | SETTLEMENT_METHOD_LIGHTNING_PREPAID
    )
}

fn validate_invoice_leg_hash_link(report: &mut ChainValidationReport, leg: &InvoiceBundleLeg) {
    if leg.invoice_hash != crypto::sha256_hex(leg.invoice_bolt11.as_bytes()) {
        report.push_issue(
            ISSUE_INVOICE_HASH_MISMATCH,
            TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice leg invoice_hash must equal SHA256(invoice_bolt11)",
        );
    }
}

fn is_lower_hex_len(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::{fs, path::PathBuf};

    #[derive(Debug, Deserialize)]
    struct KernelConformanceFixture {
        artifacts: ArtifactVectors,
    }

    #[derive(Debug, Deserialize)]
    struct ArtifactVectors {
        descriptor: ArtifactVector<DescriptorPayload>,
        offer: ArtifactVector<OfferPayload>,
        quote: ArtifactVector<QuotePayload>,
        deal: ArtifactVector<DealPayload>,
        invoice_bundle: ArtifactVector<InvoiceBundlePayload>,
        receipt: ArtifactVector<ReceiptPayload>,
        free_offer: ArtifactVector<OfferPayload>,
        free_quote: ArtifactVector<QuotePayload>,
        free_deal: ArtifactVector<DealPayload>,
        free_receipt: ArtifactVector<ReceiptPayload>,
    }

    #[derive(Debug, Deserialize)]
    struct ArtifactVector<T> {
        artifact: SignedArtifact<T>,
    }

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/kernel_v1.json")
    }

    fn load_fixture() -> KernelConformanceFixture {
        let bytes = fs::read_to_string(fixture_path()).expect("read conformance fixture");
        serde_json::from_str(&bytes).expect("parse conformance fixture")
    }

    fn issue_codes(report: &ChainValidationReport) -> Vec<String> {
        report
            .issues
            .iter()
            .map(|issue| issue.code.clone())
            .collect()
    }

    #[test]
    fn validates_canonical_paid_chain_partials() {
        let fixture = load_fixture();
        let descriptor = &fixture.artifacts.descriptor.artifact;
        let offer = &fixture.artifacts.offer.artifact;
        let quote = &fixture.artifacts.quote.artifact;
        let deal = &fixture.artifacts.deal.artifact;
        let invoice_bundle = &fixture.artifacts.invoice_bundle.artifact;
        let receipt = &fixture.artifacts.receipt.artifact;

        let descriptor_offer = validate_descriptor_offer(descriptor, offer, None);
        assert!(
            descriptor_offer.valid,
            "unexpected issues: {:?}",
            descriptor_offer.issues
        );

        let offer_quote = validate_offer_quote(offer, quote, None);
        assert!(
            offer_quote.valid,
            "unexpected issues: {:?}",
            offer_quote.issues
        );

        let quote_deal = validate_quote_deal(quote, deal, None);
        assert!(
            quote_deal.valid,
            "unexpected issues: {:?}",
            quote_deal.issues
        );

        let quote_invoice_bundle_deal =
            validate_quote_invoice_bundle_deal(quote, invoice_bundle, deal, None);
        assert!(
            quote_invoice_bundle_deal.valid,
            "unexpected issues: {:?}",
            quote_invoice_bundle_deal.issues
        );

        let quote_deal_receipt = validate_quote_deal_receipt(quote, deal, receipt, None);
        assert!(
            quote_deal_receipt.valid,
            "unexpected issues: {:?}",
            quote_deal_receipt.issues
        );
    }

    #[test]
    fn validates_canonical_free_chain_partials_without_invoice_bundle() {
        let fixture = load_fixture();
        let descriptor = &fixture.artifacts.descriptor.artifact;
        let offer = &fixture.artifacts.free_offer.artifact;
        let quote = &fixture.artifacts.free_quote.artifact;
        let deal = &fixture.artifacts.free_deal.artifact;
        let receipt = &fixture.artifacts.free_receipt.artifact;

        let descriptor_offer = validate_descriptor_offer(descriptor, offer, None);
        assert!(
            descriptor_offer.valid,
            "unexpected issues: {:?}",
            descriptor_offer.issues
        );

        let offer_quote = validate_offer_quote(offer, quote, None);
        assert!(
            offer_quote.valid,
            "unexpected issues: {:?}",
            offer_quote.issues
        );

        let quote_deal = validate_quote_deal(quote, deal, None);
        assert!(
            quote_deal.valid,
            "unexpected issues: {:?}",
            quote_deal.issues
        );

        let quote_deal_receipt = validate_quote_deal_receipt(quote, deal, receipt, None);
        assert!(
            quote_deal_receipt.valid,
            "unexpected issues: {:?}",
            quote_deal_receipt.issues
        );
    }

    #[test]
    fn reports_deterministic_envelope_issues_before_link_issues() {
        let fixture = load_fixture();
        let offer = &fixture.artifacts.offer.artifact;
        let mut quote = fixture.artifacts.quote.artifact.clone();
        quote.artifact_type = ARTIFACT_TYPE_DEAL.to_string();
        quote.payload.offer_hash = "aa".repeat(32);

        let report = validate_offer_quote(offer, &quote, None);
        assert!(!report.valid);
        let codes = issue_codes(&report);
        assert_eq!(
            &codes[..3],
            &[
                ISSUE_ARTIFACT_TYPE_MISMATCH.to_string(),
                ISSUE_ARTIFACT_ENVELOPE_INVALID.to_string(),
                ISSUE_OFFER_HASH_MISMATCH.to_string()
            ]
        );
    }

    #[test]
    fn reports_partial_chain_hash_mismatch_without_requiring_full_path() {
        let fixture = load_fixture();
        let quote = &fixture.artifacts.quote.artifact;
        let free_deal = &fixture.artifacts.free_deal.artifact;

        let report = validate_quote_deal(quote, free_deal, None);
        assert!(!report.valid);
        assert!(
            issue_codes(&report).contains(&ISSUE_QUOTE_HASH_MISMATCH.to_string()),
            "expected quote_hash_mismatch, got {:?}",
            report.issues
        );
    }

    #[test]
    fn reports_invoice_bundle_link_failures_with_stable_codes() {
        let fixture = load_fixture();
        let quote = &fixture.artifacts.quote.artifact;
        let deal = &fixture.artifacts.deal.artifact;
        let bundle = &fixture.artifacts.free_receipt.artifact;

        let report = validate_quote_deal_receipt(quote, deal, bundle, None);
        assert!(!report.valid);
        assert!(
            issue_codes(&report).contains(&ISSUE_SETTLEMENT_METHOD_MISMATCH.to_string()),
            "expected settlement_method_mismatch, got {:?}",
            report.issues
        );
    }

    #[test]
    fn reports_expiry_when_now_is_provided() {
        let fixture = load_fixture();
        let offer = &fixture.artifacts.offer.artifact;
        let quote = &fixture.artifacts.quote.artifact;

        let report = validate_offer_quote(offer, quote, Some(quote.payload.expires_at + 1));
        assert!(!report.valid);
        assert!(
            issue_codes(&report).contains(&ISSUE_ARTIFACT_EXPIRED.to_string()),
            "expected artifact_expired, got {:?}",
            report.issues
        );
    }
}
