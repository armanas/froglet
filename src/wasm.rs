use crate::{canonical_json, crypto};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FROGLET_SCHEMA_V1: &str = "froglet/v1";
pub const WORKLOAD_KIND_COMPUTE_WASM_V1: &str = "compute.wasm.v1";
pub const WASM_SUBMISSION_TYPE_V1: &str = "wasm_submission";
pub const WASM_RUN_JSON_ABI_V1: &str = "froglet.wasm.run_json.v1";
pub const WASM_HOST_JSON_ABI_V1: &str = "froglet.wasm.host_json.v1";
pub const WASM_MODULE_FORMAT: &str = "application/wasm";
pub const JCS_JSON_FORMAT: &str = "application/json+jcs";
pub const WASM_CAPABILITY_HTTP_FETCH: &str = "net.http.fetch";
pub const WASM_CAPABILITY_HTTP_FETCH_AUTH_PREFIX: &str = "net.http.fetch.auth.";
pub const WASM_CAPABILITY_SQLITE_QUERY_READ_PREFIX: &str = "db.sqlite.query.read.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComputeWasmWorkload {
    pub schema_version: String,
    pub workload_kind: String,
    pub abi_version: String,
    pub module_format: String,
    pub module_hash: String,
    pub input_format: String,
    pub input_hash: String,
    #[serde(default)]
    pub requested_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WasmSubmission {
    pub schema_version: String,
    pub submission_type: String,
    pub workload: ComputeWasmWorkload,
    pub module_bytes_hex: String,
    #[serde(default = "default_input_value")]
    pub input: Value,
}

#[derive(Debug, Clone)]
pub struct VerifiedWasmSubmission {
    pub module_bytes: Vec<u8>,
    pub input: Value,
    pub abi_version: String,
    pub requested_capabilities: Vec<String>,
}

impl WasmSubmission {
    pub fn workload_hash(&self) -> Result<String, String> {
        let workload_bytes = canonical_json::to_vec(&self.workload).map_err(|e| e.to_string())?;
        Ok(crypto::sha256_hex(workload_bytes))
    }

    pub fn validate_limits(
        &self,
        max_module_hex_bytes: usize,
        max_input_bytes: usize,
    ) -> Result<(), String> {
        if self.module_bytes_hex.len() > max_module_hex_bytes {
            return Err("wasm module too large".to_string());
        }

        let input_bytes = canonical_json::to_vec(&self.input).map_err(|e| e.to_string())?;
        if input_bytes.len() > max_input_bytes {
            return Err("wasm input too large".to_string());
        }

        Ok(())
    }

    pub fn verify(&self) -> Result<VerifiedWasmSubmission, String> {
        if self.schema_version != FROGLET_SCHEMA_V1 {
            return Err(format!(
                "unsupported wasm submission schema_version: {}",
                self.schema_version
            ));
        }

        if self.submission_type != WASM_SUBMISSION_TYPE_V1 {
            return Err(format!(
                "unsupported wasm submission_type: {}",
                self.submission_type
            ));
        }

        let workload = &self.workload;
        if workload.schema_version != FROGLET_SCHEMA_V1 {
            return Err(format!(
                "unsupported wasm workload schema_version: {}",
                workload.schema_version
            ));
        }

        if workload.workload_kind != WORKLOAD_KIND_COMPUTE_WASM_V1 {
            return Err(format!(
                "unsupported wasm workload_kind: {}",
                workload.workload_kind
            ));
        }

        if workload.abi_version != WASM_RUN_JSON_ABI_V1
            && workload.abi_version != WASM_HOST_JSON_ABI_V1
        {
            return Err(format!(
                "unsupported wasm abi_version: {}",
                workload.abi_version
            ));
        }

        if workload.module_format != WASM_MODULE_FORMAT {
            return Err(format!(
                "unsupported wasm module_format: {}",
                workload.module_format
            ));
        }

        if workload.input_format != JCS_JSON_FORMAT {
            return Err(format!(
                "unsupported wasm input_format: {}",
                workload.input_format
            ));
        }

        let requested_capabilities =
            normalize_requested_capabilities(&workload.requested_capabilities)?;
        match workload.abi_version.as_str() {
            WASM_RUN_JSON_ABI_V1 if !requested_capabilities.is_empty() => {
                return Err(
                    "requested_capabilities are not supported by froglet.wasm.run_json.v1"
                        .to_string(),
                );
            }
            WASM_HOST_JSON_ABI_V1 => {
                validate_host_capability_dependencies(&requested_capabilities)?
            }
            _ => {}
        }

        let module_bytes =
            hex::decode(&self.module_bytes_hex).map_err(|_| "invalid hex encoding".to_string())?;
        let computed_module_hash = crypto::sha256_hex(&module_bytes);
        if computed_module_hash != workload.module_hash {
            return Err("module hash does not match module bytes".to_string());
        }

        let input_bytes = canonical_json::to_vec(&self.input).map_err(|e| e.to_string())?;
        let computed_input_hash = crypto::sha256_hex(&input_bytes);
        if computed_input_hash != workload.input_hash {
            return Err("input hash does not match canonical input".to_string());
        }

        Ok(VerifiedWasmSubmission {
            module_bytes,
            input: self.input.clone(),
            abi_version: workload.abi_version.clone(),
            requested_capabilities,
        })
    }
}

impl ComputeWasmWorkload {
    pub fn new(module_bytes: &[u8], input: &Value) -> Result<Self, String> {
        let input_bytes = canonical_json::to_vec(input).map_err(|e| e.to_string())?;
        Ok(Self {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            workload_kind: WORKLOAD_KIND_COMPUTE_WASM_V1.to_string(),
            abi_version: WASM_RUN_JSON_ABI_V1.to_string(),
            module_format: WASM_MODULE_FORMAT.to_string(),
            module_hash: crypto::sha256_hex(module_bytes),
            input_format: JCS_JSON_FORMAT.to_string(),
            input_hash: crypto::sha256_hex(input_bytes),
            requested_capabilities: Vec::new(),
        })
    }
}

fn default_input_value() -> Value {
    Value::Null
}

pub fn normalize_requested_capabilities(capabilities: &[String]) -> Result<Vec<String>, String> {
    let mut normalized = capabilities.to_vec();
    normalized.sort();
    normalized.dedup();

    for capability in &normalized {
        validate_capability_name(capability)?;
    }

    Ok(normalized)
}

fn validate_capability_name(capability: &str) -> Result<(), String> {
    let valid = capability == WASM_CAPABILITY_HTTP_FETCH
        || capability.starts_with(WASM_CAPABILITY_HTTP_FETCH_AUTH_PREFIX)
        || capability.starts_with(WASM_CAPABILITY_SQLITE_QUERY_READ_PREFIX);

    if !valid {
        return Err(format!("unsupported requested_capability: {capability}"));
    }

    for segment in capability.split('.') {
        if segment.is_empty()
            || !segment
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
        {
            return Err(format!("invalid requested_capability: {capability}"));
        }
    }

    Ok(())
}

fn validate_host_capability_dependencies(capabilities: &[String]) -> Result<(), String> {
    let has_http_fetch = capabilities
        .iter()
        .any(|capability| capability == WASM_CAPABILITY_HTTP_FETCH);

    for capability in capabilities {
        if capability.starts_with(WASM_CAPABILITY_HTTP_FETCH_AUTH_PREFIX) && !has_http_fetch {
            return Err(format!(
                "{capability} requires {WASM_CAPABILITY_HTTP_FETCH}"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const VALID_WASM_HEX: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";

    #[test]
    fn workload_hash_is_stable_across_input_key_order() {
        let module_bytes = hex::decode(VALID_WASM_HEX).unwrap();
        let first_input = json!({
            "b": 2,
            "a": 1
        });
        let second_input = json!({
            "a": 1,
            "b": 2
        });

        let first_submission = WasmSubmission {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            submission_type: WASM_SUBMISSION_TYPE_V1.to_string(),
            workload: ComputeWasmWorkload::new(&module_bytes, &first_input).unwrap(),
            module_bytes_hex: VALID_WASM_HEX.to_string(),
            input: first_input,
        };
        let second_submission = WasmSubmission {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            submission_type: WASM_SUBMISSION_TYPE_V1.to_string(),
            workload: ComputeWasmWorkload::new(&module_bytes, &second_input).unwrap(),
            module_bytes_hex: VALID_WASM_HEX.to_string(),
            input: second_input,
        };

        assert_eq!(
            first_submission.workload_hash().unwrap(),
            second_submission.workload_hash().unwrap()
        );
    }

    #[test]
    fn submission_verification_rejects_hash_mismatch() {
        let input = json!({"answer": 42});
        let submission = WasmSubmission {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            submission_type: WASM_SUBMISSION_TYPE_V1.to_string(),
            workload: ComputeWasmWorkload {
                schema_version: FROGLET_SCHEMA_V1.to_string(),
                workload_kind: WORKLOAD_KIND_COMPUTE_WASM_V1.to_string(),
                abi_version: WASM_RUN_JSON_ABI_V1.to_string(),
                module_format: WASM_MODULE_FORMAT.to_string(),
                module_hash: "00".repeat(32),
                input_format: JCS_JSON_FORMAT.to_string(),
                input_hash: "11".repeat(32),
                requested_capabilities: Vec::new(),
            },
            module_bytes_hex: VALID_WASM_HEX.to_string(),
            input,
        };

        assert!(submission.verify().is_err());
    }

    #[test]
    fn submission_verification_rejects_unsupported_abi_version() {
        let module_bytes = hex::decode(VALID_WASM_HEX).unwrap();
        let input = json!({"answer": 42});
        let mut submission = WasmSubmission {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            submission_type: WASM_SUBMISSION_TYPE_V1.to_string(),
            workload: ComputeWasmWorkload::new(&module_bytes, &input).unwrap(),
            module_bytes_hex: VALID_WASM_HEX.to_string(),
            input,
        };
        submission.workload.abi_version = "froglet.wasm.run_json.v0".to_string();

        let error = submission
            .verify()
            .expect_err("expected abi validation failure");
        assert!(error.contains("abi_version"), "unexpected error: {error}");
    }

    #[test]
    fn submission_verification_rejects_requested_capabilities() {
        let module_bytes = hex::decode(VALID_WASM_HEX).unwrap();
        let input = json!({"answer": 42});
        let mut submission = WasmSubmission {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            submission_type: WASM_SUBMISSION_TYPE_V1.to_string(),
            workload: ComputeWasmWorkload::new(&module_bytes, &input).unwrap(),
            module_bytes_hex: VALID_WASM_HEX.to_string(),
            input,
        };
        submission.workload.requested_capabilities = vec![WASM_CAPABILITY_HTTP_FETCH.to_string()];

        let error = submission
            .verify()
            .expect_err("expected capability validation failure");
        assert!(
            error.contains("requested_capabilities"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn submission_verification_rejects_input_hash_mismatch() {
        let module_bytes = hex::decode(VALID_WASM_HEX).unwrap();
        let input = json!({"answer": 42});
        let mut submission = WasmSubmission {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            submission_type: WASM_SUBMISSION_TYPE_V1.to_string(),
            workload: ComputeWasmWorkload::new(&module_bytes, &input).unwrap(),
            module_bytes_hex: VALID_WASM_HEX.to_string(),
            input,
        };
        submission.workload.input_hash = "22".repeat(32);

        let error = submission
            .verify()
            .expect_err("expected input hash validation failure");
        assert!(error.contains("input hash"), "unexpected error: {error}");
    }
}
