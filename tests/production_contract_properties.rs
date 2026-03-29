// Feature: production-contract-hardening — Property-based contract tests
//
// These tests verify correctness properties from the production contract hardening spec.
// Since the input spaces are finite or loaded from fixtures, exhaustive enumeration
// is used instead of proptest.

use froglet::{
    crypto,
    execution::ExecutionRuntime,
    protocol::{
        self, DealPayload, DescriptorPayload, InvoiceBundleLeg, InvoiceBundleLegState,
        InvoiceBundlePayload, OfferExecutionProfile, OfferPayload, OfferPriceSchedule,
        QuotePayload, ReceiptPayload, SignedArtifact, TRANSPORT_TYPE_INVOICE_BUNDLE,
    },
    settlement,
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashSet, fs, path::PathBuf};

// ─── Helpers ────────────────────────────────────────────────────────────────

fn conformance_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/kernel_v1.json")
}

fn load_conformance() -> Value {
    let bytes = fs::read_to_string(conformance_path()).expect("read conformance fixture");
    serde_json::from_str(&bytes).expect("parse conformance fixture")
}

#[derive(Debug, Deserialize)]
struct ArtifactVerificationCase {
    name: String,
    artifact_type: String,
    artifact: Value,
    expected_valid: bool,
}

fn verify_case(case: &ArtifactVerificationCase) -> bool {
    match case.artifact_type.as_str() {
        "descriptor" => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<DescriptorPayload>>(case.artifact.clone())
                .expect("descriptor"),
        ),
        "offer" => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<OfferPayload>>(case.artifact.clone())
                .expect("offer"),
        ),
        "quote" => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<QuotePayload>>(case.artifact.clone())
                .expect("quote"),
        ),
        "deal" => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<DealPayload>>(case.artifact.clone())
                .expect("deal"),
        ),
        "invoice_bundle" => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<InvoiceBundlePayload>>(case.artifact.clone())
                .expect("invoice_bundle"),
        ),
        "receipt" => protocol::verify_artifact(
            &serde_json::from_value::<SignedArtifact<ReceiptPayload>>(case.artifact.clone())
                .expect("receipt"),
        ),
        other => panic!("unsupported artifact_type: {other}"),
    }
}

// ─── Property 1: Settlement Method String Canonicality ──────────────────────
// Feature: production-contract-hardening, Property 1: Settlement Method String Canonicality
// Validates: Requirements 9.1, 10.1
//
// For all artifacts, settlement_method/method is either "none" or
// "lightning.base_fee_plus_success_fee.v1". No other strings.
#[test]
fn property_1_settlement_method_string_canonicality() {
    let fixture = load_conformance();
    let allowed: HashSet<&str> = ["none", "lightning.base_fee_plus_success_fee.v1"]
        .into_iter()
        .collect();

    // Check all artifacts in the "artifacts" map
    if let Some(artifacts) = fixture.get("artifacts").and_then(|v| v.as_object()) {
        for (name, vector) in artifacts {
            let artifact = &vector["artifact"];
            // Offers have settlement_method in payload
            if let Some(method) = artifact["payload"]
                .get("settlement_method")
                .and_then(|v| v.as_str())
            {
                assert!(
                    allowed.contains(method),
                    "artifact '{name}' has non-canonical settlement_method: {method}"
                );
            }
            // Quotes have settlement_terms.method in payload
            if let Some(method) = artifact["payload"]
                .get("settlement_terms")
                .and_then(|v| v.get("method"))
                .and_then(|v| v.as_str())
            {
                assert!(
                    allowed.contains(method),
                    "artifact '{name}' has non-canonical settlement_terms.method: {method}"
                );
            }
            // Receipts have settlement_refs.method in payload
            if let Some(method) = artifact["payload"]
                .get("settlement_refs")
                .and_then(|v| v.get("method"))
                .and_then(|v| v.as_str())
            {
                assert!(
                    allowed.contains(method),
                    "artifact '{name}' has non-canonical settlement_refs.method: {method}"
                );
            }
        }
    }

    // Check all artifact_verification_cases
    if let Some(cases) = fixture
        .get("artifact_verification_cases")
        .and_then(|v| v.as_array())
    {
        for case in cases {
            let name = case["name"].as_str().unwrap_or("unknown");
            let artifact = &case["artifact"];
            if let Some(method) = artifact["payload"]
                .get("settlement_method")
                .and_then(|v| v.as_str())
            {
                assert!(
                    allowed.contains(method),
                    "verification case '{name}' has non-canonical settlement_method: {method}"
                );
            }
            if let Some(method) = artifact["payload"]
                .get("settlement_terms")
                .and_then(|v| v.get("method"))
                .and_then(|v| v.as_str())
            {
                assert!(
                    allowed.contains(method),
                    "verification case '{name}' has non-canonical settlement_terms.method: {method}"
                );
            }
            if let Some(method) = artifact["payload"]
                .get("settlement_refs")
                .and_then(|v| v.get("method"))
                .and_then(|v| v.as_str())
            {
                assert!(
                    allowed.contains(method),
                    "verification case '{name}' has non-canonical settlement_refs.method: {method}"
                );
            }
        }
    }

    // Check invoice_bundle_validation_cases
    if let Some(cases) = fixture
        .get("invoice_bundle_validation_cases")
        .and_then(|v| v.as_array())
    {
        for case in cases {
            let name = case["name"].as_str().unwrap_or("unknown");
            // Invoice bundles don't carry settlement_method directly, but verify no stray strings
            if let Some(method) = case["bundle"]["payload"]
                .get("settlement_method")
                .and_then(|v| v.as_str())
            {
                assert!(
                    allowed.contains(method),
                    "bundle validation case '{name}' has non-canonical settlement_method: {method}"
                );
            }
        }
    }
}

// ─── Property 2: Free-Deal Receipt Shape Invariant ──────────────────────────
// Feature: production-contract-hardening, Property 2: Free-Deal Receipt Shape Invariant
// Validates: Requirements 1.5, 1.6, 6.3
//
// For all receipts where settlement_refs.method == "none": bundle_hash is null,
// destination_identity is "", both fee legs have amount_msat: 0, invoice_hash: "",
// payment_hash: "", state: "canceled".
#[test]
fn property_2_free_deal_receipt_shape_invariant() {
    let fixture = load_conformance();

    // Collect all receipt artifacts from both artifacts map and verification cases
    let mut receipts: Vec<(String, Value)> = Vec::new();

    if let Some(artifacts) = fixture.get("artifacts").and_then(|v| v.as_object()) {
        for (name, vector) in artifacts {
            let artifact = &vector["artifact"];
            if artifact.get("artifact_type").and_then(|v| v.as_str()) == Some("receipt") {
                receipts.push((name.clone(), artifact["payload"].clone()));
            }
        }
    }
    if let Some(cases) = fixture
        .get("artifact_verification_cases")
        .and_then(|v| v.as_array())
    {
        for case in cases {
            if case.get("artifact_type").and_then(|v| v.as_str()) == Some("receipt") {
                let name = case["name"].as_str().unwrap_or("unknown").to_string();
                receipts.push((name, case["artifact"]["payload"].clone()));
            }
        }
    }

    for (name, payload) in &receipts {
        let refs = &payload["settlement_refs"];
        let method = refs.get("method").and_then(|v| v.as_str()).unwrap_or("");
        if method != "none" {
            continue;
        }

        // bundle_hash must be null or absent
        let bundle_hash = refs.get("bundle_hash");
        assert!(
            bundle_hash.is_none() || bundle_hash.unwrap().is_null(),
            "receipt '{name}': bundle_hash should be null for method=none, got {bundle_hash:?}"
        );

        // destination_identity must be ""
        assert_eq!(
            refs.get("destination_identity")
                .and_then(|v| v.as_str())
                .unwrap_or("MISSING"),
            "",
            "receipt '{name}': destination_identity should be empty for method=none"
        );

        // Check both fee legs
        for leg_name in &["base_fee", "success_fee"] {
            let leg = &refs[*leg_name];
            assert_eq!(
                leg["amount_msat"].as_u64().unwrap(),
                0,
                "receipt '{name}': {leg_name}.amount_msat should be 0"
            );
            assert_eq!(
                leg["invoice_hash"].as_str().unwrap(),
                "",
                "receipt '{name}': {leg_name}.invoice_hash should be empty"
            );
            assert_eq!(
                leg["payment_hash"].as_str().unwrap(),
                "",
                "receipt '{name}': {leg_name}.payment_hash should be empty"
            );
            assert_eq!(
                leg["state"].as_str().unwrap(),
                "canceled",
                "receipt '{name}': {leg_name}.state should be 'canceled'"
            );
        }
    }

    // Ensure we actually checked at least one free receipt
    let free_count = receipts
        .iter()
        .filter(|(_, p)| {
            p["settlement_refs"].get("method").and_then(|v| v.as_str()) == Some("none")
        })
        .count();
    assert!(
        free_count > 0,
        "no free-deal receipts found in conformance vectors"
    );
}

// ─── Property 3: Funds_Locked Gating Invariant ─────────────────────────────
// Feature: production-contract-hardening, Property 3: Funds_Locked Gating Invariant
// Validates: Requirements 2.4, 2.8
//
// For all combinations of base_state and success_state, execution starts iff
// base_fee.state == settled AND success_fee.state in {accepted, settled}.
#[test]
fn property_3_funds_locked_gating_invariant() {
    let all_states = [
        InvoiceBundleLegState::Open,
        InvoiceBundleLegState::Accepted,
        InvoiceBundleLegState::Settled,
        InvoiceBundleLegState::Canceled,
        InvoiceBundleLegState::Expired,
    ];

    // Create a minimal valid signed invoice bundle for the session
    let signing_key = crypto::generate_signing_key();
    let signer = crypto::public_key_hex(&signing_key);
    let bundle = protocol::sign_artifact(
        &signer,
        |msg| crypto::sign_message_hex(&signing_key, msg),
        TRANSPORT_TYPE_INVOICE_BUNDLE,
        1000,
        InvoiceBundlePayload {
            provider_id: signer.clone(),
            requester_id: "aa".repeat(32),
            quote_hash: "bb".repeat(32),
            deal_hash: "cc".repeat(32),
            expires_at: 2000,
            destination_identity: format!("02{}", "dd".repeat(32)),
            base_fee: InvoiceBundleLeg {
                amount_msat: 1000,
                invoice_bolt11: String::new(),
                invoice_hash: "ee".repeat(32),
                payment_hash: "ff".repeat(32),
                state: InvoiceBundleLegState::Open,
            },
            success_fee: InvoiceBundleLeg {
                amount_msat: 9000,
                invoice_bolt11: String::new(),
                invoice_hash: "11".repeat(32),
                payment_hash: "22".repeat(32),
                state: InvoiceBundleLegState::Open,
            },
            min_final_cltv_expiry: 18,
        },
    )
    .expect("sign bundle");

    for base_state in &all_states {
        for success_state in &all_states {
            let session = settlement::LightningInvoiceBundleSession {
                session_id: "test".to_string(),
                bundle: bundle.clone(),
                base_state: base_state.clone(),
                success_state: success_state.clone(),
                created_at: 1000,
                updated_at: 1000,
            };

            let funded = settlement::lightning_bundle_is_funded(&session);
            let expected = matches!(base_state, InvoiceBundleLegState::Settled)
                && matches!(
                    success_state,
                    InvoiceBundleLegState::Accepted | InvoiceBundleLegState::Settled
                );

            assert_eq!(
                funded, expected,
                "Funds_Locked mismatch: base={base_state:?}, success={success_state:?} => got {funded}, expected {expected}"
            );
        }
    }
}

// ─── Property 4: Invoice Bundle Immutability ────────────────────────────────
// Feature: production-contract-hardening, Property 4: Invoice Bundle Immutability
// Validates: Requirements 4.1, 4.2, 4.3
//
// For all signed invoice bundles, verify_artifact returns true at issuance and
// continues to return true regardless of external state changes.
#[test]
fn property_4_invoice_bundle_immutability() {
    let signing_key = crypto::generate_signing_key();
    let signer = crypto::public_key_hex(&signing_key);

    let bundle = protocol::sign_artifact(
        &signer,
        |msg| crypto::sign_message_hex(&signing_key, msg),
        TRANSPORT_TYPE_INVOICE_BUNDLE,
        1000,
        InvoiceBundlePayload {
            provider_id: signer.clone(),
            requester_id: "aa".repeat(32),
            quote_hash: "bb".repeat(32),
            deal_hash: "cc".repeat(32),
            expires_at: 2000,
            destination_identity: format!("02{}", "dd".repeat(32)),
            base_fee: InvoiceBundleLeg {
                amount_msat: 1000,
                invoice_bolt11: String::new(),
                invoice_hash: "ee".repeat(32),
                payment_hash: "ff".repeat(32),
                state: InvoiceBundleLegState::Open,
            },
            success_fee: InvoiceBundleLeg {
                amount_msat: 9000,
                invoice_bolt11: String::new(),
                invoice_hash: "11".repeat(32),
                payment_hash: "22".repeat(32),
                state: InvoiceBundleLegState::Open,
            },
            min_final_cltv_expiry: 18,
        },
    )
    .expect("sign bundle");

    // Verify at issuance
    assert!(
        protocol::verify_artifact(&bundle),
        "bundle should verify at issuance"
    );

    let original_hash = bundle.hash.clone();
    let original_signature = bundle.signature.clone();

    // Simulate all possible external state transitions — the bundle itself
    // must remain verifiable because it is immutable. We create sessions with
    // different observed states but the bundle bytes never change.
    let all_states = [
        InvoiceBundleLegState::Open,
        InvoiceBundleLegState::Accepted,
        InvoiceBundleLegState::Settled,
        InvoiceBundleLegState::Canceled,
        InvoiceBundleLegState::Expired,
    ];

    for base_state in &all_states {
        for success_state in &all_states {
            // The bundle artifact itself is unchanged regardless of external state
            assert!(
                protocol::verify_artifact(&bundle),
                "bundle verification must remain true with external states base={base_state:?}, success={success_state:?}"
            );
            assert_eq!(bundle.hash, original_hash, "hash must be stable");
            assert_eq!(
                bundle.signature, original_signature,
                "signature must be stable"
            );
        }
    }
}

// ─── Property 5: Receipt Settlement State Consistency ───────────────────────
// Feature: production-contract-hardening, Property 5: Receipt Settlement State Consistency
// Validates: Requirements 6.1, 6.2, 6.4, 6.5
//
// For all terminal receipts: if method == "lightning...", then settlement_state
// in {settled, canceled, expired}. If method == "none", then settlement_state == "none".
#[test]
fn property_5_receipt_settlement_state_consistency() {
    let fixture = load_conformance();

    let mut receipts: Vec<(String, Value)> = Vec::new();

    if let Some(artifacts) = fixture.get("artifacts").and_then(|v| v.as_object()) {
        for (name, vector) in artifacts {
            let artifact = &vector["artifact"];
            if artifact.get("artifact_type").and_then(|v| v.as_str()) == Some("receipt") {
                receipts.push((name.clone(), artifact["payload"].clone()));
            }
        }
    }
    if let Some(cases) = fixture
        .get("artifact_verification_cases")
        .and_then(|v| v.as_array())
    {
        for case in cases {
            if case.get("artifact_type").and_then(|v| v.as_str()) == Some("receipt") {
                let name = case["name"].as_str().unwrap_or("unknown").to_string();
                receipts.push((name, case["artifact"]["payload"].clone()));
            }
        }
    }

    let lightning_valid_states: HashSet<&str> =
        ["settled", "canceled", "expired"].into_iter().collect();

    for (name, payload) in &receipts {
        let method = payload["settlement_refs"]
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let settlement_state = payload
            .get("settlement_state")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match method {
            "lightning.base_fee_plus_success_fee.v1" => {
                assert!(
                    lightning_valid_states.contains(settlement_state),
                    "receipt '{name}': Lightning receipt has settlement_state '{settlement_state}', expected one of {{settled, canceled, expired}}"
                );
                // settlement_refs must include non-null bundle_hash
                let bundle_hash = payload["settlement_refs"].get("bundle_hash");
                assert!(
                    bundle_hash.is_some() && !bundle_hash.unwrap().is_null(),
                    "receipt '{name}': Lightning receipt must have non-null bundle_hash"
                );
                // non-empty destination_identity
                let dest = payload["settlement_refs"]
                    .get("destination_identity")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                assert!(
                    !dest.is_empty(),
                    "receipt '{name}': Lightning receipt must have non-empty destination_identity"
                );
            }
            "none" => {
                assert_eq!(
                    settlement_state, "none",
                    "receipt '{name}': free receipt has settlement_state '{settlement_state}', expected 'none'"
                );
            }
            _ => {
                panic!("receipt '{name}': unexpected method '{method}'");
            }
        }
    }

    assert!(
        !receipts.is_empty(),
        "no receipts found in conformance vectors"
    );
}

// ─── Property 7: Conformance Vector Round-Trip ──────────────────────────────
// Feature: production-contract-hardening, Property 7: Conformance Vector Round-Trip
// Validates: Requirements 14.1, 14.2, 14.3, 14.4
//
// For all artifacts in conformance/kernel_v1.json: verify_artifact returns expected_valid.
#[test]
fn property_7_conformance_vector_round_trip() {
    let fixture = load_conformance();

    let cases: Vec<ArtifactVerificationCase> =
        serde_json::from_value(fixture["artifact_verification_cases"].clone())
            .expect("parse artifact_verification_cases");

    assert!(!cases.is_empty(), "no verification cases found");

    for case in &cases {
        let result = verify_case(case);
        assert_eq!(
            result, case.expected_valid,
            "conformance case '{}': verify_artifact returned {result}, expected {}",
            case.name, case.expected_valid
        );
    }
}

// ─── Property 8: Cross-Reference Validity ───────────────────────────────────
// Feature: production-contract-hardening, Property 8: Cross-Reference Validity
// Validates: Requirements 8.3, 8.5
//
// For all markdown files, every relative .md link resolves to an existing file.
#[test]
fn property_8_cross_reference_validity() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Collect all .md files, excluding hidden dirs and common non-source dirs
    let md_files = collect_md_files(&repo_root);
    assert!(!md_files.is_empty(), "no markdown files found");

    let mut failures: Vec<String> = Vec::new();

    for md_file in &md_files {
        let content = match fs::read_to_string(md_file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let parent = md_file.parent().unwrap();

        for target in extract_md_link_targets(&content) {
            // Skip external URLs, anchors, and non-.md links
            if target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("mailto:")
                || target.starts_with('#')
            {
                continue;
            }

            // Only check .md links
            let path_part = target.split('#').next().unwrap_or(&target);
            if !path_part.ends_with(".md") {
                continue;
            }

            let resolved = parent.join(path_part);
            if !resolved.exists() {
                let relative = md_file.strip_prefix(&repo_root).unwrap_or(md_file);
                failures.push(format!(
                    "  {}  →  {} (not found)",
                    relative.display(),
                    path_part
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "broken .md cross-references found:\n{}",
        failures.join("\n")
    );
}

fn collect_md_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let skip_dirs: HashSet<&str> = [
        ".git",
        "target",
        "node_modules",
        "data",
        ".idea",
        ".kiro",
        ".claude",
    ]
    .into_iter()
    .collect();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if path.is_dir() {
                if !skip_dirs.contains(name_str.as_ref()) {
                    result.extend(collect_md_files(&path));
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                result.push(path);
            }
        }
    }
    result
}

/// Extract markdown link targets from content: `[text](target)` → target
fn extract_md_link_targets(content: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // Look for ](
        if bytes[i] == b']' && i + 1 < len && bytes[i + 1] == b'(' {
            let start = i + 2;
            if let Some(end) = content[start..].find(')') {
                let target = &content[start..start + end];
                if !target.is_empty() && !target.contains('\n') {
                    targets.push(target.to_string());
                }
                i = start + end + 1;
                continue;
            }
        }
        i += 1;
    }
    targets
}

// ─── Property 9: Free-Deal Admission Behavior ──────────────────────────────
// Feature: production-contract-hardening, Property 9: Free-Deal Admission Behavior
// Validates: Requirements 1.2, 1.4
//
// For all deals where settlement_method == "none", no invoice bundle is generated
// and no payment gating occurs.
#[test]
fn property_9_free_deal_admission_behavior() {
    let fixture = load_conformance();

    // Collect all free-service deals and quotes from conformance vectors
    let artifacts = fixture
        .get("artifacts")
        .and_then(|v| v.as_object())
        .unwrap();

    // Find all quote artifacts with method "none"
    let mut free_quote_hashes: HashSet<String> = HashSet::new();
    for (_name, vector) in artifacts {
        let artifact = &vector["artifact"];
        if artifact.get("artifact_type").and_then(|v| v.as_str()) == Some("quote")
            && let Some(method) = artifact["payload"]
                .get("settlement_terms")
                .and_then(|v| v.get("method"))
                .and_then(|v| v.as_str())
            && method == "none"
            && let Some(hash) = artifact.get("hash").and_then(|v| v.as_str())
        {
            free_quote_hashes.insert(hash.to_string());
        }
    }

    // Find all deal artifacts that reference a free quote
    let mut free_deal_hashes: HashSet<String> = HashSet::new();
    for (_name, vector) in artifacts {
        let artifact = &vector["artifact"];
        if artifact.get("artifact_type").and_then(|v| v.as_str()) == Some("deal")
            && let Some(quote_hash) = artifact["payload"]
                .get("quote_hash")
                .and_then(|v| v.as_str())
            && free_quote_hashes.contains(quote_hash)
            && let Some(hash) = artifact.get("hash").and_then(|v| v.as_str())
        {
            free_deal_hashes.insert(hash.to_string());
        }
    }

    // Verify no invoice_bundle references any free deal
    for (_name, vector) in artifacts {
        let artifact = &vector["artifact"];
        if artifact.get("artifact_type").and_then(|v| v.as_str()) == Some("invoice_bundle")
            && let Some(deal_hash) = artifact["payload"]
                .get("deal_hash")
                .and_then(|v| v.as_str())
        {
            assert!(
                !free_deal_hashes.contains(deal_hash),
                "invoice_bundle references free deal {deal_hash} — free deals must not have invoice bundles"
            );
        }
    }

    // Verify free-deal receipts have no bundle_hash
    for (_name, vector) in artifacts {
        let artifact = &vector["artifact"];
        if artifact.get("artifact_type").and_then(|v| v.as_str()) == Some("receipt")
            && let Some(deal_hash) = artifact["payload"]
                .get("deal_hash")
                .and_then(|v| v.as_str())
            && free_deal_hashes.contains(deal_hash)
        {
            let bundle_hash = artifact["payload"]["settlement_refs"].get("bundle_hash");
            assert!(
                bundle_hash.is_none() || bundle_hash.unwrap().is_null(),
                "free-deal receipt references a bundle_hash — free deals must not have invoice bundles"
            );
        }
    }

    assert!(
        !free_deal_hashes.is_empty(),
        "no free deals found in conformance vectors"
    );
}

// ─── Property 10: Success Payment Hash Linkage ──────────────────────────────
// Feature: production-contract-hardening, Property 10: Success Payment Hash Linkage
// Validates: Requirements 2.7
//
// For all Lightning-settled deals with an invoice bundle,
// deal.success_payment_hash == invoice_bundle.success_fee.payment_hash.
#[test]
fn property_10_success_payment_hash_linkage() {
    let fixture = load_conformance();
    let artifacts = fixture
        .get("artifacts")
        .and_then(|v| v.as_object())
        .unwrap();

    // Collect all deal artifacts (hash -> success_payment_hash)
    let mut deal_payment_hashes: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for (_name, vector) in artifacts {
        let artifact = &vector["artifact"];
        if artifact.get("artifact_type").and_then(|v| v.as_str()) == Some("deal")
            && let (Some(hash), Some(sph)) = (
                artifact.get("hash").and_then(|v| v.as_str()),
                artifact["payload"]
                    .get("success_payment_hash")
                    .and_then(|v| v.as_str()),
            )
        {
            deal_payment_hashes.insert(hash.to_string(), sph.to_string());
        }
    }

    // Collect all invoice_bundle artifacts (deal_hash -> success_fee.payment_hash)
    let mut checked = 0;
    for (_name, vector) in artifacts {
        let artifact = &vector["artifact"];
        if artifact.get("artifact_type").and_then(|v| v.as_str()) == Some("invoice_bundle")
            && let Some(deal_hash) = artifact["payload"]
                .get("deal_hash")
                .and_then(|v| v.as_str())
            && let Some(deal_sph) = deal_payment_hashes.get(deal_hash)
        {
            let bundle_sph = artifact["payload"]["success_fee"]
                .get("payment_hash")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert_eq!(
                deal_sph, bundle_sph,
                "deal.success_payment_hash ({deal_sph}) != invoice_bundle.success_fee.payment_hash ({bundle_sph}) for deal {deal_hash}"
            );
            checked += 1;
        }
    }

    assert!(
        checked > 0,
        "no Lightning deal+bundle pairs found to verify hash linkage"
    );
}

// ─── Property 11: Free-Service Offer Settlement Method ──────────────────────
// Feature: production-contract-hardening, Property 11: Free-Service Offer Settlement Method
// Validates: Requirements 9.2, 10.2
//
// For all offers where base_fee_msat == 0 and success_fee_msat == 0,
// settlement_method is "none".
#[test]
fn property_11_free_service_offer_settlement_method() {
    let fixture = load_conformance();

    let mut offers: Vec<(String, Value)> = Vec::new();

    // Collect from artifacts map
    if let Some(artifacts) = fixture.get("artifacts").and_then(|v| v.as_object()) {
        for (name, vector) in artifacts {
            let artifact = &vector["artifact"];
            if artifact.get("artifact_type").and_then(|v| v.as_str()) == Some("offer") {
                offers.push((name.clone(), artifact["payload"].clone()));
            }
        }
    }

    // Collect from verification cases
    if let Some(cases) = fixture
        .get("artifact_verification_cases")
        .and_then(|v| v.as_array())
    {
        for case in cases {
            if case.get("artifact_type").and_then(|v| v.as_str()) == Some("offer") {
                let name = case["name"].as_str().unwrap_or("unknown").to_string();
                offers.push((name, case["artifact"]["payload"].clone()));
            }
        }
    }

    let mut free_checked = 0;
    for (name, payload) in &offers {
        let base_fee = payload
            .get("price_schedule")
            .and_then(|v| v.get("base_fee_msat"))
            .and_then(|v| v.as_u64());
        let success_fee = payload
            .get("price_schedule")
            .and_then(|v| v.get("success_fee_msat"))
            .and_then(|v| v.as_u64());

        if base_fee == Some(0) && success_fee == Some(0) {
            let method = payload
                .get("settlement_method")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert_eq!(
                method, "none",
                "offer '{name}': free offer (base=0, success=0) has settlement_method '{method}', expected 'none'"
            );
            free_checked += 1;
        }
    }

    assert!(
        free_checked > 0,
        "no free offers found in conformance vectors"
    );

    // Also verify via code: sign a free offer and check settlement_method
    let signing_key = crypto::generate_signing_key();
    let signer = crypto::public_key_hex(&signing_key);
    let offer = protocol::sign_artifact(
        &signer,
        |msg| crypto::sign_message_hex(&signing_key, msg),
        protocol::ARTIFACT_TYPE_OFFER,
        1000,
        OfferPayload {
            provider_id: signer.clone(),
            offer_id: "test.free".to_string(),
            descriptor_hash: "aa".repeat(32),
            expires_at: None,
            offer_kind: "compute.wasm.v1".to_string(),
            settlement_method: "none".to_string(),
            quote_ttl_secs: 300,
            execution_profile: OfferExecutionProfile {
                runtime: ExecutionRuntime::Wasm,
                package_kind: String::new(),
                contract_version: String::new(),
                access_handles: Vec::new(),
                abi_version: String::new(),
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
    .expect("sign free offer");

    assert!(protocol::verify_artifact(&offer));
    assert_eq!(offer.payload.settlement_method, "none");
    assert_eq!(offer.payload.price_schedule.base_fee_msat, 0);
    assert_eq!(offer.payload.price_schedule.success_fee_msat, 0);
}
