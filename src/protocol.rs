use crate::{canonical_json, crypto, jobs::JobSpec, pricing::ServiceId, wasm::WasmSubmission};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const FROGLET_SCHEMA_V1: &str = "froglet/v1";

pub const ARTIFACT_TYPE_DESCRIPTOR: &str = "descriptor";
pub const ARTIFACT_TYPE_OFFER: &str = "offer";
pub const ARTIFACT_TYPE_QUOTE: &str = "quote";
pub const ARTIFACT_TYPE_DEAL: &str = "deal";
pub const ARTIFACT_TYPE_RECEIPT: &str = "receipt";
pub const ARTIFACT_TYPE_CURATED_LIST: &str = "curated_list";
pub const TRANSPORT_TYPE_INVOICE_BUNDLE: &str = "invoice_bundle";

pub const ARTIFACT_KIND_DESCRIPTOR: &str = ARTIFACT_TYPE_DESCRIPTOR;
pub const ARTIFACT_KIND_OFFER: &str = ARTIFACT_TYPE_OFFER;
pub const ARTIFACT_KIND_QUOTE: &str = ARTIFACT_TYPE_QUOTE;
pub const ARTIFACT_KIND_DEAL: &str = ARTIFACT_TYPE_DEAL;
pub const ARTIFACT_KIND_RECEIPT: &str = ARTIFACT_TYPE_RECEIPT;
pub const ARTIFACT_KIND_CURATED_LIST: &str = ARTIFACT_TYPE_CURATED_LIST;
pub const TRANSPORT_KIND_INVOICE_BUNDLE: &str = TRANSPORT_TYPE_INVOICE_BUNDLE;

pub const LINKED_IDENTITY_KIND_NOSTR: &str = "nostr";
pub const LINKED_IDENTITY_SCOPE_PUBLICATION_NOSTR: &str = "publication.nostr";
pub const LINKED_IDENTITY_SIGNATURE_ALGORITHM_BIP340: &str = "secp256k1_schnorr_bip340";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedArtifact<T> {
    pub artifact_type: String,
    pub schema_version: String,
    pub signer: String,
    pub created_at: i64,
    pub payload_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hash: String,
    pub payload: T,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkedIdentity {
    pub identity_kind: String,
    pub identity: String,
    pub scope: Vec<String>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    pub signature_algorithm: String,
    pub linked_signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportEndpoint {
    pub transport: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    pub priority: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportEndpoints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clearnet_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onion_url: Option<String>,
    pub tor_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettlementDescriptor {
    pub methods: Vec<String>,
    pub reservations: bool,
    pub receipts: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedDescriptor {
    pub pull_api: bool,
    pub cursor_type: String,
    pub cursor_semantics: String,
    pub feed_path: String,
    pub artifact_path_template: String,
    pub max_page_size: usize,
    pub artifact_kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DescriptorCapabilities {
    pub service_kinds: Vec<String>,
    pub execution_runtimes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_deals: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DescriptorPayload {
    pub provider_id: String,
    #[serde(default)]
    pub descriptor_seq: u64,
    pub protocol_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_identities: Vec<LinkedIdentity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transport_endpoints: Vec<TransportEndpoint>,
    pub capabilities: DescriptorCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfferExecutionProfile {
    pub runtime: String,
    pub abi_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    pub max_input_bytes: usize,
    pub max_runtime_ms: u64,
    pub max_memory_bytes: usize,
    pub max_output_bytes: usize,
    pub fuel_limit: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfferPriceSchedule {
    pub base_fee_msat: u64,
    pub success_fee_msat: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfferPayload {
    pub provider_id: String,
    pub offer_id: String,
    pub descriptor_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    pub offer_kind: String,
    pub settlement_method: String,
    pub quote_ttl_secs: u64,
    pub execution_profile: OfferExecutionProfile,
    pub price_schedule: OfferPriceSchedule,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terms_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratedListEntry {
    pub provider_id: String,
    pub descriptor_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratedListPayload {
    pub schema_version: String,
    pub list_type: String,
    pub curator_id: String,
    pub list_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub entries: Vec<CuratedListEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuoteSettlementTerms {
    pub method: String,
    pub destination_identity: String,
    pub base_fee_msat: u64,
    pub success_fee_msat: u64,
    pub max_base_invoice_expiry_secs: u64,
    pub max_success_hold_expiry_secs: u64,
    pub min_final_cltv_expiry: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionLimits {
    pub max_input_bytes: usize,
    pub max_runtime_ms: u64,
    pub max_memory_bytes: usize,
    pub max_output_bytes: usize,
    pub fuel_limit: u64,
}

pub type ReceiptLimitsApplied = ExecutionLimits;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotePayload {
    pub provider_id: String,
    pub requester_id: String,
    pub descriptor_hash: String,
    pub offer_hash: String,
    pub expires_at: i64,
    pub workload_kind: String,
    pub workload_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities_granted: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extension_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_use: Option<String>,
    pub settlement_terms: QuoteSettlementTerms,
    pub execution_limits: ExecutionLimits,
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
    pub provider_id: String,
    pub requester_id: String,
    pub quote_hash: String,
    pub deal_hash: String,
    pub expires_at: i64,
    pub destination_identity: String,
    pub base_fee: InvoiceBundleLeg,
    pub success_fee: InvoiceBundleLeg,
    pub min_final_cltv_expiry: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DealPayload {
    pub requester_id: String,
    pub provider_id: String,
    pub quote_hash: String,
    pub workload_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extension_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_deal_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_nonce: Option<String>,
    pub success_payment_hash: String,
    pub admission_deadline: i64,
    pub completion_deadline: i64,
    pub acceptance_deadline: i64,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptLegState {
    Open,
    Accepted,
    Settled,
    Canceled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiptSettlementLeg {
    pub amount_msat: u64,
    pub invoice_hash: String,
    pub payment_hash: String,
    pub state: ReceiptLegState,
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
pub struct ReceiptSettlementRefs {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_hash: Option<String>,
    pub destination_identity: String,
    pub base_fee: ReceiptSettlementLeg,
    pub success_fee: ReceiptSettlementLeg,
}

pub type ReceiptSettlement = ReceiptSettlementRefs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptPayload {
    pub provider_id: String,
    pub requester_id: String,
    pub deal_hash: String,
    pub quote_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extension_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    pub finished_at: i64,
    pub deal_state: String,
    pub execution_state: String,
    pub settlement_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_format: Option<String>,
    pub executor: ReceiptExecutor,
    pub limits_applied: ExecutionLimits,
    pub settlement_refs: ReceiptSettlementRefs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkloadSpec {
    Wasm {
        submission: Box<WasmSubmission>,
    },
    OciWasm {
        submission: Box<crate::wasm::OciWasmSubmission>,
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
            WorkloadSpec::OciWasm { .. } => "oci_wasm",
            WorkloadSpec::EventsQuery { .. } => "events_query",
        }
    }

    pub fn workload_kind(&self) -> &'static str {
        match self {
            WorkloadSpec::Wasm { .. } => crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_V1,
            WorkloadSpec::OciWasm { .. } => crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_OCI_V1,
            WorkloadSpec::EventsQuery { .. } => "events.query",
        }
    }

    pub fn service_id(&self) -> ServiceId {
        match self {
            WorkloadSpec::Wasm { .. } => ServiceId::ExecuteWasm,
            WorkloadSpec::OciWasm { .. } => ServiceId::ExecuteWasm,
            WorkloadSpec::EventsQuery { .. } => ServiceId::EventsQuery,
        }
    }

    pub fn resource_kind(&self) -> &'static str {
        match self {
            WorkloadSpec::EventsQuery { .. } => "data",
            WorkloadSpec::Wasm { .. } => "compute",
            WorkloadSpec::OciWasm { .. } => "compute",
        }
    }

    pub fn runtime(&self) -> Option<&'static str> {
        match self {
            WorkloadSpec::Wasm { .. } => Some("wasm"),
            WorkloadSpec::OciWasm { .. } => Some("wasm"),
            WorkloadSpec::EventsQuery { .. } => None,
        }
    }

    pub fn abi_version(&self) -> Option<&str> {
        match self {
            WorkloadSpec::Wasm { submission } => Some(submission.workload.abi_version.as_str()),
            WorkloadSpec::OciWasm { submission } => Some(submission.workload.abi_version.as_str()),
            WorkloadSpec::EventsQuery { .. } => None,
        }
    }

    pub fn request_hash(&self) -> Result<String, String> {
        match self {
            WorkloadSpec::Wasm { submission } => submission.workload_hash(),
            WorkloadSpec::OciWasm { submission } => submission.workload_hash(),
            WorkloadSpec::EventsQuery { .. } => {
                let encoded = canonical_json::to_vec(self).map_err(|e| e.to_string())?;
                Ok(crypto::sha256_hex(encoded))
            }
        }
    }

    pub fn requested_capabilities(&self) -> &[String] {
        match self {
            WorkloadSpec::Wasm { submission } => &submission.workload.requested_capabilities,
            WorkloadSpec::OciWasm { submission } => &submission.workload.requested_capabilities,
            WorkloadSpec::EventsQuery { .. } => &[],
        }
    }
}

impl From<JobSpec> for WorkloadSpec {
    fn from(value: JobSpec) -> Self {
        match value {
            JobSpec::Wasm { submission } => WorkloadSpec::Wasm {
                submission: Box::new(submission),
            },
            JobSpec::OciWasm { submission } => WorkloadSpec::OciWasm {
                submission: Box::new(submission),
            },
        }
    }
}

pub fn sign_artifact<T: Serialize + Clone>(
    signer: &str,
    sign_message_hex: impl Fn(&[u8]) -> String,
    artifact_type: &str,
    created_at: i64,
    payload: T,
) -> Result<SignedArtifact<T>, String> {
    let payload_hash = payload_hash(&payload)?;
    let signing_bytes = canonical_signing_bytes(
        FROGLET_SCHEMA_V1,
        artifact_type,
        signer,
        created_at,
        &payload_hash,
        &payload,
    )?;
    let hash = crypto::sha256_hex(&signing_bytes);
    let signature = sign_message_hex(&signing_bytes);

    Ok(SignedArtifact {
        artifact_type: artifact_type.to_string(),
        schema_version: FROGLET_SCHEMA_V1.to_string(),
        signer: signer.to_string(),
        created_at,
        payload_hash,
        hash,
        payload,
        signature,
    })
}

pub fn linked_identity_scope_hash(scope: &[String]) -> Result<String, String> {
    let bytes = canonical_json::to_vec(&scope.to_vec()).map_err(|e| e.to_string())?;
    Ok(crypto::sha256_hex(bytes))
}

pub fn linked_identity_challenge_bytes(
    provider_id: &str,
    identity_kind: &str,
    identity: &str,
    scope: &[String],
    created_at: i64,
    expires_at: Option<i64>,
) -> Result<Vec<u8>, String> {
    let scope_hash = linked_identity_scope_hash(scope)?;
    Ok(format!(
        "froglet:identity_link:v1\n{provider_id}\n{identity_kind}\n{identity}\n{scope_hash}\n{created_at}\n{}",
        expires_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    )
    .into_bytes())
}

pub fn linked_identity_has_scope(identity: &LinkedIdentity, required_scope: &str) -> bool {
    identity.scope.iter().any(|scope| scope == required_scope)
}

pub fn verify_artifact<T: Serialize>(artifact: &SignedArtifact<T>) -> bool {
    if artifact.schema_version != FROGLET_SCHEMA_V1 {
        return false;
    }

    let payload_hash = match payload_hash(&artifact.payload) {
        Ok(hash) => hash,
        Err(_) => return false,
    };

    if payload_hash != artifact.payload_hash {
        return false;
    }

    let signing_bytes = match canonical_signing_bytes(
        &artifact.schema_version,
        &artifact.artifact_type,
        &artifact.signer,
        artifact.created_at,
        &artifact.payload_hash,
        &artifact.payload,
    ) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    let computed_hash = crypto::sha256_hex(&signing_bytes);
    if artifact.hash != computed_hash {
        return false;
    }

    crypto::verify_message(&artifact.signer, &artifact.signature, &signing_bytes)
}

pub fn artifact_hash<T: Serialize>(artifact: &SignedArtifact<T>) -> Result<String, String> {
    let payload_hash = payload_hash(&artifact.payload)?;
    canonical_signing_bytes(
        &artifact.schema_version,
        &artifact.artifact_type,
        &artifact.signer,
        artifact.created_at,
        &payload_hash,
        &artifact.payload,
    )
    .map(crypto::sha256_hex)
}

pub fn artifact_value<T: Serialize>(artifact: &SignedArtifact<T>) -> Result<Value, String> {
    serde_json::to_value(artifact).map_err(|e| e.to_string())
}

pub fn payload_hash<T: Serialize>(payload: &T) -> Result<String, String> {
    let bytes = canonical_json::to_vec(payload).map_err(|e| e.to_string())?;
    Ok(crypto::sha256_hex(bytes))
}

pub fn canonical_signing_bytes<T: Serialize>(
    schema_version: &str,
    artifact_type: &str,
    signer: &str,
    created_at: i64,
    payload_hash: &str,
    payload: &T,
) -> Result<Vec<u8>, String> {
    canonical_json::to_vec(&json!([
        schema_version,
        artifact_type,
        signer,
        created_at,
        payload_hash,
        payload
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
    use rand::{RngCore, SeedableRng, rngs::StdRng};
    use serde::Serialize;

    fn seeded_signing_key(rng: &mut StdRng) -> crypto::NodeSigningKey {
        loop {
            let mut seed = [0_u8; 32];
            rng.fill_bytes(&mut seed);
            if let Ok(key) = crypto::signing_key_from_seed_bytes(&seed) {
                return key;
            }
        }
    }

    fn random_hex(rng: &mut StdRng, bytes_len: usize) -> String {
        let mut bytes = vec![0_u8; bytes_len];
        rng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    fn flip_hex_char(value: &mut String, index: usize) {
        let replacement = match value.as_bytes()[index] {
            b'0' => "1",
            _ => "0",
        };
        value.replace_range(index..index + 1, replacement);
    }

    #[test]
    fn signed_artifact_roundtrip_verifies() {
        let signing_key = crypto::generate_signing_key();
        let signer = crypto::public_key_hex(&signing_key);
        let artifact = sign_artifact(
            &signer,
            |message| crypto::sign_message_hex(&signing_key, message),
            ARTIFACT_TYPE_QUOTE,
            123,
            QuotePayload {
                provider_id: signer.clone(),
                requester_id: "11".repeat(32),
                descriptor_hash: "22".repeat(32),
                offer_hash: "33".repeat(32),
                expires_at: 456,
                workload_kind: "compute.wasm.v1".to_string(),
                workload_hash: "44".repeat(32),
                capabilities_granted: Vec::new(),
                extension_refs: Vec::new(),
                quote_use: None,
                settlement_terms: QuoteSettlementTerms {
                    method: "lightning.base_fee_plus_success_fee.v1".to_string(),
                    destination_identity: "02".to_string() + &"55".repeat(32),
                    base_fee_msat: 1000,
                    success_fee_msat: 9000,
                    max_base_invoice_expiry_secs: 30,
                    max_success_hold_expiry_secs: 30,
                    min_final_cltv_expiry: 18,
                },
                execution_limits: ExecutionLimits {
                    max_input_bytes: 1,
                    max_runtime_ms: 2,
                    max_memory_bytes: 3,
                    max_output_bytes: 4,
                    fuel_limit: 5,
                },
            },
        )
        .unwrap();

        assert!(verify_artifact(&artifact));
        assert_eq!(artifact_hash(&artifact).unwrap(), artifact.hash);
    }

    #[test]
    fn signed_artifact_with_empty_hash_is_rejected() {
        let signing_key = crypto::generate_signing_key();
        let signer = crypto::public_key_hex(&signing_key);
        let mut artifact = sign_artifact(
            &signer,
            |message| crypto::sign_message_hex(&signing_key, message),
            ARTIFACT_TYPE_QUOTE,
            123,
            QuotePayload {
                provider_id: signer.clone(),
                requester_id: "11".repeat(32),
                descriptor_hash: "22".repeat(32),
                offer_hash: "33".repeat(32),
                expires_at: 456,
                workload_kind: "compute.wasm.v1".to_string(),
                workload_hash: "44".repeat(32),
                capabilities_granted: Vec::new(),
                extension_refs: Vec::new(),
                quote_use: None,
                settlement_terms: QuoteSettlementTerms {
                    method: "lightning.base_fee_plus_success_fee.v1".to_string(),
                    destination_identity: "02".to_string() + &"55".repeat(32),
                    base_fee_msat: 1000,
                    success_fee_msat: 9000,
                    max_base_invoice_expiry_secs: 30,
                    max_success_hold_expiry_secs: 30,
                    min_final_cltv_expiry: 18,
                },
                execution_limits: ExecutionLimits {
                    max_input_bytes: 1,
                    max_runtime_ms: 2,
                    max_memory_bytes: 3,
                    max_output_bytes: 4,
                    fuel_limit: 5,
                },
            },
        )
        .unwrap();
        artifact.hash.clear();

        assert!(!verify_artifact(&artifact));
    }

    #[test]
    fn payload_hash_is_stable_across_object_key_order() {
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
    fn linked_identity_challenge_is_stable() {
        let challenge = linked_identity_challenge_bytes(
            "provider-1",
            LINKED_IDENTITY_KIND_NOSTR,
            "abcd",
            &[LINKED_IDENTITY_SCOPE_PUBLICATION_NOSTR.to_string()],
            123,
            None,
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(challenge).unwrap(),
            "froglet:identity_link:v1\nprovider-1\nnostr\nabcd\n32195491daf0054358d4a777b5e185517820f15dd18d1170f4e8351d9de46d68\n123\n-"
        );
    }

    #[test]
    fn randomized_artifact_tampering_breaks_verification() {
        let mut rng = StdRng::seed_from_u64(0x0F06_A1E7);

        for iteration in 0..32_i64 {
            let signing_key = seeded_signing_key(&mut rng);
            let signer = crypto::public_key_hex(&signing_key);
            let artifact = sign_artifact(
                &signer,
                |message| crypto::sign_message_hex(&signing_key, message),
                ARTIFACT_TYPE_QUOTE,
                1_700_000_000 + iteration,
                QuotePayload {
                    provider_id: signer.clone(),
                    requester_id: random_hex(&mut rng, 32),
                    descriptor_hash: random_hex(&mut rng, 32),
                    offer_hash: random_hex(&mut rng, 32),
                    expires_at: 1_700_000_100 + iteration,
                    workload_kind: "compute.wasm.v1".to_string(),
                    workload_hash: random_hex(&mut rng, 32),
                    capabilities_granted: Vec::new(),
                    extension_refs: Vec::new(),
                    quote_use: None,
                    settlement_terms: QuoteSettlementTerms {
                        method: "lightning.base_fee_plus_success_fee.v1".to_string(),
                        destination_identity: format!("02{}", random_hex(&mut rng, 32)),
                        base_fee_msat: 1_000 + iteration as u64,
                        success_fee_msat: 9_000 + iteration as u64,
                        max_base_invoice_expiry_secs: 60,
                        max_success_hold_expiry_secs: 120,
                        min_final_cltv_expiry: 18,
                    },
                    execution_limits: ExecutionLimits {
                        max_input_bytes: 1_024 + iteration as usize,
                        max_runtime_ms: 2_000 + iteration as u64,
                        max_memory_bytes: 4_096 + iteration as usize,
                        max_output_bytes: 2_048 + iteration as usize,
                        fuel_limit: 50_000 + iteration as u64,
                    },
                },
            )
            .expect("artifact should sign");

            assert!(
                verify_artifact(&artifact),
                "iteration {iteration} should verify before tampering"
            );

            let mut tampered_schema = artifact.clone();
            tampered_schema.schema_version = "froglet/v2".to_string();
            assert!(
                !verify_artifact(&tampered_schema),
                "iteration {iteration} should fail on schema tampering"
            );

            let mut tampered_type = artifact.clone();
            tampered_type.artifact_type = ARTIFACT_TYPE_DEAL.to_string();
            assert!(
                !verify_artifact(&tampered_type),
                "iteration {iteration} should fail on artifact_type tampering"
            );

            let mut tampered_created_at = artifact.clone();
            tampered_created_at.created_at += 1;
            assert!(
                !verify_artifact(&tampered_created_at),
                "iteration {iteration} should fail on created_at tampering"
            );

            let mut tampered_payload = artifact.clone();
            tampered_payload.payload.workload_hash = random_hex(&mut rng, 32);
            assert!(
                !verify_artifact(&tampered_payload),
                "iteration {iteration} should fail on payload tampering"
            );

            let mut tampered_payload_hash = artifact.clone();
            flip_hex_char(&mut tampered_payload_hash.payload_hash, 0);
            assert!(
                !verify_artifact(&tampered_payload_hash),
                "iteration {iteration} should fail on payload_hash tampering"
            );

            let mut tampered_hash = artifact.clone();
            flip_hex_char(&mut tampered_hash.hash, 0);
            assert!(
                !verify_artifact(&tampered_hash),
                "iteration {iteration} should fail on artifact hash tampering"
            );

            let mut tampered_signer = artifact.clone();
            tampered_signer.signer = random_hex(&mut rng, 32);
            assert!(
                !verify_artifact(&tampered_signer),
                "iteration {iteration} should fail on signer tampering"
            );

            let mut tampered_signature = artifact.clone();
            flip_hex_char(&mut tampered_signature.signature, 0);
            assert!(
                !verify_artifact(&tampered_signature),
                "iteration {iteration} should fail on signature tampering"
            );
        }
    }
}
