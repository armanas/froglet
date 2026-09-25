//! Native MCP bridge for clean-host agent installation.
//!
//! Publication delegates to the same canonical project loader and Publication
//! module as the CLI. The native adapter therefore adds no second manifest,
//! consent, or registration implementation and needs no Node.js runtime.

use super::CliError;
use super::invoke::{
    InvokeOptions, invoke_local_service, invoke_remote_service, resolve_runtime_auth_token,
};
use froglet_publish_engine::{DaemonClient, plan_publication, publish};
use reqwest::{Method, Response};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const DEFAULT_PROVIDER_URL: &str = "http://127.0.0.1:8080";
const DEFAULT_RUNTIME_URL: &str = "http://127.0.0.1:8081";
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_CONTROL_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_PUBLICATION_LOG_RESPONSE_BYTES: usize = 1024 * 1024;

pub async fn run(args: Vec<String>) -> Result<(), CliError> {
    match args.as_slice() {
        [] => serve_stdio().await,
        [flag] if flag == "--probe" => {
            let proof = local_proof().await?;
            let encoded = serde_json::to_string_pretty(&proof).map_err(|error| {
                CliError::Other(format!("failed to serialize native MCP proof: {error}"))
            })?;
            println!("{encoded}");
            Ok(())
        }
        _ => Err(CliError::BadArgs(
            "usage: froglet-node mcp [--probe]".to_string(),
        )),
    }
}

async fn serve_stdio() -> Result<(), CliError> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    let mut client_name: Option<String> = None;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_message(
                    &mut stdout,
                    &json_rpc_error(Value::Null, -32700, format!("invalid JSON: {error}")),
                )
                .await?;
                continue;
            }
        };
        if request.get("method").and_then(Value::as_str) == Some("initialize") {
            client_name = request
                .pointer("/params/clientInfo/name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty() && name.len() <= 120)
                .map(str::to_string);
        }
        if request.get("method").and_then(Value::as_str) == Some("tools/call")
            && let Some(name) = &client_name
        {
            let root = super::prepare::data_root();
            if root.is_dir() {
                let evidence = json!({"client_name":name, "process_id":std::process::id(), "observed_at":crate::settlement::current_unix_timestamp(), "evidence":"initialized_stdio_tool_call", "currently_connected":"not_proven"});
                if let Err(error) = super::prepare::atomic_private_write(
                    &root.join("agent-session.json"),
                    evidence.to_string().as_bytes(),
                ) {
                    eprintln!("could not record agent session: {error}");
                }
            }
        }
        if let Some(response) = handle_request(request).await {
            write_message(&mut stdout, &response).await?;
        }
    }
    Ok(())
}

async fn write_message(stdout: &mut tokio::io::Stdout, message: &Value) -> Result<(), CliError> {
    let mut encoded = serde_json::to_vec(message)
        .map_err(|error| CliError::Other(format!("MCP response serialization failed: {error}")))?;
    encoded.push(b'\n');
    stdout.write_all(&encoded).await?;
    stdout.flush().await?;
    Ok(())
}

async fn handle_request(request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");

    let id = id?;
    let result = match method {
        "initialize" => Ok(initialize_result(&request)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => handle_tool_call(&request).await,
        _ => {
            return Some(json_rpc_error(
                id,
                -32601,
                format!("unknown method {method:?}"),
            ));
        }
    };

    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json_rpc_error(id, -32602, error),
    })
}

fn initialize_result(_request: &Value) -> Value {
    // A server must report a protocol version it actually implements. Echoing
    // an arbitrary client version would claim compatibility that this narrow
    // native bridge has not established.
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "froglet-native",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [{
            "name": "froglet",
            "description": "Inspect, invoke, prove, publish, and operate publications through the locally installed Froglet node. marketplace_publish is a two-step plan/approval action for public hosting; lifecycle actions delegate to the node's canonical provider-control API.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "status",
                            "prepare_service",
                            "check_updates",
                            "doctor",
                            "open_status",
                            "invoke_service",
                            "local_proof",
                            "marketplace_publish",
                            "publication_status",
                            "publication_logs",
                            "publication_pause",
                            "publication_resume",
                            "publication_rollback",
                            "publication_unpublish",
                            "managed_operation_status",
                            "managed_operation_reconcile",
                            "managed_operation_compensate"
                        ]
                    },
                    "service_id": {
                        "type": "string",
                        "description": "Canonical local publication service identifier. Required for invoke_service and publication lifecycle actions."
                    },
                    "input": {},
                    "source": {"type":"string", "description":"Absolute path to JSON, CSV, SQLite, WAT or Wasm source. Preparation only."},
                    "destination": {"type":"string", "description":"Explicit absolute directory for generated service material; existing unrelated projects are never overwritten."},
                    "selection": {"type":"object", "additionalProperties":{"type":"array", "items":{"type":"string"}}, "description":"Source collection names mapped to the fields to include. JSON arrays and CSV use rows."},
                    "csv_columns": {"type":"array", "items":{"type":"object", "properties":{"name":{"type":"string"},"type":{"enum":["string","integer","number","boolean"]},"nullable":{"type":"boolean"},"indexed":{"type":"boolean"}}, "required":["name","type"], "additionalProperties":false}},
                    "example_input": {"description":"A meaningful local example. Required for Wasm; a selected-row lookup is generated for data."},
                    "provider_id": {"type":"string", "pattern":"^[0-9a-f]{64}$", "description":"Provider identity from a shared service link. Selects remote invocation."},
                    "idempotency_key": {"type":"string", "description":"Reuse the same key and input to reconcile an uncertain invocation; use a new key for a new call."},
                    "provider_url": {"type":"string", "description":"Public HTTPS origin from a shared service link. Requires provider_id."},
                    "revision_hash": {
                        "type": "string",
                        "pattern": "^[0-9a-f]{64}$",
                        "description": "Exact immutable target revision. Required for publication_rollback."
                    },
                    "confirm_service_id": {
                        "type": "string",
                        "description": "Must exactly equal service_id for publication_unpublish. This prevents an inferred or stale destructive action."
                    },
                    "operation_id": {
                        "type": "string",
                        "pattern": "^[0-9a-f]{64}$",
                        "description": "Exact durable managed-publication operation identifier. Required for managed_operation actions."
                    },
                    "confirm_operation_id": {
                        "type": "string",
                        "pattern": "^[0-9a-f]{64}$",
                        "description": "Must exactly equal operation_id for managed reconciliation or compensation, because either action may compensate unproven external resources."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Maximum lifecycle records returned by publication_logs; defaults to 50."
                    },
                    "project_dir": {
                        "type": "string",
                        "description": "Directory containing froglet-service.toml and its source or data file. Required for marketplace_publish."
                    },
                    "host": {
                        "type": "string",
                        "enum": ["local", "relay", "tor", "self"],
                        "description": "Optional hosting override. Relay is the dependency-free public default."
                    },
                    "marketplace_url": {
                        "type": "string",
                        "description": "Optional public marketplace URL override."
                    },
                    "consent_hash": {
                        "type": "string",
                        "pattern": "^[0-9a-f]{64}$",
                        "description": "Exact hash returned by the first non-mutating marketplace_publish call. Supply only after user approval."
                    }
                },
                "required": ["action"]
            }
        }]
    })
}

async fn handle_tool_call(request: &Value) -> Result<Value, String> {
    let params = request
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| "tools/call params must be an object".to_string())?;
    if params.get("name").and_then(Value::as_str) != Some("froglet") {
        return Err("native bridge exposes only the froglet tool".to_string());
    }
    let arguments = params
        .get("arguments")
        .and_then(Value::as_object)
        .ok_or_else(|| "froglet arguments must be an object".to_string())?;
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| "froglet action is required".to_string())?;

    let payload = match action {
        "status" => status_snapshot().await,
        "prepare_service" => {
            let mut request = arguments.clone();
            request.remove("action");
            let request = serde_json::from_value(Value::Object(request))
                .map_err(|error| format!("invalid prepare_service request: {error}"))?;
            super::prepare::prepare(request, &super::prepare::data_root()).await
        }
        "check_updates" => super::prepare::check_updates(&super::prepare::data_root()),
        "doctor" => Ok(super::doctor::snapshot(&super::prepare::data_root()).await),
        "open_status" => crate::local_status::open(&super::prepare::data_root()).await,
        "local_proof" => local_proof().await,
        "invoke_service" => {
            let service_id = arguments
                .get("service_id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "invoke_service requires service_id".to_string())?;
            invoke_selected(
                service_id,
                arguments.get("input").cloned().unwrap_or(Value::Null),
                invoke_string(arguments, "provider_id")?,
                invoke_string(arguments, "provider_url")?,
                invoke_string(arguments, "idempotency_key")?,
            )
            .await
        }
        "marketplace_publish" => marketplace_publish(arguments).await,
        "publication_status"
        | "publication_logs"
        | "publication_pause"
        | "publication_resume"
        | "publication_rollback"
        | "publication_unpublish" => publication_control(action, arguments).await,
        "managed_operation_status"
        | "managed_operation_reconcile"
        | "managed_operation_compensate" => managed_operation_control(action, arguments).await,
        _ => return Err(format!("unsupported native action {action:?}")),
    };

    match payload {
        Ok(payload) => tool_result(payload, false),
        Err(error) => tool_result(super::doctor::error_report(&error), true),
    }
}

async fn managed_operation_control(
    action: &str,
    arguments: &serde_json::Map<String, Value>,
) -> Result<Value, CliError> {
    let daemon = DaemonClient::from_env().map_err(CliError::Engine)?;
    managed_operation_control_with_client(action, arguments, &daemon).await
}

async fn managed_operation_control_with_client(
    action: &str,
    arguments: &serde_json::Map<String, Value>,
    daemon: &DaemonClient,
) -> Result<Value, CliError> {
    let operation_id = required_lowercase_hash(arguments, "operation_id", action)?;
    let method = match action {
        "managed_operation_status" => Method::GET,
        "managed_operation_reconcile" | "managed_operation_compensate" => {
            let confirmation = required_lowercase_hash(arguments, "confirm_operation_id", action)?;
            if confirmation != operation_id {
                return Err(CliError::BadArgs(
                    "confirm_operation_id must exactly equal operation_id".to_string(),
                ));
            }
            Method::POST
        }
        _ => {
            return Err(CliError::BadArgs(format!(
                "unsupported managed operation action {action:?}"
            )));
        }
    };
    let suffix = match action {
        "managed_operation_status" => "",
        "managed_operation_reconcile" => "/reconcile",
        "managed_operation_compensate" => "/compensate",
        _ => unreachable!("validated managed operation action"),
    };
    let mut url = daemon.daemon_url.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.set_path(&format!(
        "/v1/provider/managed-publications/operations/{operation_id}{suffix}"
    ));

    let token = daemon
        .control_auth
        .resolve()
        .await
        .map_err(CliError::Engine)?;
    if token.is_empty() {
        return Err(CliError::Other(
            "provider-control token is empty; restart the daemon or configure its token path"
                .to_string(),
        ));
    }
    let client = crate::tls::reqwest_client_builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| CliError::Other(format!("failed to build HTTP client: {error}")))?;
    let response = client
        .request(method, url.clone())
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| CliError::Daemon(format!("{action} request to {url} failed: {error}")))?;
    read_control_response(action, response, MAX_CONTROL_RESPONSE_BYTES).await
}

fn required_lowercase_hash<'a>(
    arguments: &'a serde_json::Map<String, Value>,
    field: &'static str,
    action: &str,
) -> Result<&'a str, CliError> {
    let value = arguments
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::BadArgs(format!("{action} requires {field}")))?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(CliError::BadArgs(format!(
            "{field} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(value)
}

async fn publication_control(
    action: &str,
    arguments: &serde_json::Map<String, Value>,
) -> Result<Value, CliError> {
    let daemon = DaemonClient::from_env().map_err(CliError::Engine)?;
    publication_control_with_client(action, arguments, &daemon).await
}

async fn publication_control_with_client(
    action: &str,
    arguments: &serde_json::Map<String, Value>,
    daemon: &DaemonClient,
) -> Result<Value, CliError> {
    let service_id = required_publication_id(arguments, "service_id")?;
    let (method, mut path, query, response_limit) = match action {
        "publication_status" => (
            Method::GET,
            vec!["v1", "provider", "publications", service_id],
            None,
            MAX_CONTROL_RESPONSE_BYTES,
        ),
        "publication_logs" => {
            let limit = publication_log_limit(arguments)?;
            (
                Method::GET,
                vec!["v1", "provider", "publications", service_id, "logs"],
                Some(("limit", limit.to_string())),
                MAX_PUBLICATION_LOG_RESPONSE_BYTES,
            )
        }
        "publication_pause" => (
            Method::POST,
            vec!["v1", "provider", "publications", service_id, "pause"],
            None,
            MAX_CONTROL_RESPONSE_BYTES,
        ),
        "publication_resume" => (
            Method::POST,
            vec!["v1", "provider", "publications", service_id, "resume"],
            None,
            MAX_CONTROL_RESPONSE_BYTES,
        ),
        "publication_rollback" => {
            let revision_hash = required_revision_hash(arguments)?;
            (
                Method::POST,
                vec![
                    "v1",
                    "provider",
                    "publications",
                    service_id,
                    "rollback",
                    revision_hash,
                ],
                None,
                MAX_CONTROL_RESPONSE_BYTES,
            )
        }
        "publication_unpublish" => {
            let confirmation = arguments
                .get("confirm_service_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    CliError::BadArgs(
                        "publication_unpublish requires confirm_service_id exactly equal to service_id"
                            .to_string(),
                    )
                })?;
            if confirmation != service_id {
                return Err(CliError::BadArgs(
                    "publication_unpublish confirm_service_id must exactly equal service_id"
                        .to_string(),
                ));
            }
            (
                Method::POST,
                vec!["v1", "provider", "publications", service_id, "unpublish"],
                None,
                MAX_CONTROL_RESPONSE_BYTES,
            )
        }
        _ => {
            return Err(CliError::BadArgs(format!(
                "unsupported publication lifecycle action {action:?}"
            )));
        }
    };
    let mut url = daemon.daemon_url.clone();
    url.set_query(None);
    url.set_fragment(None);
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            CliError::BadArgs("FROGLET_DAEMON_URL cannot be used as a URL base".to_string())
        })?;
        segments.clear();
        for segment in path.drain(..) {
            segments.push(segment);
        }
    }
    if let Some((name, value)) = query {
        url.query_pairs_mut().append_pair(name, &value);
    }

    let token = daemon
        .control_auth
        .resolve()
        .await
        .map_err(CliError::Engine)?;
    if token.is_empty() {
        return Err(CliError::Other(
            "provider-control token is empty; restart the daemon or configure its token path"
                .to_string(),
        ));
    }
    let client = crate::tls::reqwest_client_builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| CliError::Other(format!("failed to build HTTP client: {error}")))?;
    let response = client
        .request(method, url.clone())
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| CliError::Daemon(format!("{action} request to {url} failed: {error}")))?;
    read_control_response(action, response, response_limit).await
}

fn required_publication_id<'a>(
    arguments: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, CliError> {
    let value = arguments
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CliError::BadArgs(format!("publication action requires {field}")))?;
    if value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(CliError::BadArgs(format!(
            "{field} must contain 1-128 ASCII letters, digits, dots, underscores, or hyphens"
        )));
    }
    Ok(value)
}

fn required_revision_hash(arguments: &serde_json::Map<String, Value>) -> Result<&str, CliError> {
    let value = arguments
        .get("revision_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CliError::BadArgs("publication_rollback requires revision_hash".to_string())
        })?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(CliError::BadArgs(
            "revision_hash must be 64 lowercase hexadecimal characters".to_string(),
        ));
    }
    Ok(value)
}

fn publication_log_limit(arguments: &serde_json::Map<String, Value>) -> Result<u64, CliError> {
    match arguments.get("limit") {
        None => Ok(50),
        Some(Value::Number(value)) => value
            .as_u64()
            .filter(|value| (1..=100).contains(value))
            .ok_or_else(|| {
                CliError::BadArgs(
                    "publication_logs limit must be an integer from 1 to 100".to_string(),
                )
            }),
        Some(_) => Err(CliError::BadArgs(
            "publication_logs limit must be an integer from 1 to 100".to_string(),
        )),
    }
}

async fn read_control_response(
    action: &str,
    mut response: Response,
    max_bytes: usize,
) -> Result<Value, CliError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(CliError::Daemon(format!(
            "{action} response exceeds the {max_bytes}-byte limit"
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| CliError::Daemon(format!("{action} response read failed: {error}")))?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CliError::Daemon(format!(
                "{action} response exceeds the {max_bytes}-byte limit"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    let payload = serde_json::from_slice::<Value>(&body).map_err(|error| {
        CliError::Daemon(format!(
            "{action} returned HTTP {status} with invalid JSON: {error}"
        ))
    })?;
    if !status.is_success() {
        let detail = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("provider-control request failed");
        return Err(CliError::Daemon(format!(
            "{action} returned HTTP {status}: {detail}"
        )));
    }
    Ok(payload)
}

async fn marketplace_publish(
    arguments: &serde_json::Map<String, Value>,
) -> Result<Value, CliError> {
    let project_dir = arguments
        .get("project_dir")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CliError::BadArgs(
                "marketplace_publish requires project_dir containing froglet-service.toml"
                    .to_string(),
            )
        })?;
    let project_dir = std::fs::canonicalize(project_dir).map_err(|error| {
        CliError::BadArgs(format!(
            "marketplace_publish project_dir {project_dir:?} is not readable: {error}"
        ))
    })?;
    if !project_dir.is_dir() {
        return Err(CliError::BadArgs(format!(
            "marketplace_publish project_dir {:?} is not a directory",
            project_dir
        )));
    }

    let host = optional_string(arguments, "host")?;
    let marketplace_url = optional_string(arguments, "marketplace_url")?;
    let consent_hash = optional_string(arguments, "consent_hash")?;
    if consent_hash.as_deref().is_some_and(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(CliError::BadArgs(
            "consent_hash must be a 64-character lowercase hexadecimal hash".to_string(),
        ));
    }

    let input = super::publish::load_publish_input(
        &project_dir,
        host.as_deref(),
        marketplace_url.as_deref(),
        consent_hash.clone(),
    )
    .await?;
    let daemon = DaemonClient::from_env().map_err(CliError::Engine)?;
    if consent_hash.is_none() {
        let plan = plan_publication(&input, &daemon)
            .await
            .map_err(CliError::Engine)?;
        if plan.status == "approval_required" {
            return serde_json::to_value(plan).map_err(|error| {
                CliError::Other(format!("serialize native publication consent: {error}"))
            });
        }
    }
    let output = publish(input, &daemon).await.map_err(CliError::Engine)?;
    super::publish::remember_publication(&output);
    serde_json::to_value(output)
        .map_err(|error| CliError::Other(format!("serialize native publication result: {error}")))
}

fn optional_string(
    arguments: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, CliError> {
    match arguments.get(field) {
        None => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => {
            Ok(Some(value.trim().to_string()))
        }
        Some(_) => Err(CliError::BadArgs(format!(
            "marketplace_publish {field} must be a non-empty string when supplied"
        ))),
    }
}

fn tool_result(payload: Value, is_error: bool) -> Result<Value, String> {
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("failed to serialize native MCP payload: {error}"))?;
    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": payload,
        "isError": is_error
    }))
}

fn json_rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message}
    })
}

async fn status_snapshot() -> Result<Value, CliError> {
    let provider_url = base_url("FROGLET_PROVIDER_URL", DEFAULT_PROVIDER_URL);
    let runtime_url = base_url("FROGLET_RUNTIME_URL", DEFAULT_RUNTIME_URL);
    let client = crate::tls::reqwest_client_builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| CliError::Other(format!("failed to build HTTP client: {error}")))?;
    check_health(&client, "provider", &provider_url).await?;
    check_health(&client, "runtime", &runtime_url).await?;

    Ok(json!({
        "status": "ok",
        "release": std::env::var("FROGLET_RELEASE").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string()),
        "state_path": data_dir().display().to_string(),
        "provider_url": provider_url,
        "runtime_url": runtime_url,
        "native_mcp": true,
        "source_updates": super::prepare::check_updates(&super::prepare::data_root()).unwrap_or_else(|e| super::doctor::error_report(&e)),
        "next_action": "Call doctor for component health or open_status for the local read-only page"
    }))
}

async fn check_health(
    client: &reqwest::Client,
    label: &str,
    base_url: &str,
) -> Result<(), CliError> {
    let url = format!("{base_url}/health");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| CliError::Daemon(format!("{label} health request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(CliError::Daemon(format!(
            "{label} health returned HTTP {} at {url}",
            response.status()
        )));
    }
    Ok(())
}

async fn local_proof() -> Result<Value, CliError> {
    let status = status_snapshot().await?;
    let invocation = invoke("demo.add", json!({"a": 20, "b": 22})).await?;
    if invocation.pointer("/result/sum").and_then(Value::as_i64) != Some(42) {
        return Err(CliError::Other(format!(
            "demo.add local proof returned an unexpected result: {invocation}"
        )));
    }
    Ok(json!({
        "status": "ok",
        "node": status,
        "proof": {
            "service_id": "demo.add",
            "input": {"a": 20, "b": 22},
            "expected_sum": 42,
            "invocation": invocation
        }
    }))
}

fn invoke_string<'a>(
    arguments: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match arguments.get(key) {
        None => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.as_str())),
        _ => Err(format!("{key} must be a non-empty string")),
    }
}

async fn invoke(service_id: &str, input: Value) -> Result<Value, CliError> {
    invoke_selected(service_id, input, None, None, None).await
}

async fn invoke_selected(
    service_id: &str,
    input: Value,
    provider_id: Option<&str>,
    provider_url: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<Value, CliError> {
    let options = InvokeOptions {
        service_id: service_id.to_string(),
        input,
        daemon_url: base_url("FROGLET_PROVIDER_URL", DEFAULT_PROVIDER_URL),
        runtime_url: base_url("FROGLET_RUNTIME_URL", DEFAULT_RUNTIME_URL),
        runtime_token: resolve_runtime_auth_token().await?,
        provider_id_override: provider_id.map(str::to_string),
        idempotency_key: idempotency_key.map(str::to_string),
        wait_timeout: Duration::from_secs(60),
        poll_interval: Duration::from_millis(250),
    };
    let report = if provider_id.is_some() || provider_url.is_some() {
        invoke_remote_service(&options, provider_url).await?
    } else {
        invoke_local_service(&options).await?
    };
    if report.terminal && !matches!(report.status.as_str(), "succeeded" | "completed" | "done") {
        return Err(CliError::Other(format!(
            "invocation {} ended in status {:?}: {}",
            report.deal_id,
            report.status,
            report.error.as_deref().unwrap_or("no error detail")
        )));
    }
    serde_json::to_value(report)
        .map_err(|error| CliError::Other(format!("invoke report serialization failed: {error}")))
}

fn base_url(variable: &str, default: &str) -> String {
    std::env::var(variable)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn data_dir() -> PathBuf {
    std::env::var_os("FROGLET_DATA_ROOT")
        .or_else(|| std::env::var_os("FROGLET_DATA_DIR"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".froglet/data")))
        .unwrap_or_else(|| PathBuf::from("./data"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Path, Query},
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use froglet_publish_engine::ControlAuth;
    use std::collections::HashMap;
    use tokio::net::TcpListener;
    use url::Url;

    #[tokio::test]
    async fn initialize_and_tools_list_are_shape_stable() {
        let initialized = handle_request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2025-06-18"}
        }))
        .await
        .expect("initialize response");
        assert_eq!(
            initialized.pointer("/result/serverInfo/name"),
            Some(&json!("froglet-native"))
        );
        assert_eq!(
            initialized.pointer("/result/protocolVersion"),
            Some(&json!(MCP_PROTOCOL_VERSION))
        );

        let future_version = handle_request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "initialize",
            "params": {"protocolVersion": "2099-01-01"}
        }))
        .await
        .expect("future initialize response");
        assert_eq!(
            future_version.pointer("/result/protocolVersion"),
            Some(&json!(MCP_PROTOCOL_VERSION))
        );

        let listed = handle_request(json!({
            "jsonrpc": "2.0",
            "id": "tools",
            "method": "tools/list",
            "params": {}
        }))
        .await
        .expect("tools/list response");
        assert_eq!(
            listed.pointer("/result/tools/0/name"),
            Some(&json!("froglet"))
        );
        let actions = listed
            .pointer("/result/tools/0/inputSchema/properties/action/enum")
            .and_then(Value::as_array)
            .expect("action enum");
        assert_eq!(actions.len(), 17);
        for action in ["prepare_service", "doctor", "check_updates", "open_status"] {
            assert!(actions.iter().any(|value| value == action));
        }
        assert!(actions.iter().any(|action| action == "marketplace_publish"));
        assert!(actions.iter().any(|action| action == "publication_logs"));
        assert!(
            actions
                .iter()
                .any(|action| action == "publication_unpublish")
        );
        assert!(
            actions
                .iter()
                .any(|action| action == "managed_operation_status")
        );
        assert!(
            actions
                .iter()
                .any(|action| action == "managed_operation_compensate")
        );
    }

    #[tokio::test]
    async fn notifications_do_not_receive_responses() {
        assert!(
            handle_request(json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .await
            .is_none()
        );
    }

    fn arguments(value: Value) -> serde_json::Map<String, Value> {
        value.as_object().expect("arguments object").clone()
    }

    async fn test_daemon(app: Router) -> (DaemonClient, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let daemon = DaemonClient::new(
            Url::parse(&format!("http://{address}")).unwrap(),
            ControlAuth::Value("provider-secret".to_string()),
        )
        .unwrap();
        (daemon, task)
    }

    fn assert_control_auth(headers: &HeaderMap) {
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer provider-secret")
        );
    }

    #[tokio::test]
    async fn publication_lifecycle_actions_use_exact_control_paths() {
        async fn status(headers: HeaderMap) -> Json<Value> {
            assert_control_auth(&headers);
            Json(json!({
                "publication": {
                    "service_id": "demo.service",
                    "status": "active",
                    "selected_revision_hash": "a".repeat(64)
                }
            }))
        }
        async fn logs(
            headers: HeaderMap,
            Query(query): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            assert_control_auth(&headers);
            assert_eq!(query.get("limit").map(String::as_str), Some("7"));
            Json(json!({"operations": [{"operation": "publish"}]}))
        }
        async fn rollback(headers: HeaderMap, Path(revision_hash): Path<String>) -> Json<Value> {
            assert_control_auth(&headers);
            assert_eq!(revision_hash, "b".repeat(64));
            Json(json!({"operation": "rollback"}))
        }
        async fn unpublish(headers: HeaderMap) -> Json<Value> {
            assert_control_auth(&headers);
            Json(json!({"operation": "unpublish"}))
        }

        let app = Router::new()
            .route("/v1/provider/publications/demo.service", get(status))
            .route("/v1/provider/publications/demo.service/logs", get(logs))
            .route(
                "/v1/provider/publications/demo.service/rollback/:revision_hash",
                post(rollback),
            )
            .route(
                "/v1/provider/publications/demo.service/unpublish",
                post(unpublish),
            );
        let (daemon, task) = test_daemon(app).await;

        let status = publication_control_with_client(
            "publication_status",
            &arguments(json!({"service_id": "demo.service"})),
            &daemon,
        )
        .await
        .unwrap();
        assert_eq!(
            status.pointer("/publication/status"),
            Some(&json!("active"))
        );
        let logs = publication_control_with_client(
            "publication_logs",
            &arguments(json!({"service_id": "demo.service", "limit": 7})),
            &daemon,
        )
        .await
        .unwrap();
        assert_eq!(logs["operations"][0]["operation"], "publish");
        let rollback = publication_control_with_client(
            "publication_rollback",
            &arguments(json!({
                "service_id": "demo.service",
                "revision_hash": "b".repeat(64)
            })),
            &daemon,
        )
        .await
        .unwrap();
        assert_eq!(rollback["operation"], "rollback");
        let unpublish = publication_control_with_client(
            "publication_unpublish",
            &arguments(json!({
                "service_id": "demo.service",
                "confirm_service_id": "demo.service"
            })),
            &daemon,
        )
        .await
        .unwrap();
        assert_eq!(unpublish["operation"], "unpublish");
        task.abort();
    }

    #[tokio::test]
    async fn unpublish_requires_exact_confirmation_before_http() {
        let daemon = DaemonClient::new(
            Url::parse("http://127.0.0.1:9").unwrap(),
            ControlAuth::Value("provider-secret".to_string()),
        )
        .unwrap();
        let missing = publication_control_with_client(
            "publication_unpublish",
            &arguments(json!({"service_id": "demo.service"})),
            &daemon,
        )
        .await
        .unwrap_err();
        assert!(missing.to_string().contains("confirm_service_id"));
        let mismatched = publication_control_with_client(
            "publication_unpublish",
            &arguments(json!({
                "service_id": "demo.service",
                "confirm_service_id": "other.service"
            })),
            &daemon,
        )
        .await
        .unwrap_err();
        assert!(mismatched.to_string().contains("exactly equal"));
    }

    #[tokio::test]
    async fn managed_operation_actions_use_exact_control_paths_and_confirmation() {
        async fn status(headers: HeaderMap, Path(operation_id): Path<String>) -> Json<Value> {
            assert_control_auth(&headers);
            Json(json!({"operation": {"operation_id": operation_id, "phase": "active"}}))
        }
        async fn reconcile(headers: HeaderMap, Path(operation_id): Path<String>) -> Json<Value> {
            assert_control_auth(&headers);
            Json(json!({"operation": {"operation_id": operation_id, "phase": "deployed"}}))
        }
        async fn compensate(headers: HeaderMap, Path(operation_id): Path<String>) -> Json<Value> {
            assert_control_auth(&headers);
            Json(json!({"operation": {"operation_id": operation_id, "phase": "compensated"}}))
        }

        let operation_id = "c".repeat(64);
        let app = Router::new()
            .route(
                "/v1/provider/managed-publications/operations/:operation_id",
                get(status),
            )
            .route(
                "/v1/provider/managed-publications/operations/:operation_id/reconcile",
                post(reconcile),
            )
            .route(
                "/v1/provider/managed-publications/operations/:operation_id/compensate",
                post(compensate),
            );
        let (daemon, task) = test_daemon(app).await;

        let status = managed_operation_control_with_client(
            "managed_operation_status",
            &arguments(json!({"operation_id": operation_id})),
            &daemon,
        )
        .await
        .unwrap();
        assert_eq!(status.pointer("/operation/phase"), Some(&json!("active")));

        let reconcile = managed_operation_control_with_client(
            "managed_operation_reconcile",
            &arguments(json!({
                "operation_id": operation_id,
                "confirm_operation_id": operation_id
            })),
            &daemon,
        )
        .await
        .unwrap();
        assert_eq!(
            reconcile.pointer("/operation/phase"),
            Some(&json!("deployed"))
        );

        let compensate = managed_operation_control_with_client(
            "managed_operation_compensate",
            &arguments(json!({
                "operation_id": operation_id,
                "confirm_operation_id": operation_id
            })),
            &daemon,
        )
        .await
        .unwrap();
        assert_eq!(
            compensate.pointer("/operation/phase"),
            Some(&json!("compensated"))
        );
        task.abort();
    }

    #[tokio::test]
    async fn managed_operation_mutation_requires_exact_confirmation_before_http() {
        let daemon = DaemonClient::new(
            Url::parse("http://127.0.0.1:9").unwrap(),
            ControlAuth::Value("provider-secret".to_string()),
        )
        .unwrap();
        let operation_id = "d".repeat(64);
        let missing = managed_operation_control_with_client(
            "managed_operation_reconcile",
            &arguments(json!({"operation_id": operation_id})),
            &daemon,
        )
        .await
        .unwrap_err();
        assert!(missing.to_string().contains("confirm_operation_id"));

        let mismatched = managed_operation_control_with_client(
            "managed_operation_compensate",
            &arguments(json!({
                "operation_id": operation_id,
                "confirm_operation_id": "e".repeat(64)
            })),
            &daemon,
        )
        .await
        .unwrap_err();
        assert!(mismatched.to_string().contains("exactly equal"));
    }

    #[tokio::test]
    async fn publication_control_bounds_responses_and_sanitizes_http_errors() {
        async fn oversized() -> Json<Value> {
            Json(json!({"operations": ["x".repeat(MAX_PUBLICATION_LOG_RESPONSE_BYTES)]}))
        }
        async fn conflict() -> (StatusCode, Json<Value>) {
            (
                StatusCode::CONFLICT,
                Json(json!({"error": "publication cannot be resumed"})),
            )
        }
        let app = Router::new()
            .route(
                "/v1/provider/publications/demo.service/logs",
                get(oversized),
            )
            .route(
                "/v1/provider/publications/demo.service/resume",
                post(conflict),
            );
        let (daemon, task) = test_daemon(app).await;

        let oversized = publication_control_with_client(
            "publication_logs",
            &arguments(json!({"service_id": "demo.service"})),
            &daemon,
        )
        .await
        .unwrap_err();
        assert!(oversized.to_string().contains("response exceeds"));
        let conflict = publication_control_with_client(
            "publication_resume",
            &arguments(json!({"service_id": "demo.service"})),
            &daemon,
        )
        .await
        .unwrap_err();
        let error = conflict.to_string();
        assert!(error.contains("409 Conflict"));
        assert!(error.contains("publication cannot be resumed"));
        assert!(!error.contains("provider-secret"));
        task.abort();
    }
}
