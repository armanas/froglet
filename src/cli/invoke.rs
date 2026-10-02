//! Local and remote service invocation through the existing requester runtime.
//! Remote metadata is fetched with bounded, DNS-pinned HTTPS; the runtime
//! remains responsible for signed offers, quotes, deals, and receipt checks.

use super::{CliError, pop_flag, pop_kv};
use crate::api::{
    ProviderServiceRecord, ProviderServiceResponse, RuntimeCreateDealRequest,
    RuntimeCreateDealResponse, RuntimeDealResponse, RuntimeInvocationQuery, RuntimeProviderRef,
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
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
pub(super) const MAX_INLINE_WASM_HEX_BYTES: usize = 512 * 1024;
const MAX_INLINE_WASM_INPUT_BYTES: usize = 128 * 1024;

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

const REMOTE_INVOKE_HINT: &str = "use --provider-id <identity> --provider-url <https-origin>, or native MCP invoke_service with provider_id and provider_url";

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
    /// Private invitation file; contents are sent only to the authenticated local runtime.
    pub access_token_file: Option<PathBuf>,
    /// Caller-asserted provider id; remote calls verify this against the service,
    /// signed publication revision (when supplied), quote, and receipt.
    pub provider_id_override: Option<String>,
    /// Reuse this key only to reconcile the same invocation after uncertainty.
    pub idempotency_key: Option<String>,
    /// Explicit per-call Lightning price cap. Remote calls default to zero.
    pub max_price_sats: Option<u64>,
    /// How long to poll for a terminal deal state. Zero means "do not
    /// poll" (`--no-wait`).
    pub wait_timeout: Duration,
    pub poll_interval: Duration,
}

/// Outcome of one invoke. `terminal` distinguishes "the deal finished"
/// from "still in flight when we stopped polling".
#[derive(Debug, Serialize)]
pub struct InvokeReport {
    pub stage: String,
    pub code: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub service_id: String,
    pub workload_kind: String,
    pub workload_hash: String,
    pub quote_hash: String,
    pub deal_hash: String,
    pub execution_limits: crate::protocol::ExecutionLimits,
    pub provider_id: String,
    pub provider_url: String,
    pub deal_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub idempotency_key: String,
    pub receipt_verification: Value,
    pub next_action: String,
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
    let max_price_sats = pop_kv(&mut args, "--max-price-sats")
        .map(|value| {
            value.parse::<u64>().map_err(|_| {
                CliError::BadArgs("--max-price-sats must be a non-negative integer".into())
            })
        })
        .transpose()?
        .unwrap_or(0);
    let mut provider_id_override = pop_kv(&mut args, "--provider-id");
    let mut provider_url_override = pop_kv(&mut args, "--provider-url");
    let idempotency_key = pop_kv(&mut args, "--idempotency-key");
    let access_token_file = pop_kv(&mut args, "--access-token-file").map(PathBuf::from);

    if args.is_empty() || args.len() > 2 || args[0].starts_with("--") {
        return Err(CliError::BadArgs(
            "usage: froglet-node invoke <service_id> [json_input] [--json] [--no-wait] \
             [--timeout-secs N] [--provider-id ID] [--provider-url HTTPS_ORIGIN] \
             [--idempotency-key KEY] [--max-price-sats N] [--access-token-file FILE]\n  json_input defaults to null; pass '-' to \
             read it from stdin"
                .to_string(),
        ));
    }
    let mut service_link = None;
    let service_id = if args[0].contains("://") {
        if provider_id_override.is_some() || provider_url_override.is_some() {
            return Err(CliError::BadArgs(
                "a share URL cannot be combined with provider overrides".into(),
            ));
        }
        let link = super::service_link::ServiceLink::parse(&args[0])?;
        provider_id_override = Some(link.provider_id.clone());
        provider_url_override = Some(link.provider_url.clone());
        let service_id = link.service_id.clone();
        service_link = Some(link);
        service_id
    } else {
        args[0].clone()
    };
    let input = parse_input_arg(args.get(1).map(String::as_str))?;

    let daemon_url = base_url_from_env(
        "FROGLET_DAEMON_URL",
        &base_url_from_env("FROGLET_PROVIDER_URL", DEFAULT_DAEMON_URL),
    );
    let runtime_url = base_url_from_env("FROGLET_RUNTIME_URL", DEFAULT_RUNTIME_URL);

    let options = InvokeOptions {
        service_id,
        input,
        daemon_url,
        runtime_url,
        runtime_token: resolve_runtime_auth_token().await?,
        access_token_file,
        provider_id_override,
        idempotency_key,
        max_price_sats: Some(max_price_sats),
        wait_timeout: if no_wait {
            Duration::ZERO
        } else {
            Duration::from_secs(timeout_secs)
        },
        poll_interval: POLL_INTERVAL,
    };

    let remote_url = provider_url_override
        .as_deref()
        .filter(|url| url.trim_end_matches('/') != options.daemon_url);
    let report = if let Some(link) = service_link.as_ref() {
        invoke_shared_service(&options, link).await?
    } else if remote_url.is_some() || options.provider_id_override.is_some() {
        invoke_remote_service(&options, remote_url).await?
    } else {
        invoke_local_service(&options).await?
    };

    if json_mode {
        if !SUCCESS_DEAL_STATUSES.contains(&report.status.as_str()) && (report.terminal || !no_wait)
        {
            return Err(CliError::Structured {
                report: serde_json::to_value(&report)
                    .map_err(|e| CliError::Other(e.to_string()))?,
                exit_code: 1,
            });
        }
        let encoded = serde_json::to_string_pretty(&report)
            .map_err(|error| CliError::Other(format!("failed to serialize report: {error}")))?;
        println!("{encoded}");
    } else {
        print_human_report(&report)?;
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
         Follow up with GET {}/v1/runtime/deals/{} (runtime Bearer token), or repeat the exact \
         invocation with idempotency key {} to reconcile{}",
        report.deal_id,
        report.status,
        options.runtime_url,
        report.deal_id,
        report.idempotency_key,
        report
            .payment_intent_path
            .as_deref()
            .map(|path| format!("; this deal is awaiting payment (payment_intent_path: {path})"))
            .unwrap_or_default(),
    )))
}

/// Full local invoke: service record → workload → runtime deal → poll.
pub async fn invoke_local_service(options: &InvokeOptions) -> Result<InvokeReport, CliError> {
    let http = crate::tls::reqwest_client_builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|error| CliError::Other(format!("failed to build HTTP client: {error}")))?;

    if let Some(report) = recover_invocation(&http, options, Some(&options.daemon_url)).await? {
        return Ok(report);
    }
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

    invoke_resolved_service(
        &http,
        options,
        &service,
        node_id,
        options.daemon_url.clone(),
        options.max_price_sats,
    )
    .await
}

/// Resolve the selected provider without sending local authentication to it.
/// Omitting provider_url uses the operator-configured marketplace via runtime.
pub async fn invoke_remote_service(
    options: &InvokeOptions,
    provider_url: Option<&str>,
) -> Result<InvokeReport, CliError> {
    invoke_remote_service_inner(options, provider_url, None).await
}

pub async fn invoke_shared_service(
    options: &InvokeOptions,
    link: &super::service_link::ServiceLink,
) -> Result<InvokeReport, CliError> {
    invoke_remote_service_inner(options, Some(&link.provider_url), Some(link)).await
}

async fn invoke_remote_service_inner(
    options: &InvokeOptions,
    provider_url: Option<&str>,
    link: Option<&super::service_link::ServiceLink>,
) -> Result<InvokeReport, CliError> {
    let provider_id = options.provider_id_override.as_deref().ok_or_else(|| {
        CliError::BadArgs("remote invocation requires provider_id from the service link".into())
    })?;
    if provider_id.len() != 64
        || !provider_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(CliError::BadArgs(
            "provider_id must be a 64-character lowercase hexadecimal identity".into(),
        ));
    }
    let http = crate::tls::reqwest_client_builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| CliError::Other(e.to_string()))?;
    if let Some(report) = recover_invocation(&http, options, provider_url).await? {
        return Ok(report);
    }
    if let Some(link) = link {
        let inspected = super::service_link::inspect(link).await?;
        match inspected["availability"]["execution_access"].as_str() {
            Some("private") => return Err(CliError::Other("provider_access_required: private execution is only available to the provider".into())),
            Some("invite") if options.access_token_file.is_none() => return Err(CliError::Other("invitation_required: ask the provider for a credential file and supply access_token_file; do not paste it into a prompt or share link".into())),
            _ => {}
        }
        if inspected["free_call_supported"] != true && options.max_price_sats.unwrap_or(0) == 0 {
            return Err(CliError::Other("payment_required: paid sharing calls require an explicit price cap and configured payment backend".into()));
        }
    }
    // Preserve explicit references to this node, without treating arbitrary
    // loopback URLs as trusted remote services.
    if provider_url.is_none()
        && fetch_local_node_id(&http, &options.daemon_url)
            .await
            .ok()
            .as_deref()
            == Some(provider_id)
    {
        return invoke_local_service(options).await;
    }
    let endpoint = match provider_url {
        Some(url) => url.to_string(),
        None => {
            let response = http
                .get(format!(
                    "{}/v1/runtime/providers/{}",
                    options.runtime_url, provider_id
                ))
                .bearer_auth(&options.runtime_token)
                .send()
                .await
                .map_err(|e| CliError::Daemon(e.to_string()))?;
            if !response.status().is_success() {
                return Err(CliError::Daemon(format!(
                    "provider lookup returned {}; supply the provider URL from the service link",
                    response.status()
                )));
            }
            let detail: Value = crate::http_body::read_json_response_limited(
                response,
                1024 * 1024,
                "provider lookup",
            )
            .await
            .map_err(CliError::Daemon)?;
            let provider = detail.get("provider").unwrap_or(&detail);
            if provider.get("provider_id").and_then(Value::as_str) != Some(provider_id) {
                return Err(CliError::Other(
                    "provider_identity_mismatch: marketplace returned another provider".into(),
                ));
            }
            select_remote_endpoint(provider.get("transport_endpoints").unwrap_or(&Value::Null))?
        }
    };
    let endpoint = remote_origin(&endpoint)?;
    let service_url = format!(
        "{endpoint}/v1/provider/services/{}",
        urlencoding::encode(&options.service_id)
    );
    let response: ProviderServiceResponse = crate::safe_fetch::safe_fetch_json(
        &service_url,
        crate::safe_fetch::FetchPolicy {
            max_bytes: 1024 * 1024,
            timeout_ms: 15_000,
            ..Default::default()
        },
    )
    .await
    .map_err(|e| CliError::Daemon(format!("provider_unavailable: {e}")))?;
    validate_remote_service(
        &response.service,
        &options.service_id,
        provider_id,
        options.max_price_sats.unwrap_or(0),
    )?;
    if let Some(revision) = &response.publication_revision {
        revision
            .verify()
            .map_err(|e| CliError::Other(format!("invalid_publication_revision: {e}")))?;
        if revision.payload.provider_id != provider_id
            || revision.payload.service_id != options.service_id
            || revision.payload.offer_id != response.service.offer_id
            || response.service.binding_hash.as_deref()
                != Some(revision.payload.binding_hash.as_str())
        {
            return Err(CliError::Other(
                "service_identity_mismatch: public revision differs from service metadata".into(),
            ));
        }
    }
    invoke_resolved_service(
        &http,
        options,
        &response.service,
        provider_id.to_string(),
        endpoint,
        Some(options.max_price_sats.unwrap_or(0)),
    )
    .await
}

fn select_remote_endpoint(endpoints: &Value) -> Result<String, CliError> {
    let mut candidates = endpoints
        .as_array()
        .into_iter()
        .flatten()
        .filter(|ep| {
            ep["uri"]
                .as_str()
                .is_some_and(|uri| remote_origin(uri).is_ok())
        })
        .collect::<Vec<_>>();
    let featured = |ep: &&Value| {
        ep["features"]
            .as_array()
            .is_some_and(|values| values.iter().any(|v| v == "quote_http"))
    };
    if candidates.iter().any(featured) {
        candidates.retain(featured);
    }
    candidates.sort_by_key(|ep| ep["priority"].as_u64().unwrap_or(u64::MAX));
    candidates
        .first()
        .and_then(|ep| ep["uri"].as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            CliError::Other("provider_unavailable: no public HTTPS service endpoint".into())
        })
}

fn remote_origin(raw: &str) -> Result<String, CliError> {
    let url = reqwest::Url::parse(raw)
        .map_err(|e| CliError::BadArgs(format!("invalid provider_url: {e}")))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(CliError::BadArgs("provider_url must be a public HTTPS origin without credentials, path, query, or fragment".into()));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn validate_remote_service(
    service: &ProviderServiceRecord,
    service_id: &str,
    provider_id: &str,
    max_price_sats: u64,
) -> Result<(), CliError> {
    if service.service_id != service_id || service.provider_id != provider_id {
        return Err(CliError::Other(
            "service_identity_mismatch: remote service does not match the shared identity".into(),
        ));
    }
    if service.price_sats != 0
        || service.base_fee_msat != 0
        || service.success_fee_msat != 0
        || service.settlement_method != "none"
    {
        let total_msat = service
            .base_fee_msat
            .checked_add(service.success_fee_msat)
            .ok_or_else(|| CliError::Other("payment_required: service price overflows".into()))?;
        if max_price_sats == 0
            || total_msat > max_price_sats.saturating_mul(1000)
            || service.price_sats > max_price_sats
            || !matches!(service.price_currency.as_deref(), None | Some("sat"))
            || !matches!(
                service.settlement_method.as_str(),
                "lightning.prepaid.v1" | "lightning.base_fee_plus_success_fee.v1"
            )
        {
            return Err(CliError::Other("payment_required: paid sharing calls require a sufficient explicit Lightning price cap in sats; Stripe calls use the runtime payment-token API".into()));
        }
    }
    Ok(())
}

/// Native agent computation is deliberately limited to the pure JSON Wasm ABI.
/// Hashing and submission construction reuse the canonical workload types;
/// the requester runtime owns Quote acceptance, signing, spending and replay.
pub fn build_inline_wasm_submission(
    module_hex: &str,
    input: Value,
) -> Result<crate::wasm::WasmSubmission, CliError> {
    if module_hex.is_empty() || module_hex.len() > MAX_INLINE_WASM_HEX_BYTES {
        return Err(CliError::BadArgs(
            "wasm_module_hex must contain 1–524288 hexadecimal characters".into(),
        ));
    }
    let module = hex::decode(module_hex)
        .map_err(|_| CliError::BadArgs("wasm_module_hex must be valid even-length hex".into()))?;
    if !module.starts_with(b"\0asm\x01\0\0\0") {
        return Err(CliError::BadArgs(
            "wasm_module_hex must contain a Wasm v1 binary, not WAT or source text".into(),
        ));
    }
    let submission = crate::wasm::WasmSubmission {
        schema_version: FROGLET_SCHEMA_V1.into(),
        submission_type: crate::wasm::WASM_SUBMISSION_TYPE_V1.into(),
        workload: crate::wasm::ComputeWasmWorkload::new(&module, &input)
            .map_err(CliError::BadArgs)?,
        // Normalize equivalent encodings before exact-spec idempotency comparison.
        module_bytes_hex: hex::encode(module),
        input,
    };
    submission
        .validate_limits(MAX_INLINE_WASM_HEX_BYTES, MAX_INLINE_WASM_INPUT_BYTES)
        .map_err(CliError::BadArgs)?;
    submission.verify().map_err(CliError::BadArgs)?;
    Ok(submission)
}

pub(crate) fn validate_idempotency_key(key: &str) -> Result<(), CliError> {
    if key.trim().is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(CliError::BadArgs(
            "idempotency_key must contain 1–128 UTF-8 bytes".into(),
        ));
    }
    Ok(())
}

/// Submit or reconcile one requester-supplied program. Generic Wasm retries
/// use POST's full-spec comparison; the service-specific GET recovery query
/// cannot distinguish programs and must not be used here.
pub async fn run_inline_wasm(
    options: &InvokeOptions,
    provider_url: Option<&str>,
    module_hex: &str,
) -> Result<InvokeReport, CliError> {
    let key = options.idempotency_key.as_deref().ok_or_else(|| {
        CliError::BadArgs("run_compute requires an explicit idempotency_key".into())
    })?;
    validate_idempotency_key(key)?;
    let submission = build_inline_wasm_submission(module_hex, options.input.clone())?;
    let spec = WorkloadSpec::Wasm {
        submission: Box::new(submission),
    };
    let workload_hash = spec.request_hash().map_err(CliError::BadArgs)?;
    let http = crate::tls::reqwest_client_builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| CliError::Other(error.to_string()))?;
    let provider_id = match options.provider_id_override.as_deref() {
        Some(id) => {
            if id.len() != 64
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(CliError::BadArgs(
                    "provider_id must be a 64-character lowercase hexadecimal identity".into(),
                ));
            }
            id.to_string()
        }
        None if provider_url.is_some() => {
            return Err(CliError::BadArgs(
                "provider_url requires provider_id".into(),
            ));
        }
        None => fetch_local_node_id(&http, &options.daemon_url).await?,
    };
    // The configured local origin is trusted by the existing runtime only for
    // its own provider identity. All remote selection/egress remains there.
    let endpoint = match provider_url {
        Some(url) => Some(url.to_string()),
        None if options.provider_id_override.is_none() => Some(options.daemon_url.clone()),
        None => None,
    };
    let request = RuntimeCreateDealRequest {
        provider: RuntimeProviderRef {
            provider_id: Some(provider_id.clone()),
            provider_url: endpoint,
        },
        offer_id: "execute.compute".into(),
        spec,
        max_price_sats: Some(options.max_price_sats.unwrap_or(0)),
        idempotency_key: Some(key.to_string()),
        payment: None,
    };
    let preserve_retry = |error: CliError, task_id: Option<&str>| {
        let mut report = super::doctor::error_report(&error);
        report["idempotency_key"] = serde_json::json!(key);
        report["workload_hash"] = serde_json::json!(workload_hash);
        report["stage"] = serde_json::json!("requester_compute");
        report["retryable"] = serde_json::json!(false);
        if report.get("deal_id").is_none() {
            report["next_action"] = serde_json::json!(
                "If submission is uncertain, repeat the exact program, input, provider and idempotency_key. Do not replace the key. An unpersisted remote acceptance may remain unresolved; a retry is not a guarantee of recovery."
            );
        }
        let error = CliError::Structured {
            report,
            exit_code: error.exit_code(),
        };
        match task_id {
            Some(task_id) => preserve_task_reference(error, task_id, "requester_compute", "failed"),
            None => error,
        }
    };
    let created = create_runtime_deal(&http, options, &request)
        .await
        .map_err(|error| preserve_retry(error, None))?;
    let task_id = created.deal.deal_id.clone();
    if created.deal.quote.payload.workload_hash != workload_hash
        || created.deal.quote.payload.workload_kind != crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_V1
        || created.provider_id != provider_id
        || created.deal.idempotency_key.as_deref() != Some(key)
    {
        return Err(preserve_retry(
            CliError::Daemon(
                "compute_response_mismatch: runtime returned another workload, provider or retry key"
                    .into(),
            ),
            Some(&task_id),
        ));
    }
    finish_invocation(&http, options, created, provider_id, key.to_string())
        .await
        .map_err(|error| preserve_retry(error, Some(&task_id)))
}

fn preserve_task_reference(
    error: CliError,
    task_id: &str,
    stage: &str,
    verification_status: &str,
) -> CliError {
    let mut report = super::doctor::error_report(&error);
    report["stage"] = serde_json::json!(stage);
    report["deal_id"] = serde_json::json!(task_id);
    report["task_id"] = serde_json::json!(task_id);
    report["retryable"] = serde_json::json!(false);
    report["receipt_verification"] =
        serde_json::json!({"status":verification_status, "verified":false});
    report["next_action"] = serde_json::json!(
        "Inspect or refresh this same task reference with get_task. Returned evidence was not accepted; this error does not authorize new work or payment."
    );
    CliError::Structured {
        report,
        exit_code: error.exit_code(),
    }
}

/// Read the local requester's durable operation. This never submits new work
/// or authorizes payment. Runtime authentication still scopes the read.
pub async fn get_task(options: &InvokeOptions, task_id: &str) -> Result<InvokeReport, CliError> {
    if task_id.is_empty() || task_id.len() > 256 || task_id.chars().any(char::is_control) {
        return Err(CliError::BadArgs(
            "task_id must contain 1–256 non-control bytes".into(),
        ));
    }
    let http = crate::tls::reqwest_client_builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| CliError::Other(error.to_string()))?;
    let deal = poll_runtime_deal(&http, options, task_id)
        .await
        .map_err(|error| {
            preserve_task_reference(error, task_id, "requester_task_read", "not_checked")
        })?;
    if deal.deal_id != task_id {
        return Err(preserve_task_reference(
            CliError::Daemon("task_identity_mismatch: runtime returned another task".into()),
            task_id,
            "requester_task_read",
            "failed",
        ));
    }
    let provider = options
        .provider_id_override
        .as_deref()
        .unwrap_or(&deal.provider_id);
    let payment_intent_path = (deal.status == "payment_pending"
        && deal.quote.payload.settlement_terms.method != "none")
        .then(|| {
            format!(
                "/v1/runtime/deals/{}/payment-intent",
                urlencoding::encode(task_id)
            )
        });
    let report = report_from_deal("", &deal, provider, payment_intent_path).map_err(|error| {
        preserve_task_reference(error, task_id, "requester_task_read", "failed")
    })?;
    Ok(finalize_report(report))
}

async fn invoke_resolved_service(
    http: &reqwest::Client,
    options: &InvokeOptions,
    service: &ProviderServiceRecord,
    node_id: String,
    provider_url: String,
    max_price_sats: Option<u64>,
) -> Result<InvokeReport, CliError> {
    let execution = build_service_addressed_execution(service, options.input.clone())
        .map_err(CliError::Other)?;

    let idempotency_key = options
        .idempotency_key
        .clone()
        .unwrap_or_else(|| format!("native-{}", hex::encode(rand::random::<[u8; 16]>())));
    validate_idempotency_key(&idempotency_key)?;
    let request = RuntimeCreateDealRequest {
        provider: RuntimeProviderRef {
            provider_id: Some(node_id.clone()),
            provider_url: Some(provider_url),
        },
        offer_id: service.offer_id.clone(),
        spec: WorkloadSpec::Execution {
            execution: Box::new(execution),
        },
        max_price_sats,
        idempotency_key: Some(idempotency_key.clone()),
        payment: None,
    };
    let created = create_runtime_deal(http, options, &request).await.map_err(|error| {
        let mut report = super::doctor::error_report(&error);
        report["idempotency_key"] = serde_json::json!(idempotency_key);
        report["stage"] = serde_json::json!("requester_execution");
        report["retryable"] = serde_json::json!(false);
        report["next_action"] = serde_json::json!("If the result is uncertain, repeat the exact input with this idempotency_key; the runtime reconciles its durable intent before making a new deal. Never substitute a new key merely to retry.");
        CliError::Structured { report, exit_code: error.exit_code() }
    })?;

    finish_invocation(http, options, created, node_id, idempotency_key).await
}

async fn recover_invocation(
    http: &reqwest::Client,
    options: &InvokeOptions,
    provider_url: Option<&str>,
) -> Result<Option<InvokeReport>, CliError> {
    let Some(key) = options.idempotency_key.as_deref() else {
        return Ok(None);
    };
    validate_idempotency_key(key)?;
    let query = RuntimeInvocationQuery {
        idempotency_key: key.to_string(),
        service_id: options.service_id.clone(),
        input_hash: crypto::sha256_hex(
            canonical_json::to_vec(&options.input).map_err(|e| CliError::BadArgs(e.to_string()))?,
        ),
        provider_id: options.provider_id_override.clone(),
        provider_url: provider_url.map(str::to_string),
        max_price_sats: options.max_price_sats,
    };
    let response = http
        .get(format!("{}/v1/runtime/deals", options.runtime_url))
        .bearer_auth(&options.runtime_token)
        .query(&query)
        .send()
        .await
        .map_err(|e| CliError::Daemon(format!("invocation recovery unavailable: {e}")))?;
    // Older runtimes expose POST only. A missing operation may follow the
    // normal admission path; auth, conflicts and storage errors must not.
    if matches!(
        response.status(),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED
    ) {
        return Ok(None);
    }
    let status = response.status();
    let value: Value = crate::http_body::read_json_response_limited(
        response,
        16 * 1024 * 1024,
        "invocation recovery",
    )
    .await
    .map_err(CliError::Daemon)?;
    if !status.is_success() {
        return Err(CliError::Daemon(format!(
            "invocation recovery returned {status}: {value}"
        )));
    }
    let created: RuntimeCreateDealResponse = serde_json::from_value(value)
        .map_err(|e| CliError::Daemon(format!("invalid invocation recovery response: {e}")))?;
    if created.deal.idempotency_key.as_deref() != Some(key)
        || options
            .provider_id_override
            .as_deref()
            .is_some_and(|id| id != created.provider_id)
        || created.deal.provider_id != created.provider_id
    {
        return Err(CliError::Daemon(
            "invocation recovery identity mismatch".into(),
        ));
    }
    let provider = created.provider_id.clone();
    finish_invocation(http, options, created, provider, key.to_string())
        .await
        .map(Some)
}

async fn finish_invocation(
    http: &reqwest::Client,
    options: &InvokeOptions,
    created: RuntimeCreateDealResponse,
    node_id: String,
    idempotency_key: String,
) -> Result<InvokeReport, CliError> {
    if created.deal.idempotency_key.as_deref() != Some(idempotency_key.as_str()) {
        return Err(CliError::Daemon(
            "invocation retry identity mismatch".into(),
        ));
    }
    let mut report = report_from_deal(
        &options.service_id,
        &created.deal,
        &node_id,
        created.payment_intent_path,
    )?;
    if report.terminal || options.wait_timeout.is_zero() {
        return Ok(finalize_report(report));
    }

    let deadline = tokio::time::Instant::now() + options.wait_timeout;
    loop {
        tokio::time::sleep(options.poll_interval).await;
        let deal = match poll_runtime_deal(http, options, &report.deal_id).await {
            Ok(deal) => deal,
            Err(error) => {
                // The mutation already has a durable deal identity. Preserve it
                // in the report even if subsequent safe reads are unavailable.
                report.error = Some(error.to_string());
                report.next_action = "Status could not be refreshed. Use get_task with this deal_id; reconcile using this exact idempotency_key and input if needed. Do not create another invocation key.".into();
                report.code = "status_unavailable".into();
                return Ok(report);
            }
        };
        if deal.deal_id != report.deal_id
            || deal.idempotency_key.as_deref() != Some(idempotency_key.as_str())
            || deal.quote.hash != report.quote_hash
            || deal.deal.hash != report.deal_hash
        {
            return Err(CliError::Daemon(
                "invocation polling identity mismatch".into(),
            ));
        }
        report = report_from_deal(
            &options.service_id,
            &deal,
            &node_id,
            report.payment_intent_path,
        )?;
        if report.terminal || tokio::time::Instant::now() >= deadline {
            return Ok(finalize_report(report));
        }
    }
}

fn report_from_deal(
    service_id: &str,
    deal: &crate::requester_deals::RequesterDealRecord,
    provider_id: &str,
    payment_intent_path: Option<String>,
) -> Result<InvokeReport, CliError> {
    let chain = crate::protocol::validate_quote_deal(&deal.quote, &deal.deal, None);
    if !chain.valid
        || deal.provider_id != provider_id
        || deal.quote.payload.provider_id != provider_id
        || deal.workload_kind != deal.quote.payload.workload_kind
    {
        return Err(CliError::Daemon("invocation_evidence_invalid: Quote/Deal signatures, links or provider/workload identity do not verify".into()));
    }
    let receipt_verification = verify_invocation_receipt(deal, provider_id)?;
    Ok(InvokeReport {
        stage: "requester_execution".into(),
        code: "invocation_pending".into(),
        retryable: false,
        receipt_verification,
        next_action: "If still pending, use get_task with task_id=deal_id before retrying; reuse its exact idempotency_key, program and input to reconcile.".into(),
        idempotency_key: deal.idempotency_key.clone().unwrap_or_default(),
        service_id: service_id.into(),
        workload_kind: deal.quote.payload.workload_kind.clone(),
        workload_hash: deal.quote.payload.workload_hash.clone(),
        quote_hash: deal.quote.hash.clone(),
        deal_hash: deal.deal.hash.clone(),
        execution_limits: deal.quote.payload.execution_limits.clone(),
        provider_id: provider_id.into(),
        provider_url: deal.provider_url.clone(),
        deal_id: deal.deal_id.clone(),
        status: deal.status.clone(),
        terminal: is_terminal_deal_status(&deal.status),
        result: deal.result.clone(),
        result_hash: deal.result_hash.clone(),
        error: deal.error.clone(),
        payment_intent_path,
    })
}

fn finalize_report(mut report: InvokeReport) -> InvokeReport {
    if SUCCESS_DEAL_STATUSES.contains(&report.status.as_str()) {
        report.stage = "requester_execution_verified".into();
        report.code = "ok".into();
        report.next_action = "Review the result and receipt verification. A signed receipt does not independently establish result quality.".into();
    } else if report.terminal {
        report.code = "execution_failed".into();
        report.next_action = "Inspect the failure and signed evidence before changing the input or starting another call.".into();
    }
    report
}

fn verify_invocation_receipt(
    deal: &crate::requester_deals::RequesterDealRecord,
    provider: &str,
) -> Result<Value, CliError> {
    let Some(receipt) = &deal.receipt else {
        if SUCCESS_DEAL_STATUSES.contains(&deal.status.as_str()) {
            return Err(CliError::Daemon(format!(
                "receipt_missing: successful deal {} has no receipt; execution is not verified",
                deal.deal_id
            )));
        }
        return Ok(serde_json::json!({"status":"not_available", "verified":false}));
    };
    crate::api::verify_provider_receipt_artifact(
        receipt,
        &deal.quote,
        &deal.deal,
        provider,
        &deal.deal.payload.requester_id,
        deal.result.as_ref(),
        deal.result_hash.as_deref(),
    )
    .map_err(|(_, error)| CliError::Daemon(format!("receipt_verification_failed: {error}")))?;
    let signed_status = match receipt.payload.deal_state.as_str() {
        "succeeded" => "succeeded",
        "rejected" => "rejected",
        "failed" | "canceled" => "failed",
        _ => {
            return Err(CliError::Daemon(
                "receipt_verification_failed: unknown signed state".into(),
            ));
        }
    };
    if deal.status != signed_status {
        return Err(CliError::Daemon(
            "receipt_verification_failed: unsigned task status differs from signed Receipt".into(),
        ));
    }
    Ok(
        serde_json::json!({"status":"verified", "verified":true, "receipt_hash":receipt.hash,
        "deal_state":receipt.payload.deal_state,
        "execution_state":receipt.payload.execution_state,
        "settlement_state":receipt.payload.settlement_state,
        "limits_applied":receipt.payload.limits_applied,
        "failure_code":receipt.payload.failure_code,
        "checks":["signatures", "quote_deal_receipt_links", "provider_identity", "result_hash", "receipt_status"],
        "boundary":"Does not independently prove output correctness or external settlement."}),
    )
}

/// Rust mirror of `buildServiceAddressedExecution` in
/// `integrations/shared/froglet-lib/froglet-client.js`. Field-for-field
/// parity matters: the daemon hashes this workload into the quote, so a
/// shape divergence between the CLI and the JS client would produce
/// different `workload_hash`es for the same service + input.
pub(crate) fn build_service_addressed_execution(
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

    let bound_builtin = package_kind == ExecutionPackageKind::Builtin && binding_hash.is_some();
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
            service_id: (package_kind != ExecutionPackageKind::Builtin || bound_builtin)
                .then(|| service.service_id.clone()),
            request_envelope: None,
        },
        mounts: service.mounts.clone(),
        input,
        module_hash: None,
        module_bytes_hex: None,
        source_hash: None,
        inline_source: None,
        python_bundle: None,
        oci_reference: None,
        oci_digest: None,
        builtin_name: None,
    };
    match package_kind {
        ExecutionPackageKind::InlineSource => workload.source_hash = binding_hash,
        ExecutionPackageKind::InlineModule | ExecutionPackageKind::OciImage => {
            workload.module_hash = binding_hash
        }
        ExecutionPackageKind::Builtin => {
            workload.builtin_name = builtin_name;
            workload.module_hash = binding_hash;
        }
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
pub(crate) async fn resolve_runtime_auth_token() -> Result<String, CliError> {
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
    let mut builder = http
        .post(&url)
        .bearer_auth(&options.runtime_token)
        .json(request);
    if let Some(path) = options.access_token_file.as_deref() {
        let token = read_access_token_file(path)?;
        let mut value = reqwest::header::HeaderValue::from_str(&token)
            .map_err(|_| CliError::BadArgs("invalid access token".into()))?;
        value.set_sensitive(true);
        builder = builder.header("x-froglet-access-token", value);
    }
    let response = builder.send().await.map_err(|error| {
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
    if let Some(deal_id) = body.get("deal_id").and_then(Value::as_str)
        && !deal_id.is_empty()
        && deal_id.len() <= 256
        && !deal_id.chars().any(char::is_control)
    {
        // A configured transport may persist requester intent before a lost
        // provider response. Keep the durable reference; acceptance is unknown.
        let error = CliError::Daemon(format!(
            "POST {url} returned HTTP {status}: {}",
            body.get("error")
                .and_then(Value::as_str)
                .unwrap_or("submission state is uncertain")
        ));
        let mut report = super::doctor::error_report(&error);
        report["deal_id"] = serde_json::json!(deal_id);
        report["task_id"] = serde_json::json!(deal_id);
        report["idempotency_key"] = serde_json::json!(request.idempotency_key);
        if body.get("status").and_then(Value::as_str) == Some("submission_pending") {
            report["status"] = serde_json::json!("submission_pending");
        }
        report["receipt_verification"] =
            serde_json::json!({"status":"not_checked", "verified":false});
        report["next_action"] = serde_json::json!(
            "Use get_task with this existing task reference. Provider acceptance is not yet established; do not submit with another key or authorize payment merely to recover."
        );
        return Err(CliError::Structured {
            report,
            exit_code: 1,
        });
    }
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
    let mut last_error = None;
    let mut response = None;
    for attempt in 0..2 {
        match http
            .get(&url)
            .bearer_auth(&options.runtime_token)
            .send()
            .await
        {
            Ok(value) if !value.status().is_server_error() || attempt == 1 => {
                response = Some(value);
                break;
            }
            Ok(value) => last_error = Some(format!("HTTP {}", value.status())),
            Err(error) => last_error = Some(error.to_string()),
        }
        if attempt == 0 {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    let response = response.ok_or_else(|| {
        CliError::Daemon(format!(
            "GET {url} failed: {}",
            last_error.unwrap_or_default()
        ))
    })?;
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

fn print_human_report(report: &InvokeReport) -> Result<(), CliError> {
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
        let encoded = serde_json::to_string_pretty(result)
            .map_err(|error| CliError::Other(format!("failed to serialize result: {error}")))?;
        println!("result:\n{encoded}");
    } else if !report.terminal {
        println!("result:   (pending — deal has not reached a terminal state)");
    }
    Ok(())
}

/// Never accept an invite through command-line token text, URLs, or MCP output.
pub fn read_access_token_file(
    path: &std::path::Path,
) -> Result<zeroize::Zeroizing<String>, CliError> {
    if !path.is_absolute() {
        return Err(CliError::BadArgs(
            "access_token_file must be absolute".into(),
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 257 {
        return Err(CliError::BadArgs(
            "access token must be a small regular file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o600 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(CliError::BadArgs(
                "access token file must be owned by this user with mode 0600".into(),
            ));
        }
    }
    let mut token = String::new();
    file.take(258).read_to_string(&mut token)?;
    let token = zeroize::Zeroizing::new(token.trim_end_matches(['\n', '\r']).to_string());
    if !(32..=256).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(CliError::BadArgs("invalid access token file".into()));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::ExecutionMount;
    use serde_json::json;

    #[test]
    fn remote_endpoint_selection_keeps_https_quote_feature_and_priority() {
        assert_eq!(
            select_remote_endpoint(&json!([
                {"uri":"http://plain.example", "priority":0, "features":["quote_http"]},
                {"uri":"https://other.example", "priority":0},
                {"uri":"https://slow.example", "priority":9, "features":["quote_http"]},
                {"uri":"https://fast.example", "priority":1, "features":["quote_http"]}
            ]))
            .unwrap(),
            "https://fast.example"
        );
        for invalid in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/path",
            "https://example.com/?secret=1",
        ] {
            assert!(remote_origin(invalid).is_err());
        }
    }

    #[test]
    fn remote_metadata_requires_exact_identity_and_free_terms() {
        let mut service = python_service_record();
        service.settlement_method = "none".into();
        assert!(validate_remote_service(&service, "text.summarize", &"aa".repeat(32), 0).is_ok());
        assert!(validate_remote_service(&service, "other", &"aa".repeat(32), 0).is_err());
        assert!(validate_remote_service(&service, "text.summarize", &"bb".repeat(32), 0).is_err());
        service.success_fee_msat = 1;
        assert!(
            validate_remote_service(&service, "text.summarize", &"aa".repeat(32), 0)
                .unwrap_err()
                .to_string()
                .contains("payment_required")
        );
    }

    #[test]
    fn remote_paid_calls_require_an_explicit_sufficient_lightning_cap() {
        let mut service = python_service_record();
        service.price_sats = 30;
        service.base_fee_msat = 30_000;
        service.settlement_method = "lightning.prepaid.v1".into();
        service.price_currency = Some("sat".into());
        let validate = |service: &ProviderServiceRecord, cap| {
            validate_remote_service(service, "text.summarize", &"aa".repeat(32), cap)
        };
        assert!(validate(&service, 0).is_err());
        assert!(validate(&service, 29).is_err());
        assert!(validate(&service, 30).is_ok());
        service.success_fee_msat = 1;
        assert!(validate(&service, 30).is_err());
        assert!(validate(&service, 31).is_ok());
        service.price_currency = Some("usd".into());
        assert!(validate(&service, 31).is_err());
        service.price_currency = Some("sat".into());
        service.settlement_method = "stripe_mpp.v1".into();
        assert!(validate(&service, 31).is_err());
        service.settlement_method = "lightning.prepaid.v1".into();
        service.base_fee_msat = u64::MAX;
        assert!(validate(&service, u64::MAX).is_err());
    }

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
    fn immutable_builtin_service_is_service_addressed_and_keeps_binding() {
        let binding_hash = "cc".repeat(32);
        let mut service = python_service_record();
        service.runtime = "builtin".to_string();
        service.package_kind = "builtin".to_string();
        service.entrypoint_kind = "builtin".to_string();
        service.entrypoint = "data.catalog".to_string();
        service.service_id = "data.catalog".to_string();
        service.contract_version = crate::builtins::DATA_QUERY_JSON_CONTRACT_V1.to_string();
        service.binding_hash = Some(binding_hash.clone());

        let workload =
            build_service_addressed_execution(&service, json!({"limit": 10})).expect("workload");

        assert_eq!(workload.builtin_name.as_deref(), Some("data.catalog"));
        assert_eq!(workload.module_hash.as_deref(), Some(binding_hash.as_str()));
        assert_eq!(workload.binding_hash(), Some(binding_hash.as_str()));
        assert_eq!(
            workload.security.service_id.as_deref(),
            Some("data.catalog")
        );
        assert!(workload.is_service_addressed());
        assert!(workload.validate_basic().is_ok());
        assert_eq!(
            workload.request_hash().expect("request hash"),
            "045a1210bc07d291d91916beb4acfac6a801e1ddc0cbb0f44a0d6728bd7bc67f"
        );
    }

    #[test]
    fn immutable_builtin_service_accepts_legacy_module_hash_field() {
        let binding_hash = "dd".repeat(32);
        let mut service = python_service_record();
        service.runtime = "builtin".to_string();
        service.package_kind = "builtin".to_string();
        service.entrypoint_kind = "builtin".to_string();
        service.entrypoint = "data.legacy".to_string();
        service.service_id = "data.legacy".to_string();
        service.contract_version = crate::builtins::DATA_QUERY_JSON_CONTRACT_V1.to_string();
        service.binding_hash = None;
        service.module_hash = Some(binding_hash.clone());

        let workload = build_service_addressed_execution(&service, Value::Null).expect("workload");

        assert_eq!(workload.module_hash.as_deref(), Some(binding_hash.as_str()));
        assert_eq!(workload.security.service_id.as_deref(), Some("data.legacy"));
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
    fn inline_compute_normalizes_bytes_and_bounds_input_before_submission() {
        let module = "0061736D01000000";
        let built = build_inline_wasm_submission(module, serde_json::json!({"b":2,"a":1})).unwrap();
        assert_eq!(built.module_bytes_hex, module.to_lowercase());
        assert_eq!(
            built.workload.abi_version,
            crate::wasm::WASM_RUN_JSON_ABI_V1
        );
        assert!(built.workload.requested_capabilities.is_empty());
        assert!(built.verify().is_ok());
        for module in ["", "not-wasm", "0", "0061736d02000000"] {
            assert!(build_inline_wasm_submission(module, Value::Null).is_err());
        }
        assert!(
            build_inline_wasm_submission(&"00".repeat(MAX_INLINE_WASM_HEX_BYTES), Value::Null)
                .is_err()
        );
        assert!(
            build_inline_wasm_submission(
                module,
                serde_json::json!("x".repeat(MAX_INLINE_WASM_INPUT_BYTES))
            )
            .is_err()
        );
    }

    #[test]
    fn native_key_limit_matches_runtime_utf8_byte_limit() {
        assert!(validate_idempotency_key(&"é".repeat(64)).is_ok());
        for key in ["".into(), " ".into(), "x".repeat(129), "é".repeat(65)] {
            assert!(validate_idempotency_key(&key).is_err());
        }
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
