use super::*;

#[derive(Debug, Serialize)]
pub struct NodeCapabilities {
    pub api_version: String,
    pub version: String,
    pub identity: IdentityInfo,
    pub discovery: DiscoveryInfo,
    pub reference_discovery: ReferenceDiscoveryInfo,
    pub transports: TransportsInfo,
    pub execution: ExecutionInfo,
    pub limits: LimitsInfo,
    pub pricing: PricingInfo,
    pub payments: PaymentsInfo,
    pub faas: FaaSInfo,
}

#[derive(Debug, Serialize)]
pub struct IdentityInfo {
    pub node_id: String,
    pub public_key: String,
}

#[derive(Debug, Serialize)]
pub struct DiscoveryInfo {
    pub mode: String,
}

#[derive(Debug, Serialize)]
pub struct ReferenceDiscoveryInfo {
    pub enabled: bool,
    pub publish_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_register_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TransportsInfo {
    pub clearnet: ClearnetInfo,
    pub tor: TorInfo,
}

#[derive(Debug, Serialize)]
pub struct ClearnetInfo {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TorInfo {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onion_url: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ExecutionInfo {
    pub wasm: WasmInfo,
}

#[derive(Debug, Serialize)]
pub struct WasmInfo {
    pub enabled: bool,
    pub fuel_limit: u64,
    pub entrypoints: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LimitsInfo {
    pub events_query_limit_default: usize,
    pub events_query_limit_max: usize,
    pub body_limit_bytes: usize,
    pub wasm_hex_limit_bytes: usize,
    pub wasm_input_limit_bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct PaymentsInfo {
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifier_mode: Option<String>,
    pub accepted_payment_methods: Vec<String>,
    pub reservations: bool,
    pub receipts: bool,
}

#[derive(Debug, Serialize)]
pub struct FaaSInfo {
    pub jobs_api: bool,
    pub async_jobs: bool,
    pub idempotency_keys: bool,
    pub runtimes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteWasmRequest {
    pub submission: WasmSubmission,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeEventEnvelope {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: String,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

impl NodeEventEnvelope {
    pub fn canonical_signing_bytes(&self) -> Vec<u8> {
        canonical_json::to_vec(&json!([
            self.id,
            self.pubkey,
            self.created_at,
            self.kind,
            self.tags,
            self.content
        ]))
        .expect("node event signing bytes should serialize canonically")
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishRequest {
    pub event: NodeEventEnvelope,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryRequest {
    pub kinds: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateJobRequest {
    #[serde(flatten)]
    pub spec: JobSpec,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateQuoteRequest {
    pub offer_id: String,
    pub requester_id: String,
    #[serde(flatten)]
    pub spec: WorkloadSpec,
    #[serde(default)]
    pub max_price_sats: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateDealRequest {
    pub quote: SignedArtifact<QuotePayload>,
    pub deal: SignedArtifact<DealPayload>,
    #[serde(flatten)]
    pub spec: WorkloadSpec,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
}

#[derive(Debug, Serialize)]
pub struct ConfidentialSessionResponse {
    pub profile: SignedArtifact<ConfidentialProfilePayload>,
    pub session: SignedArtifact<ConfidentialSessionPayload>,
    pub attestation: AttestationBundle,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyReceiptRequest {
    pub receipt: SignedArtifact<ReceiptPayload>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyInvoiceBundleRequest {
    pub bundle: SignedArtifact<InvoiceBundlePayload>,
    pub quote: SignedArtifact<QuotePayload>,
    pub deal: SignedArtifact<DealPayload>,
    #[serde(default)]
    pub requester_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VerifyInvoiceBundleResponse {
    pub valid: bool,
    pub bundle_hash: String,
    pub quote_hash: String,
    pub deal_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_requester_id: Option<String>,
    pub issues: Vec<settlement::InvoiceBundleValidationIssue>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseDealPreimageRequest {
    pub success_preimage: String,
    #[serde(default)]
    pub expected_result_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MockPayDealRequest {
    pub success_preimage: String,
}

#[derive(Debug, Serialize)]
pub struct RuntimeWalletBalanceResponse {
    pub backend: String,
    pub mode: String,
    pub balance_known: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_sats: Option<u64>,
    pub accepted_payment_methods: Vec<String>,
    pub reservations: bool,
    pub receipts: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeProviderRef {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub provider_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeSearchRequest {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_inactive: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeProviderDetailsResponse {
    pub discovery: DiscoveryNodeRecord,
    pub descriptor: SignedArtifact<DescriptorPayload>,
    pub offers: Vec<SignedArtifact<OfferPayload>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeCreateDealRequest {
    pub provider: RuntimeProviderRef,
    pub offer_id: String,
    #[serde(flatten)]
    pub spec: WorkloadSpec,
    #[serde(default)]
    pub max_price_sats: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub payment: Option<ProvidedPayment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeCreateDealResponse {
    pub provider_id: String,
    pub provider_url: String,
    pub quote: SignedArtifact<QuotePayload>,
    pub deal: requester_deals::RequesterDealRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_intent_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_intent: Option<settlement::LightningWalletIntent>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeDealResponse {
    pub deal: requester_deals::RequesterDealRecord,
}

#[derive(Debug, Serialize)]
pub struct RuntimeAcceptDealResponse {
    pub deal: requester_deals::RequesterDealRecord,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeMockPayDealResponse {
    pub deal: requester_deals::RequesterDealRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_intent_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_intent: Option<settlement::LightningWalletIntent>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeAcceptDealRequest {
    #[serde(default)]
    pub expected_result_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyCuratedListRequest {
    pub curated_list: SignedArtifact<CuratedListPayload>,
}

#[derive(Debug, Serialize)]
pub struct VerifyCuratedListResponse {
    pub valid: bool,
    pub list_hash: String,
    pub curator_id: String,
    pub list_id: String,
    pub expires_at: i64,
}

#[derive(Debug, Serialize)]
pub struct RuntimeNostrProviderPublicationsResponse {
    pub descriptor_summary: nostr::NostrEvent,
    pub offer_summaries: Vec<nostr::NostrEvent>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeNostrReceiptPublicationResponse {
    pub receipt_summary: nostr::NostrEvent,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyNostrEventRequest {
    pub event: nostr::NostrEvent,
}

#[derive(Debug, Serialize)]
pub struct VerifyNostrEventResponse {
    pub valid: bool,
    pub event_id: String,
    pub pubkey: String,
    pub kind: u32,
}

#[derive(Debug, Serialize)]
pub struct RuntimeArchiveExportResponse {
    pub schema_version: String,
    pub export_type: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub exported_at: i64,
    pub artifact_documents: Vec<db::ArtifactDocumentRecord>,
    pub artifact_feed: Vec<db::ArtifactFeedEntryRecord>,
    pub execution_evidence: Vec<db::ExecutionEvidenceRecord>,
    pub lightning_invoice_bundles: Vec<db::LightningInvoiceBundleRecord>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeDealPaymentIntentResponse {
    pub payment_intent: settlement::LightningWalletIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderManagedOfferDefinition {
    pub offer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub offer_kind: String,
    pub runtime: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entrypoint_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub contract_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<ExecutionMount>,
    #[serde(default = "super::default_service_mode")]
    pub mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    pub max_input_bytes: usize,
    pub max_runtime_ms: u64,
    pub max_memory_bytes: usize,
    pub max_output_bytes: usize,
    pub fuel_limit: u64,
    pub price_sats: u64,
    #[serde(default = "super::default_offer_publication_state")]
    pub publication_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_bytes_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default)]
    pub source_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidential_profile_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderControlOfferRecord {
    pub publication_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub source_kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub runtime: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub package_kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub entrypoint_kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub entrypoint: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub contract_version: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<ExecutionMount>,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    pub offer: SignedArtifact<OfferPayload>,
}

#[derive(Debug, Deserialize)]
pub struct ProviderControlPublishArtifactRequest {
    pub service_id: String,
    #[serde(default)]
    pub offer_id: Option<String>,
    #[serde(default)]
    pub artifact_path: Option<String>,
    #[serde(default)]
    pub wasm_module_hex: Option<String>,
    #[serde(default)]
    pub oci_reference: Option<String>,
    #[serde(default)]
    pub oci_digest: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub package_kind: Option<String>,
    #[serde(default)]
    pub entrypoint_kind: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub contract_version: Option<String>,
    #[serde(default)]
    pub mounts: Option<Vec<ExecutionMount>>,
    #[serde(default)]
    pub inline_source: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    pub price_sats: u64,
    #[serde(default)]
    pub publication_state: Option<String>,
    #[serde(default)]
    pub input_schema: Option<Value>,
    #[serde(default)]
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderControlArtifactRef {
    pub kind: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderControlEvidence {
    pub provider_id: String,
    pub descriptor_hash: String,
    pub offer_hash: String,
    pub offer_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderControlMutationResponse {
    pub request_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    pub summary: String,
    pub artifacts: Vec<ProviderControlArtifactRef>,
    pub evidence: ProviderControlEvidence,
    pub offer: ProviderControlOfferRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderServiceRecord {
    pub service_id: String,
    pub offer_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub offer_kind: String,
    #[serde(default = "super::default_service_resource_kind")]
    pub resource_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runtime: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entrypoint_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub contract_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<ExecutionMount>,
    pub mode: String,
    pub price_sats: u64,
    pub publication_state: String,
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_bytes_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oci_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oci_digest: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderServicesResponse {
    pub services: Vec<ProviderServiceRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderServiceResponse {
    pub service: ProviderServiceRecord,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::WorkloadSpec;
    use serde_json::json;

    #[test]
    fn runtime_create_deal_request_roundtrip_preserves_wire_shape() {
        let request = RuntimeCreateDealRequest {
            provider: RuntimeProviderRef {
                provider_id: Some("11".repeat(32)),
                provider_url: Some("https://provider.example".to_string()),
            },
            offer_id: "service.echo".to_string(),
            spec: WorkloadSpec::EventsQuery {
                kinds: vec!["froglet.test".to_string()],
                limit: Some(25),
            },
            max_price_sats: Some(42),
            idempotency_key: Some("idem-1".to_string()),
            payment: None,
        };

        let value = serde_json::to_value(&request).expect("serialize runtime create deal request");
        assert_eq!(value["provider"]["provider_id"], json!("11".repeat(32)));
        assert_eq!(
            value["provider"]["provider_url"],
            json!("https://provider.example")
        );
        assert_eq!(value["offer_id"], json!("service.echo"));
        assert_eq!(value["kind"], json!("events_query"));
        assert_eq!(value["kinds"], json!(["froglet.test"]));
        assert_eq!(value["limit"], json!(25));
        assert_eq!(value["max_price_sats"], json!(42));
        assert_eq!(value["idempotency_key"], json!("idem-1"));

        let roundtrip: RuntimeCreateDealRequest =
            serde_json::from_value(value).expect("deserialize runtime create deal request");
        assert_eq!(
            roundtrip.provider.provider_id.as_deref(),
            Some("1111111111111111111111111111111111111111111111111111111111111111")
        );
        assert_eq!(
            roundtrip.provider.provider_url.as_deref(),
            Some("https://provider.example")
        );
        assert_eq!(roundtrip.offer_id, "service.echo");
        assert_eq!(roundtrip.max_price_sats, Some(42));
        assert_eq!(roundtrip.idempotency_key.as_deref(), Some("idem-1"));
        match roundtrip.spec {
            WorkloadSpec::EventsQuery { kinds, limit } => {
                assert_eq!(kinds, vec!["froglet.test".to_string()]);
                assert_eq!(limit, Some(25));
            }
            other => panic!("unexpected spec after roundtrip: {other:?}"),
        }
    }
}
