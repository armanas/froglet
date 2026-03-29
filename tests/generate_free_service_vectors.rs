use froglet::execution::ExecutionRuntime;
/// Generates free-service conformance vectors and compares them against
/// `conformance/kernel_v1.json`.
///
/// Run the ignored test with:
/// `cargo test --test generate_free_service_vectors generate_free_service_conformance_vectors -- --ignored --nocapture`
///
/// The default test path is read-only and fails if the checked-in fixture no
/// longer matches the generated vectors.
use froglet::{
    canonical_json, crypto,
    protocol::{
        self, ARTIFACT_TYPE_DEAL, ARTIFACT_TYPE_OFFER, ARTIFACT_TYPE_QUOTE, ARTIFACT_TYPE_RECEIPT,
        DealPayload, ExecutionLimits, OfferExecutionProfile, OfferPayload, OfferPriceSchedule,
        QuotePayload, QuoteSettlementTerms, ReceiptExecutor, ReceiptLegState, ReceiptPayload,
        ReceiptSettlementLeg, ReceiptSettlementRefs, SignedArtifact,
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

const PROVIDER_SEED: [u8; 32] = [0x11; 32];
const REQUESTER_SEED: [u8; 32] = [0x22; 32];

fn provider_key() -> crypto::NodeSigningKey {
    crypto::signing_key_from_seed_bytes(&PROVIDER_SEED).unwrap()
}

fn requester_key() -> crypto::NodeSigningKey {
    crypto::signing_key_from_seed_bytes(&REQUESTER_SEED).unwrap()
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/kernel_v1.json")
}

fn artifact_vector_json<T: Serialize + Clone>(artifact: &SignedArtifact<T>) -> Value {
    let signing_bytes = protocol::canonical_signing_bytes(
        &artifact.schema_version,
        &artifact.artifact_type,
        &artifact.signer,
        artifact.created_at,
        &artifact.payload_hash,
        &artifact.payload,
    )
    .unwrap();

    json!({
        "canonical_signing_bytes_hex": hex::encode(&signing_bytes),
        "payload_hash": protocol::payload_hash(&artifact.payload).unwrap(),
        "artifact_hash": protocol::artifact_hash(artifact).unwrap(),
        "artifact": serde_json::to_value(artifact).unwrap(),
    })
}

fn verification_case_json<T: Serialize + Clone>(
    name: &str,
    artifact_type: &str,
    artifact: &SignedArtifact<T>,
    expected_valid: bool,
) -> Value {
    json!({
        "name": name,
        "artifact_type": artifact_type,
        "artifact": serde_json::to_value(artifact).unwrap(),
        "expected_valid": expected_valid,
    })
}

struct GeneratedFreeServiceVectors {
    offer: SignedArtifact<OfferPayload>,
    quote: SignedArtifact<QuotePayload>,
    deal: SignedArtifact<DealPayload>,
    receipt: SignedArtifact<ReceiptPayload>,
    tampered_offer: SignedArtifact<OfferPayload>,
    tampered_quote: SignedArtifact<QuotePayload>,
    tampered_deal: SignedArtifact<DealPayload>,
    tampered_receipt: SignedArtifact<ReceiptPayload>,
}

impl GeneratedFreeServiceVectors {
    fn artifact_cases(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("free_offer", artifact_vector_json(&self.offer)),
            ("free_quote", artifact_vector_json(&self.quote)),
            ("free_deal", artifact_vector_json(&self.deal)),
            ("free_receipt", artifact_vector_json(&self.receipt)),
        ]
    }

    fn verification_cases(&self) -> Vec<Value> {
        vec![
            verification_case_json("free_offer_valid", ARTIFACT_TYPE_OFFER, &self.offer, true),
            verification_case_json(
                "free_offer_tampered_descriptor_hash",
                ARTIFACT_TYPE_OFFER,
                &self.tampered_offer,
                false,
            ),
            verification_case_json("free_quote_valid", ARTIFACT_TYPE_QUOTE, &self.quote, true),
            verification_case_json(
                "free_quote_tampered_offer_hash",
                ARTIFACT_TYPE_QUOTE,
                &self.tampered_quote,
                false,
            ),
            verification_case_json("free_deal_valid", ARTIFACT_TYPE_DEAL, &self.deal, true),
            verification_case_json(
                "free_deal_tampered_acceptance_deadline",
                ARTIFACT_TYPE_DEAL,
                &self.tampered_deal,
                false,
            ),
            verification_case_json(
                "free_receipt_valid",
                ARTIFACT_TYPE_RECEIPT,
                &self.receipt,
                true,
            ),
            verification_case_json(
                "free_receipt_tampered_result_hash",
                ARTIFACT_TYPE_RECEIPT,
                &self.tampered_receipt,
                false,
            ),
        ]
    }
}

fn generated_conformance_path() -> Value {
    json!({
        "artifact_order": ["descriptor", "free_offer", "free_quote", "free_deal", "free_receipt"],
        "description": "Descriptor -> Free Offer -> Free Quote -> Free Deal -> Free Receipt canonical free-service kernel path (no InvoiceBundle)"
    })
}

fn build_generated_free_service_vectors() -> GeneratedFreeServiceVectors {
    let provider_signing_key = provider_key();
    let requester_signing_key = requester_key();
    let provider_id = crypto::public_key_hex(&provider_signing_key);
    let requester_id = crypto::public_key_hex(&requester_signing_key);

    let descriptor_hash =
        "dbc62553ea46192d7ff2eeb26b9aa14344ce1866f9b8c915e26a1c77dc5f23cd".to_string();
    let workload_hash =
        "1f43a59401d1e3474924c25ddf0ac617ecfb1f0d1dce3b64deb39634bb9906ea".to_string();
    let workload_kind = "compute.wasm.v1".to_string();

    let offer = protocol::sign_artifact(
        &provider_id,
        |msg| crypto::sign_message_hex(&provider_signing_key, msg),
        ARTIFACT_TYPE_OFFER,
        1700001001,
        OfferPayload {
            provider_id: provider_id.clone(),
            offer_id: "execute.wasm.free".to_string(),
            descriptor_hash: descriptor_hash.clone(),
            expires_at: None,
            offer_kind: workload_kind.clone(),
            settlement_method: "none".to_string(),
            quote_ttl_secs: 300,
            execution_profile: OfferExecutionProfile {
                runtime: ExecutionRuntime::Wasm,
                package_kind: String::new(),
                contract_version: String::new(),
                access_handles: Vec::new(),
                abi_version: "froglet.wasm.run_json.v1".to_string(),
                capabilities: Vec::new(),
                max_input_bytes: 131072,
                max_runtime_ms: 30000,
                max_memory_bytes: 8388608,
                max_output_bytes: 131072,
                fuel_limit: 50000000,
            },
            price_schedule: OfferPriceSchedule {
                base_fee_msat: 0,
                success_fee_msat: 0,
            },
            terms_hash: None,
            confidential_profile_hash: None,
        },
    )
    .unwrap();

    let quote = protocol::sign_artifact(
        &provider_id,
        |msg| crypto::sign_message_hex(&provider_signing_key, msg),
        ARTIFACT_TYPE_QUOTE,
        1700001002,
        QuotePayload {
            provider_id: provider_id.clone(),
            requester_id: requester_id.clone(),
            descriptor_hash: descriptor_hash.clone(),
            offer_hash: offer.hash.clone(),
            expires_at: 1700001600,
            workload_kind: workload_kind.clone(),
            workload_hash: workload_hash.clone(),
            confidential_session_hash: None,
            capabilities_granted: Vec::new(),
            extension_refs: Vec::new(),
            quote_use: None,
            settlement_terms: QuoteSettlementTerms {
                method: "none".to_string(),
                destination_identity: String::new(),
                base_fee_msat: 0,
                success_fee_msat: 0,
                max_base_invoice_expiry_secs: 0,
                max_success_hold_expiry_secs: 0,
                min_final_cltv_expiry: 0,
            },
            execution_limits: ExecutionLimits {
                max_input_bytes: 131072,
                max_runtime_ms: 30000,
                max_memory_bytes: 8388608,
                max_output_bytes: 131072,
                fuel_limit: 50000000,
            },
        },
    )
    .unwrap();

    let deal = protocol::sign_artifact(
        &requester_id,
        |msg| crypto::sign_message_hex(&requester_signing_key, msg),
        ARTIFACT_TYPE_DEAL,
        1700001003,
        DealPayload {
            requester_id: requester_id.clone(),
            provider_id: provider_id.clone(),
            quote_hash: quote.hash.clone(),
            workload_hash: workload_hash.clone(),
            confidential_session_hash: None,
            extension_refs: Vec::new(),
            authority_ref: None,
            supersedes_deal_hash: None,
            client_nonce: None,
            success_payment_hash:
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            admission_deadline: 1700001600,
            completion_deadline: 1700001630,
            acceptance_deadline: 1700001930,
        },
    )
    .unwrap();

    let result = json!({"answer": 42});
    let result_hash = canonical_json::to_vec(&result)
        .map(crypto::sha256_hex)
        .unwrap();
    let receipt = protocol::sign_artifact(
        &provider_id,
        |msg| crypto::sign_message_hex(&provider_signing_key, msg),
        ARTIFACT_TYPE_RECEIPT,
        1700001005,
        ReceiptPayload {
            provider_id: provider_id.clone(),
            requester_id,
            deal_hash: deal.hash.clone(),
            quote_hash: quote.hash.clone(),
            extension_refs: Vec::new(),
            acceptance_ref: None,
            started_at: Some(1700001003),
            finished_at: 1700001005,
            deal_state: "succeeded".to_string(),
            execution_state: "succeeded".to_string(),
            settlement_state: "none".to_string(),
            result_hash: Some(result_hash),
            confidential_session_hash: None,
            result_envelope_hash: None,
            result_format: Some("application/json+jcs".to_string()),
            executor: ReceiptExecutor {
                runtime: "wasm".to_string(),
                runtime_version: "conformance-example".to_string(),
                execution_mode: None,
                attestation_platform: None,
                measurement: None,
                abi_version: Some("froglet.wasm.run_json.v1".to_string()),
                module_hash: Some(
                    "30191e30b1685384f8594e10553ef52eec22e36c33611bb35ba1a6b625d61d3c".to_string(),
                ),
                capabilities_granted: Vec::new(),
            },
            limits_applied: ExecutionLimits {
                max_input_bytes: 131072,
                max_runtime_ms: 30000,
                max_memory_bytes: 8388608,
                max_output_bytes: 131072,
                fuel_limit: 50000000,
            },
            settlement_refs: ReceiptSettlementRefs {
                method: "none".to_string(),
                bundle_hash: None,
                destination_identity: String::new(),
                base_fee: ReceiptSettlementLeg {
                    amount_msat: 0,
                    invoice_hash: String::new(),
                    payment_hash: String::new(),
                    state: ReceiptLegState::Canceled,
                },
                success_fee: ReceiptSettlementLeg {
                    amount_msat: 0,
                    invoice_hash: String::new(),
                    payment_hash: String::new(),
                    state: ReceiptLegState::Canceled,
                },
            },
            failure_code: None,
            failure_message: None,
            result_ref: None,
        },
    )
    .unwrap();

    assert!(
        protocol::verify_artifact(&offer),
        "offer verification failed"
    );
    assert!(
        protocol::verify_artifact(&quote),
        "quote verification failed"
    );
    assert!(protocol::verify_artifact(&deal), "deal verification failed");
    assert!(
        protocol::verify_artifact(&receipt),
        "receipt verification failed"
    );

    let mut tampered_offer = offer.clone();
    tampered_offer.payload.descriptor_hash =
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();

    let mut tampered_quote = quote.clone();
    tampered_quote.payload.offer_hash =
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();

    let mut tampered_deal = deal.clone();
    tampered_deal.payload.acceptance_deadline += 1;

    let mut tampered_receipt = receipt.clone();
    tampered_receipt.payload.result_hash =
        Some("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string());

    assert!(
        !protocol::verify_artifact(&tampered_offer),
        "tampered offer should fail"
    );
    assert!(
        !protocol::verify_artifact(&tampered_quote),
        "tampered quote should fail"
    );
    assert!(
        !protocol::verify_artifact(&tampered_deal),
        "tampered deal should fail"
    );
    assert!(
        !protocol::verify_artifact(&tampered_receipt),
        "tampered receipt should fail"
    );

    GeneratedFreeServiceVectors {
        offer,
        quote,
        deal,
        receipt,
        tampered_offer,
        tampered_quote,
        tampered_deal,
        tampered_receipt,
    }
}

fn merge_generated_vectors_into_fixture(
    fixture: &mut Value,
    vectors: &GeneratedFreeServiceVectors,
) {
    let artifacts = fixture["artifacts"].as_object_mut().unwrap();
    artifacts.retain(|name, _| !name.starts_with("free_"));
    for (name, artifact) in vectors.artifact_cases() {
        artifacts.insert(name.to_string(), artifact);
    }

    let cases = fixture["artifact_verification_cases"]
        .as_array_mut()
        .unwrap();
    cases.retain(|case| {
        !case["name"]
            .as_str()
            .map(|name| name.starts_with("free_"))
            .unwrap_or(false)
    });
    cases.extend(vectors.verification_cases());

    fixture["free_service_conformance_path"] = generated_conformance_path();
}

#[test]
fn free_service_conformance_vectors_match_fixture() {
    let vectors = build_generated_free_service_vectors();
    let fixture: Value =
        serde_json::from_str(&fs::read_to_string(fixture_path()).expect("read fixture"))
            .expect("parse fixture");

    let actual_artifacts = fixture["artifacts"]
        .as_object()
        .expect("artifacts")
        .iter()
        .filter(|(name, _)| name.starts_with("free_"))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<serde_json::Map<String, Value>>();
    let expected_artifacts = vectors
        .artifact_cases()
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect::<serde_json::Map<String, Value>>();
    assert_eq!(
        actual_artifacts, expected_artifacts,
        "fixture free-service artifacts do not match generated values"
    );

    let actual_cases: Vec<Value> = fixture["artifact_verification_cases"]
        .as_array()
        .expect("artifact_verification_cases")
        .iter()
        .filter(|case| {
            case["name"]
                .as_str()
                .map(|name| name.starts_with("free_"))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    assert_eq!(
        actual_cases,
        vectors.verification_cases(),
        "fixture free-service verification cases do not match generated values"
    );
    assert_eq!(
        fixture["free_service_conformance_path"],
        generated_conformance_path()
    );
}

#[test]
#[ignore = "manual fixture update tool"]
fn generate_free_service_conformance_vectors() {
    let vectors = build_generated_free_service_vectors();
    let mut fixture: Value =
        serde_json::from_str(&fs::read_to_string(fixture_path()).expect("read fixture"))
            .expect("parse fixture");

    merge_generated_vectors_into_fixture(&mut fixture, &vectors);

    let fixture_path = fixture_path();
    let merged = serde_json::to_string_pretty(&fixture).unwrap();
    fs::write(&fixture_path, merged.as_bytes()).expect("write merged fixture");
    println!(
        "Wrote merged conformance vectors to {}",
        fixture_path.display()
    );
}
