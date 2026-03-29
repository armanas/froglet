use froglet::{
    canonical_json, crypto, protocol,
    protocol::{
        DealPayload, DescriptorPayload, InvoiceBundlePayload, OfferPayload, QuotePayload,
        ReceiptPayload, SignedArtifact,
    },
    settlement,
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::PathBuf};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/kernel_v1.json")
}

fn load_fixture() -> KernelConformanceFixture {
    let bytes = fs::read_to_string(fixture_path()).expect("read conformance fixture");
    serde_json::from_str(&bytes).expect("parse conformance fixture")
}

#[derive(Debug, Deserialize)]
struct KernelConformanceFixture {
    schema_version: String,
    fixture_type: String,
    fixture_version: u32,
    keys: FixtureKeys,
    linked_identity: LinkedIdentityVector,
    workload_spec: protocol::WorkloadSpec,
    result: Value,
    artifacts: ArtifactVectors,
    artifact_verification_cases: Vec<ArtifactVerificationCase>,
    invoice_bundle_validation_cases: Vec<InvoiceBundleValidationCase>,
    conformance_path: ConformancePath,
    free_service_conformance_path: ConformancePath,
}

#[derive(Debug, Deserialize)]
struct FixtureKeys {
    provider_id: String,
    provider_destination_identity: String,
    requester_id: String,
    nostr_publication_id: String,
}

#[derive(Debug, Deserialize)]
struct LinkedIdentityVector {
    provider_id: String,
    identity_kind: String,
    identity: String,
    scope: Vec<String>,
    created_at: i64,
    expires_at: Option<i64>,
    scope_hash: String,
    challenge_utf8: String,
    challenge_hex: String,
    linked_signature: String,
}

#[derive(Debug, Deserialize)]
struct ArtifactVectors {
    descriptor: ArtifactVector<DescriptorPayload>,
    offer: ArtifactVector<OfferPayload>,
    quote: ArtifactVector<QuotePayload>,
    deal: ArtifactVector<DealPayload>,
    invoice_bundle: ArtifactVector<InvoiceBundlePayload>,
    receipt: ArtifactVector<ReceiptPayload>,
    // Free-service round-trip vectors
    free_offer: ArtifactVector<OfferPayload>,
    free_quote: ArtifactVector<QuotePayload>,
    free_deal: ArtifactVector<DealPayload>,
    free_receipt: ArtifactVector<ReceiptPayload>,
}

#[derive(Debug, Deserialize)]
struct ArtifactVector<T> {
    canonical_signing_bytes_hex: String,
    payload_hash: String,
    artifact_hash: String,
    artifact: SignedArtifact<T>,
}

#[derive(Debug, Deserialize)]
struct ArtifactVerificationCase {
    name: String,
    artifact_type: String,
    artifact: Value,
    expected_valid: bool,
}

#[derive(Debug, Deserialize)]
struct InvoiceBundleValidationCase {
    name: String,
    bundle: SignedArtifact<InvoiceBundlePayload>,
    expected_requester_id: Option<String>,
    expected_valid: bool,
    expected_issue_codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ConformancePath {
    artifact_order: Vec<String>,
    description: String,
}

fn assert_artifact_vector<T>(vector: &ArtifactVector<T>)
where
    T: Serialize,
{
    let signing_bytes = protocol::canonical_signing_bytes(
        &vector.artifact.schema_version,
        &vector.artifact.artifact_type,
        &vector.artifact.signer,
        vector.artifact.created_at,
        &vector.artifact.payload_hash,
        &vector.artifact.payload,
    )
    .expect("canonical signing bytes");
    assert_eq!(
        hex::encode(&signing_bytes),
        vector.canonical_signing_bytes_hex
    );
    assert_eq!(
        protocol::payload_hash(&vector.artifact.payload).expect("payload hash"),
        vector.payload_hash
    );
    assert_eq!(
        protocol::artifact_hash(&vector.artifact).expect("artifact hash"),
        vector.artifact_hash
    );
    assert!(protocol::verify_artifact(&vector.artifact));
}

fn verify_case(case: &ArtifactVerificationCase) -> bool {
    match case.artifact_type.as_str() {
        protocol::ARTIFACT_TYPE_DESCRIPTOR => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<DescriptorPayload>>(case.artifact.clone())
                .expect("descriptor artifact case"),
        ),
        protocol::ARTIFACT_TYPE_OFFER => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<OfferPayload>>(case.artifact.clone())
                .expect("offer artifact case"),
        ),
        protocol::ARTIFACT_TYPE_QUOTE => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<QuotePayload>>(case.artifact.clone())
                .expect("quote artifact case"),
        ),
        protocol::ARTIFACT_TYPE_DEAL => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<DealPayload>>(case.artifact.clone())
                .expect("deal artifact case"),
        ),
        protocol::TRANSPORT_TYPE_INVOICE_BUNDLE => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<InvoiceBundlePayload>>(case.artifact.clone())
                .expect("invoice bundle artifact case"),
        ),
        protocol::ARTIFACT_TYPE_RECEIPT => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<ReceiptPayload>>(case.artifact.clone())
                .expect("receipt artifact case"),
        ),
        other => panic!("unsupported artifact_type in fixture: {other}"),
    }
}

#[test]
fn kernel_conformance_artifact_vectors_match_exact_bytes() {
    let fixture = load_fixture();
    assert_eq!(fixture.schema_version, protocol::FROGLET_SCHEMA_V1);
    assert_eq!(fixture.fixture_type, "froglet_kernel_conformance_vectors");
    assert_eq!(fixture.fixture_version, 1);
    assert_eq!(
        fixture.conformance_path.artifact_order,
        vec![
            "descriptor",
            "offer",
            "quote",
            "deal",
            "invoice_bundle",
            "receipt"
        ]
    );
    assert_eq!(
        fixture.conformance_path.description,
        "Descriptor -> Offer -> Quote -> Deal -> InvoiceBundle -> Receipt canonical kernel path"
    );

    assert_artifact_vector(&fixture.artifacts.descriptor);
    assert_artifact_vector(&fixture.artifacts.offer);
    assert_artifact_vector(&fixture.artifacts.quote);
    assert_artifact_vector(&fixture.artifacts.deal);
    assert_artifact_vector(&fixture.artifacts.invoice_bundle);
    assert_artifact_vector(&fixture.artifacts.receipt);

    assert_eq!(
        fixture.artifacts.descriptor.artifact.payload.provider_id,
        fixture.keys.provider_id
    );
    assert_eq!(
        fixture.artifacts.quote.artifact.payload.requester_id,
        fixture.keys.requester_id
    );
    assert_eq!(
        fixture
            .artifacts
            .quote
            .artifact
            .payload
            .settlement_terms
            .destination_identity,
        fixture.keys.provider_destination_identity
    );
    assert_eq!(
        fixture.artifacts.offer.artifact.payload.descriptor_hash,
        fixture.artifacts.descriptor.artifact.hash
    );
    assert_eq!(
        fixture.artifacts.quote.artifact.payload.descriptor_hash,
        fixture.artifacts.descriptor.artifact.hash
    );
    assert_eq!(
        fixture.artifacts.quote.artifact.payload.offer_hash,
        fixture.artifacts.offer.artifact.hash
    );
    assert_eq!(
        fixture.artifacts.deal.artifact.payload.quote_hash,
        fixture.artifacts.quote.artifact.hash
    );
    assert_eq!(
        fixture.artifacts.invoice_bundle.artifact.payload.quote_hash,
        fixture.artifacts.quote.artifact.hash
    );
    assert_eq!(
        fixture.artifacts.invoice_bundle.artifact.payload.deal_hash,
        fixture.artifacts.deal.artifact.hash
    );
    assert_eq!(
        fixture.artifacts.receipt.artifact.payload.quote_hash,
        fixture.artifacts.quote.artifact.hash
    );
    assert_eq!(
        fixture.artifacts.receipt.artifact.payload.deal_hash,
        fixture.artifacts.deal.artifact.hash
    );
    assert_eq!(
        fixture
            .artifacts
            .receipt
            .artifact
            .payload
            .settlement_refs
            .bundle_hash
            .as_deref(),
        Some(fixture.artifacts.invoice_bundle.artifact.hash.as_str())
    );
    assert_eq!(
        fixture.workload_spec.request_hash().expect("workload hash"),
        fixture.artifacts.quote.artifact.payload.workload_hash
    );
    assert_eq!(
        canonical_json::to_vec(&fixture.result)
            .map(crypto::sha256_hex)
            .expect("result hash"),
        fixture
            .artifacts
            .receipt
            .artifact
            .payload
            .result_hash
            .clone()
            .expect("receipt result hash")
    );

    // --- Free-service round-trip vector assertions ---
    assert_artifact_vector(&fixture.artifacts.free_offer);
    assert_artifact_vector(&fixture.artifacts.free_quote);
    assert_artifact_vector(&fixture.artifacts.free_deal);
    assert_artifact_vector(&fixture.artifacts.free_receipt);

    // Free-service conformance path
    assert_eq!(
        fixture.free_service_conformance_path.artifact_order,
        vec![
            "descriptor",
            "free_offer",
            "free_quote",
            "free_deal",
            "free_receipt"
        ]
    );

    // Free-service offer uses settlement_method "none" with zero fees
    assert_eq!(
        fixture
            .artifacts
            .free_offer
            .artifact
            .payload
            .settlement_method,
        "none"
    );
    assert_eq!(
        fixture
            .artifacts
            .free_offer
            .artifact
            .payload
            .price_schedule
            .base_fee_msat,
        0
    );
    assert_eq!(
        fixture
            .artifacts
            .free_offer
            .artifact
            .payload
            .price_schedule
            .success_fee_msat,
        0
    );

    // Free-service offer references the same descriptor
    assert_eq!(
        fixture
            .artifacts
            .free_offer
            .artifact
            .payload
            .descriptor_hash,
        fixture.artifacts.descriptor.artifact.hash
    );

    // Free-service quote references the free offer
    assert_eq!(
        fixture.artifacts.free_quote.artifact.payload.offer_hash,
        fixture.artifacts.free_offer.artifact.hash
    );
    assert_eq!(
        fixture
            .artifacts
            .free_quote
            .artifact
            .payload
            .settlement_terms
            .method,
        "none"
    );
    assert_eq!(
        fixture
            .artifacts
            .free_quote
            .artifact
            .payload
            .settlement_terms
            .destination_identity,
        ""
    );
    assert_eq!(
        fixture
            .artifacts
            .free_quote
            .artifact
            .payload
            .settlement_terms
            .base_fee_msat,
        0
    );
    assert_eq!(
        fixture
            .artifacts
            .free_quote
            .artifact
            .payload
            .settlement_terms
            .success_fee_msat,
        0
    );

    // Free-service deal references the free quote
    assert_eq!(
        fixture.artifacts.free_deal.artifact.payload.quote_hash,
        fixture.artifacts.free_quote.artifact.hash
    );

    // Free-service receipt references the free deal and quote
    assert_eq!(
        fixture.artifacts.free_receipt.artifact.payload.deal_hash,
        fixture.artifacts.free_deal.artifact.hash
    );
    assert_eq!(
        fixture.artifacts.free_receipt.artifact.payload.quote_hash,
        fixture.artifacts.free_quote.artifact.hash
    );
    assert_eq!(
        fixture
            .artifacts
            .free_receipt
            .artifact
            .payload
            .settlement_state,
        "none"
    );
    assert_eq!(
        fixture
            .artifacts
            .free_receipt
            .artifact
            .payload
            .settlement_refs
            .method,
        "none"
    );
    assert!(
        fixture
            .artifacts
            .free_receipt
            .artifact
            .payload
            .settlement_refs
            .bundle_hash
            .is_none()
    );
    assert_eq!(
        fixture
            .artifacts
            .free_receipt
            .artifact
            .payload
            .settlement_refs
            .destination_identity,
        ""
    );
}

#[test]
fn kernel_conformance_linked_identity_challenge_is_stable() {
    let fixture = load_fixture();
    let scope_hash =
        protocol::linked_identity_scope_hash(&fixture.linked_identity.scope).expect("scope hash");
    assert_eq!(scope_hash, fixture.linked_identity.scope_hash);

    let challenge = protocol::linked_identity_challenge_bytes(
        &fixture.linked_identity.provider_id,
        &fixture.linked_identity.identity_kind,
        &fixture.linked_identity.identity,
        &fixture.linked_identity.scope,
        fixture.linked_identity.created_at,
        fixture.linked_identity.expires_at,
    )
    .expect("challenge bytes");
    assert_eq!(
        String::from_utf8(challenge.clone()).expect("utf8"),
        fixture.linked_identity.challenge_utf8
    );
    assert_eq!(
        hex::encode(challenge),
        fixture.linked_identity.challenge_hex
    );
    assert_eq!(
        fixture
            .artifacts
            .descriptor
            .artifact
            .payload
            .linked_identities[0]
            .linked_signature,
        fixture.linked_identity.linked_signature
    );
    assert_eq!(
        fixture
            .artifacts
            .descriptor
            .artifact
            .payload
            .linked_identities[0]
            .identity,
        fixture.keys.nostr_publication_id
    );
}

#[test]
fn kernel_conformance_artifact_verification_cases_match_expectations() {
    let fixture = load_fixture();
    for case in &fixture.artifact_verification_cases {
        assert_eq!(
            verify_case(case),
            case.expected_valid,
            "artifact verification case failed: {}",
            case.name
        );
    }
}

#[test]
fn kernel_conformance_invoice_bundle_validation_cases_match_expectations() {
    let fixture = load_fixture();
    let quote = &fixture.artifacts.quote.artifact;
    let deal = &fixture.artifacts.deal.artifact;
    for case in &fixture.invoice_bundle_validation_cases {
        let report = settlement::validate_lightning_invoice_bundle(
            &case.bundle,
            quote,
            deal,
            case.expected_requester_id.as_deref(),
        );
        let actual_codes = report
            .issues
            .iter()
            .map(|issue| issue.code.clone())
            .collect::<BTreeSet<_>>();
        let expected_codes = case
            .expected_issue_codes
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();

        assert_eq!(
            report.valid, case.expected_valid,
            "bundle validation validity mismatch: {}",
            case.name
        );
        assert_eq!(
            actual_codes, expected_codes,
            "bundle validation issue mismatch: {}",
            case.name
        );
        assert_eq!(report.quote_hash, quote.hash);
        assert_eq!(report.deal_hash, deal.hash);
    }
}
