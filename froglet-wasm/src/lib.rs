//! The signing half of the Froglet kernel, compiled for the browser.
//!
//! `froglet-verify` compiles the verifying half of `froglet-protocol` to WebAssembly. This crate compiles the signing
//! half: fresh identities, artifact signing, and canonical hashing. The froglet.dev playground uses both, so every key,
//! signature, and hash in it is made by the same Rust code a node uses. Nothing here is protocol logic of its own; each
//! function is a thin wrapper over `froglet_protocol` that takes and returns strings, as `froglet-verify`'s WebAssembly
//! surface does.
//!
//! It is not a wallet. Seeds are plain hex strings held by the caller and nothing is stored, so it suits throwaway
//! identities such as the playground's. It signs only the five artifact types a free deal produces.

use froglet_protocol::protocol::{
    self as kernel, ARTIFACT_TYPE_DEAL, ARTIFACT_TYPE_DESCRIPTOR, ARTIFACT_TYPE_OFFER,
    ARTIFACT_TYPE_QUOTE, ARTIFACT_TYPE_RECEIPT,
};
use froglet_protocol::{canonical_json, crypto};
use serde_json::Value;

#[cfg(target_arch = "wasm32")]
mod wasm;

/// The artifact types this crate signs: the five that make up a free deal.
pub const SIGNABLE_TYPES: [&str; 5] = [
    ARTIFACT_TYPE_DESCRIPTOR,
    ARTIFACT_TYPE_OFFER,
    ARTIFACT_TYPE_QUOTE,
    ARTIFACT_TYPE_DEAL,
    ARTIFACT_TYPE_RECEIPT,
];

fn key_from_seed(seed_hex: &str) -> Result<crypto::NodeSigningKey, String> {
    let bytes = hex::decode(seed_hex).map_err(|error| format!("seed must be hex: {error}"))?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "seed must be 32 bytes (64 hex characters)".to_string())?;
    crypto::signing_key_from_seed_bytes(&seed)
}

/// A fresh identity as JSON: `{"seed_hex": ..., "public_key": ...}`. The public key is the 32-byte x-only secp256k1 key
/// that appears as `signer`, `provider_id`, and `requester_id`. The randomness comes from the platform, which in a
/// browser is `crypto.getRandomValues`.
pub fn new_identity() -> String {
    let key = crypto::generate_signing_key();
    serde_json::json!({
        "seed_hex": hex::encode(crypto::signing_key_seed_bytes(&key)),
        "public_key": crypto::public_key_hex(&key),
    })
    .to_string()
}

/// The public key (hex) that a seed signs as.
pub fn public_key_from_seed(seed_hex: &str) -> Result<String, String> {
    Ok(crypto::public_key_hex(&key_from_seed(seed_hex)?))
}

/// Sign `payload_json` as `seed_hex`'s identity and return the signed envelope as JSON: the artifact type, schema
/// version, signer, `created_at`, `payload_hash`, `hash`, payload, and BIP-340 signature, exactly as
/// `froglet_protocol::protocol::sign_artifact` builds them.
pub fn sign_artifact(
    seed_hex: &str,
    artifact_type: &str,
    created_at: i64,
    payload_json: &str,
) -> Result<String, String> {
    if !SIGNABLE_TYPES.contains(&artifact_type) {
        return Err(format!(
            "artifact_type must be one of {}; got {artifact_type:?}",
            SIGNABLE_TYPES.join(", ")
        ));
    }
    let key = key_from_seed(seed_hex)?;
    let payload: Value = serde_json::from_str(payload_json)
        .map_err(|error| format!("payload is not JSON: {error}"))?;
    let signer = crypto::public_key_hex(&key);
    let artifact = kernel::sign_artifact(
        &signer,
        |message| crypto::sign_message_hex(&key, message),
        artifact_type,
        created_at,
        payload,
    )?;
    serde_json::to_string(&artifact).map_err(|error| error.to_string())
}

/// The RFC 8785 canonical text of a JSON document: the exact bytes a node hands to a Wasm function as its input, and the
/// bytes that [`canonical_sha256`] hashes.
pub fn canonicalize(json: &str) -> Result<String, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|error| format!("input is not JSON: {error}"))?;
    let canonical = canonical_json::to_vec(&value).map_err(|error| error.to_string())?;
    String::from_utf8(canonical).map_err(|error| error.to_string())
}

/// SHA-256 (hex) of the RFC 8785 canonical form of a JSON document: how payload, workload, input, and result hashes
/// are made.
pub fn canonical_sha256(json: &str) -> Result<String, String> {
    Ok(crypto::sha256_hex(canonicalize(json)?.as_bytes()))
}

/// SHA-256 (hex) of raw bytes: how a module hash is made.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    crypto::sha256_hex(bytes)
}
