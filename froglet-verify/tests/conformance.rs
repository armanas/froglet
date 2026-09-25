//! The offline verifier must reproduce the frozen conformance vectors.
//!
//! These tests are the executable form of the froglet.dev claim that a third
//! party can verify a Froglet chain offline with no node and no network: they
//! run the shipped verifier API over `conformance/kernel_v1.json` exactly as
//! an auditor would.

use std::{fs, path::PathBuf};

use froglet_verify::{
    DocumentStatus, SemanticsOutcome, documents_form_chain, extract_documents,
    validate_chain_documents, verify_document,
};
use serde_json::Value;

fn conformance_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance")
}

fn load_fixture(name: &str) -> Value {
    let path = conformance_dir().join(name);
    let bytes = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&bytes).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn artifact(fixture: &Value, name: &str) -> Value {
    fixture["artifacts"][name]["artifact"].clone()
}

fn paid_chain(fixture: &Value) -> Vec<Value> {
    [
        "descriptor",
        "offer",
        "quote",
        "deal",
        "invoice_bundle",
        "receipt",
    ]
    .iter()
    .map(|name| artifact(fixture, name))
    .collect()
}

fn free_chain(fixture: &Value) -> Vec<Value> {
    [
        "descriptor",
        "free_offer",
        "free_quote",
        "free_deal",
        "free_receipt",
    ]
    .iter()
    .map(|name| artifact(fixture, name))
    .collect()
}

fn sign_provider_payload(
    fixture: &Value,
    artifact_type: &str,
    created_at: i64,
    payload: Value,
) -> Value {
    use froglet_protocol::{crypto, protocol::sign_artifact};

    let seed_hex = fixture["keys"]["provider_seed_hex"]
        .as_str()
        .expect("fixture carries the provider seed");
    let signing_key = crypto::signing_key_from_seed_bytes(&hex_to_array(seed_hex))
        .expect("fixture seed is a valid key");
    let signer = crypto::public_key_hex(&signing_key);
    let artifact = sign_artifact(
        &signer,
        |message| crypto::sign_message_hex(&signing_key, message),
        artifact_type,
        created_at,
        payload,
    )
    .expect("sign provider artifact");
    serde_json::to_value(artifact).expect("serialize provider artifact")
}

#[test]
fn every_kernel_v1_artifact_verifies_individually() {
    let fixture = load_fixture("kernel_v1.json");
    let artifacts = fixture["artifacts"]
        .as_object()
        .expect("artifacts is an object");

    for (name, vector) in artifacts {
        let document = vector["artifact"].clone();
        let report = verify_document(&document, None);
        assert!(
            report.envelope_valid,
            "{name}: envelope must verify — {:?}",
            report.detail
        );
        assert_eq!(
            report.status,
            DocumentStatus::Verified,
            "{name}: expected full verification, got {:?} ({:?})",
            report.status,
            report.semantics
        );
        assert!(
            matches!(report.semantics, SemanticsOutcome::Valid),
            "{name}: semantics must be evaluated and valid"
        );
        assert_eq!(
            report.hash,
            vector["artifact_hash"].as_str().unwrap(),
            "{name}: reported hash must match the recorded artifact_hash"
        );
    }
}

#[test]
fn kernel_v1_artifact_verification_cases_match_expectations() {
    let fixture = load_fixture("kernel_v1.json");
    let cases = fixture["artifact_verification_cases"]
        .as_array()
        .expect("artifact_verification_cases is an array");
    assert!(!cases.is_empty(), "fixture must carry verification cases");

    for case in cases {
        let name = case["name"].as_str().expect("case name");
        let expected_valid = case["expected_valid"].as_bool().expect("expected_valid");
        let report = verify_document(&case["artifact"], None);
        let observed_valid = report.status == DocumentStatus::Verified;
        assert_eq!(
            observed_valid, expected_valid,
            "{name}: expected valid={expected_valid}, got {:?} ({:?})",
            report.status, report.detail
        );
    }
}

#[test]
fn paid_and_free_chains_validate_end_to_end() {
    let fixture = load_fixture("kernel_v1.json");

    let paid = paid_chain(&fixture);
    assert!(
        documents_form_chain(&paid),
        "paid fixture must form a chain"
    );
    let paid_report = validate_chain_documents(&paid, None);
    assert!(
        paid_report.valid,
        "paid chain must validate: structure={:?} chain={:?}",
        paid_report.structure_errors, paid_report.chain
    );
    assert!(paid_report.chain_evaluated);

    let free = free_chain(&fixture);
    assert!(
        documents_form_chain(&free),
        "free fixture must form a chain"
    );
    let free_report = validate_chain_documents(&free, None);
    assert!(
        free_report.valid,
        "free chain must validate: structure={:?} chain={:?}",
        free_report.structure_errors, free_report.chain
    );
}

#[test]
fn lightning_chain_without_invoice_bundle_is_rejected() {
    let fixture = load_fixture("kernel_v1.json");
    let documents: Vec<Value> = paid_chain(&fixture)
        .into_iter()
        .filter(|document| document["artifact_type"] != "invoice_bundle")
        .collect();

    assert!(documents_form_chain(&documents));
    let report = validate_chain_documents(&documents, None);
    assert!(!report.valid);
    let codes: Vec<String> = report
        .chain
        .expect("chain evaluated")
        .reports
        .iter()
        .flat_map(|path| path.issues.iter().map(|issue| issue.code.clone()))
        .collect();
    assert!(
        codes
            .iter()
            .any(|code| code == "invoice_bundle_required_for_lightning_method"),
        "expected missing-bundle issue, got {codes:?}"
    );
}

#[test]
fn tampering_any_artifact_field_breaks_the_envelope() {
    let fixture = load_fixture("kernel_v1.json");
    let mut receipt = artifact(&fixture, "receipt");
    receipt["payload"]["deal_state"] = Value::String("succeeded-but-tampered".to_string());

    let report = verify_document(&receipt, None);
    assert!(
        !report.envelope_valid,
        "tampered payload must fail the envelope"
    );
    assert_eq!(report.status, DocumentStatus::Invalid);
}

#[test]
fn signed_invalid_descriptor_and_offer_semantics_are_rejected() {
    let fixture = load_fixture("kernel_v1.json");

    let descriptor = artifact(&fixture, "descriptor");
    let mut forged_link_payload = descriptor["payload"].clone();
    forged_link_payload["linked_identities"][0]["linked_signature"] =
        Value::String("00".repeat(64));
    let forged_link = sign_provider_payload(
        &fixture,
        "descriptor",
        descriptor["created_at"].as_i64().expect("created_at"),
        forged_link_payload,
    );
    let forged_link_report = verify_document(&forged_link, None);
    assert!(forged_link_report.envelope_valid);
    assert_eq!(forged_link_report.status, DocumentStatus::Invalid);
    assert!(matches!(
        forged_link_report.semantics,
        SemanticsOutcome::Invalid { ref error }
            if error == "descriptor linked Nostr identity signature is invalid"
    ));

    let mut wrong_version_payload = descriptor["payload"].clone();
    wrong_version_payload["protocol_version"] = Value::String("froglet/v999".to_string());
    let wrong_version = sign_provider_payload(
        &fixture,
        "descriptor",
        descriptor["created_at"].as_i64().expect("created_at"),
        wrong_version_payload,
    );
    assert_eq!(
        verify_document(&wrong_version, None).status,
        DocumentStatus::Invalid
    );

    let offer = artifact(&fixture, "offer");
    let mut unknown_method_payload = offer["payload"].clone();
    unknown_method_payload["settlement_method"] = Value::String("future.rail.v9".to_string());
    let unknown_method = sign_provider_payload(
        &fixture,
        "offer",
        offer["created_at"].as_i64().expect("created_at"),
        unknown_method_payload,
    );
    let unknown_method_report = verify_document(&unknown_method, None);
    assert!(unknown_method_report.envelope_valid);
    assert_eq!(unknown_method_report.status, DocumentStatus::Invalid);
}

#[test]
fn mixing_chains_reports_hash_mismatch() {
    let fixture = load_fixture("kernel_v1.json");
    let documents = vec![
        artifact(&fixture, "descriptor"),
        artifact(&fixture, "offer"),
        artifact(&fixture, "quote"),
        artifact(&fixture, "free_deal"),
    ];

    let report = validate_chain_documents(&documents, None);
    assert!(!report.valid, "a deal from another chain must not validate");
    let codes: Vec<String> = report
        .chain
        .expect("chain evaluated")
        .reports
        .iter()
        .flat_map(|path| path.issues.iter().map(|issue| issue.code.clone()))
        .collect();
    assert!(
        codes.iter().any(|code| code == "quote_hash_mismatch"),
        "expected quote_hash_mismatch, got {codes:?}"
    );
}

#[test]
fn genuinely_signed_unknown_payload_fields_report_envelope_only() {
    // A future kernel may add payload fields. An older verifier must report
    // the cryptographic truth — the envelope verifies over the exact signed
    // bytes — instead of a false signature failure caused by its own typed
    // round-trip dropping the unknown field. This is the envelope-first rule
    // in docs/SPEC.md, and it is why verify_document decodes into
    // SignedArtifact<Value> before it decodes into the typed payload.
    use froglet_protocol::crypto;
    use froglet_protocol::protocol::{ARTIFACT_TYPE_RECEIPT, sign_artifact};

    let fixture = load_fixture("kernel_v1.json");
    let seed_hex = fixture["keys"]["provider_seed_hex"]
        .as_str()
        .expect("fixture carries the provider seed");
    let seed_bytes: [u8; 32] = hex_to_array(seed_hex);
    let signing_key =
        crypto::signing_key_from_seed_bytes(&seed_bytes).expect("fixture seed is a valid key");
    let signer = crypto::public_key_hex(&signing_key);

    let mut payload = artifact(&fixture, "receipt")["payload"].clone();
    payload["unknown_future_field"] = Value::String("added by a later kernel".to_string());

    let future_receipt = sign_artifact(
        &signer,
        |message| crypto::sign_message_hex(&signing_key, message),
        ARTIFACT_TYPE_RECEIPT,
        1_700_000_000,
        payload,
    )
    .expect("sign the future-shaped receipt");
    let document = serde_json::to_value(future_receipt).expect("serialize");

    let report = verify_document(&document, None);
    assert!(
        report.envelope_valid,
        "envelope must verify over the exact signed bytes: {:?}",
        report.detail
    );
    assert_eq!(
        report.status,
        DocumentStatus::EnvelopeOnly,
        "unknown payload fields must downgrade to envelope_only, not Invalid: {:?}",
        report.semantics
    );
    assert!(
        matches!(report.semantics, SemanticsOutcome::NotEvaluated { .. }),
        "semantics must be reported as not evaluated, got {:?}",
        report.semantics
    );
}

fn hex_to_array(value: &str) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks(2).take(32).enumerate() {
        let pair = std::str::from_utf8(chunk).expect("ascii hex");
        bytes[index] = u8::from_str_radix(pair, 16).expect("valid hex byte");
    }
    bytes
}

#[test]
fn a_conformance_fixture_verifies_straight_from_disk_in_chain_order() {
    // README and docs/SPEC.md tell a reader to point the verifier at a fixture
    // file directly. That has to keep working: it is the whole five-minute
    // proof. Fixtures store `artifacts` as a name→vector map, so the extractor
    // must order them by conformance_path.artifact_order.
    for (name, expected_order) in [
        (
            "kernel_v1.json",
            vec![
                "descriptor",
                "offer",
                "quote",
                "deal",
                "invoice_bundle",
                "receipt",
            ],
        ),
        (
            "x402_v1.json",
            vec!["descriptor", "offer", "quote", "deal", "receipt"],
        ),
    ] {
        let fixture = load_fixture(name);
        let documents = extract_documents(&fixture)
            .unwrap_or_else(|error| panic!("{name}: extract failed: {error}"));

        let kinds: Vec<&str> = documents
            .iter()
            .filter_map(|document| document["artifact_type"].as_str())
            .collect();
        assert_eq!(kinds, expected_order, "{name}: chain order");

        let report = validate_chain_documents(&documents, None);
        assert!(
            report.valid,
            "{name}: fixture must verify straight from disk: structure={:?} chain={:?}",
            report.structure_errors, report.chain
        );
    }
}

#[test]
fn extract_documents_accepts_page_array_and_single_shapes() {
    let fixture = load_fixture("kernel_v1.json");
    let single = artifact(&fixture, "receipt");

    assert_eq!(extract_documents(&single).unwrap().len(), 1);

    let array = Value::Array(vec![single.clone(), artifact(&fixture, "deal")]);
    assert_eq!(extract_documents(&array).unwrap().len(), 2);

    let page = serde_json::json!({ "artifacts": [single.clone()] });
    assert_eq!(extract_documents(&page).unwrap().len(), 1);

    let wrapper = serde_json::json!({ "artifact": single });
    assert_eq!(extract_documents(&wrapper).unwrap().len(), 1);

    let bad = serde_json::json!({ "nothing": true });
    assert!(extract_documents(&bad).is_err());
}
