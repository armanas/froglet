/// Verify a signed artifact document: payload hash integrity AND signature.
///
/// 1. Recomputes SHA-256 of the canonical JSON payload and checks it matches `payload_hash`.
/// 2. Verifies the BIP340 Schnorr signature over the canonical signing bytes.
///
/// This prevents substitution attacks where an attacker replaces the payload
/// while reusing a signature from a different artifact.
pub fn verify_artifact_document(document: &serde_json::Value) -> bool {
    let signer = match document.get("signer").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };
    let signature = match document.get("signature").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };
    let claimed_payload_hash = match document.get("payload_hash").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };
    let payload = match document.get("payload") {
        Some(p) => p,
        None => return false,
    };

    // Step 1: verify payload_hash matches actual payload content
    let computed_hash = match froglet_protocol::protocol::payload_hash(payload) {
        Ok(h) => h,
        Err(_) => return false,
    };
    if computed_hash != claimed_payload_hash {
        return false;
    }

    // Step 2: reconstruct canonical signing bytes and verify signature
    let schema_version = document
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let artifact_type = document
        .get("artifact_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let created_at = document
        .get("created_at")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let signing_bytes = match froglet_protocol::protocol::canonical_signing_bytes(
        schema_version,
        artifact_type,
        signer,
        created_at,
        claimed_payload_hash,
        payload,
    ) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    froglet_protocol::crypto::verify_message(signer, signature, &signing_bytes)
}
