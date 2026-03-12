use crate::{canonical_json, crypto, jobs::JobSpec, pricing::ServiceId, wasm::WasmSubmission};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const ARTIFACT_KIND_DESCRIPTOR: &str = "descriptor";
pub const ARTIFACT_KIND_OFFER: &str = "offer";
pub const ARTIFACT_KIND_QUOTE: &str = "quote";
pub const ARTIFACT_KIND_DEAL: &str = "deal";
pub const ARTIFACT_KIND_RECEIPT: &str = "receipt";
pub const TRANSPORT_KIND_INVOICE_BUNDLE: &str = "invoice_bundle";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedArtifact<T> {
    pub kind: String,
    pub actor_id: String,
    pub created_at: i64,
    pub payload_hash: String,
    pub hash: String,
    pub payload: T,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescriptorPayload {
    pub protocol_version: String,
    pub node_id: String,
    pub public_key: String,
    pub version: String,
    pub discovery_mode: String,
    pub transports: TransportEndpoints,
    pub settlement: SettlementDescriptor,
    pub feeds: FeedDescriptor,
    pub runtimes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportEndpoints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clearnet_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onion_url: Option<String>,
    pub tor_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementDescriptor {
    pub methods: Vec<String>,
    pub reservations: bool,
    pub receipts: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedDescriptor {
    pub pull_api: bool,
    pub cursor_type: String,
    pub cursor_semantics: String,
    pub feed_path: String,
    pub artifact_path_template: String,
    pub max_page_size: usize,
    pub artifact_kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OfferConstraints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_query_limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferPayload {
    pub offer_id: String,
    pub service_id: String,
    pub resource_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    pub price_sats: u64,
    pub payment_required: bool,
    pub payment_methods: Vec<String>,
    pub constraints: OfferConstraints,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotePayload {
    pub quote_id: String,
    pub offer_id: String,
    pub service_id: String,
    pub workload_kind: String,
    pub workload_hash: String,
    pub price_sats: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_terms: Option<QuoteSettlementTerms>,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuoteSettlementTerms {
    pub destination_identity: String,
    pub base_fee_msat: u64,
    pub success_fee_msat: u64,
    pub max_base_invoice_expiry_secs: u64,
    pub max_success_hold_expiry_secs: u64,
    pub min_final_cltv_expiry: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceBundleLegState {
    Open,
    Accepted,
    Settled,
    Canceled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvoiceBundleLeg {
    pub amount_msat: u64,
    pub invoice_bolt11: String,
    pub invoice_hash: String,
    pub payment_hash: String,
    pub state: InvoiceBundleLegState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvoiceBundlePayload {
    pub schema_version: String,
    pub bundle_type: String,
    pub provider_id: String,
    pub requester_id: String,
    pub quote_hash: String,
    pub deal_hash: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub destination_identity: String,
    pub base_invoice: InvoiceBundleLeg,
    pub success_hold_invoice: InvoiceBundleLeg,
    pub min_final_cltv_expiry: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentLock {
    pub kind: String,
    pub token_hash: String,
    pub amount_sats: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettlementStatus {
    Reserved,
    Committed,
    Released,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptSettlement {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SettlementStatus>,
    pub reserved_amount_sats: u64,
    pub committed_amount_sats: u64,
    pub payment_lock: PaymentLock,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptExecutor {
    pub runtime: String,
    pub runtime_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abi_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities_granted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptLimitsApplied {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_input_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_runtime_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DealPayload {
    pub deal_id: String,
    pub quote_id: String,
    pub offer_id: String,
    pub service_id: String,
    pub workload_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_lock: Option<PaymentLock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub deadline: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptPayload {
    pub receipt_id: String,
    pub deal_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deal_hash: Option<String>,
    pub quote_id: String,
    pub offer_id: String,
    pub service_id: String,
    pub workload_hash: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_paid_sats: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_lock: Option<PaymentLock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement: Option<ReceiptSettlement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor: Option<ReceiptExecutor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits_applied: Option<ReceiptLimitsApplied>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<ReceiptFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub completed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkloadSpec {
    Wasm {
        submission: WasmSubmission,
    },
    EventsQuery {
        kinds: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
}

impl WorkloadSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            WorkloadSpec::Wasm { .. } => "wasm",
            WorkloadSpec::EventsQuery { .. } => "events_query",
        }
    }

    pub fn workload_kind(&self) -> &'static str {
        match self {
            WorkloadSpec::Wasm { .. } => crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_V1,
            WorkloadSpec::EventsQuery { .. } => "events_query",
        }
    }

    pub fn service_id(&self) -> ServiceId {
        match self {
            WorkloadSpec::Wasm { .. } => ServiceId::ExecuteWasm,
            WorkloadSpec::EventsQuery { .. } => ServiceId::EventsQuery,
        }
    }

    pub fn resource_kind(&self) -> &'static str {
        match self {
            WorkloadSpec::EventsQuery { .. } => "data",
            WorkloadSpec::Wasm { .. } => "compute",
        }
    }

    pub fn runtime(&self) -> Option<&'static str> {
        match self {
            WorkloadSpec::Wasm { .. } => Some("wasm"),
            WorkloadSpec::EventsQuery { .. } => None,
        }
    }

    pub fn request_hash(&self) -> Result<String, String> {
        match self {
            WorkloadSpec::Wasm { submission } => submission.workload_hash(),
            WorkloadSpec::EventsQuery { .. } => {
                let encoded = canonical_json::to_vec(self).map_err(|e| e.to_string())?;
                Ok(crypto::sha256_hex(encoded))
            }
        }
    }
}

impl From<JobSpec> for WorkloadSpec {
    fn from(value: JobSpec) -> Self {
        match value {
            JobSpec::Wasm { submission } => WorkloadSpec::Wasm { submission },
        }
    }
}

pub fn sign_artifact<T: Serialize + Clone>(
    actor_id: &str,
    sign_message_hex: impl Fn(&[u8]) -> String,
    kind: &str,
    created_at: i64,
    payload: T,
) -> Result<SignedArtifact<T>, String> {
    let payload_hash = payload_hash(&payload)?;
    let signing_bytes =
        canonical_signing_bytes(kind, actor_id, created_at, &payload_hash, &payload)?;
    let hash = crypto::sha256_hex(&signing_bytes);
    let signature = sign_message_hex(&signing_bytes);

    Ok(SignedArtifact {
        kind: kind.to_string(),
        actor_id: actor_id.to_string(),
        created_at,
        payload_hash,
        hash,
        payload,
        signature,
    })
}

pub fn verify_artifact<T: Serialize>(artifact: &SignedArtifact<T>) -> bool {
    verify_artifact_with_current_encoding(artifact)
        || verify_artifact_with_legacy_encoding(artifact)
}

pub fn artifact_value<T: Serialize>(artifact: &SignedArtifact<T>) -> Result<Value, String> {
    serde_json::to_value(artifact).map_err(|e| e.to_string())
}

pub fn payload_hash<T: Serialize>(payload: &T) -> Result<String, String> {
    let bytes = canonical_json::to_vec(payload).map_err(|e| e.to_string())?;
    Ok(crypto::sha256_hex(bytes))
}

pub fn canonical_signing_bytes<T: Serialize>(
    kind: &str,
    actor_id: &str,
    created_at: i64,
    payload_hash: &str,
    payload: &T,
) -> Result<Vec<u8>, String> {
    canonical_json::to_vec(&json!([kind, actor_id, created_at, payload_hash, payload]))
        .map_err(|e| e.to_string())
}

fn verify_artifact_with_current_encoding<T: Serialize>(artifact: &SignedArtifact<T>) -> bool {
    let payload_hash = match payload_hash(&artifact.payload) {
        Ok(hash) => hash,
        Err(_) => return false,
    };

    if payload_hash != artifact.payload_hash {
        return false;
    }

    let signing_bytes = match canonical_signing_bytes(
        &artifact.kind,
        &artifact.actor_id,
        artifact.created_at,
        &artifact.payload_hash,
        &artifact.payload,
    ) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    if crypto::sha256_hex(&signing_bytes) != artifact.hash {
        return false;
    }

    crypto::verify_message(&artifact.actor_id, &artifact.signature, &signing_bytes)
}

fn verify_artifact_with_legacy_encoding<T: Serialize>(artifact: &SignedArtifact<T>) -> bool {
    let payload_hash = match legacy_payload_hash(&artifact.payload) {
        Ok(hash) => hash,
        Err(_) => return false,
    };

    if payload_hash != artifact.payload_hash {
        return false;
    }

    let signing_bytes = match legacy_canonical_signing_bytes(
        &artifact.kind,
        &artifact.actor_id,
        artifact.created_at,
        &artifact.payload_hash,
        &artifact.payload,
    ) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    if crypto::sha256_hex(&signing_bytes) != artifact.hash {
        return false;
    }

    crypto::verify_message(&artifact.actor_id, &artifact.signature, &signing_bytes)
}

fn legacy_payload_hash<T: Serialize>(payload: &T) -> Result<String, String> {
    let payload_value = serde_json::to_value(payload).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&payload_value).map_err(|e| e.to_string())?;
    Ok(crypto::sha256_hex(bytes))
}

fn legacy_canonical_signing_bytes<T: Serialize>(
    kind: &str,
    actor_id: &str,
    created_at: i64,
    payload_hash: &str,
    payload: &T,
) -> Result<Vec<u8>, String> {
    let payload_value = serde_json::to_value(payload).map_err(|e| e.to_string())?;
    serde_json::to_vec(&json!([
        kind,
        actor_id,
        created_at,
        payload_hash,
        payload_value
    ]))
    .map_err(|e| e.to_string())
}

pub fn new_artifact_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto;
    use serde::Serialize;

    #[test]
    fn signed_artifact_roundtrip_verifies() {
        let signing_key = crypto::generate_signing_key();
        let actor_id = crypto::public_key_hex(&signing_key);
        let artifact = sign_artifact(
            &actor_id,
            |message| crypto::sign_message_hex(&signing_key, message),
            ARTIFACT_KIND_QUOTE,
            123,
            QuotePayload {
                quote_id: "q1".to_string(),
                offer_id: "execute.wasm".to_string(),
                service_id: "execute.wasm".to_string(),
                workload_kind: "wasm".to_string(),
                workload_hash: "abc".to_string(),
                price_sats: 5,
                payment_method: Some("cashu".to_string()),
                settlement_terms: None,
                expires_at: 456,
            },
        )
        .unwrap();

        assert!(verify_artifact(&artifact));
    }

    #[test]
    fn workload_hash_is_stable_across_object_key_order() {
        #[derive(Serialize)]
        struct CanonicalPayload {
            config: Value,
        }

        let first = CanonicalPayload {
            config: json!({
                "b": 2,
                "a": 1
            }),
        };
        let second = CanonicalPayload {
            config: json!({
                "a": 1,
                "b": 2
            }),
        };

        assert_eq!(
            payload_hash(&first).unwrap(),
            payload_hash(&second).unwrap()
        );
    }

    #[test]
    fn legacy_signed_artifacts_still_verify() {
        let signing_key = crypto::generate_signing_key();
        let actor_id = crypto::public_key_hex(&signing_key);
        let payload = QuotePayload {
            quote_id: "q1".to_string(),
            offer_id: "execute.wasm".to_string(),
            service_id: "execute.wasm".to_string(),
            workload_kind: "wasm".to_string(),
            workload_hash: "abc".to_string(),
            price_sats: 5,
            payment_method: Some("cashu".to_string()),
            settlement_terms: None,
            expires_at: 456,
        };
        let payload_hash = legacy_payload_hash(&payload).unwrap();
        let signing_bytes = legacy_canonical_signing_bytes(
            ARTIFACT_KIND_QUOTE,
            &actor_id,
            123,
            &payload_hash,
            &payload,
        )
        .unwrap();
        let artifact = SignedArtifact {
            kind: ARTIFACT_KIND_QUOTE.to_string(),
            actor_id: actor_id.clone(),
            created_at: 123,
            payload_hash,
            hash: crypto::sha256_hex(&signing_bytes),
            payload,
            signature: crypto::sign_message_hex(&signing_key, &signing_bytes),
        };

        assert!(verify_artifact(&artifact));
    }
}
