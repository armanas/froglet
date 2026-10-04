//! The browser signing build must reproduce the frozen conformance vectors.
//!
//! These tests run the crate's own API over `conformance/kernel_v1.json`: with the vectors' fixed seeds it has to produce
//! every signed artifact byte for byte, and with fresh identities it has to produce artifacts that the kernel verifies.
//! That is what lets the playground claim that its keys, signatures, and hashes are the real kernel's.

use std::{fs, path::PathBuf};

use froglet_protocol::protocol::{self as kernel, SignedArtifact};
use froglet_wasm::{
    SIGNABLE_TYPES, canonical_sha256, canonicalize, new_identity, public_key_from_seed,
    sha256_bytes, sign_artifact,
};
use serde_json::Value;

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/kernel_v1.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn seed_for<'a>(fixture: &'a Value, signer: &str) -> &'a str {
    let keys = &fixture["keys"];
    let seed = if signer == keys["provider_id"] {
        &keys["provider_seed_hex"]
    } else if signer == keys["requester_id"] {
        &keys["requester_seed_hex"]
    } else {
        panic!("no vector seed for signer {signer}");
    };
    seed.as_str().expect("seed is a string")
}

fn artifacts(fixture: &Value) -> impl Iterator<Item = (&String, &Value)> {
    fixture["artifacts"]
        .as_object()
        .expect("fixture has artifacts")
        .iter()
}

#[test]
fn signs_every_vector_artifact_byte_for_byte() {
    let fixture = fixture();
    let mut signed = 0;
    for (name, entry) in artifacts(&fixture) {
        let artifact = &entry["artifact"];
        let kind = artifact["artifact_type"].as_str().expect("artifact_type");
        if !SIGNABLE_TYPES.contains(&kind) {
            continue;
        }
        let seed = seed_for(&fixture, artifact["signer"].as_str().expect("signer"));
        let created_at = artifact["created_at"].as_i64().expect("created_at");
        let payload = artifact["payload"].to_string();

        let once = sign_artifact(seed, kind, created_at, &payload)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let again = sign_artifact(seed, kind, created_at, &payload).expect("second signature");
        assert_eq!(once, again, "{name}: signing is deterministic");

        let produced: Value = serde_json::from_str(&once).expect("signed artifact is JSON");
        assert_eq!(
            &produced, artifact,
            "{name}: hash, payload_hash and signature match the vector"
        );
        signed += 1;
    }
    // Ten vector artifacts; the invoice bundle is the one type this crate does not sign.
    assert_eq!(signed, 9);
}

#[test]
fn hashes_every_vector_payload_the_way_the_vectors_record_it() {
    let fixture = fixture();
    for (name, entry) in artifacts(&fixture) {
        let hash = canonical_sha256(&entry["artifact"]["payload"].to_string()).expect("hash");
        assert_eq!(
            hash,
            entry["payload_hash"].as_str().expect("payload_hash"),
            "{name}"
        );
        assert_eq!(
            hash,
            entry["artifact"]["payload_hash"]
                .as_str()
                .expect("payload_hash"),
            "{name}"
        );
    }
}

#[test]
fn hashes_the_vector_workload_module_and_input() {
    let fixture = fixture();
    let submission = &fixture["workload_spec"]["submission"];
    let workload = &submission["workload"];

    let module = hex_bytes(
        submission["module_bytes_hex"]
            .as_str()
            .expect("module bytes"),
    );
    assert_eq!(
        sha256_bytes(&module),
        workload["module_hash"].as_str().expect("module_hash")
    );
    assert_eq!(
        canonical_sha256(&submission["input"].to_string()).expect("input hash"),
        workload["input_hash"].as_str().expect("input_hash")
    );
    assert_eq!(
        canonical_sha256(&workload.to_string()).expect("workload hash"),
        fixture["artifacts"]["free_quote"]["artifact"]["payload"]["workload_hash"]
            .as_str()
            .expect("workload_hash")
    );
}

fn hex_bytes(text: &str) -> Vec<u8> {
    hex::decode(text).expect("hex")
}

#[test]
fn fresh_identities_sign_artifacts_that_the_kernel_verifies() {
    let fixture = fixture();
    let identity: Value = serde_json::from_str(&new_identity()).expect("identity is JSON");
    let seed = identity["seed_hex"].as_str().expect("seed_hex");
    let public_key = identity["public_key"].as_str().expect("public_key");
    assert_eq!(seed.len(), 64);
    assert_eq!(public_key.len(), 64);
    assert_eq!(public_key_from_seed(seed).expect("public key"), public_key);

    let mut payload = fixture["artifacts"]["free_offer"]["artifact"]["payload"].clone();
    payload["provider_id"] = Value::String(public_key.to_string());
    let signed = sign_artifact(seed, "offer", 1_700_000_100, &payload.to_string()).expect("sign");

    let artifact: SignedArtifact<Value> = serde_json::from_str(&signed).expect("signed artifact");
    assert_eq!(artifact.signer, public_key);
    assert!(
        kernel::verify_artifact(&artifact),
        "the kernel verifies what this crate signs"
    );

    let mut altered = artifact.clone();
    altered.created_at += 1;
    assert!(
        !kernel::verify_artifact(&altered),
        "a change after signing is caught"
    );
}

#[test]
fn every_identity_is_new() {
    let first: Value = serde_json::from_str(&new_identity()).expect("identity");
    let second: Value = serde_json::from_str(&new_identity()).expect("identity");
    assert_ne!(first["seed_hex"], second["seed_hex"]);
    assert_ne!(first["public_key"], second["public_key"]);
}

#[test]
fn hashes_bytes_the_standard_way() {
    assert_eq!(
        sha256_bytes(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_bytes(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn canonical_hashing_ignores_key_order_and_whitespace() {
    assert_eq!(
        canonical_sha256(r#"{"b": 2, "a": [1, {"d": 4, "c": 3}]}"#).expect("hash"),
        canonical_sha256(r#"{"a":[1,{"c":3,"d":4}],"b":2}"#).expect("hash")
    );
}

#[test]
fn canonicalizes_the_way_a_node_writes_a_functions_input() {
    // Keys are sorted at every depth, insignificant whitespace goes, and strings keep their own characters.
    assert_eq!(
        canonicalize(r#"{"b": 7, "a": 6}"#).expect("canonical"),
        r#"{"a":6,"b":7}"#
    );
    assert_eq!(
        canonicalize(r#"{ "z": [3, 1, {"y": 1, "x": 2}], "a": "é\n" }"#).expect("canonical"),
        "{\"a\":\"é\\n\",\"z\":[3,1,{\"x\":2,\"y\":1}]}"
    );
    assert_eq!(canonicalize("  null ").expect("canonical"), "null");
}

#[test]
fn hashes_exactly_the_text_it_canonicalizes() {
    let fixture = fixture();
    let submission = &fixture["workload_spec"]["submission"];
    let input = submission["input"].to_string();
    let text = canonicalize(&input).expect("canonical input");
    assert_eq!(
        sha256_bytes(text.as_bytes()),
        submission["workload"]["input_hash"]
            .as_str()
            .expect("input_hash"),
        "the vector's input hash is the hash of the canonical text a function would receive"
    );
    assert_eq!(
        canonical_sha256(&input).expect("hash"),
        sha256_bytes(text.as_bytes())
    );
}

#[test]
fn refuses_what_it_should_not_sign_or_hash() {
    let seed = "11".repeat(32);
    let payload = "{}";

    for kind in [
        "invoice_bundle",
        "curated_list",
        "confidential_session",
        "Offer",
        "",
    ] {
        let error = sign_artifact(&seed, kind, 1, payload).unwrap_err();
        assert!(
            error.contains("artifact_type must be one of"),
            "{kind}: {error}"
        );
    }
    for bad_seed in [
        "",
        "zz",
        &"11".repeat(31),
        &"11".repeat(33),
        &"00".repeat(32),
    ] {
        assert!(
            sign_artifact(bad_seed, "offer", 1, payload).is_err(),
            "seed {bad_seed:?} is refused"
        );
        assert!(
            public_key_from_seed(bad_seed).is_err(),
            "seed {bad_seed:?} has no public key"
        );
    }
    assert!(
        sign_artifact(&seed, "offer", 1, "{not json")
            .unwrap_err()
            .contains("payload is not JSON")
    );
    assert!(
        canonical_sha256("{not json")
            .unwrap_err()
            .contains("input is not JSON")
    );
    assert!(
        canonicalize("{not json")
            .unwrap_err()
            .contains("input is not JSON")
    );
}
