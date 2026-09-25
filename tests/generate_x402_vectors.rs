/// Generates the `x402.eip3009.v1` conformance vectors and compares them
/// against `conformance/x402_v1.json`.
///
/// Regenerate with:
/// `cargo test --test generate_x402_vectors generate_x402_conformance_vectors -- --ignored --nocapture`
///
/// The default test path is read-only and fails if the checked-in fixture no
/// longer matches the generated vectors. `conformance/kernel_v1.json` is never
/// touched: x402 is an additive settlement method and the frozen kernel
/// vectors must keep verifying byte-for-byte.
use froglet::execution::ExecutionRuntime;
use froglet::{
    canonical_json, crypto,
    protocol::{
        self, ARTIFACT_TYPE_DEAL, ARTIFACT_TYPE_DESCRIPTOR, ARTIFACT_TYPE_OFFER,
        ARTIFACT_TYPE_QUOTE, ARTIFACT_TYPE_RECEIPT, DealPayload, DescriptorCapabilities,
        DescriptorPayload, ExecutionLimits, OfferExecutionProfile, OfferPayload,
        OfferPriceSchedule, PAYMENT_METHOD_X402_USDC, QuotePayload, QuoteSettlementTerms,
        ReceiptExecutor, ReceiptLegState, ReceiptPayload, ReceiptSettlementLeg,
        ReceiptSettlementRefs, SETTLEMENT_METHOD_X402_EIP3009, SignedArtifact, TransportEndpoint,
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

const PROVIDER_SEED: [u8; 32] = [0x11; 32];
const REQUESTER_SEED: [u8; 32] = [0x22; 32];

/// Deterministic non-secret test values. The payee address is the provider's
/// declared USDC destination; the nonce and tx hash are fixed so the fixture
/// is byte-reproducible.
const PAYEE_ADDRESS: &str = "a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
const PAYER_ADDRESS: &str = "70997970c51812dc3a010c7d01b50e0d17dc79c8";
const AUTHORIZATION_NONCE: &str =
    "3f1e5c0a7b92d48e6fa1c3b5d7092e4a8c6b1d3f5a7e9c2b4d6f8a0c2e4b6d80";
const SETTLE_TX_HASH: &str = "8d4f2a6c0e1b3d5f7a9c2e4b6d8f0a1c3e5b7d9f1a3c5e7b9d1f3a5c7e9b1d3f";
const BASE_FEE_MSAT: u64 = 250_000;
const CREATED_AT: i64 = 1_700_000_000;

fn provider_key() -> crypto::NodeSigningKey {
    crypto::signing_key_from_seed_bytes(&PROVIDER_SEED).unwrap()
}

fn requester_key() -> crypto::NodeSigningKey {
    crypto::signing_key_from_seed_bytes(&REQUESTER_SEED).unwrap()
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/x402_v1.json")
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

/// The content-addressed evidence bundle an x402 receipt commits to via
/// `settlement_refs.bundle_hash`. v1 vectors carry the authorization and
/// settlement facts; the payer's EIP-712 signature and its offline recovery
/// are specified by the evidence-refs chapter and covered by
/// `conformance/evidence_refs_v1.json`, not here.
fn evidence_bundle() -> Value {
    json!({
        "evidence_format": "froglet-evidence/v1",
        "kind": "x402.eip3009.v1",
        "content": {
            "x402_version": 1,
            "scheme": "exact",
            "network": "base-sepolia",
            "chain_id": 84532,
            "authorization": {
                "domain": {
                    "name": "USD Coin",
                    "version": "2",
                    "chainId": 84532,
                    "verifyingContract": "0x036cbd53842c5426634e7929541ec2318f3dcf7e"
                },
                "message": {
                    "from": format!("0x{PAYER_ADDRESS}"),
                    "to": format!("0x{PAYEE_ADDRESS}"),
                    "value": "250",
                    "validAfter": "0",
                    "validBefore": "1700003600",
                    "nonce": format!("0x{AUTHORIZATION_NONCE}")
                }
            },
            "settlement": {
                "tx_hash": format!("0x{SETTLE_TX_HASH}"),
                "facilitator": "https://x402.org/facilitator"
            }
        }
    })
}

fn evidence_bundle_hash(bundle: &Value) -> String {
    crypto::sha256_hex(canonical_json::to_vec(bundle).unwrap())
}

fn descriptor_payload(provider_id: &str) -> DescriptorPayload {
    DescriptorPayload {
        provider_id: provider_id.to_string(),
        protocol_version: "froglet/v1".to_string(),
        descriptor_seq: 1,
        capabilities: DescriptorCapabilities {
            service_kinds: vec!["compute.wasm.v1".to_string()],
            execution_runtimes: vec!["wasm".to_string()],
            max_concurrent_deals: None,
        },
        transport_endpoints: vec![TransportEndpoint {
            transport: "https".to_string(),
            uri: "https://provider.example".to_string(),
            created_at: Some(CREATED_AT),
            expires_at: None,
            priority: 10,
            features: vec![
                "quote_http".to_string(),
                "artifact_fetch".to_string(),
                "receipt_poll".to_string(),
            ],
        }],
        accepted_payment_methods: vec![PAYMENT_METHOD_X402_USDC.to_string()],
        expires_at: None,
        linked_identities: Vec::new(),
    }
}

fn settled_settlement_refs(bundle_hash: &str) -> ReceiptSettlementRefs {
    ReceiptSettlementRefs {
        method: SETTLEMENT_METHOD_X402_EIP3009.to_string(),
        bundle_hash: Some(bundle_hash.to_string()),
        destination_identity: PAYEE_ADDRESS.to_string(),
        base_fee: ReceiptSettlementLeg {
            amount_msat: BASE_FEE_MSAT,
            // invoice_hash carries the on-chain settle transaction hash.
            invoice_hash: SETTLE_TX_HASH.to_string(),
            // payment_hash carries the EIP-3009 authorization nonce.
            payment_hash: AUTHORIZATION_NONCE.to_string(),
            state: ReceiptLegState::Settled,
        },
        success_fee: ReceiptSettlementLeg {
            amount_msat: 0,
            invoice_hash: String::new(),
            payment_hash: String::new(),
            state: ReceiptLegState::Canceled,
        },
    }
}

struct GeneratedX402Vectors {
    descriptor: SignedArtifact<DescriptorPayload>,
    offer: SignedArtifact<OfferPayload>,
    quote: SignedArtifact<QuotePayload>,
    deal: SignedArtifact<DealPayload>,
    receipt: SignedArtifact<ReceiptPayload>,
    receipt_missing_bundle_hash: SignedArtifact<ReceiptPayload>,
    receipt_uppercase_nonce: SignedArtifact<ReceiptPayload>,
    receipt_succeeded_but_unsettled: SignedArtifact<ReceiptPayload>,
    receipt_canceled_with_tx_hash: SignedArtifact<ReceiptPayload>,
    receipt_prefixed_destination: SignedArtifact<ReceiptPayload>,
    bundle: Value,
    bundle_hash: String,
}

fn generate() -> GeneratedX402Vectors {
    let provider_key = provider_key();
    let requester_key = requester_key();
    let provider_id = crypto::public_key_hex(&provider_key);
    let requester_id = crypto::public_key_hex(&requester_key);
    let provider_sign = |message: &[u8]| crypto::sign_message_hex(&provider_key, message);
    let requester_sign = |message: &[u8]| crypto::sign_message_hex(&requester_key, message);

    let bundle = evidence_bundle();
    let bundle_hash = evidence_bundle_hash(&bundle);

    let descriptor = protocol::sign_artifact(
        &provider_id,
        provider_sign,
        ARTIFACT_TYPE_DESCRIPTOR,
        CREATED_AT,
        descriptor_payload(&provider_id),
    )
    .unwrap();

    let offer = protocol::sign_artifact(
        &provider_id,
        provider_sign,
        ARTIFACT_TYPE_OFFER,
        CREATED_AT,
        OfferPayload {
            provider_id: provider_id.clone(),
            descriptor_hash: descriptor.hash.clone(),
            offer_id: "execute.wasm".to_string(),
            offer_kind: "compute.wasm.v1".to_string(),
            settlement_method: SETTLEMENT_METHOD_X402_EIP3009.to_string(),
            quote_ttl_secs: 300,
            execution_profile: OfferExecutionProfile {
                runtime: ExecutionRuntime::Wasm,
                package_kind: String::new(),
                contract_version: String::new(),
                access_handles: Vec::new(),
                abi_version: "froglet.wasm.run_json.v1".to_string(),
                capabilities: Vec::new(),
                max_input_bytes: 131_072,
                max_runtime_ms: 30_000,
                max_memory_bytes: 8_388_608,
                max_output_bytes: 131_072,
                fuel_limit: 50_000_000,
            },
            price_schedule: OfferPriceSchedule {
                base_fee_msat: BASE_FEE_MSAT,
                success_fee_msat: 0,
            },
            expires_at: None,
            terms_hash: None,
            confidential_profile_hash: None,
        },
    )
    .unwrap();

    let execution_limits = ExecutionLimits {
        max_input_bytes: 131_072,
        max_runtime_ms: 30_000,
        max_memory_bytes: 8_388_608,
        max_output_bytes: 131_072,
        fuel_limit: 50_000_000,
    };

    let quote = protocol::sign_artifact(
        &provider_id,
        provider_sign,
        ARTIFACT_TYPE_QUOTE,
        CREATED_AT,
        QuotePayload {
            provider_id: provider_id.clone(),
            requester_id: requester_id.clone(),
            descriptor_hash: descriptor.hash.clone(),
            offer_hash: offer.hash.clone(),
            expires_at: CREATED_AT + 300,
            workload_kind: "compute.wasm.v1".to_string(),
            workload_hash: "11".repeat(32),
            confidential_session_hash: None,
            capabilities_granted: Vec::new(),
            extension_refs: Vec::new(),
            quote_use: None,
            settlement_terms: QuoteSettlementTerms {
                method: SETTLEMENT_METHOD_X402_EIP3009.to_string(),
                destination_identity: PAYEE_ADDRESS.to_string(),
                base_fee_msat: BASE_FEE_MSAT,
                success_fee_msat: 0,
                // Lightning-specific invoice/hold parameters are unused by an
                // atomic on-chain transfer.
                max_base_invoice_expiry_secs: 0,
                max_success_hold_expiry_secs: 0,
                min_final_cltv_expiry: 0,
            },
            execution_limits: execution_limits.clone(),
        },
    )
    .unwrap();

    let deal = protocol::sign_artifact(
        &requester_id,
        requester_sign,
        ARTIFACT_TYPE_DEAL,
        CREATED_AT,
        DealPayload {
            requester_id: requester_id.clone(),
            provider_id: provider_id.clone(),
            quote_hash: quote.hash.clone(),
            workload_hash: "11".repeat(32),
            confidential_session_hash: None,
            extension_refs: Vec::new(),
            authority_ref: None,
            supersedes_deal_hash: None,
            client_nonce: None,
            success_payment_hash: "22".repeat(32),
            admission_deadline: CREATED_AT + 60,
            completion_deadline: CREATED_AT + 120,
            acceptance_deadline: CREATED_AT + 180,
        },
    )
    .unwrap();

    let base_receipt = ReceiptPayload {
        provider_id: provider_id.clone(),
        requester_id: requester_id.clone(),
        deal_hash: deal.hash.clone(),
        quote_hash: quote.hash.clone(),
        extension_refs: Vec::new(),
        acceptance_ref: None,
        started_at: Some(CREATED_AT + 1),
        finished_at: CREATED_AT + 2,
        deal_state: "succeeded".to_string(),
        execution_state: "succeeded".to_string(),
        settlement_state: "settled".to_string(),
        result_hash: Some("33".repeat(32)),
        confidential_session_hash: None,
        result_envelope_hash: None,
        result_format: Some("application/json".to_string()),
        executor: ReceiptExecutor {
            runtime: "wasm".to_string(),
            runtime_version: "wasmtime-43".to_string(),
            execution_mode: None,
            attestation_platform: None,
            measurement: None,
            abi_version: Some("froglet.wasm.run_json.v1".to_string()),
            module_hash: Some("44".repeat(32)),
            capabilities_granted: Vec::new(),
        },
        limits_applied: execution_limits,
        settlement_refs: settled_settlement_refs(&bundle_hash),
        failure_code: None,
        failure_message: None,
        result_ref: None,
    };

    let sign_receipt = |payload: ReceiptPayload| {
        protocol::sign_artifact(
            &provider_id,
            provider_sign,
            ARTIFACT_TYPE_RECEIPT,
            CREATED_AT + 2,
            payload,
        )
        .unwrap()
    };

    let receipt = sign_receipt(base_receipt.clone());

    let receipt_missing_bundle_hash = sign_receipt(ReceiptPayload {
        settlement_refs: ReceiptSettlementRefs {
            bundle_hash: None,
            ..settled_settlement_refs(&bundle_hash)
        },
        ..base_receipt.clone()
    });

    let receipt_uppercase_nonce = sign_receipt(ReceiptPayload {
        settlement_refs: ReceiptSettlementRefs {
            base_fee: ReceiptSettlementLeg {
                payment_hash: AUTHORIZATION_NONCE.to_uppercase(),
                ..settled_settlement_refs(&bundle_hash).base_fee
            },
            ..settled_settlement_refs(&bundle_hash)
        },
        ..base_receipt.clone()
    });

    let receipt_succeeded_but_unsettled = sign_receipt(ReceiptPayload {
        settlement_state: "canceled".to_string(),
        settlement_refs: ReceiptSettlementRefs {
            bundle_hash: None,
            base_fee: ReceiptSettlementLeg {
                invoice_hash: String::new(),
                state: ReceiptLegState::Canceled,
                ..settled_settlement_refs(&bundle_hash).base_fee
            },
            ..settled_settlement_refs(&bundle_hash)
        },
        ..base_receipt.clone()
    });

    let receipt_canceled_with_tx_hash = sign_receipt(ReceiptPayload {
        deal_state: "canceled".to_string(),
        execution_state: "not_started".to_string(),
        settlement_state: "canceled".to_string(),
        result_hash: None,
        result_format: None,
        settlement_refs: ReceiptSettlementRefs {
            bundle_hash: None,
            base_fee: ReceiptSettlementLeg {
                state: ReceiptLegState::Canceled,
                ..settled_settlement_refs(&bundle_hash).base_fee
            },
            ..settled_settlement_refs(&bundle_hash)
        },
        ..base_receipt.clone()
    });

    let receipt_prefixed_destination = sign_receipt(ReceiptPayload {
        settlement_refs: ReceiptSettlementRefs {
            destination_identity: format!("0x{PAYEE_ADDRESS}"),
            ..settled_settlement_refs(&bundle_hash)
        },
        ..base_receipt.clone()
    });

    GeneratedX402Vectors {
        descriptor,
        offer,
        quote,
        deal,
        receipt,
        receipt_missing_bundle_hash,
        receipt_uppercase_nonce,
        receipt_succeeded_but_unsettled,
        receipt_canceled_with_tx_hash,
        receipt_prefixed_destination,
        bundle,
        bundle_hash,
    }
}

fn fixture_json(vectors: &GeneratedX402Vectors) -> Value {
    json!({
        "fixture_type": "froglet.x402-settlement-conformance",
        "fixture_version": 1,
        "schema_version": "froglet/v1",
        "settlement_method": SETTLEMENT_METHOD_X402_EIP3009,
        "keys": {
            "provider_seed_hex": hex::encode(PROVIDER_SEED),
            "requester_seed_hex": hex::encode(REQUESTER_SEED),
        },
        "evidence": {
            "bundle": vectors.bundle,
            "bundle_hash": vectors.bundle_hash,
            "note": "bundle_hash is sha256(JCS(bundle)) and is committed to by receipt.settlement_refs.bundle_hash. Offline recovery of the payer's EIP-712 signature is specified by the evidence-refs chapter and covered by conformance/evidence_refs_v1.json.",
        },
        "artifacts": {
            "descriptor": artifact_vector_json(&vectors.descriptor),
            "offer": artifact_vector_json(&vectors.offer),
            "quote": artifact_vector_json(&vectors.quote),
            "deal": artifact_vector_json(&vectors.deal),
            "receipt": artifact_vector_json(&vectors.receipt),
        },
        "conformance_path": {
            "artifact_order": ["descriptor", "offer", "quote", "deal", "receipt"],
            "description": "x402.eip3009.v1 is a single-leg prepaid-style method: there is no invoice_bundle transport artifact.",
        },
        "artifact_verification_cases": [
            verification_case_json("x402_receipt_valid", ARTIFACT_TYPE_RECEIPT, &vectors.receipt, true),
            verification_case_json("x402_receipt_missing_bundle_hash", ARTIFACT_TYPE_RECEIPT, &vectors.receipt_missing_bundle_hash, false),
            verification_case_json("x402_receipt_uppercase_nonce", ARTIFACT_TYPE_RECEIPT, &vectors.receipt_uppercase_nonce, false),
            verification_case_json("x402_receipt_succeeded_but_unsettled", ARTIFACT_TYPE_RECEIPT, &vectors.receipt_succeeded_but_unsettled, false),
            verification_case_json("x402_receipt_canceled_with_tx_hash", ARTIFACT_TYPE_RECEIPT, &vectors.receipt_canceled_with_tx_hash, false),
            verification_case_json("x402_receipt_prefixed_destination", ARTIFACT_TYPE_RECEIPT, &vectors.receipt_prefixed_destination, false),
        ],
    })
}

fn render(fixture: &Value) -> String {
    let mut rendered = serde_json::to_string_pretty(fixture).unwrap();
    rendered.push('\n');
    rendered
}

#[test]
#[ignore = "manual fixture update tool"]
fn generate_x402_conformance_vectors() {
    let vectors = generate();
    let rendered = render(&fixture_json(&vectors));
    fs::write(fixture_path(), &rendered).unwrap();
    println!("wrote {}", fixture_path().display());
}

#[test]
fn checked_in_x402_fixture_matches_generated_vectors() {
    let vectors = generate();
    let expected = render(&fixture_json(&vectors));
    let actual = fs::read_to_string(fixture_path()).expect(
        "conformance/x402_v1.json is missing; regenerate with \
         `cargo test --test generate_x402_vectors generate_x402_conformance_vectors -- --ignored`",
    );
    assert_eq!(
        actual, expected,
        "conformance/x402_v1.json is stale; regenerate it with the ignored generator test"
    );
}
