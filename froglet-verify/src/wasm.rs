//! wasm-bindgen surface: strings in, JSON-report strings out. One stable ABI
//! with no structural marshalling, consumed by the froglet.dev verify page
//! and the `froglet-verify-wasm` npm package.

use wasm_bindgen::prelude::*;

use crate::{validate_chain_documents, verify_document};

fn error_json(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

fn parse_now(now_unix: Option<f64>) -> Option<i64> {
    now_unix.map(|now| now as i64)
}

/// Verify a single artifact document. Returns the `DocumentReport` as JSON,
/// or `{"error": ...}` when the input is not JSON.
#[wasm_bindgen]
pub fn verify_document_json(document_json: &str, now_unix: Option<f64>) -> String {
    let document: serde_json::Value = match serde_json::from_str(document_json) {
        Ok(value) => value,
        Err(error) => return error_json(&format!("input is not valid JSON: {error}")),
    };
    let report = verify_document(&document, parse_now(now_unix));
    serde_json::to_string(&report).unwrap_or_else(|error| error_json(&error.to_string()))
}

/// Verify a set of artifact documents and the full chain they form. Accepts
/// the same shapes as the CLI: a single artifact, an array, or an
/// `{"artifacts": [...]}` page. Returns the `ChainDocumentsReport` as JSON.
#[wasm_bindgen]
pub fn validate_chain_json(documents_json: &str, now_unix: Option<f64>) -> String {
    let input: serde_json::Value = match serde_json::from_str(documents_json) {
        Ok(value) => value,
        Err(error) => return error_json(&format!("input is not valid JSON: {error}")),
    };
    let documents = match crate::extract_documents(&input) {
        Ok(documents) => documents,
        Err(error) => return error_json(&error),
    };
    let report = validate_chain_documents(&documents, parse_now(now_unix));
    serde_json::to_string(&report).unwrap_or_else(|error| error_json(&error.to_string()))
}
