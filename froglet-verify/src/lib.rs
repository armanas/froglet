//! Offline verifier for Froglet signed-artifact chains.
//!
//! This crate is the distribution facade over the `froglet-protocol` kernel:
//! a JSON-document-level API for counterparties and auditors who hold artifact
//! JSON (from a feed page, a dispute bundle, or a conformance fixture) and
//! want a verdict without running a node. It performs no network calls, reads
//! no clock (expiry checks are opt-in via `now`), and builds for
//! `wasm32-unknown-unknown`.
//!
//! Verification algorithm (normative, see docs/SPEC.md):
//! the envelope is checked FIRST over `SignedArtifact<serde_json::Value>`, so
//! the cryptographic verdict is computed over the exact signed bytes even when
//! the payload carries fields this verifier version does not know. Only then
//! is the payload decoded into its typed form for semantic validation. A
//! document whose envelope verifies but whose payload carries unknown fields
//! reports `envelope_only`, never a false signature failure.

use serde::Serialize;
use serde_json::Value;

pub use froglet_protocol::protocol::{
    ARTIFACT_TYPE_DEAL, ARTIFACT_TYPE_DESCRIPTOR, ARTIFACT_TYPE_OFFER, ARTIFACT_TYPE_QUOTE,
    ARTIFACT_TYPE_RECEIPT, ChainValidationIssue, ChainValidationReport, DealPayload,
    DescriptorPayload, FullChain, FullChainReport, InvoiceBundlePayload, OfferPayload,
    QuotePayload, ReceiptPayload, SignedArtifact, TRANSPORT_TYPE_INVOICE_BUNDLE, VerifiedArtifact,
    VerifyError, validate_full_chain, verify_artifact, verify_typed_document,
};

#[cfg(target_arch = "wasm32")]
mod wasm;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    /// Envelope cryptography and kind-specific semantic rules both hold.
    Verified,
    /// Envelope cryptography holds; semantic rules were not evaluated (kind
    /// unknown to this verifier version, or payload carries unknown fields).
    EnvelopeOnly,
    /// Envelope or semantic validation failed.
    Invalid,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SemanticsOutcome {
    Valid,
    Invalid { error: String },
    NotEvaluated { reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentReport {
    pub artifact_type: String,
    pub signer: String,
    pub hash: String,
    pub status: DocumentStatus,
    pub envelope_valid: bool,
    pub semantics: SemanticsOutcome,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<String>,
    /// Informational only: set when `now` was supplied and the payload carries
    /// an integer `expires_at`. Chain validation is where expiry is normative.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expired: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChainDocumentsReport {
    pub valid: bool,
    pub chain_evaluated: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub structure_errors: Vec<String>,
    pub artifacts: Vec<DocumentReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<FullChainReport>,
}

/// The `artifact_type` claimed by a document, if it is an object carrying one.
pub fn classify_document(document: &Value) -> Option<&str> {
    document.get("artifact_type").and_then(Value::as_str)
}

fn string_field(document: &Value, field: &str) -> String {
    document
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Verify one artifact document: envelope first over the raw JSON value, then
/// kind-specific semantics via the typed kernel validators.
pub fn verify_document(document: &Value, now: Option<i64>) -> DocumentReport {
    let artifact_type = classify_document(document).unwrap_or("unknown").to_string();
    let signer = string_field(document, "signer");
    let hash = string_field(document, "hash");

    let envelope: SignedArtifact<Value> = match serde_json::from_value(document.clone()) {
        Ok(envelope) => envelope,
        Err(error) => {
            return DocumentReport {
                artifact_type,
                signer,
                hash,
                status: DocumentStatus::Invalid,
                envelope_valid: false,
                semantics: SemanticsOutcome::NotEvaluated {
                    reason: "document is not a signed artifact envelope".to_string(),
                },
                detail: vec![format!("envelope decode failed: {error}")],
                expired: None,
            };
        }
    };

    let envelope_valid = verify_artifact(&envelope);
    let mut detail = Vec::new();
    if !envelope_valid {
        detail.push(
            "envelope hash, payload hash, schema version, or signature is invalid".to_string(),
        );
    }

    let semantics = match verify_typed_document(document, &envelope.artifact_type) {
        Ok(_) => SemanticsOutcome::Valid,
        Err(VerifyError::UnknownKind(kind)) => SemanticsOutcome::NotEvaluated {
            reason: format!("no semantic validator for artifact kind {kind} in this verifier"),
        },
        Err(VerifyError::Decode(error)) => SemanticsOutcome::Invalid {
            error: format!(
                "payload does not conform to the {} schema: {error}",
                envelope.artifact_type
            ),
        },
        Err(VerifyError::Semantic(error)) => SemanticsOutcome::Invalid { error },
        Err(VerifyError::SignatureFailed) => {
            if envelope_valid {
                // The raw envelope verifies but the typed round-trip does not:
                // the payload carries fields unknown to this verifier version.
                SemanticsOutcome::NotEvaluated {
                    reason: "payload carries fields unknown to this verifier version; semantic rules were not evaluated"
                        .to_string(),
                }
            } else {
                SemanticsOutcome::Invalid {
                    error: "signature verification failed".to_string(),
                }
            }
        }
    };

    if let SemanticsOutcome::Invalid { error } = &semantics {
        detail.push(error.clone());
    }
    if let SemanticsOutcome::NotEvaluated { reason } = &semantics {
        detail.push(reason.clone());
    }

    let expired = now.and_then(|now| {
        envelope
            .payload
            .get("expires_at")
            .and_then(Value::as_i64)
            .map(|expires_at| expires_at < now)
    });

    let status = if !envelope_valid {
        DocumentStatus::Invalid
    } else {
        match &semantics {
            SemanticsOutcome::Valid => DocumentStatus::Verified,
            SemanticsOutcome::NotEvaluated { .. } => DocumentStatus::EnvelopeOnly,
            SemanticsOutcome::Invalid { .. } => DocumentStatus::Invalid,
        }
    };

    DocumentReport {
        artifact_type,
        signer,
        hash,
        status,
        envelope_valid,
        semantics,
        detail,
        expired,
    }
}

struct ChainSlots {
    descriptor: Option<SignedArtifact<DescriptorPayload>>,
    offer: Option<SignedArtifact<OfferPayload>>,
    quote: Option<SignedArtifact<QuotePayload>>,
    deal: Option<SignedArtifact<DealPayload>>,
    invoice_bundle: Option<SignedArtifact<InvoiceBundlePayload>>,
    receipt: Option<SignedArtifact<ReceiptPayload>>,
}

impl ChainSlots {
    fn new() -> Self {
        Self {
            descriptor: None,
            offer: None,
            quote: None,
            deal: None,
            invoice_bundle: None,
            receipt: None,
        }
    }

    fn core_kinds_present(&self) -> bool {
        self.descriptor.is_some()
            && self.offer.is_some()
            && self.quote.is_some()
            && self.deal.is_some()
    }
}

fn fill_slot<T: serde::de::DeserializeOwned>(
    slot: &mut Option<SignedArtifact<T>>,
    kind: &str,
    document: &Value,
    structure_errors: &mut Vec<String>,
) {
    if slot.is_some() {
        structure_errors.push(format!("more than one {kind} artifact supplied"));
        return;
    }
    match serde_json::from_value::<SignedArtifact<T>>(document.clone()) {
        Ok(artifact) => *slot = Some(artifact),
        Err(error) => {
            structure_errors.push(format!("{kind} artifact could not be decoded: {error}"))
        }
    }
}

/// Whether the supplied documents form exactly one candidate chain
/// (one descriptor, offer, quote, and deal; at most one invoice_bundle and
/// receipt). Used by callers that auto-detect chain inputs.
pub fn documents_form_chain(documents: &[Value]) -> bool {
    let mut counts = [0usize; 6];
    for document in documents {
        match classify_document(document) {
            Some(kind) if kind == ARTIFACT_TYPE_DESCRIPTOR => counts[0] += 1,
            Some(kind) if kind == ARTIFACT_TYPE_OFFER => counts[1] += 1,
            Some(kind) if kind == ARTIFACT_TYPE_QUOTE => counts[2] += 1,
            Some(kind) if kind == ARTIFACT_TYPE_DEAL => counts[3] += 1,
            Some(kind) if kind == TRANSPORT_TYPE_INVOICE_BUNDLE => counts[4] += 1,
            Some(kind) if kind == ARTIFACT_TYPE_RECEIPT => counts[5] += 1,
            _ => {}
        }
    }
    counts[0] == 1
        && counts[1] == 1
        && counts[2] == 1
        && counts[3] == 1
        && counts[4] <= 1
        && counts[5] <= 1
}

/// Verify every supplied document individually, then assemble and validate
/// the full chain they form. Documents of kinds outside the chain are still
/// individually verified but do not participate in chain validation.
pub fn validate_chain_documents(documents: &[Value], now: Option<i64>) -> ChainDocumentsReport {
    let mut structure_errors = Vec::new();
    let mut slots = ChainSlots::new();
    let mut artifacts = Vec::with_capacity(documents.len());

    for document in documents {
        artifacts.push(verify_document(document, now));
        match classify_document(document) {
            Some(kind) if kind == ARTIFACT_TYPE_DESCRIPTOR => {
                fill_slot(&mut slots.descriptor, kind, document, &mut structure_errors)
            }
            Some(kind) if kind == ARTIFACT_TYPE_OFFER => {
                fill_slot(&mut slots.offer, kind, document, &mut structure_errors)
            }
            Some(kind) if kind == ARTIFACT_TYPE_QUOTE => {
                fill_slot(&mut slots.quote, kind, document, &mut structure_errors)
            }
            Some(kind) if kind == ARTIFACT_TYPE_DEAL => {
                fill_slot(&mut slots.deal, kind, document, &mut structure_errors)
            }
            Some(kind) if kind == TRANSPORT_TYPE_INVOICE_BUNDLE => fill_slot(
                &mut slots.invoice_bundle,
                kind,
                document,
                &mut structure_errors,
            ),
            Some(kind) if kind == ARTIFACT_TYPE_RECEIPT => {
                fill_slot(&mut slots.receipt, kind, document, &mut structure_errors)
            }
            _ => {}
        }
    }

    if !slots.core_kinds_present() {
        structure_errors.push(
            "chain validation requires exactly one descriptor, offer, quote, and deal".to_string(),
        );
    }

    let chain = if structure_errors.is_empty() {
        let full_chain = FullChain {
            descriptor: slots.descriptor.as_ref().expect("core kinds present"),
            offer: slots.offer.as_ref().expect("core kinds present"),
            quote: slots.quote.as_ref().expect("core kinds present"),
            invoice_bundle: slots.invoice_bundle.as_ref(),
            deal: slots.deal.as_ref().expect("core kinds present"),
            receipt: slots.receipt.as_ref(),
        };
        Some(validate_full_chain(&full_chain, now))
    } else {
        None
    };

    let chain_evaluated = chain.is_some();
    let valid = structure_errors.is_empty()
        && artifacts
            .iter()
            .all(|report| report.status != DocumentStatus::Invalid)
        && chain.as_ref().map(|report| report.valid).unwrap_or(false);

    ChainDocumentsReport {
        valid,
        chain_evaluated,
        structure_errors,
        artifacts,
        chain,
    }
}

/// Unwrap the supported input shapes into a flat list of artifact documents:
/// a single artifact object, a `{"artifacts": [...]}` feed page, a
/// `{"artifact": {...}}` wrapper, an array of any of those, or a conformance
/// fixture whose `artifacts` is a name→vector map.
///
/// A conformance fixture is ordered by its `conformance_path.artifact_order`
/// when present, so the documents come back in chain order rather than the
/// map's arbitrary key order.
pub fn extract_documents(input: &Value) -> Result<Vec<Value>, String> {
    fn unwrap_item(item: &Value) -> Result<Value, String> {
        if item.get("artifact_type").is_some() {
            return Ok(item.clone());
        }
        if let Some(inner) = item.get("artifact")
            && inner.get("artifact_type").is_some()
        {
            return Ok(inner.clone());
        }
        Err("item is not an artifact document (no artifact_type)".to_string())
    }

    match input {
        Value::Array(items) => items.iter().map(unwrap_item).collect(),
        Value::Object(_) => match input.get("artifacts") {
            Some(Value::Array(items)) => items.iter().map(unwrap_item).collect(),
            Some(Value::Object(vectors)) => {
                // Conformance-fixture shape. Prefer the declared chain order;
                // fall back to map order for fixtures that declare none.
                let order: Vec<String> = input
                    .get("conformance_path")
                    .and_then(|path| path.get("artifact_order"))
                    .and_then(Value::as_array)
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(|name| name.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_else(|| vectors.keys().cloned().collect());

                order
                    .iter()
                    .filter_map(|name| vectors.get(name))
                    .map(unwrap_item)
                    .collect()
            }
            _ => Ok(vec![unwrap_item(input)?]),
        },
        _ => Err("input must be an artifact object, an array, or an artifacts page".to_string()),
    }
}
