//! `froglet-node invoke <service_id> [json_input]` — call a service
//! published on the LOCAL node through the runtime deal flow.
//!
//! v1 scope: exactly the flow `froglet-node publish` advertises.
//!
//! 1. Fetch the canonical service record from the local provider API
//!    (`GET /v1/provider/services/:id`).
//! 2. Build a service-addressed execution workload — the Rust mirror of
//!    `buildServiceAddressedExecution` in
//!    `integrations/shared/froglet-lib/froglet-client.js`. That JS
//!    builder stays the single source of truth for REMOTE resolution;
//!    this module only covers the local-provider case.
//! 3. `POST /v1/runtime/deals` on the runtime API (quote → deal →
//!    execute happens daemon-side).
//! 4. Poll `GET /v1/runtime/deals/:id` until the deal reaches a
//!    terminal state (execution is spawned asynchronously provider-side
//!    even for `mode = "sync"` services).
//!
//! Remote providers are intentionally NOT resolved here — marketplace
//! search, SSRF validation, and transport pinning live in the JS
//! client; the CLI points users at the MCP `invoke_service` action.

use super::{CliError, pop_flag, pop_kv};
use crate::api::{
    ProviderServiceRecord, ProviderServiceResponse, RuntimeCreateDealRequest,
    RuntimeCreateDealResponse, RuntimeDealResponse, RuntimeProviderRef,
};
use crate::execution::{
    CONTRACT_BUILTIN_EVENTS_QUERY_V1, ExecutionEntrypoint, ExecutionEntrypointKind,
    ExecutionPackageKind, ExecutionRuntime, ExecutionSecurity, ExecutionSecurityMode,
    ExecutionWorkload, WORKLOAD_KIND_EXECUTION_V1, default_contract_version_for,
    default_entrypoint_for, default_entrypoint_kind_for,
};
use crate::protocol::WorkloadSpec;
use crate::wasm::{FROGLET_SCHEMA_V1, JCS_JSON_FORMAT};
use crate::{canonical_json, crypto};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:8080";
const DEFAULT_RUNTIME_URL: &str = "http://127.0.0.1:8081";
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 60;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Must exceed the runtime API's own 65s wait-route timeout so a slow
/// provider sync surfaces the runtime's error instead of a client abort.
const HTTP_TIMEOUT: Duration = Duration::from_secs(70);

/// Deal states after which polling stops. Mirrors `TERMINAL_DEAL_STATES`
/// in `integrations/shared/froglet-lib/froglet-client.js`.
const TERMINAL_DEAL_STATUSES: &[&str] = &[
    "succeeded",
    "failed",
    "rejected",
    "cancelled",
    "completed",
    "done",
    "error",
];
const SUCCESS_DEAL_STATUSES: &[&str] = &["succeeded", "completed", "done"];

const REMOTE_INVOKE_HINT: &str = "remote providers are not supported by `froglet-node invoke` \
     (v1 is local-only); use the MCP `froglet` tool action `invoke_service` with \
     {\"service_id\": \"...\", \"provider_id\": \"...\"} (Claude Code, Codex, Cursor, etc. via \
     `npx froglet-mcp`)";

/// Everything `invoke_local_service` needs, resolved from argv + env by
/// [`run`]. Carried explicitly so integration tests can drive the full
/// flow against in-process listeners without touching the environment.
pub struct InvokeOptions {
    pub service_id: String,
    pub input: Value,
    /// Local provider/public API base URL (the daemon port).
    pub daemon_url: String,
    /// Local runtime API base URL.
    pub runtime_url: String,
    /// Bearer token for the runtime API.
    pub runtime_token: String,
    /// Caller-asserted provider id; must match the local node identity.
    pub provider_id_override: Option<String>,
    /// How long to poll for a terminal deal state. Zero means "do not
    /// poll" (`--no-wait`).
    pub wait_timeout: Duration,
    pub poll_interval: Duration,
}

/// Outcome of one invoke. `terminal` distinguishes "the deal finished"
/// from "still in flight when we stopped polling".
#[derive(Debug, Serialize)]
pub struct InvokeReport {
    pub service_id: String,
    pub provider_id: String,
    pub provider_url: String,
    pub deal_id: String,
    pub status: String,
    pub terminal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_intent_path: Option<String>,
}

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    let no_wait = pop_flag(&mut args, "--no-wait");
    let timeout_secs = match pop_kv(&mut args, "--timeout-secs") {
        Some(raw) => raw.parse::<u64>().map_err(|_| {
            CliError::BadArgs(format!(
                "--timeout-secs expects a number of seconds, got {raw:?}"
            ))
        })?,
        None => DEFAULT_WAIT_TIMEOUT_SECS,
    };
    let provider_id_override = pop_kv(&mut args, "--provider-id");
    let provider_url_override = pop_kv(&mut args, "--provider-url");

    if args.is_empty() || args.len() > 2 || args[0].starts_with("--") {
        return Err(CliError::BadArgs(
            "usage: froglet-node invoke <service_id> [json_input] [--json] [--no-wait] \
             [--timeout-secs N] [--provider-id ID]\n  json_input defaults to null; pass '-' to \
             read it from stdin"
                .to_string(),
        ));
    }
    let service_id = args[0].clone();
    let input = parse_input_arg(args.get(1).map(String::as_str))?;

    let daemon_url = base_url_from_env("FROGLET_DAEMON_URL", DEFAULT_DAEMON_URL);
    let runtime_url = base_url_from_env("FROGLET_RUNTIME_URL", DEFAULT_RUNTIME_URL);

    // Any provider URL other than the local daemon is a remote target.
    if let Some(url) = provider_url_override
        && url.trim_end_matches('/') != daemon_url
    {
        return Err(CliError::Other(format!(
            "--provider-url {url} is not the local daemon ({daemon_url}); {REMOTE_INVOKE_HINT}"
        )));
    }

    let options = InvokeOptions {
        service_id,
        input,
        daemon_url,
        runtime_url,
        runtime_token: resolve_runtime_auth_token().await?,
        provider_id_override,
        wait_timeout: if no_wait {
            Duration::ZERO
        } else {
            Duration::from_secs(timeout_secs)
        },
        poll_interval: POLL_INTERVAL,
    };

    let report = invoke_local_service(&options).await?;

    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("report serializes")
        );
    } else {
        print_human_report(&report);
    }

    if SUCCESS_DEAL_STATUSES.contains(&report.status.as_str()) {
        return Ok(());
    }
    if report.terminal {
        return Err(CliError::Other(format!(
            "service execution ended in status {:?}: {}",
            report.status,
            report.error.as_deref().unwrap_or("no error detail"),
        )));
    }
    if no_wait {
        // Deal created and handed off; not waiting was requested.
        return Ok(());
    }
    Err(CliError::Other(format!(
        "deal {} did not reach a terminal state within {timeout_secs}s (current status {:?}). \
         Follow up with GET {}/v1/runtime/deals/{} (runtime Bearer token) or the MCP `get_task` \
         action{}",
        report.deal_id,
        report.status,
        options.runtime_url,
        report.deal_id,
        report
            .payment_intent_path
            .as_deref()
            .map(|path| format!("; this deal is awaiting payment (payment_intent_path: {path})"))
            .unwrap_or_default(),
    )))
}

/// Full local invoke: service record → workload → runtime deal → poll.
pub async fn invoke_local_service(options: &InvokeOptions) -> Result<InvokeReport, CliError> {
    let http = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|error| CliError::Other(format!("failed to build HTTP client: {error}")))?;

    let service = fetch_local_service(&http, &options.daemon_url, &options.service_id).await?;
    let node_id = fetch_local_node_id(&http, &options.daemon_url).await?;

    if let Some(requested) = options
        .provider_id_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && requested != node_id
    {
        return Err(CliError::Other(format!(
            "--provider-id {requested} is not this node ({node_id}); {REMOTE_INVOKE_HINT}"
        )));
    }
    if !service.provider_id.is_empty() && service.provider_id != node_id {
        return Err(CliError::Other(format!(
            "service {} is owned by provider {} but the local node is {node_id}; \
             {REMOTE_INVOKE_HINT}",
            options.service_id, service.provider_id
        )));
    }

    let execution = build_service_addressed_execution(&service, options.input.clone())
        .map_err(CliError::Other)?;

    let request = RuntimeCreateDealRequest {
        provider: RuntimeProviderRef {
            provider_id: Some(node_id.clone()),
            provider_url: Some(options.daemon_url.clone()),
        },
        offer_id: service.offer_id.clone(),
        spec: WorkloadSpec::Execution {
            execution: Box::new(execution),
        },
        max_price_sats: None,
        idempotency_key: None,
        payment: None,
    };
    let created = create_runtime_deal(&http, options, &request).await?;

    let mut report = InvokeReport {
        service_id: options.service_id.clone(),
        provider_id: created.provider_id,
        provider_url: created.provider_url,
        deal_id: created.deal.deal_id.clone(),
        status: created.deal.status.clone(),
        terminal: is_terminal_deal_status(&created.deal.status),
        result: created.deal.result,
        result_hash: created.deal.result_hash,
        error: created.deal.error,
        payment_intent_path: created.payment_intent_path,
    };
    if report.terminal || options.wait_timeout.is_zero() {
        return Ok(report);
    }

    let deadline = tokio::time::Instant::now() + options.wait_timeout;
    loop {
        tokio::time::sleep(options.poll_interval).await;
        let deal = poll_runtime_deal(&http, options, &report.deal_id).await?;
        report.status = deal.status;
        report.terminal = is_terminal_deal_status(&report.status);
        report.result = deal.result;
        report.result_hash = deal.result_hash;
        report.error = deal.error;
        if report.terminal || tokio::time::Instant::now() >= deadline {
            return Ok(report);
        }
    }
}

/// Rust mirror of `buildServiceAddressedExecution` in
/// `integrations/shared/froglet-lib/froglet-client.js`. Field-for-field
/// parity matters: the daemon hashes this workload into the quote, so a
/// shape divergence between the CLI and the JS client would produce
/// different `workload_hash`es for the same service + input.
fn build_service_addressed_execution(
    service: &ProviderServiceRecord,
    input: Value,
) -> Result<ExecutionWorkload, String> {
    let runtime_name = service.runtime.trim();
    if runtime_name.is_empty() {
        return Err(format!(
            "service {} does not declare a runtime; the record cannot be invoked",
            service.service_id
        ));
    }
    let runtime = ExecutionRuntime::parse(runtime_name)?;
    let package_kind_name = service.package_kind.trim();
    if package_kind_name.is_empty() {
        return Err(format!(
            "service {} does not declare a package_kind; the record cannot be invoked",
            service.service_id
        ));
    }
    let package_kind = ExecutionPackageKind::parse(package_kind_name)?;

    let entrypoint_kind = if service.entrypoint_kind.trim().is_empty() {
        default_entrypoint_kind_for(&runtime)
    } else {
        ExecutionEntrypointKind::parse(&service.entrypoint_kind)?
    };
    // JS quirk preserved: a handler entrypoint that looks like a file
    // path ("handler.py", "src/main.py") is a packaging artifact, not a
    // callable symbol — fall back to the runtime default.
    let recorded_entrypoint = if service.entrypoint.trim().is_empty() {
        ""
    } else {
        service.entrypoint.as_str()
    };
    let looks_like_path = recorded_entrypoint.contains('/')
        || recorded_entrypoint.contains('\\')
        || recorded_entrypoint.ends_with(".py");
    let entrypoint = if recorded_entrypoint.is_empty()
        || (entrypoint_kind == ExecutionEntrypointKind::Handler && looks_like_path)
    {
        default_entrypoint_for(&runtime, &entrypoint_kind).to_string()
    } else {
        recorded_entrypoint.to_string()
    };
    let contract_version = if service.contract_version.trim().is_empty() {
        default_contract_version_for(&runtime, &package_kind, &entrypoint_kind).to_string()
    } else {
        service.contract_version.clone()
    };

    let binding_hash = [
        service.binding_hash.as_deref(),
        service.module_hash.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|value| !value.is_empty())
    .map(str::to_string);
    if package_kind != ExecutionPackageKind::Builtin && binding_hash.is_none() {
        return Err(format!(
            "service {} does not expose a binding hash",
            service.service_id
        ));
    }

    let builtin_name = (package_kind == ExecutionPackageKind::Builtin).then(|| {
        [service.entrypoint.as_str(), service.service_id.as_str()]
            .into_iter()
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| entrypoint.clone())
    });
    let effective_entrypoint = builtin_name.clone().unwrap_or(entrypoint);
    let workload_kind = builtin_name
        .clone()
        .unwrap_or_else(|| WORKLOAD_KIND_EXECUTION_V1.to_string());
    // Builtin offers registered before per-service contract versions all
    // carry the events_query contract; rewrite it to the service's own.
    let contract_version = match builtin_name.as_deref() {
        Some(name)
            if contract_version == CONTRACT_BUILTIN_EVENTS_QUERY_V1 && name != "events.query" =>
        {
            format!("froglet.builtin.{name}.v1")
        }
        _ => contract_version,
    };

    let mut requested_access = BTreeSet::new();
    for mount in &service.mounts {
        requested_access.insert(format!(
            "mount.{}.{}.{}",
            mount.kind,
            if mount.read_only { "read" } else { "write" },
            mount.handle
        ));
    }
    for capability in &service.capabilities {
        let capability = capability.trim();
        if !capability.is_empty() {
            requested_access.insert(capability.to_string());
        }
    }

    let input_bytes = canonical_json::to_vec(&input)
        .map_err(|error| format!("workload input is not canonical-JSON encodable: {error}"))?;

    let mut workload = ExecutionWorkload {
        schema_version: FROGLET_SCHEMA_V1.to_string(),
        workload_kind,
        runtime,
        package_kind: package_kind.clone(),
        entrypoint: ExecutionEntrypoint {
            kind: entrypoint_kind,
            value: effective_entrypoint,
        },
        contract_version,
        input_format: JCS_JSON_FORMAT.to_string(),
        input_hash: crypto::sha256_hex(input_bytes),
        requested_access: requested_access.into_iter().collect(),
        security: ExecutionSecurity {
            mode: ExecutionSecurityMode::Standard,
            confidential_session_hash: None,
            service_id: (package_kind != ExecutionPackageKind::Builtin)
                .then(|| service.service_id.clone()),
            request_envelope: None,
        },
        mounts: service.mounts.clone(),
        input,
        module_hash: None,
        module_bytes_hex: None,
        source_hash: None,
        inline_source: None,
        oci_reference: None,
        oci_digest: None,
        builtin_name: None,
    };
    match package_kind {
        ExecutionPackageKind::InlineSource => workload.source_hash = binding_hash,
        ExecutionPackageKind::InlineModule | ExecutionPackageKind::OciImage => {
            workload.module_hash = binding_hash
        }
        ExecutionPackageKind::Builtin => workload.builtin_name = builtin_name,
    }
    Ok(workload)
}

fn is_terminal_deal_status(status: &str) -> bool {
    let status = status.to_ascii_lowercase();
    TERMINAL_DEAL_STATUSES.contains(&status.as_str())
}

fn parse_input_arg(raw: Option<&str>) -> Result<Value, CliError> {
    let text = match raw {
        None => return Ok(Value::Null),
        Some("-") => {
            let mut buffer = String::new();
            std::io::stdin().read_to_string(&mut buffer)?;
            buffer
        }
        Some(arg) => arg.to_string(),
    };
    serde_json::from_str(&text).map_err(|error| {
        CliError::BadArgs(format!(
            "json_input is not valid JSON ({error}); quote it for your shell, e.g. \
             froglet-node invoke my.service '{{\"key\": \"value\"}}'"
        ))
    })
}

fn base_url_from_env(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Resolve the runtime API Bearer token the same way the MCP server and
/// the publish engine resolve their tokens: explicit env value, then an
/// env-pointed file, then the daemon's `<data-dir>/runtime/auth.token`
/// convention (probing both the daemon and agent-bootstrap layouts).
async fn resolve_runtime_auth_token() -> Result<String, CliError> {
    if let Ok(token) = std::env::var("FROGLET_RUNTIME_AUTH_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }

    let path = if let Ok(path) = std::env::var("FROGLET_RUNTIME_AUTH_TOKEN_PATH") {
        PathBuf::from(path)
    } else if let Ok(data_dir) =
        std::env::var("FROGLET_DATA_ROOT").or_else(|_| std::env::var("FROGLET_DATA_DIR"))
    {
        PathBuf::from(data_dir).join("runtime/auth.token")
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let candidates = [
            home.join(".froglet/runtime/auth.token"),
            home.join(".froglet/data/runtime/auth.token"),
        ];
        candidates
            .iter()
            .find(|candidate| candidate.exists())
            .cloned()
            .unwrap_or_else(|| candidates[0].clone())
    } else {
        return Err(CliError::Other(
            "cannot locate the runtime auth token: set FROGLET_RUNTIME_AUTH_TOKEN, \
             FROGLET_RUNTIME_AUTH_TOKEN_PATH, or FROGLET_DATA_DIR"
                .to_string(),
        ));
    };

    let token = tokio::fs::read_to_string(&path).await.map_err(|error| {
        CliError::Other(format!(
            "could not read the runtime auth token file {path:?}: {error}. The daemon writes it \
             on startup; override with FROGLET_RUNTIME_AUTH_TOKEN or \
             FROGLET_RUNTIME_AUTH_TOKEN_PATH"
        ))
    })?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(CliError::Other(format!(
            "runtime auth token file {path:?} is empty; restart the daemon or set \
             FROGLET_RUNTIME_AUTH_TOKEN"
        )));
    }
    Ok(token)
}

async fn fetch_local_service(
    http: &reqwest::Client,
    daemon_url: &str,
    service_id: &str,
) -> Result<ProviderServiceRecord, CliError> {
    let url = format!(
        "{daemon_url}/v1/provider/services/{}",
        urlencoding::encode(service_id)
    );
    let response = http.get(&url).send().await.map_err(|error| {
        CliError::Daemon(format!(
            "GET {url} failed: {error}; is froglet-node running? (daemon URL comes from \
             FROGLET_DAEMON_URL, default {DEFAULT_DAEMON_URL})"
        ))
    })?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(CliError::Other(format!(
            "service {service_id:?} is not published on this node. List local services with \
             GET {daemon_url}/v1/provider/services, or publish one with `froglet-node publish`. \
             For a service on a remote provider: {REMOTE_INVOKE_HINT}"
        )));
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CliError::Daemon(format!(
            "GET {url} returned HTTP {status}: {body}"
        )));
    }
    let parsed: ProviderServiceResponse = response
        .json()
        .await
        .map_err(|error| CliError::Daemon(format!("service record JSON parse failed: {error}")))?;
    Ok(parsed.service)
}

async fn fetch_local_node_id(http: &reqwest::Client, daemon_url: &str) -> Result<String, CliError> {
    #[derive(Deserialize)]
    struct IdentityView {
        node_id: String,
    }
    #[derive(Deserialize)]
    struct CapabilitiesView {
        identity: IdentityView,
    }

    let url = format!("{daemon_url}/v1/node/capabilities");
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|error| CliError::Daemon(format!("GET {url} failed: {error}")))?;
    if !response.status().is_success() {
        return Err(CliError::Daemon(format!(
            "GET {url} returned HTTP {}: is froglet-node running?",
            response.status()
        )));
    }
    let capabilities: CapabilitiesView = response
        .json()
        .await
        .map_err(|error| CliError::Daemon(format!("capabilities JSON parse failed: {error}")))?;
    Ok(capabilities.identity.node_id)
}

async fn create_runtime_deal(
    http: &reqwest::Client,
    options: &InvokeOptions,
    request: &RuntimeCreateDealRequest,
) -> Result<RuntimeCreateDealResponse, CliError> {
    let url = format!("{}/v1/runtime/deals", options.runtime_url);
    let response = http
        .post(&url)
        .bearer_auth(&options.runtime_token)
        .json(request)
        .send()
        .await
        .map_err(|error| {
            CliError::Daemon(format!(
                "POST {url} failed: {error}; is the runtime API up? (runtime URL comes from \
                 FROGLET_RUNTIME_URL, default {DEFAULT_RUNTIME_URL})"
            ))
        })?;
    let status = response.status();
    if status.is_success() {
        return response.json().await.map_err(|error| {
            CliError::Daemon(format!("runtime deal response JSON parse failed: {error}"))
        });
    }

    let body: Value = response
        .json()
        .await
        .unwrap_or_else(|_| Value::String("<non-JSON response body>".to_string()));
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(CliError::Daemon(format!(
            "the runtime API rejected the auth token ({body}); the daemon writes the expected \
             token to <data-dir>/runtime/auth.token — override with FROGLET_RUNTIME_AUTH_TOKEN \
             or FROGLET_RUNTIME_AUTH_TOKEN_PATH"
        )));
    }
    // Requester spend policy refusals carry a stable `code`; surface the
    // remediation (env var / reset endpoint) instead of a generic HTTP
    // failure. Mirrors `createRuntimeDeal` in froglet-client.js.
    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        if let Some(code) = body
            .get("code")
            .and_then(Value::as_str)
            .filter(|code| code.starts_with("spend_"))
        {
            let detail = body
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| body.to_string());
            let remaining = body
                .get("remaining_msat")
                .and_then(Value::as_u64)
                .map(|value| format!(" remaining_msat={value}."))
                .unwrap_or_default();
            return Err(CliError::Other(format!(
                "Deal refused by the requester spend policy ({code}): {detail}.{remaining}"
            )));
        }
        return Err(CliError::Other(format!(
            "POST {url} failed with 402: {body}"
        )));
    }
    // Dual-mode nodes trust their own bound provider listener, so this
    // refusal means the runtime is running without a local provider
    // surface (split deployment) — the env override is the remediation.
    let body_text = body.to_string();
    if status == reqwest::StatusCode::BAD_REQUEST
        && body_text.contains("FROGLET_RUNTIME_PROVIDER_BASE_URL")
    {
        return Err(CliError::Other(format!(
            "the runtime refused the local provider URL: {body_text}. If this runtime runs \
             separately from the provider, set FROGLET_RUNTIME_PROVIDER_BASE_URL={} in the \
             runtime daemon's environment and restart it",
            options.daemon_url
        )));
    }
    Err(CliError::Daemon(format!(
        "POST {url} returned HTTP {status}: {body_text}"
    )))
}

async fn poll_runtime_deal(
    http: &reqwest::Client,
    options: &InvokeOptions,
    deal_id: &str,
) -> Result<crate::requester_deals::RequesterDealRecord, CliError> {
    let url = format!(
        "{}/v1/runtime/deals/{}",
        options.runtime_url,
        urlencoding::encode(deal_id)
    );
    let response = http
        .get(&url)
        .bearer_auth(&options.runtime_token)
        .send()
        .await
        .map_err(|error| CliError::Daemon(format!("GET {url} failed: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CliError::Daemon(format!(
            "GET {url} returned HTTP {status}: {body}"
        )));
    }
    let parsed: RuntimeDealResponse = response.json().await.map_err(|error| {
        CliError::Daemon(format!("runtime deal poll JSON parse failed: {error}"))
    })?;
    Ok(parsed.deal)
}

fn print_human_report(report: &InvokeReport) {
    println!("service:  {}", report.service_id);
    println!("provider: {}", report.provider_id);
    println!("deal:     {}", report.deal_id);
    println!("status:   {}", report.status);
    if let Some(path) = report.payment_intent_path.as_deref() {
        println!("payment:  {path}");
    }
    if let Some(error) = report.error.as_deref() {
        println!("error:    {error}");
    }
    if let Some(result) = report.result.as_ref() {
        println!(
            "result:\n{}",
            serde_json::to_string_pretty(result).expect("result serializes")
        );
    } else if !report.terminal {
        println!("result:   (pending — deal has not reached a terminal state)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::ExecutionMount;
    use serde_json::json;

    fn python_service_record() -> ProviderServiceRecord {
        serde_json::from_value(json!({
            "service_id": "text.summarize",
            "offer_id": "text.summarize",
            "offer_kind": "text.summarize",
            "resource_kind": "service",
            "summary": "Summarize text",
            "runtime": "python",
            "package_kind": "inline_source",
            "entrypoint_kind": "handler",
            "entrypoint": "handler.py",
            "contract_version": "",
            "mode": "sync",
            "price_sats": 0,
            "publication_state": "active",
            "provider_id": "aa".repeat(32),
            "binding_hash": "bb".repeat(32),
        }))
        .expect("service record")
    }

    #[test]
    fn python_inline_source_service_builds_service_addressed_workload() {
        let input = json!({"text": "hello"});
        let workload = build_service_addressed_execution(&python_service_record(), input.clone())
            .expect("workload");

        assert_eq!(workload.schema_version, FROGLET_SCHEMA_V1);
        assert_eq!(workload.workload_kind, WORKLOAD_KIND_EXECUTION_V1);
        assert_eq!(workload.runtime, ExecutionRuntime::Python);
        assert_eq!(workload.package_kind, ExecutionPackageKind::InlineSource);
        // "handler.py" looks like a file path → runtime default symbol.
        assert_eq!(workload.entrypoint.value, "handler");
        assert_eq!(workload.entrypoint.kind, ExecutionEntrypointKind::Handler);
        assert_eq!(
            workload.contract_version,
            crate::execution::CONTRACT_PYTHON_HANDLER_JSON_V1
        );
        assert_eq!(
            workload.security.service_id.as_deref(),
            Some("text.summarize")
        );
        assert_eq!(workload.security.mode, ExecutionSecurityMode::Standard);
        assert_eq!(
            workload.source_hash.as_deref(),
            Some("bb".repeat(32).as_str())
        );
        assert!(workload.module_hash.is_none());
        assert!(workload.inline_source.is_none());
        assert_eq!(
            workload.input_hash,
            crypto::sha256_hex(canonical_json::to_vec(&input).expect("canonical input"))
        );
        assert!(workload.is_service_addressed());
        assert!(workload.validate_basic().is_ok());
    }

    #[test]
    fn builtin_service_uses_builtin_workload_kind_and_contract_rewrite() {
        let mut service = python_service_record();
        service.runtime = "builtin".to_string();
        service.package_kind = "builtin".to_string();
        service.entrypoint_kind = "builtin".to_string();
        service.entrypoint = "demo.add".to_string();
        service.service_id = "demo.add".to_string();
        service.contract_version = CONTRACT_BUILTIN_EVENTS_QUERY_V1.to_string();
        service.binding_hash = None;

        let workload =
            build_service_addressed_execution(&service, json!({"a": 1, "b": 2})).expect("workload");
        assert_eq!(workload.workload_kind, "demo.add");
        assert_eq!(workload.builtin_name.as_deref(), Some("demo.add"));
        assert_eq!(workload.entrypoint.value, "demo.add");
        assert_eq!(workload.contract_version, "froglet.builtin.demo.add.v1");
        // Builtin executions are not service-addressed (no security.service_id).
        assert!(workload.security.service_id.is_none());
        assert!(workload.validate_basic().is_ok());
    }

    #[test]
    fn wasm_module_service_maps_binding_to_module_hash() {
        let mut service = python_service_record();
        service.runtime = "wasm".to_string();
        service.package_kind = "inline_module".to_string();
        service.entrypoint_kind = "".to_string();
        service.entrypoint = "".to_string();
        service.binding_hash = None;
        service.module_hash = Some("cc".repeat(32));

        let workload = build_service_addressed_execution(&service, Value::Null).expect("workload");
        assert_eq!(
            workload.module_hash.as_deref(),
            Some("cc".repeat(32).as_str())
        );
        assert!(workload.source_hash.is_none());
        assert_eq!(workload.entrypoint.value, "run");
        assert_eq!(workload.contract_version, crate::wasm::WASM_RUN_JSON_ABI_V1);
        assert!(workload.validate_basic().is_ok());
    }

    #[test]
    fn missing_binding_hash_is_rejected() {
        let mut service = python_service_record();
        service.binding_hash = None;
        service.module_hash = None;
        let error = build_service_addressed_execution(&service, Value::Null).unwrap_err();
        assert!(error.contains("binding hash"), "got: {error}");
    }

    #[test]
    fn mount_access_is_declared_and_sorted() {
        let mut service = python_service_record();
        service.mounts = vec![ExecutionMount {
            handle: "cache".to_string(),
            kind: "redis".to_string(),
            read_only: false,
            binding: None,
        }];
        service.capabilities = vec!["net.fetch".to_string(), " ".to_string()];

        let workload = build_service_addressed_execution(&service, Value::Null).expect("workload");
        assert_eq!(
            workload.requested_access,
            vec![
                "mount.redis.write.cache".to_string(),
                "net.fetch".to_string()
            ]
        );
        assert!(workload.validate_basic().is_ok());
    }

    #[test]
    fn terminal_status_classification_matches_js_client() {
        for status in [
            "succeeded",
            "FAILED",
            "rejected",
            "cancelled",
            "completed",
            "done",
            "error",
        ] {
            assert!(
                is_terminal_deal_status(status),
                "{status} should be terminal"
            );
        }
        for status in ["accepted", "running", "payment_pending", "result_ready", ""] {
            assert!(
                !is_terminal_deal_status(status),
                "{status} should not be terminal"
            );
        }
    }
}
