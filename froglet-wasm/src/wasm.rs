//! wasm-bindgen surface: strings and bytes in, strings out, as `froglet-verify`'s WebAssembly surface does. It adds
//! only argument checks that JavaScript needs; the work is in the crate root.

use wasm_bindgen::prelude::*;

fn js_error(message: String) -> JsValue {
    JsValue::from_str(&message)
}

/// See [`crate::new_identity`].
#[wasm_bindgen]
pub fn new_identity() -> String {
    crate::new_identity()
}

/// See [`crate::public_key_from_seed`].
#[wasm_bindgen]
pub fn public_key_from_seed(seed_hex: &str) -> Result<String, JsValue> {
    crate::public_key_from_seed(seed_hex).map_err(js_error)
}

/// See [`crate::sign_artifact`]. `created_at` is a JavaScript number, so it must be a whole number of seconds.
#[wasm_bindgen]
pub fn sign_artifact_json(
    seed_hex: &str,
    artifact_type: &str,
    created_at: f64,
    payload_json: &str,
) -> Result<String, JsValue> {
    if !created_at.is_finite()
        || created_at.fract() != 0.0
        || created_at.abs() > 9_007_199_254_740_991.0
    {
        return Err(js_error(
            "created_at must be a whole number of seconds".to_string(),
        ));
    }
    crate::sign_artifact(seed_hex, artifact_type, created_at as i64, payload_json).map_err(js_error)
}

/// See [`crate::canonicalize`].
#[wasm_bindgen]
pub fn canonicalize_json(json: &str) -> Result<String, JsValue> {
    crate::canonicalize(json).map_err(js_error)
}

/// See [`crate::canonical_sha256`].
#[wasm_bindgen]
pub fn canonical_sha256_json(json: &str) -> Result<String, JsValue> {
    crate::canonical_sha256(json).map_err(js_error)
}

/// See [`crate::sha256_bytes`].
#[wasm_bindgen]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    crate::sha256_bytes(bytes)
}
