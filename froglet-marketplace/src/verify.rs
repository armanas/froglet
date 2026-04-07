/// Verify a signed artifact document's signature against its payload_hash.
///
/// This checks that `signer` signed `payload_hash` with a valid BIP340 Schnorr
/// signature.  It does NOT verify that `payload_hash` matches the actual payload
/// content — that requires typed deserialization via
/// `froglet_protocol::protocol::verify_artifact`.
pub fn verify_artifact_signature(document: &serde_json::Value) -> bool {
    let signer = match document.get("signer").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };
    let signature = match document.get("signature").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };
    let payload_hash = match document.get("payload_hash").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };
    froglet_protocol::crypto::verify_signature(signer, signature, payload_hash)
}
