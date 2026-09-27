//! Prepare an ordinary Wasm project for one approved HTTP JSON operation.
//! No upstream call or publication happens during preparation.
use super::CliError;
use crate::http_operation::HttpOperation;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareHttpService {
    pub destination: PathBuf,
    pub service_id: String,
    pub summary: String,
    pub operation: HttpOperation,
    pub example_input: Value,
}

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    super::pop_flag(&mut args, "--json");
    let path = super::pop_kv(&mut args, "--request")
        .ok_or_else(|| bad("prepare-http-service --request FILE [--json]"))?;
    if !args.is_empty() {
        return Err(bad("unexpected prepare-http-service arguments"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 128 * 1024 {
        return Err(bad("HTTP preparation request exceeds 128KiB"));
    }
    let request = serde_json::from_slice(&bytes)
        .map_err(|e| bad(format!("invalid HTTP preparation request: {e}")))?;
    println!("{}", prepare(request)?);
    Ok(())
}
fn bad(message: impl Into<String>) -> CliError {
    CliError::BadArgs(message.into())
}

pub fn prepare(request: PrepareHttpService) -> Result<Value, CliError> {
    request.operation.validate().map_err(bad)?;
    request
        .operation
        .validate_input(&request.example_input)
        .map_err(bad)?;
    if !request.destination.is_absolute() || request.destination.exists() {
        return Err(bad("destination must be a new absolute project directory"));
    }
    if request.summary.trim().is_empty()
        || request.summary.len() > 500
        || request.summary.chars().any(char::is_control)
    {
        return Err(bad("summary must contain 1–500 non-control characters"));
    }
    let module = crate::http_operation::module(&request.operation).map_err(bad)?;
    let module_hash = crate::crypto::sha256_hex(&module);
    let operation_hash = request.operation.hash().map_err(bad)?;
    let manifest = json!({
        "schema_version":"froglet-service/v4","service_id":request.service_id,"summary":request.summary,
        "runtime":"wasm","package_kind":"inline_module","entrypoint":"operation.wasm","entrypoint_kind":"module",
        "contract_version":crate::wasm::WASM_HOST_JSON_ABI_V1,"capabilities":request.operation.capabilities().map_err(bad)?,
        "input_schema_json":serde_json::to_string(&request.operation.input_schema).map_err(|e|bad(e.to_string()))?,
        "output_schema_json":serde_json::to_string(&request.operation.output_schema).map_err(|e|bad(e.to_string()))?,
        "starter":serde_json::to_string(&request.example_input).map_err(|e|bad(e.to_string()))?,
        "hosting":{"default":"local"},"settlement":{"method":"none"},"price":{"sats":0},
        "limits":{"max_input_bytes":request.operation.max_request_bytes,"max_output_bytes":request.operation.max_response_bytes,"max_runtime_ms":request.operation.timeout_ms,"max_memory_bytes":8388608},
        "verification":{"input":request.example_input}
    });
    let text = toml::to_string_pretty(&manifest).map_err(|e| bad(e.to_string()))?;
    froglet_protocol::manifest::ServiceManifest::from_toml(&text)?;
    let parent = request
        .destination
        .parent()
        .ok_or_else(|| bad("destination has no parent"))?;
    if !parent.is_dir() {
        return Err(bad("destination parent must exist"));
    }
    let staging = tempfile::Builder::new()
        .prefix(".froglet-http-")
        .tempdir_in(parent)?;
    super::prepare::atomic_private_write(&staging.path().join("operation.wasm"), &module)?;
    super::prepare::atomic_private_write(
        &staging.path().join("froglet-service.toml"),
        text.as_bytes(),
    )?;
    super::prepare::atomic_private_write(
        &staging.path().join("operation.json"),
        serde_json::to_vec_pretty(&request.operation)
            .map_err(|e| bad(e.to_string()))?
            .as_slice(),
    )?;
    // Reserve the final directory exclusively; concurrent preparation cannot
    // replace an existing service or its source.
    std::fs::create_dir(&request.destination)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&request.destination, std::fs::Permissions::from_mode(0o700))?;
    }
    for name in ["operation.wasm", "froglet-service.toml", "operation.json"] {
        std::fs::rename(staging.path().join(name), request.destination.join(name))?;
    }
    let endpoint = reqwest::Url::parse(&request.operation.url).map_err(|e| bad(e.to_string()))?;
    Ok(
        json!({"status":"prepared","project_dir":request.destination,"module_sha256":module_hash,"operation_hash":operation_hash,
        "operation":request.operation,"example_input":request.example_input,"upstream_called":false,"local_example_verified":false,
        "operator_requirements":{"http_operations_only":true,"operation_hashes":[operation_hash],"allowed_host":endpoint.host_str(),"auth_profile":"configure the referenced credential privately on the provider; never put it in this project"},
        "next_action":"Configure the exact operation hash and host in the provider Wasm HTTP policy, with operations_only=true and finite admission allowances. Restart to load it. Use marketplace_publish with project_dir and host=local to verify and publish on this provider. Verification makes one upstream call; preparation does not. Private/invite providers use direct authenticated invocation over their configured public HTTPS endpoint; public marketplace activation currently requires anonymous canaries and is a separate step.",
        "public_disclosure":"endpoint, fixed request fields, schemas and example are in the approved package; no credentials belong here"}),
    )
}
