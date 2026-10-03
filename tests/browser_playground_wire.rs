//! What the docs-site playground says about the wire must be what a node sends.
//!
//! `froglet-wasm/tests/fixtures/node_service_exchange.json` holds the responses of a real `froglet-node` that had published
//! a Wasm function and served one free deal for it. The playground's provider and consumer are written in TypeScript
//! against those shapes, and its tests read the same file. These tests keep the file honest from the node's side: every
//! capture must survive the node's own types unchanged, the artifacts and their chain must verify, and the workload the
//! playground builds must hash the way the node hashes it. When the node changes what it sends, one of these fails, and
//! the fixture and the playground are updated together.

use std::{fs, path::PathBuf};

use froglet::{api::ProviderServiceRecord, deals::DealRecord, execution::ExecutionWorkload};
use froglet_protocol::{
    canonical_json, crypto,
    protocol::{DescriptorPayload, OfferPayload, QuotePayload, ReceiptPayload, SignedArtifact},
};
use froglet_verify::{
    DocumentStatus, documents_form_chain, validate_chain_documents, verify_document,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("froglet-wasm/tests/fixtures/node_service_exchange.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// The value must survive the node's own type unchanged: read into it and written back gives the same JSON, so no
/// field the node now ignores, renames, or newly requires is hiding in the capture.
fn assert_round_trips<T: Serialize + DeserializeOwned>(name: &str, value: &Value) {
    let typed: T = serde_json::from_value(value.clone()).unwrap_or_else(|error| {
        panic!("{name}: the node's type no longer reads the captured JSON: {error}")
    });
    assert_eq!(
        &serde_json::to_value(&typed).expect("the node's type serialises"),
        value,
        "{name}: the node's type writes different JSON than was captured"
    );
}

fn canonical_hash(value: &Value) -> String {
    crypto::sha256_hex(canonical_json::to_vec(value).expect("canonical JSON"))
}

fn offers(fixture: &Value) -> &Vec<Value> {
    fixture["offers"]["offers"].as_array().expect("offers list")
}

fn service_offer(fixture: &Value) -> &Value {
    let offer_id = fixture["service"]["service"]["offer_id"]
        .as_str()
        .expect("offer_id");
    offers(fixture)
        .iter()
        .find(|offer| offer["payload"]["offer_id"] == offer_id)
        .expect("the service has an offer")
}

#[test]
fn every_captured_response_round_trips_through_the_nodes_own_types() {
    let fixture = fixture();
    assert_round_trips::<SignedArtifact<DescriptorPayload>>("descriptor", &fixture["descriptor"]);
    for offer in offers(&fixture) {
        assert_round_trips::<SignedArtifact<OfferPayload>>("offer", offer);
    }
    assert_round_trips::<SignedArtifact<QuotePayload>>("quote", &fixture["quote"]);
    assert_round_trips::<DealRecord>("deal record as created", &fixture["deal_created"]);
    assert_round_trips::<DealRecord>("deal record as finished", &fixture["deal_final"]);
    assert_round_trips::<SignedArtifact<ReceiptPayload>>(
        "receipt",
        &fixture["deal_final"]["receipt"],
    );
    assert_round_trips::<ProviderServiceRecord>("service record", &fixture["service"]["service"]);
    for service in fixture["services"]["services"]
        .as_array()
        .expect("services list")
    {
        assert_round_trips::<ProviderServiceRecord>("listed service record", service);
    }
}

#[test]
fn the_captured_artifacts_and_their_chain_verify_offline() {
    let fixture = fixture();
    let chain = vec![
        fixture["descriptor"].clone(),
        service_offer(&fixture).clone(),
        fixture["quote"].clone(),
        fixture["deal_final"]["deal"].clone(),
        fixture["deal_final"]["receipt"].clone(),
    ];
    for document in &chain {
        let report = verify_document(document, None);
        assert_eq!(
            report.status,
            DocumentStatus::Verified,
            "{}: {:?}",
            report.artifact_type,
            report.semantics
        );
    }
    assert!(documents_form_chain(&chain));
    let report = validate_chain_documents(&chain, None);
    assert!(
        report.valid,
        "{:?} {:?}",
        report.structure_errors, report.chain
    );
}

#[test]
fn the_playgrounds_execution_hashes_the_way_the_node_hashes_it() {
    let fixture = fixture();
    let service = &fixture["service"]["service"];
    let input = &fixture["input"];
    // Built field by field from the service record, as the playground's consumer and the project's JS client build it.
    let execution = json!({
        "schema_version": "froglet/v1",
        "workload_kind": "compute.execution.v1",
        "runtime": service["runtime"],
        "package_kind": service["package_kind"],
        "entrypoint": { "kind": service["entrypoint_kind"], "value": service["entrypoint"] },
        "contract_version": service["contract_version"],
        "input_format": "application/json+jcs",
        "input_hash": canonical_hash(input),
        "security": { "mode": "standard", "service_id": service["service_id"] },
        "input": input,
        "module_hash": service["binding_hash"],
    });

    let typed: ExecutionWorkload = serde_json::from_value(execution.clone())
        .expect("the node reads the workload the playground builds");
    assert_eq!(
        serde_json::to_value(&typed).expect("serialises"),
        execution,
        "the node drops or adds no field when it reads the playground's workload"
    );
    assert_eq!(
        typed.request_hash().expect("request hash"),
        fixture["quote"]["payload"]["workload_hash"]
            .as_str()
            .expect("workload_hash"),
        "the node's request hash is the workload_hash it put in the quote"
    );
    assert!(typed.is_service_addressed());
}

#[test]
fn the_captured_exchange_is_consistent_the_way_the_playground_relies_on() {
    let fixture = fixture();
    let service = &fixture["service"]["service"];
    let offer = service_offer(&fixture);
    let quote = &fixture["quote"];
    let created = &fixture["deal_created"];
    let finished = &fixture["deal_final"];
    let receipt = &finished["receipt"];

    // Exact bytes retained with this capture, not a host-dependent rebuild of the same Rust source.
    let module = hex::decode(
        fixture["published_module_hex"]
            .as_str()
            .expect("retained published module hex"),
    )
    .expect("retained published module bytes");
    assert_eq!(
        crypto::sha256_hex(module),
        service["module_hash"].as_str().expect("module_hash")
    );

    // One provider throughout, and the service is offered under its own id.
    let provider = fixture["descriptor"]["payload"]["provider_id"].clone();
    for (name, id) in [
        ("service", &service["provider_id"]),
        ("offer", &offer["payload"]["provider_id"]),
        ("quote", &quote["payload"]["provider_id"]),
        ("receipt", &receipt["payload"]["provider_id"]),
    ] {
        assert_eq!(id, &provider, "{name}");
    }
    assert_eq!(offer["payload"]["offer_kind"], "compute.execution.v1");
    assert_eq!(
        offer["payload"]["descriptor_hash"],
        fixture["descriptor"]["hash"]
    );
    assert_eq!(service["module_hash"], service["binding_hash"]);
    assert_eq!(service["publication_state"], "active");

    // The chain links by hash, in the order the playground shows it.
    assert_eq!(quote["payload"]["offer_hash"], offer["hash"]);
    assert_eq!(
        quote["payload"]["descriptor_hash"],
        fixture["descriptor"]["hash"]
    );
    assert_eq!(finished["deal"]["payload"]["quote_hash"], quote["hash"]);
    assert_eq!(receipt["payload"]["deal_hash"], finished["deal"]["hash"]);
    assert_eq!(receipt["payload"]["quote_hash"], quote["hash"]);
    assert_eq!(
        receipt["payload"]["executor"]["module_hash"],
        service["module_hash"]
    );

    // A deal is accepted first and finishes later, and the playground polls for it.
    assert_eq!(created["status"], "accepted");
    assert_eq!(created["deal_id"], finished["deal_id"]);
    assert_eq!(finished["status"], "succeeded");
    assert_eq!(finished["workload_kind"], "compute.execution.v1");
    assert!(created.get("receipt").is_none() && created.get("result").is_none());

    // The receipt commits to the result by canonical hash.
    assert_eq!(receipt["payload"]["result_hash"], finished["result_hash"]);
    assert_eq!(finished["result_hash"], canonical_hash(&finished["result"]));
    assert_eq!(finished["result"], json!({ "sum": 13, "product": 42 }));
}
