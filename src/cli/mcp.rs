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
    let mut schema = json!({
        "tools": [{
            "name": "froglet",
            "description": "Inspect, invoke, compute, prove, publish, and operate publications through the locally installed Froglet node. run_compute accepts one bounded Wasm v1 JSON program as hex or an absolute local file path with an explicit idempotency_key; execution limits come from the provider's signed Quote. Paid work requires an explicit max_price_sats and the existing requester wallet/budget. get_task reads an existing deal without resubmitting or paying. marketplace_publish is a two-step plan/approval action for public hosting; lifecycle actions delegate to the node's canonical provider-control API.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "status",
                            "safeguards_status", "safeguards_pause", "safeguards_resume",
                            "invite_create", "invite_list", "invite_revoke",
                            "prepare_service", "prepare_http_service",
                            "check_updates",
                            "doctor",
                            "open_status",
                            "inspect_service", "download_file", "file_abort",
                            "invoke_service",
                            "run_compute", "get_task",
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
                        "description": "Service identifier; invoke_service accepts service_url instead. Required for publication lifecycle actions."
                    },
                    "max_price_sats": {"type":"integer", "minimum":0, "description":"Explicit per-call Lightning price ceiling for invoke_service/run_compute; defaults to 0 (free only). Paid calls also require a buyer wallet and runtime cumulative spend budget."},
                    "service_url": {"type":"string", "description":"Froglet share URL. inspect_service verifies public metadata without executing; invoke_service resolves the same URL. Cannot be combined with service_id or provider overrides."},
                    "response_format": {"enum":["full","compact"], "description":"full (default) includes JSON text for older clients. compact avoids duplicating structured results in text for inspect_service, invoke_service, run_compute and get_task."},
                    "summary": {"type":"string", "description":"Plain-language service purpose and scope for prepare_service, up to 500 characters."},
                    "research_profile": {"type":"object", "description":"Optional selected-data declaration froglet.research-profile/v1: exact fields/types/units, identifier namespaces/versions/prefixes, public provenance and mapping assumptions. Preparation checks every selected row; publication signs it inside output_schema. Declarations do not establish scientific truth or authorization. Omit private names, paths or credentials."},
                    "input": {},
                    "wasm_module_hex": {"type":"string", "minLength":16,"maxLength":524288,"pattern":"^[0-9a-fA-F]+$", "description":"run_compute: complete inline Wasm v1 binary in hex, exporting the froglet.wasm.run_json.v1 ABI. Source text, OCI packages, host capabilities and other runtimes are not supported by this native action."},
                    "task_id": {"type":"string", "description":"Existing requester deal_id for read-only get_task. A pending submission returns this durable reference as deal_id."},
                    "timeout_secs": {"type":"integer", "minimum":0,"maximum":60,"description":"run_compute polling duration after submission; defaults to 15 seconds. Zero returns immediately after admission. A timeout returns the existing deal and idempotency key for get_task recovery."},
                    "source": {"type":"string", "description":"Absolute path to JSON, CSV, SQLite, WAT or Wasm, or a regular file when file options are supplied. Preparation only."},
                    "file": {"type":"object", "description":"Download-only file options; explicitly choose filename, expiry and finite allowances.", "additionalProperties":false,"required":["filename","expires_at","max_downloads","max_transfer_bytes"],"properties":{"filename":{"type":"string"},"media_type":{"type":"string"},"expires_at":{"type":"integer"},"max_downloads":{"type":"integer","minimum":1},"max_transfer_bytes":{"type":"integer","minimum":1}}},
                    "destination": {"type":"string", "description":"Explicit absolute directory for generated service material; existing unrelated projects are never overwritten."},
                    "selection": {"type":"object", "additionalProperties":{"type":"array", "items":{"type":"string"}}, "description":"Source collection names mapped to the fields to include. JSON arrays and CSV use rows."},
                    "csv_columns": {"type":"array", "items":{"type":"object", "properties":{"name":{"type":"string"},"type":{"enum":["string","integer","number","boolean"]},"nullable":{"type":"boolean"},"indexed":{"type":"boolean"}}, "required":["name","type"], "additionalProperties":false}},
                    "example_input": {"description":"A meaningful local example. Required for Wasm; a selected-row lookup is generated for data."},
                    "provider_id": {"type":"string", "pattern":"^[0-9a-f]{64}$", "description":"Provider identity from a shared service link. Selects remote invocation."},
                    "idempotency_key": {"type":"string", "description":"Required for run_compute, 1–128 UTF-8 bytes. Reuse the same key, program, input and provider to reconcile an uncertain submission; use a new key for a new call."},
                    "provider_url": {"type":"string", "description":"Provider origin; requires provider_id. invoke_service requires a public HTTPS share-link origin. run_compute delegates endpoint/egress validation and configured transport selection to the local requester runtime."},
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
    });
    schema["tools"][0]["inputSchema"]["properties"].as_object_mut().unwrap().extend(json!({
                    "wasm_module_path": {"type":"string", "description":"Native run_compute only: absolute path to a local regular nonsymlink Wasm v1 binary, at most 262144 bytes. Mutually exclusive with wasm_module_hex. Reads the compiled program and uses the same bounded inline execution path; no source compilation or host capabilities."},
                    "operation": {"type":"object","description":"Fixed HTTP operation: HTTPS url, GET/POST method, input_schema/output_schema (draft 2020-12), optional auth_profile/fixed_body, timeout_ms, max_request_bytes and max_response_bytes. Provider separately approves its exact hash; preparation makes no upstream call."},
                    "access_token_file": {"type":"string","description":"Absolute private mode-0600 invitation file for invocation or download; never paste credentials into prompts."},
                    "token_file": {"type":"string","description":"New absolute private file in which invite_create stores the credential; never overwritten."},
                    "name": {"type":"string","description":"Invitation recipient label."},
                    "expires_at": {"type":"integer","description":"Invitation expiry, Unix seconds, within 30 days."},
                    "max_requests": {"type":"integer","minimum":2,"maximum":10000,"description":"Invitation quote/deal request allowance; a normal invocation uses two requests."},
                    "invite_id": {"type":"string","description":"Invitation id from invite_list/create to revoke immediately."},
                    "reason": {"type":"string","description":"Reason for pausing new provider work."}
    }).as_object().unwrap().clone());
    schema
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

    let compact = match arguments.get("response_format").and_then(Value::as_str) {
        None if !arguments.contains_key("response_format") => false,
        Some("full")
            if matches!(
                action,
                "inspect_service" | "invoke_service" | "run_compute" | "get_task"
            ) =>
        {
            false
        }
        Some("compact")
            if matches!(
                action,
                "inspect_service" | "invoke_service" | "run_compute" | "get_task"
            ) =>
        {
            true
        }
        _ => {
            return Err(
                "response_format is only supported for inspection/invocation and must be full or compact".into(),
            );
        }
    };
    let payload = match action {
        "status" => status_snapshot().await,
        "file_abort" | "safeguards_status" | "safeguards_pause" | "safeguards_resume"
        | "invite_create" | "invite_list" | "invite_revoke" => {
            super::safeguards::action(action, arguments).await
        }
        "prepare_http_service" => {
            let mut request = arguments.clone();
            request.remove("action");
            let request = serde_json::from_value(Value::Object(request))
                .map_err(|e| format!("invalid prepare_http_service request: {e}"))?;
            super::http_service::prepare(request)
        }
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
        "inspect_service" => {
            let link = selected_service_link(arguments)?
                .ok_or_else(|| "inspect_service requires service_url".to_string())?;
            super::service_link::inspect(&link).await
        }
        "download_file" => {
            let link =
                selected_service_link(arguments)?.ok_or("download_file requires service_url")?;
            let destination = invoke_string(arguments, "destination")?
                .ok_or("download_file requires destination")?;
            let token = invoke_string(arguments, "access_token_file")?;
            super::download::download(
                &link,
                std::path::Path::new(destination),
                token.map(std::path::Path::new),
            )
            .await
        }
        "invoke_service" if arguments.contains_key("service_url") => {
            let link = selected_service_link(arguments)?.expect("service_url validated");
            invoke_selected(
                &link.service_id,
                arguments.get("input").cloned().unwrap_or(Value::Null),
                Some(&link.provider_id),
                Some(&link.provider_url),
                invoke_string(arguments, "idempotency_key")?,
                invoke_price_cap(arguments)?,
                invoke_string(arguments, "access_token_file")?,
                Some(&link),
            )
            .await
        }

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
                invoke_price_cap(arguments)?,
                invoke_string(arguments, "access_token_file")?,
                None,
            )
            .await
        }
        "run_compute" => run_compute(arguments).await,
        "get_task" => get_task(arguments).await,
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
        Ok(payload) if compact => Ok(json!({
            "content":[{"type":"text","text":"Froglet result is in structuredContent; verification and evidence references are included there."}],
            "structuredContent":payload, "isError":false
        })),
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

fn selected_service_link(
    arguments: &serde_json::Map<String, Value>,
) -> Result<Option<super::service_link::ServiceLink>, String> {
    let Some(raw) = invoke_string(arguments, "service_url")? else {
        return Ok(None);
    };
    if ["service_id", "provider_id", "provider_url"]
        .iter()
        .any(|key| arguments.contains_key(*key))
    {
        return Err(
            "service_url cannot be combined with service_id, provider_id, or provider_url".into(),
        );
    }
    super::service_link::ServiceLink::parse(raw)
        .map(Some)
        .map_err(|error| error.to_string())
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

fn invoke_price_cap(arguments: &serde_json::Map<String, Value>) -> Result<u64, String> {
    match arguments.get("max_price_sats") {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| "max_price_sats must be a non-negative integer".into()),
    }
}

async fn invoke(service_id: &str, input: Value) -> Result<Value, CliError> {
    invoke_selected(service_id, input, None, None, None, 0, None, None).await
}

fn require_action_fields(
    arguments: &serde_json::Map<String, Value>,
    allowed: &[&str],
) -> Result<(), CliError> {
    if let Some(field) = arguments
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(CliError::BadArgs(format!(
            "unsupported field {field:?} for this native action"
        )));
    }
    Ok(())
}

async fn requester_options(
    arguments: &serde_json::Map<String, Value>,
    input: Value,
    wait_timeout: Duration,
) -> Result<InvokeOptions, CliError> {
    let string = |field| invoke_string(arguments, field).map_err(CliError::BadArgs);
    Ok(InvokeOptions {
        service_id: String::new(),
        input,
        daemon_url: base_url(
            "FROGLET_DAEMON_URL",
            &base_url("FROGLET_PROVIDER_URL", DEFAULT_PROVIDER_URL),
        ),
        runtime_url: base_url("FROGLET_RUNTIME_URL", DEFAULT_RUNTIME_URL),
        runtime_token: resolve_runtime_auth_token().await?,
        access_token_file: string("access_token_file")?.map(PathBuf::from),
        provider_id_override: string("provider_id")?.map(str::to_string),
        idempotency_key: string("idempotency_key")?.map(str::to_string),
        max_price_sats: Some(invoke_price_cap(arguments).map_err(CliError::BadArgs)?),
        wait_timeout,
        poll_interval: Duration::from_millis(250),
    })
}

fn compute_module_hex(arguments: &serde_json::Map<String, Value>) -> Result<String, CliError> {
    use std::io::Read;

    let inline = invoke_string(arguments, "wasm_module_hex").map_err(CliError::BadArgs)?;
    let path = invoke_string(arguments, "wasm_module_path").map_err(CliError::BadArgs)?;
    match (inline, path) {
        (Some(module), None) => Ok(module.to_owned()),
        (None, Some(path)) => {
            let path = std::path::Path::new(path);
            if !path.is_absolute() {
                return Err(CliError::BadArgs(
                    "wasm_module_path must be absolute".into(),
                ));
            }
            let invalid_file = || {
                CliError::BadArgs(
                    "wasm_module_path must be a readable regular nonsymlink file".into(),
                )
            };
            let metadata = std::fs::symlink_metadata(path).map_err(|_| invalid_file())?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(invalid_file());
            }
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let file = options.open(path).map_err(|_| invalid_file())?;
            let metadata = file.metadata().map_err(|_| invalid_file())?;
            if !metadata.is_file() {
                return Err(invalid_file());
            }
            let limit = super::invoke::MAX_INLINE_WASM_HEX_BYTES / 2;
            let too_large =
                || CliError::BadArgs(format!("wasm_module_path exceeds the {limit}-byte limit"));
            if metadata.len() > limit as u64 {
                return Err(too_large());
            }
            let mut module = Vec::new();
            file.take((limit + 1) as u64)
                .read_to_end(&mut module)
                .map_err(|_| invalid_file())?;
            if module.len() > limit {
                return Err(too_large());
            }
            Ok(hex::encode(module))
        }
        (Some(_), Some(_)) => Err(CliError::BadArgs(
            "wasm_module_hex and wasm_module_path are mutually exclusive".into(),
        )),
        (None, None) => Err(CliError::BadArgs(
            "run_compute requires wasm_module_hex or wasm_module_path".into(),
        )),
    }
}

async fn run_compute(arguments: &serde_json::Map<String, Value>) -> Result<Value, CliError> {
    require_action_fields(
        arguments,
        &[
            "action",
            "wasm_module_hex",
            "wasm_module_path",
            "input",
            "provider_id",
            "provider_url",
            "idempotency_key",
            "max_price_sats",
            "timeout_secs",
            "response_format",
            "access_token_file",
        ],
    )?;
    let module = compute_module_hex(arguments)?;
    let input = arguments.get("input").cloned().ok_or_else(|| {
        CliError::BadArgs("run_compute requires explicit input (null is allowed)".into())
    })?;
    let key = invoke_string(arguments, "idempotency_key")
        .map_err(CliError::BadArgs)?
        .ok_or_else(|| {
            CliError::BadArgs("run_compute requires an explicit idempotency_key".into())
        })?;
    super::invoke::validate_idempotency_key(key)?;
    super::invoke::build_inline_wasm_submission(&module, input.clone())?;
    let timeout = match arguments.get("timeout_secs") {
        None => 15,
        Some(value) => value.as_u64().filter(|value| *value <= 60).ok_or_else(|| {
            CliError::BadArgs("timeout_secs must be an integer from 0 to 60".into())
        })?,
    };
    let provider_url = invoke_string(arguments, "provider_url").map_err(CliError::BadArgs)?;
    if provider_url.is_some()
        && invoke_string(arguments, "provider_id")
            .map_err(CliError::BadArgs)?
            .is_none()
    {
        return Err(CliError::BadArgs(
            "provider_url requires provider_id".into(),
        ));
    }
    let options = requester_options(arguments, input, Duration::from_secs(timeout)).await?;
    let report = super::invoke::run_inline_wasm(&options, provider_url, &module).await?;
    let failed =
        report.terminal && !matches!(report.status.as_str(), "succeeded" | "completed" | "done");
    let payload = serde_json::to_value(report).map_err(|error| {
        CliError::Other(format!("compute report serialization failed: {error}"))
    })?;
    if failed {
        return Err(CliError::Structured {
            report: payload,
            exit_code: 1,
        });
    }
    Ok(payload)
}

async fn get_task(arguments: &serde_json::Map<String, Value>) -> Result<Value, CliError> {
    require_action_fields(
        arguments,
        &["action", "task_id", "provider_id", "response_format"],
    )?;
    let task_id = invoke_string(arguments, "task_id")
        .map_err(CliError::BadArgs)?
        .ok_or_else(|| {
            CliError::BadArgs("get_task requires task_id from an existing deal".into())
        })?;
    let options = requester_options(arguments, Value::Null, Duration::ZERO).await?;
    let report = super::invoke::get_task(&options, task_id).await?;
    serde_json::to_value(report)
        .map_err(|error| CliError::Other(format!("task report serialization failed: {error}")))
}

#[allow(clippy::too_many_arguments)]
async fn invoke_selected(
    service_id: &str,
    input: Value,
    provider_id: Option<&str>,
    provider_url: Option<&str>,
    idempotency_key: Option<&str>,
    max_price_sats: u64,
    access_token_file: Option<&str>,
    link: Option<&super::service_link::ServiceLink>,
) -> Result<Value, CliError> {
    let options = InvokeOptions {
        service_id: service_id.to_string(),
        input,
        daemon_url: base_url(
            "FROGLET_DAEMON_URL",
            &base_url("FROGLET_PROVIDER_URL", DEFAULT_PROVIDER_URL),
        ),
        runtime_url: base_url("FROGLET_RUNTIME_URL", DEFAULT_RUNTIME_URL),
        runtime_token: resolve_runtime_auth_token().await?,
        access_token_file: access_token_file.map(PathBuf::from),
        provider_id_override: provider_id.map(str::to_string),
        idempotency_key: idempotency_key.map(str::to_string),
        max_price_sats: Some(max_price_sats),
        wait_timeout: Duration::from_secs(60),
        poll_interval: Duration::from_millis(250),
    };
    let report = if let Some(link) = link {
        super::invoke::invoke_shared_service(&options, link).await?
    } else if provider_id.is_some() || provider_url.is_some() {
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
    async fn service_url_rejects_conflicting_identity_before_network_access() {
        for field in ["service_id", "provider_id", "provider_url"] {
            let mut arguments = json!({"action":"invoke_service", "service_url":format!("https://froglet.dev/s/{}/catalog", "11".repeat(32)), "input":{"op":"describe"}});
            arguments[field] = json!("conflicting");
            let response = handle_request(json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"froglet","arguments":arguments}})).await.unwrap();
            assert!(
                response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("cannot be combined")
            );
        }
    }

    #[tokio::test]
    async fn inspection_requires_a_link_and_compact_is_not_used_for_consent() {
        for arguments in [
            json!({"action":"inspect_service"}),
            json!({"action":"marketplace_publish", "response_format":"compact"}),
        ] {
            let response = handle_request(json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"froglet","arguments":arguments}})).await.unwrap();
            assert_eq!(response["error"]["code"], -32602);
        }
    }

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
        for action in [
            "run_compute",
            "get_task",
            "prepare_service",
            "doctor",
            "check_updates",
            "open_status",
            "prepare_http_service",
            "invite_create",
            "invite_list",
            "invite_revoke",
            "safeguards_status",
            "safeguards_pause",
            "safeguards_resume",
        ] {
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
    async fn compute_dispatch_rejects_incomplete_or_unsupported_work_before_network() {
        let module = hex::encode(b"\0asm\x01\0\0\0");
        for (arguments, expected) in [
            (
                json!({"action":"run_compute","wasm_module_hex":module,"input":null}),
                "idempotency_key",
            ),
            (
                json!({"action":"run_compute","wasm_module_hex":module,"idempotency_key":"key"}),
                "explicit input",
            ),
            (
                json!({"action":"run_compute","wasm_module_hex":"xyz","input":null,"idempotency_key":"key"}),
                "valid even-length hex",
            ),
            (
                json!({"action":"run_compute","wasm_module_hex":module,"input":null,"idempotency_key":"key","inline_source":"print(1)"}),
                "unsupported field",
            ),
            (
                json!({"action":"run_compute","wasm_module_hex":module,"input":null,"idempotency_key":"key","timeout_secs":61}),
                "0 to 60",
            ),
            (
                json!({"action":"get_task","task_id":"deal","max_price_sats":25}),
                "unsupported field",
            ),
        ] {
            let response = handle_request(json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"froglet","arguments":arguments}}))
            .await
            .unwrap();
            assert_eq!(response["result"]["isError"], true, "{response}");
            assert!(
                response["result"]["structuredContent"]["error"]
                    .as_str()
                    .unwrap()
                    .contains(expected),
                "{response}"
            );
        }
    }

    #[test]
    fn compute_file_and_hex_build_identical_workload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("program.wasm");
        let module = b"\0asm\x01\0\0\0";
        std::fs::write(&path, module).unwrap();
        let file_args = json!({"wasm_module_path":path});
        let inline_args = json!({"wasm_module_hex":hex::encode(module).to_uppercase()});
        let input = json!({"terms":["DEMO:a"]});
        let from_file = super::super::invoke::build_inline_wasm_submission(
            &compute_module_hex(file_args.as_object().unwrap()).unwrap(),
            input.clone(),
        )
        .unwrap();
        let from_hex = super::super::invoke::build_inline_wasm_submission(
            &compute_module_hex(inline_args.as_object().unwrap()).unwrap(),
            input,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(from_file).unwrap(),
            serde_json::to_value(from_hex).unwrap()
        );
    }

    #[tokio::test]
    async fn compute_file_rejects_ambiguous_invalid_and_oversized_paths_before_network() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("program.wasm");
        std::fs::write(&path, b"\0asm\x01\0\0\0").unwrap();
        let oversized = directory.path().join("oversized.wasm");
        std::fs::write(
            &oversized,
            vec![0; super::super::invoke::MAX_INLINE_WASM_HEX_BYTES / 2 + 1],
        )
        .unwrap();
        let invalid = directory.path().join("source.wat");
        std::fs::write(&invalid, b"(module)").unwrap();
        for (module_args, expected) in [
            (json!({}), "requires wasm_module_hex or wasm_module_path"),
            (
                json!({"wasm_module_hex":"0061736d01000000", "wasm_module_path":path}),
                "mutually exclusive",
            ),
            (
                json!({"wasm_module_path":"program.wasm"}),
                "must be absolute",
            ),
            (
                json!({"wasm_module_path":directory.path()}),
                "regular nonsymlink",
            ),
            (
                json!({"wasm_module_path":directory.path().join("missing.wasm")}),
                "regular nonsymlink",
            ),
            (json!({"wasm_module_path":oversized}), "262144-byte limit"),
            (json!({"wasm_module_path":invalid}), "Wasm v1 binary"),
        ] {
            let mut arguments = module_args;
            arguments["action"] = json!("run_compute");
            arguments["input"] = Value::Null;
            arguments["idempotency_key"] = json!("file-test");
            let response = handle_request(json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"froglet","arguments":arguments}}))
            .await
            .unwrap();
            assert_eq!(response["result"]["isError"], true, "{response}");
            let error = response["result"]["structuredContent"]["error"]
                .as_str()
                .unwrap();
            assert!(error.contains(expected), "{response}");
            assert!(
                !error.contains("(module)"),
                "source contents must not be echoed"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn compute_file_refuses_symlinks_and_fifos_without_reading_them() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("program.wasm");
        std::fs::write(&path, b"\0asm\x01\0\0\0").unwrap();
        let link = directory.path().join("linked.wasm");
        symlink(&path, &link).unwrap();
        let fifo = directory.path().join("fifo.wasm");
        let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        for candidate in [link, fifo] {
            let args = json!({"wasm_module_path":candidate});
            assert!(
                compute_module_hex(args.as_object().unwrap())
                    .unwrap_err()
                    .to_string()
                    .contains("regular nonsymlink")
            );
        }
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
