use super::*;
use froglet_protocol::publication::{
    LocalVerificationEvidence, LockedPythonBundleEnvelope, PublicationBuildEvidence,
    PublicationSettlement, SignedPublicationRevision, VerificationFixture,
};

#[derive(Debug, Serialize)]
pub struct NodeCapabilities {
    pub api_version: String,
    pub version: String,
    pub identity: IdentityInfo,
    pub discovery: DiscoveryInfo,
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
pub struct TransportsInfo {
    pub clearnet: ClearnetInfo,
    pub tor: TorInfo,
    pub relay: RelayInfo,
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
pub struct RelayInfo {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ExecutionInfo {
    pub wasm: WasmInfo,
    pub gpu: GpuInfo,
}

#[derive(Debug, Serialize)]
pub struct WasmInfo {
    pub enabled: bool,
    pub fuel_limit: u64,
    pub entrypoints: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct GpuInfo {
    pub enabled: bool,
    pub count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_runtime: Option<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LimitsInfo {
    pub events_query_limit_default: usize,
    pub events_query_limit_max: usize,
    pub body_limit_bytes: usize,
    pub wasm_hex_limit_bytes: usize,
    pub wasm_input_limit_bytes: usize,
    pub process_concurrency_limit: usize,
    pub process_output_limit_bytes: usize,
    pub container_memory_limit_bytes: u64,
    pub container_pids_limit: u64,
    pub container_cpu_limit: f64,
    pub hosted_trial_deals_per_identity: u32,
    pub hosted_trial_quota_window_secs: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub fn canonical_signing_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        canonical_json::to_vec(&json!([
            self.id,
            self.pubkey,
            self.created_at,
            self.kind,
            self.tags,
            self.content
        ]))
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

/// Summary of a recent requester-side deal for the settlement-activity list.
/// Shape-stable across backends so the MCP surface has a single response
/// pattern regardless of which settlement driver is active.
#[derive(Debug, Serialize)]
pub struct SettlementActivityItem {
    pub deal_id: String,
    pub provider_id: String,
    pub status: String,
    pub workload_kind: String,
    pub settlement_method: String,
    pub base_fee_msat: u64,
    pub success_fee_msat: u64,
    pub has_receipt: bool,
    pub has_result: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct RuntimeSettlementActivityResponse {
    pub items: Vec<SettlementActivityItem>,
    pub limit: usize,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_fee_msat: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_fee_msat: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_method: Option<PublicationSettlement>,
    /// Currency unit for `price_sats`. `"sat"` (default) = satoshis, settled
    /// via Lightning. `"usd"` = US cents, settled via Stripe. Absent means
    /// `"sat"`. Lives in the node-side definition only; not part of the signed
    /// offer payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_currency: Option<String>,
    #[serde(default = "super::default_offer_publication_state")]
    pub publication_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_hash: Option<String>,
    /// Validated higher-layer build/dependency evidence. Never enters Kernel
    /// offer bytes; it is carried into the signed Publication Revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_evidence: Option<PublicationBuildEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_bytes_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_source: Option<String>,
    /// Provider-private canonical package. Persisted in the managed definition
    /// but never copied into public descriptor/offer/service documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_bundle: Option<LockedPythonBundleEnvelope>,
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
    /// Provider-private authoring fixture. Never copy this into public service
    /// records, descriptor/feed documents, or signed offer payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationFixture>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
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

pub use froglet_protocol::publication::PublicationIntent as ProviderControlPublishArtifactRequest;

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
    /// Normalized local lifecycle status. `legacy_unverified` means this
    /// local-only offer predates or is ineligible for immutable revisions.
    pub lifecycle_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    /// Present only when the provider executed the private verification
    /// fixture successfully before persisting and exposing this offer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_verification: Option<LocalVerificationEvidence>,
    /// Provider-signed higher-layer binding of offer, executable, currency,
    /// limits, and local verification. Not a Kernel artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publication_revision: Option<SignedPublicationRevision>,
    /// Opaque lifecycle-instance compare-and-swap token. Verified publishes
    /// always return it; legacy unverified compatibility mutations do not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation_token: Option<String>,
    /// Lifecycle record selected before this revision was committed. Public
    /// completion compensation uses it to restore a replaced route instead
    /// of merely pausing the newly failed revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_publication: Option<ProviderPublicationStatus>,
    #[serde(default)]
    pub previous_transport_grants: Vec<db::PublicationTransportGrantRecord>,
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
pub struct ProviderPublicationStatus {
    pub service_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_revision_hash: Option<String>,
    pub selected_revision_hash: String,
    pub activation_token: String,
    pub revision_count: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPublicationRevisionSummary {
    pub revision_hash: String,
    pub service_id: String,
    pub offer_id: String,
    pub offer_hash: String,
    pub binding_hash: String,
    pub validation_status: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPublicationRevisionDetail {
    #[serde(flatten)]
    pub summary: ProviderPublicationRevisionSummary,
    pub signed_revision: SignedPublicationRevision,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderPublicationsResponse {
    pub publications: Vec<ProviderPublicationStatus>,
    #[serde(default)]
    pub last_successful_calls: std::collections::BTreeMap<String, i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderPublicationResponse {
    pub publication: ProviderPublicationStatus,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderPublicationRevisionsResponse {
    pub revisions: Vec<ProviderPublicationRevisionSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderPublicationRevisionResponse {
    pub revision: ProviderPublicationRevisionDetail,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderPublicationOperationsResponse {
    pub operations: Vec<db::PublicationOperationRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderPublicationMutationResponse {
    pub operation: String,
    pub publication: ProviderPublicationStatus,
}

#[derive(Debug, Deserialize)]
pub struct ExactPublicationPauseRequest {
    pub activation_token: String,
    #[serde(default)]
    pub previous_publication: Option<ProviderPublicationStatus>,
    #[serde(default)]
    pub previous_transport_grants: Vec<db::PublicationTransportGrantRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayTransportActivationRequest {
    pub service_id: String,
    pub revision_hash: String,
    pub activation_token: String,
    pub public_url: String,
    pub relay_control_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayTransportActivationResponse {
    pub status: String,
    pub public_url: String,
    pub grant: db::PublicationTransportGrantRecord,
    pub remaining_grants: usize,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    pub mode: String,
    /// Legacy whole-unit total used by existing clients. Read together with
    /// `price_currency`; full fee legs remain authoritative.
    pub price_sats: u64,
    #[serde(default)]
    pub base_fee_msat: u64,
    #[serde(default)]
    pub success_fee_msat: u64,
    #[serde(default)]
    pub settlement_method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_currency: Option<String>,
    pub publication_state: String,
    pub provider_id: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_bytes_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_source: Option<String>,
    /// Internal execution material only. `serde(skip)` prevents even
    /// authenticated service responses from returning source/dependency bytes.
    #[serde(skip)]
    pub python_bundle: Option<LockedPythonBundleEnvelope>,
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
    /// Public, provider-signed evidence for the currently active revision.
    /// Private verification inputs and authoring metadata are never included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_revision: Option<SignedPublicationRevision>,
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
