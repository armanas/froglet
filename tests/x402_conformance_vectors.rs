/// Read-only conformance runner for `conformance/x402_v1.json`.
///
/// Mirrors `tests/kernel_conformance_vectors.rs` for the additive
/// `x402.eip3009.v1` settlement method: exact signing-bytes and hash
/// reproduction, envelope verification, per-kind semantics, the accept/reject
/// case table, and the evidence-bundle hash commitment.
use froglet::{
    canonical_json, crypto,
    protocol::{
        self, ARTIFACT_TYPE_RECEIPT, DealPayload, DescriptorPayload, OfferPayload, QuotePayload,
        ReceiptPayload, SETTLEMENT_METHOD_X402_EIP3009, SignedArtifact, verify_artifact,
        verify_typed_document,
    },
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{fs, path::PathBuf};

fn load_fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/x402_v1.json");
    let bytes =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&bytes).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn vector<T: DeserializeOwned>(fixture: &Value, name: &str) -> SignedArtifact<T> {
    serde_json::from_value(fixture["artifacts"][name]["artifact"].clone())
        .unwrap_or_else(|e| panic!("decode {name}: {e}"))
}

fn assert_exact_bytes<T: Serialize + Clone>(
    fixture: &Value,
    name: &str,
    artifact: &SignedArtifact<T>,
) {
    let recorded = &fixture["artifacts"][name];
    let signing_bytes = protocol::canonical_signing_bytes(
        &artifact.schema_version,
        &artifact.artifact_type,
        &artifact.signer,
        artifact.created_at,
        &artifact.payload_hash,
        &artifact.payload,
    )
    .unwrap();

    assert_eq!(
        hex::encode(&signing_bytes),
        recorded["canonical_signing_bytes_hex"].as_str().unwrap(),
        "{name}: canonical signing bytes must reproduce byte-for-byte"
    );
    assert_eq!(
        protocol::payload_hash(&artifact.payload).unwrap(),
        recorded["payload_hash"].as_str().unwrap(),
        "{name}: payload_hash must reproduce"
    );
    assert_eq!(
        protocol::artifact_hash(artifact).unwrap(),
        recorded["artifact_hash"].as_str().unwrap(),
        "{name}: artifact_hash must reproduce"
    );
    assert!(verify_artifact(artifact), "{name}: envelope must verify");
}

#[test]
fn x402_chain_artifacts_reproduce_exact_bytes_and_verify() {
    let fixture = load_fixture();
    assert_exact_bytes(
        &fixture,
        "descriptor",
        &vector::<DescriptorPayload>(&fixture, "descriptor"),
    );
    assert_exact_bytes(
        &fixture,
        "offer",
        &vector::<OfferPayload>(&fixture, "offer"),
    );
    assert_exact_bytes(
        &fixture,
        "quote",
        &vector::<QuotePayload>(&fixture, "quote"),
    );
    assert_exact_bytes(&fixture, "deal", &vector::<DealPayload>(&fixture, "deal"));
    assert_exact_bytes(
        &fixture,
        "receipt",
        &vector::<ReceiptPayload>(&fixture, "receipt"),
    );
}

#[test]
fn x402_chain_validates_end_to_end_without_an_invoice_bundle() {
    let fixture = load_fixture();
    let descriptor = vector::<DescriptorPayload>(&fixture, "descriptor");
    let offer = vector::<OfferPayload>(&fixture, "offer");
    let quote = vector::<QuotePayload>(&fixture, "quote");
    let deal = vector::<DealPayload>(&fixture, "deal");
    let receipt = vector::<ReceiptPayload>(&fixture, "receipt");

    let report = protocol::validate_full_chain(
        &protocol::FullChain {
            descriptor: &descriptor,
            offer: &offer,
            quote: &quote,
            invoice_bundle: None,
            deal: &deal,
            receipt: Some(&receipt),
        },
        None,
    );
    assert!(
        report.valid,
        "x402 chain must validate: {:?}",
        report.reports
    );
    assert_eq!(
        receipt.payload.settlement_refs.method,
        SETTLEMENT_METHOD_X402_EIP3009
    );
}

#[test]
fn x402_artifact_verification_cases_match_expectations() {
    let fixture = load_fixture();
    let cases = fixture["artifact_verification_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 6, "expected the full accept/reject table");

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let expected_valid = case["expected_valid"].as_bool().unwrap();
        let artifact_type = case["artifact_type"].as_str().unwrap();
        let observed = verify_typed_document(&case["artifact"], artifact_type);
        assert_eq!(
            observed.is_ok(),
            expected_valid,
            "{name}: expected valid={expected_valid}, got {observed:?}"
        );
    }
}

#[test]
fn evidence_bundle_hash_is_committed_by_the_receipt() {
    let fixture = load_fixture();
    let bundle = &fixture["evidence"]["bundle"];
    let recorded_hash = fixture["evidence"]["bundle_hash"].as_str().unwrap();

    let computed = crypto::sha256_hex(canonical_json::to_vec(bundle).unwrap());
    assert_eq!(
        computed, recorded_hash,
        "bundle_hash must equal sha256(JCS(bundle))"
    );

    let receipt = vector::<ReceiptPayload>(&fixture, "receipt");
    assert_eq!(
        receipt.payload.settlement_refs.bundle_hash.as_deref(),
        Some(recorded_hash),
        "the settled receipt must commit to the evidence bundle"
    );
    // The commitment is what makes the bundle tamper-evident: change any byte
    // of the authorization and the hash no longer matches the signed receipt.
    let mut tampered = bundle.clone();
    tampered["content"]["authorization"]["message"]["value"] = Value::String("999".to_string());
    assert_ne!(
        crypto::sha256_hex(canonical_json::to_vec(&tampered).unwrap()),
        recorded_hash
    );
}

#[test]
fn x402_receipt_kind_is_reachable_through_the_typed_api() {
    let fixture = load_fixture();
    let receipt = fixture["artifacts"]["receipt"]["artifact"].clone();
    let verified = verify_typed_document(&receipt, ARTIFACT_TYPE_RECEIPT)
        .expect("x402 receipts must verify through the typed API");
    assert!(matches!(verified, protocol::VerifiedArtifact::Receipt(_)));
}
