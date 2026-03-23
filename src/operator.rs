use std::{net::SocketAddr, path::{Path, PathBuf}, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

use crate::{
    api::{
        self, ProviderControlPublishArtifactRequest,
        ProviderServiceRecord, ProviderServiceResponse, ProviderServicesResponse,
        RuntimeCreateDealRequest, RuntimeCreateDealResponse, RuntimeDealResponse,
        RuntimeProviderDetailsResponse, RuntimeProviderRef, RuntimeSearchRequest,
        artifact_provider_offer_definition, current_service_records,
        normalize_offer_publication_state, persist_provider_offer_mutation,
        provider_service_record,
    },
    config::NodeConfig,
    provider_projects::{self, ProviderProjectBuildRecord, ProviderProjectRecord, ProviderProjectStarter, ProviderProjectTestRecord},
    requester_deals,
    runtime_auth,
    state::{self, AppState},
    tls,
};

const DEFAULT_OPERATOR_LISTEN_ADDR: &str = "127.0.0.1:9191";
const DEFAULT_TAIL_LINES: usize = 100;
const MAX_TAIL_LINES: usize = 500;

#[derive(Clone)]
pub struct OperatorState {
    pub app_state: Arc<AppState>,
    pub projects_root: PathBuf,
    pub runtime_log_path: Option<PathBuf>,
    pub provider_log_path: Option<PathBuf>,
    pub runtime_restart_command: Option<String>,
    pub provider_restart_command: Option<String>,
}

#[derive(Debug, Serialize)]
struct OperatorHealthResponse {
    status: &'static str,
    service: &'static str,
}

#[derive(Debug, Deserialize)]
struct LogQuery {
    #[serde(default)]
    lines: Option<usize>,
    #[serde(default)]
    target: Option<String>,
}

#[derive(Debug, Serialize)]
struct ConsumerStatusResponse {
    service: &'static str,
    healthy: bool,
    runtime_url: String,
    runtime_auth_token_path: String,
    control_auth_token_path: String,
    restart_supported: bool,
    log_path: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProviderStatusResponse {
    service: &'static str,
    healthy: bool,
    provider_url: String,
    control_auth_token_path: String,
    projects_root: String,
    restart_supported: bool,
    log_path: Option<String>,
}

#[derive(Debug, Serialize)]
struct LogTailResponse {
    service: &'static str,
    log_path: Option<String>,
    line_count: usize,
    lines: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RestartResponse {
    service: String,
    status: String,
    restart_supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr_preview: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProviderProjectsResponse {
    projects: Vec<ProviderProjectRecord>,
}

#[derive(Debug, Serialize)]
struct ProviderProjectResponse {
    project: ProviderProjectRecord,
}

#[derive(Debug, Serialize)]
struct ProviderProjectFileResponse {
    project_id: String,
    path: String,
    contents: String,
}

#[derive(Debug, Serialize)]
struct ProviderProjectWriteResponse {
    status: &'static str,
    project_id: String,
    path: String,
}

#[derive(Debug, Deserialize)]
struct CreateProjectRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    service_id: Option<String>,
    #[serde(default)]
    offer_id: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    starter: Option<String>,
    #[serde(default)]
    price_sats: Option<u64>,
    #[serde(default)]
    publication_state: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    input_schema: Option<Value>,
    #[serde(default)]
    output_schema: Option<Value>,
    #[serde(default)]
    result_json: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct WriteProjectFileRequest {
    contents: String,
}

#[derive(Debug, Deserialize)]
struct TestProjectRequest {
    #[serde(default)]
    input: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ServiceDiscoverRequest {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    include_inactive: Option<bool>,
    #[serde(default)]
    query: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ServiceLookupRequest {
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    provider_url: Option<String>,
    service_id: String,
}

#[derive(Debug, Deserialize)]
struct InvokeServiceRequest {
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    provider_url: Option<String>,
    service_id: String,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RunComputeRequest {
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    provider_url: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    wasm_module_hex: Option<String>,
    #[serde(default)]
    oci_reference: Option<String>,
    #[serde(default)]
    oci_digest: Option<String>,
    #[serde(default)]
    execution_kind: Option<String>,
    #[serde(default)]
    abi_version: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RestartRequest {
    #[serde(default)]
    target: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WaitTaskRequest {
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    poll_interval_secs: Option<f64>,
}

#[derive(Debug, Serialize)]
struct FrogletStatusResponse {
    node_id: String,
    projects_root: String,
    raw_compute_offer_id: &'static str,
    runtime: ConsumerStatusResponse,
    provider: ProviderStatusResponse,
}

#[derive(Debug, Serialize)]
struct FrogletLogsResponse {
    logs: Vec<LogTailResponse>,
}

#[derive(Debug, Serialize)]
struct FrogletRestartResult {
    target: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr_preview: Option<String>,
}

#[derive(Debug, Serialize)]
struct FrogletRestartResponse {
    results: Vec<FrogletRestartResult>,
}

#[derive(Debug, Serialize)]
struct FrogletServiceDiscoverResponse {
    services: Vec<ProviderServiceRecord>,
}

#[derive(Debug, Serialize)]
struct FrogletServiceActionResponse {
    status: String,
    terminal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task: Option<requester_deals::RequesterDealRecord>,
}

#[derive(Debug, Serialize)]
struct FrogletTaskResponse {
    task: requester_deals::RequesterDealRecord,
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

fn error_json(status: StatusCode, body: serde_json::Value) -> Response {
    (status, Json(body)).into_response()
}

fn require_provider_control_auth(headers: &HeaderMap, state: &OperatorState) -> Result<(), Response> {
    match api::require_provider_control_auth(headers, state.app_state.as_ref()) {
        Ok(()) => Ok(()),
        Err((status, body)) => Err(error_json(status, body)),
    }
}

fn require_froglet_auth(headers: &HeaderMap, state: &OperatorState) -> Result<(), Response> {
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Err(error_json(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "missing froglet authorization" }),
        ));
    };
    let Ok(value) = value.to_str() else {
        return Err(error_json(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "invalid froglet authorization header" }),
        ));
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(error_json(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "froglet authorization must use bearer auth" }),
        ));
    };
    if token == state.app_state.consumer_control_auth_token
        || token == state.app_state.provider_control_auth_token
    {
        return Ok(());
    }
    Err(error_json(
        StatusCode::UNAUTHORIZED,
        json!({ "error": "invalid froglet authorization token" }),
    ))
}

fn loopback_base_url(listen_addr: &str) -> Result<String, String> {
    let addr: SocketAddr = listen_addr
        .parse()
        .map_err(|error| format!("invalid listen address {listen_addr}: {error}"))?;
    let host = if addr.ip().is_unspecified() {
        "127.0.0.1".to_string()
    } else if addr.ip().is_loopback() {
        addr.ip().to_string()
    } else {
        addr.ip().to_string()
    };
    Ok(format!("http://{}:{}", host, addr.port()))
}

async fn health_reachable(state: &OperatorState, url: &str) -> bool {
    match state
        .app_state
        .http_client
        .get(format!("{url}/health"))
        .send()
        .await
    {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

async fn local_json_request<T, B>(
    state: &OperatorState,
    bearer_token: &str,
    method: reqwest::Method,
    url: String,
    body: Option<&B>,
) -> Result<T, Response>
where
    T: serde::de::DeserializeOwned,
    B: serde::Serialize + ?Sized,
{
    let mut request = state
        .app_state
        .http_client
        .request(method, &url)
        .bearer_auth(bearer_token);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|error| {
        error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "upstream request failed", "details": error.to_string(), "url": url }),
        )
    })?;
    let status = response.status();
    let body_text = response.text().await.map_err(|error| {
        error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "failed to read upstream response", "details": error.to_string(), "url": url }),
        )
    })?;
    if !status.is_success() {
        let payload = serde_json::from_str::<Value>(&body_text).unwrap_or_else(|_| {
            json!({ "error": "upstream request failed", "body": body_text, "url": url })
        });
        return Err(error_json(status, payload));
    }
    serde_json::from_str(&body_text).map_err(|error| {
        error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "failed to decode upstream JSON", "details": error.to_string(), "url": url }),
        )
    })
}

fn runtime_base_url(state: &OperatorState) -> Result<String, Response> {
    loopback_base_url(&state.app_state.config.runtime_listen_addr)
        .map_err(|error| error_json(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": error })))
}

fn provider_base_url(state: &OperatorState) -> Result<String, Response> {
    loopback_base_url(&state.app_state.config.listen_addr)
        .map_err(|error| error_json(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": error })))
}

fn truncate_preview(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(500).collect())
}

fn read_tail_lines(path: &Path, requested_lines: Option<usize>) -> Result<Vec<String>, String> {
    let limit = requested_lines
        .unwrap_or(DEFAULT_TAIL_LINES)
        .clamp(1, MAX_TAIL_LINES);
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read log file {}: {error}", path.display()))?;
    let mut lines = contents.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.len() > limit {
        lines.drain(0..(lines.len() - limit));
    }
    Ok(lines)
}

async fn run_restart_command(command: Option<&str>, service: &'static str) -> Response {
    let Some(command) = command else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(RestartResponse {
                service: service.to_string(),
                status: "unsupported".to_string(),
                restart_supported: false,
                command: None,
                stdout_preview: None,
                stderr_preview: None,
            }),
        )
            .into_response();
    };
    let output = match Command::new("/bin/sh")
        .arg("-lc")
        .arg(command)
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to execute restart command: {error}") }),
            )
        }
    };
    let status = if output.status.success() {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    (
        status,
        Json(RestartResponse {
            service: service.to_string(),
            status: if output.status.success() {
                "restarted".to_string()
            } else {
                "failed".to_string()
            },
            restart_supported: true,
            command: Some(command.to_string()),
            stdout_preview: truncate_preview(&String::from_utf8_lossy(&output.stdout)),
            stderr_preview: truncate_preview(&String::from_utf8_lossy(&output.stderr)),
        }),
    )
        .into_response()
}

fn project_definition_from_build(
    state: &OperatorState,
    build: &ProviderProjectBuildRecord,
) -> Result<crate::api::ProviderManagedOfferDefinition, Response> {
    let mut definition = artifact_provider_offer_definition(
        state.app_state.as_ref(),
        ProviderControlPublishArtifactRequest {
            service_id: build.project.service_id.clone(),
            offer_id: Some(build.project.offer_id.clone()),
            artifact_path: Some(build.build_artifact_path.clone()),
            wasm_module_hex: None,
            oci_reference: None,
            oci_digest: None,
            execution_kind: Some(build.project.execution_kind.clone()),
            abi_version: Some(build.abi_version.clone()),
            summary: Some(build.project.summary.clone()),
            mode: Some(build.project.mode.clone()),
            price_sats: build.project.price_sats,
            publication_state: Some(build.project.publication_state.clone()),
            input_schema: build.project.input_schema.clone(),
            output_schema: build.project.output_schema.clone(),
        },
    )
    .map_err(|(status, body)| error_json(status, body))?;
    definition.project_id = Some(build.project.project_id.clone());
    definition.starter = build.project.starter.clone();
    definition.source_kind = "project".to_string();
    definition.source_path = Some(build.build_artifact_path.clone());
    Ok(definition)
}

async fn operator_health() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(OperatorHealthResponse {
            status: "ok",
            service: "froglet-operator",
        }),
    )
}

async fn provider_list_projects(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
) -> Response {
    if let Err(response) = require_provider_control_auth(&headers, state.as_ref()) {
        return response;
    }
    match provider_projects::list_projects(&state.projects_root) {
        Ok(projects) => (StatusCode::OK, Json(ProviderProjectsResponse { projects })).into_response(),
        Err(error) => error_json(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": error })),
    }
}

async fn provider_create_project(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    Json(payload): Json<CreateProjectRequest>,
) -> Response {
    if let Err(response) = require_provider_control_auth(&headers, state.as_ref()) {
        return response;
    }
    let starter = match payload.starter.as_deref() {
        Some(value) => match ProviderProjectStarter::parse(value) {
            Ok(starter) => Some(starter),
            Err(error) => return error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
        },
        None => None,
    };
    let publication_state = match normalize_offer_publication_state(payload.publication_state.as_deref()) {
        Ok(value) => value,
        Err(error) => return error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
    };
    let (project_id, service_id, offer_id) = match derive_project_identifiers(&payload) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let summary = default_project_summary(&payload, &service_id);
    let scaffold_result_json = infer_static_result_json(&payload);
    match provider_projects::create_project(
        &state.projects_root,
        &project_id,
        &service_id,
        &offer_id,
        starter,
        &summary,
        payload.price_sats.unwrap_or(0),
        &publication_state,
    ) {
        Ok(_) => {
            let project_dir = match provider_projects::project_dir(&state.projects_root, &project_id) {
                Ok(path) => path,
                Err(error) => return error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
            };
            if let Some(result_json) = scaffold_result_json.clone() {
                if let Err(error) =
                    provider_projects::write_static_result_project(&state.projects_root, &project_id, &result_json)
                {
                    return error_json(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "failed to scaffold static result project", "details": error }),
                    );
                }
            }
            let mut manifest = match provider_projects::load_manifest(&project_dir) {
                Ok(manifest) => manifest,
                Err(error) => {
                    return error_json(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({ "error": "failed to reload provider project manifest", "details": error }),
                    )
                }
            };
            if let Some(mode) = payload.mode.clone() {
                manifest.mode = mode;
            }
            if scaffold_result_json.is_some() {
                manifest.starter = None;
            }
            manifest.input_schema = payload.input_schema.clone();
            manifest.output_schema = payload
                .output_schema
                .clone()
                .or_else(|| scaffold_result_json.clone().map(|value| json!({ "const": value })));
            if let Err(error) = provider_projects::save_manifest(&project_dir, &manifest) {
                return error_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "failed to update provider project manifest", "details": error }),
                );
            }
            match provider_projects::get_project(&state.projects_root, &project_id) {
                Ok(project) => (StatusCode::CREATED, Json(ProviderProjectResponse { project })).into_response(),
                Err(error) => error_json(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": error })),
            }
        }
        Err(error) => error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
    }
}

async fn provider_get_project(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    AxumPath(project_id): AxumPath<String>,
) -> Response {
    if let Err(response) = require_provider_control_auth(&headers, state.as_ref()) {
        return response;
    }
    match provider_projects::get_project(&state.projects_root, &project_id) {
        Ok(project) => (StatusCode::OK, Json(ProviderProjectResponse { project })).into_response(),
        Err(error) if error.contains("No such file") || error.contains("failed to read project manifest") => {
            error_json(StatusCode::NOT_FOUND, json!({ "error": "project not found", "project_id": project_id }))
        }
        Err(error) => error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
    }
}

async fn provider_read_file(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    AxumPath((project_id, relative_path)): AxumPath<(String, String)>,
) -> Response {
    if let Err(response) = require_provider_control_auth(&headers, state.as_ref()) {
        return response;
    }
    match provider_projects::read_project_file(&state.projects_root, &project_id, &relative_path) {
        Ok(contents) => (
            StatusCode::OK,
            Json(ProviderProjectFileResponse {
                project_id,
                path: relative_path,
                contents,
            }),
        )
            .into_response(),
        Err(error) => error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
    }
}

async fn provider_write_file(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    AxumPath((project_id, relative_path)): AxumPath<(String, String)>,
    Json(payload): Json<WriteProjectFileRequest>,
) -> Response {
    if let Err(response) = require_provider_control_auth(&headers, state.as_ref()) {
        return response;
    }
    match provider_projects::write_project_file(
        &state.projects_root,
        &project_id,
        &relative_path,
        &payload.contents,
    ) {
        Ok(()) => (
            StatusCode::OK,
            Json(ProviderProjectWriteResponse {
                status: "written",
                project_id,
                path: relative_path,
            }),
        )
            .into_response(),
        Err(error) => error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
    }
}

async fn provider_build_project(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    AxumPath(project_id): AxumPath<String>,
) -> Response {
    if let Err(response) = require_provider_control_auth(&headers, state.as_ref()) {
        return response;
    }
    match provider_projects::build_project(&state.projects_root, &project_id) {
        Ok(build) => (StatusCode::OK, Json(build)).into_response(),
        Err(error) => error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
    }
}

async fn provider_test_project(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    AxumPath(project_id): AxumPath<String>,
    Json(payload): Json<TestProjectRequest>,
) -> Response {
    if let Err(response) = require_provider_control_auth(&headers, state.as_ref()) {
        return response;
    }
    match provider_projects::test_project(
        &state.projects_root,
        state.app_state.as_ref(),
        &project_id,
        payload.input,
    ) {
        Ok(result) => (StatusCode::OK, Json::<ProviderProjectTestRecord>(result)).into_response(),
        Err(error) => error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
    }
}

async fn provider_publish_project(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    AxumPath(project_id): AxumPath<String>,
) -> Response {
    if let Err(response) = require_provider_control_auth(&headers, state.as_ref()) {
        return response;
    }
    let build = match provider_projects::build_project(&state.projects_root, &project_id) {
        Ok(build) => build,
        Err(error) => {
            return error_json(
                StatusCode::BAD_REQUEST,
                json!({ "error": "provider project build failed", "details": error }),
            )
        }
    };
    let definition = match project_definition_from_build(state.as_ref(), &build) {
        Ok(definition) => definition,
        Err(response) => return response,
    };
    match persist_provider_offer_mutation(
        state.app_state.as_ref(),
        definition.clone(),
        StatusCode::CREATED,
        format!(
            "published provider project {} from project {}",
            definition.offer_id,
            build.project.project_id
        ),
    )
    .await
    {
        Ok(response) => response.into_response(),
        Err((status, body)) => error_json(status, body),
    }
}

async fn provider_publish_artifact(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    Json(payload): Json<ProviderControlPublishArtifactRequest>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    let definition = match artifact_provider_offer_definition(state.app_state.as_ref(), payload) {
        Ok(definition) => definition,
        Err((status, body)) => return error_json(status, body),
    };
    match persist_provider_offer_mutation(
        state.app_state.as_ref(),
        definition.clone(),
        StatusCode::CREATED,
        format!("published provider artifact {}", definition.offer_id),
    )
    .await
    {
        Ok(response) => response.into_response(),
        Err((status, body)) => error_json(status, body),
    }
}

async fn build_consumer_status(state: &OperatorState) -> Result<ConsumerStatusResponse, Response> {
    let runtime_url = runtime_base_url(state)?;
    Ok(ConsumerStatusResponse {
        service: "consumer",
        healthy: health_reachable(state, &runtime_url).await,
        runtime_url,
        runtime_auth_token_path: state.app_state.runtime_auth_token_path.display().to_string(),
        control_auth_token_path: state
            .app_state
            .consumer_control_auth_token_path
            .display()
            .to_string(),
        restart_supported: state.runtime_restart_command.is_some(),
        log_path: state
            .runtime_log_path
            .as_ref()
            .map(|path| path.display().to_string()),
    })
}

async fn build_provider_status(state: &OperatorState) -> Result<ProviderStatusResponse, Response> {
    let provider_url = provider_base_url(state)?;
    Ok(ProviderStatusResponse {
        service: "provider",
        healthy: health_reachable(state, &provider_url).await,
        provider_url,
        control_auth_token_path: state
            .app_state
            .provider_control_auth_token_path
            .display()
            .to_string(),
        projects_root: state.projects_root.display().to_string(),
        restart_supported: state.provider_restart_command.is_some(),
        log_path: state
            .provider_log_path
            .as_ref()
            .map(|path| path.display().to_string()),
    })
}

fn preferred_provider_url(details: &RuntimeProviderDetailsResponse) -> Result<String, Response> {
    details
        .discovery
        .descriptor
        .transports
        .clearnet_url
        .clone()
        .or_else(|| details.discovery.descriptor.transports.onion_url.clone())
        .ok_or_else(|| {
            error_json(
                StatusCode::BAD_GATEWAY,
                json!({ "error": "provider discovery record does not expose a usable URL" }),
            )
        })
}

async fn resolve_provider_reference(
    state: &OperatorState,
    provider_id: Option<&str>,
    provider_url: Option<&str>,
) -> Result<(Option<String>, String), Response> {
    if let Some(url) = provider_url {
        return Ok((provider_id.map(str::to_string), url.to_string()));
    }
    let Some(provider_id) = provider_id else {
        return Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "provider_id or provider_url is required" }),
        ));
    };
    let runtime_url = runtime_base_url(state)?;
    let details: RuntimeProviderDetailsResponse = local_json_request(
        state,
        &state.app_state.runtime_auth_token,
        reqwest::Method::GET,
        format!("{runtime_url}/v1/runtime/providers/{provider_id}"),
        Option::<&Value>::None,
    )
    .await?;
    Ok((Some(provider_id.to_string()), preferred_provider_url(&details)?))
}

async fn fetch_provider_service(
    state: &OperatorState,
    provider_id: Option<&str>,
    provider_url: Option<&str>,
    service_id: &str,
) -> Result<ProviderServiceRecord, Response> {
    if provider_id.is_none() && provider_url.is_none() {
        if let Ok(Some(local_service)) =
            provider_service_record(state.app_state.as_ref(), service_id, true).await
        {
            return Ok(local_service);
        }

        let runtime_url = runtime_base_url(state)?;
        let search: crate::discovery::DiscoverySearchResponse = local_json_request(
            state,
            &state.app_state.runtime_auth_token,
            reqwest::Method::POST,
            format!("{runtime_url}/v1/runtime/search"),
            Some(&RuntimeSearchRequest {
                limit: Some(50),
                include_inactive: Some(false),
            }),
        )
        .await?;
        let mut matches = Vec::new();
        for node in search.nodes {
            let provider_url = node
                .descriptor
                .transports
                .clearnet_url
                .clone()
                .or_else(|| node.descriptor.transports.onion_url.clone());
            let Some(provider_url) = provider_url else {
                continue;
            };
            let response: Result<ProviderServiceResponse, Response> = local_json_request(
                state,
                &state.app_state.runtime_auth_token,
                reqwest::Method::GET,
                format!(
                    "{}/v1/provider/services/{}",
                    provider_url,
                    urlencoding::encode(service_id)
                ),
                Option::<&Value>::None,
            )
            .await;
            if let Ok(response) = response {
                matches.push(response.service);
            }
        }
        return match matches.len() {
            1 => Ok(matches.remove(0)),
            0 => Err(error_json(
                StatusCode::NOT_FOUND,
                json!({ "error": "service not found", "service_id": service_id }),
            )),
            _ => Err(error_json(
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "service_id matched multiple providers; specify provider_id or provider_url",
                    "service_id": service_id,
                    "provider_ids": matches.into_iter().map(|service| service.provider_id).collect::<Vec<_>>()
                }),
            )),
        };
    }
    let (_, resolved_provider_url) =
        resolve_provider_reference(state, provider_id, provider_url).await?;
    let response: ProviderServiceResponse = local_json_request(
        state,
        &state.app_state.runtime_auth_token,
        reqwest::Method::GET,
        format!(
            "{}/v1/provider/services/{}",
            resolved_provider_url,
            urlencoding::encode(service_id)
        ),
        Option::<&Value>::None,
    )
    .await?;
    Ok(response.service)
}

async fn get_runtime_task(state: &OperatorState, deal_id: &str) -> Result<requester_deals::RequesterDealRecord, Response> {
    let runtime_url = runtime_base_url(state)?;
    let response: RuntimeDealResponse = local_json_request(
        state,
        &state.app_state.runtime_auth_token,
        reqwest::Method::GET,
        format!("{runtime_url}/v1/runtime/deals/{deal_id}"),
        Option::<&Value>::None,
    )
    .await?;
    Ok(response.deal)
}

fn task_is_terminal(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "rejected")
}

fn slugify_identifier(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_separator = false;
    for character in value.trim().chars() {
        let normalized = character.to_ascii_lowercase();
        if normalized.is_ascii_lowercase() || normalized.is_ascii_digit() {
            slug.push(normalized);
            previous_separator = false;
            continue;
        }
        if matches!(normalized, '.' | '_' | '-') {
            if !slug.is_empty() {
                slug.push(normalized);
                previous_separator = false;
            }
            continue;
        }
        if !slug.is_empty() && !previous_separator {
            slug.push('-');
            previous_separator = true;
        }
    }
    while slug.ends_with(['-', '.', '_']) {
        slug.pop();
    }
    slug
}

fn derive_project_identifiers(payload: &CreateProjectRequest) -> Result<(String, String, String), Response> {
    let seed = payload
        .project_id
        .as_deref()
        .or(payload.service_id.as_deref())
        .or(payload.offer_id.as_deref())
        .or(payload.name.as_deref())
        .or(payload.summary.as_deref())
        .ok_or_else(|| {
            error_json(
                StatusCode::BAD_REQUEST,
                json!({ "error": "create_project requires at least one of name, project_id, service_id, offer_id, or summary" }),
            )
        })?;
    let derived = slugify_identifier(seed);
    if derived.is_empty() {
        return Err(error_json(
            StatusCode::BAD_REQUEST,
            json!({ "error": "create_project name could not be normalized into a valid identifier" }),
        ));
    }
    Ok((
        payload.project_id.clone().unwrap_or_else(|| derived.clone()),
        payload.service_id.clone().unwrap_or_else(|| derived.clone()),
        payload.offer_id.clone().unwrap_or(derived),
    ))
}

fn default_project_summary(payload: &CreateProjectRequest, service_id: &str) -> String {
    if let Some(summary) = payload.summary.as_ref().map(|value| value.trim()).filter(|value| !value.is_empty()) {
        return summary.to_string();
    }
    if let Some(result_json) = payload.result_json.as_ref() {
        return format!("Returns {}", result_json);
    }
    format!("Froglet service {service_id}")
}

fn infer_static_result_json(payload: &CreateProjectRequest) -> Option<Value> {
    if let Some(result_json) = payload.result_json.clone() {
        return Some(result_json);
    }
    if payload.starter.is_some() {
        return None;
    }
    let summary = payload.summary.as_ref()?.trim();
    let lower = summary.to_ascii_lowercase();
    let marker = "returns ";
    let start = lower.find(marker)?;
    let rest = summary[start + marker.len()..].trim_start();
    let mut chars = rest.chars();
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let after = &rest[quote.len_utf8()..];
    let end = after.find(quote)?;
    Some(Value::String(after[..end].to_string()))
}

async fn froglet_status(headers: HeaderMap, State(state): State<Arc<OperatorState>>) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    let (runtime_result, provider_result) = tokio::join!(
        build_consumer_status(state.as_ref()),
        build_provider_status(state.as_ref()),
    );
    let runtime = match runtime_result {
        Ok(runtime) => runtime,
        Err(response) => return response,
    };
    let provider = match provider_result {
        Ok(provider) => provider,
        Err(response) => return response,
    };
    (
        StatusCode::OK,
        Json(FrogletStatusResponse {
            node_id: state.app_state.identity.node_id().to_string(),
            projects_root: state.projects_root.display().to_string(),
            raw_compute_offer_id: "execute.wasm",
            runtime,
            provider,
        }),
    )
        .into_response()
}

async fn froglet_tail_logs(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    Query(query): Query<LogQuery>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    let requested_target = query.lines.map(|_| ()).and(None::<()>);
    let _ = requested_target;
    let mut logs = Vec::new();
    let targets = match query.target.as_deref() {
        Some("runtime") => vec!["runtime"],
        Some("provider") => vec!["provider"],
        _ => vec!["runtime", "provider"],
    };
    for entry in targets {
        let (path, label) = if entry == "runtime" {
            (state.runtime_log_path.as_ref(), "runtime")
        } else {
            (state.provider_log_path.as_ref(), "provider")
        };
        let Some(path) = path else { continue };
        match read_tail_lines(path, query.lines) {
            Ok(lines) => logs.push(LogTailResponse {
                service: "froglet",
                log_path: Some(path.display().to_string()),
                line_count: lines.len(),
                lines,
            }),
            Err(error) => {
                return error_json(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": error }));
            }
        }
        if let Some(last) = logs.last_mut() {
            last.service = label;
        }
    }
    (StatusCode::OK, Json(FrogletLogsResponse { logs })).into_response()
}

async fn froglet_restart(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    Json(payload): Json<RestartRequest>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    let targets = match payload.target.as_deref() {
        Some("runtime") => vec!["runtime"],
        Some("provider") => vec!["provider"],
        _ => vec!["runtime", "provider"],
    };
    let mut results = Vec::new();
    for target in targets {
        let response = if target == "runtime" {
            run_restart_command(state.runtime_restart_command.as_deref(), "runtime").await
        } else {
            run_restart_command(state.provider_restart_command.as_deref(), "provider").await
        };
        let status_code = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        let payload = serde_json::from_slice::<RestartResponse>(&body).unwrap_or(RestartResponse {
            service: "froglet".to_string(),
            status: if status_code.is_success() {
                "restarted".to_string()
            } else {
                "failed".to_string()
            },
            restart_supported: false,
            command: None,
            stdout_preview: None,
            stderr_preview: None,
        });
        results.push(FrogletRestartResult {
            target: target.to_string(),
            status: payload.status.to_string(),
            stdout_preview: payload.stdout_preview,
            stderr_preview: payload.stderr_preview,
        });
    }
    (StatusCode::OK, Json(FrogletRestartResponse { results })).into_response()
}

async fn froglet_list_local_services(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    match current_service_records(state.app_state.as_ref(), true, true).await {
        Ok(services) => (StatusCode::OK, Json(ProviderServicesResponse { services })).into_response(),
        Err(error) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to list local services", "details": error }),
        ),
    }
}

async fn froglet_get_local_service(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    AxumPath(service_id): AxumPath<String>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    match provider_service_record(state.app_state.as_ref(), &service_id, true).await {
        Ok(Some(service)) => (StatusCode::OK, Json(ProviderServiceResponse { service })).into_response(),
        Ok(None) => error_json(
            StatusCode::NOT_FOUND,
            json!({ "error": "service not found", "service_id": service_id }),
        ),
        Err(error) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "failed to load local service", "details": error }),
        ),
    }
}

async fn froglet_discover_services(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    Json(payload): Json<ServiceDiscoverRequest>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    let runtime_url = match runtime_base_url(state.as_ref()) {
        Ok(url) => url,
        Err(response) => return response,
    };
    let search: crate::discovery::DiscoverySearchResponse = match local_json_request(
        state.as_ref(),
        &state.app_state.runtime_auth_token,
        reqwest::Method::POST,
        format!("{runtime_url}/v1/runtime/search"),
        Some(&RuntimeSearchRequest {
            limit: payload.limit,
            include_inactive: payload.include_inactive,
        }),
    )
    .await
    {
        Ok(response) => response,
        Err(response) => return response,
    };
    let query = payload.query.map(|value| value.to_lowercase());
    let mut services = Vec::new();
    for node in search.nodes {
        let provider_url = node
            .descriptor
            .transports
            .clearnet_url
            .clone()
            .or_else(|| node.descriptor.transports.onion_url.clone());
        let Some(provider_url) = provider_url else { continue };
        let response: Result<ProviderServicesResponse, Response> = local_json_request(
            state.as_ref(),
            &state.app_state.runtime_auth_token,
            reqwest::Method::GET,
            format!("{provider_url}/v1/provider/services"),
            Option::<&Value>::None,
        )
        .await;
        let Ok(response) = response else { continue };
        for service in response.services {
            if let Some(query) = query.as_ref()
                && !service.service_id.to_lowercase().contains(query)
                && !service.summary.to_lowercase().contains(query)
            {
                continue;
            }
            services.push(service);
        }
    }
    services.sort_by(|left, right| left.service_id.cmp(&right.service_id));
    (StatusCode::OK, Json(FrogletServiceDiscoverResponse { services })).into_response()
}

async fn froglet_get_service(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    Json(payload): Json<ServiceLookupRequest>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    match fetch_provider_service(
        state.as_ref(),
        payload.provider_id.as_deref(),
        payload.provider_url.as_deref(),
        &payload.service_id,
    )
    .await
    {
        Ok(service) => (StatusCode::OK, Json(ProviderServiceResponse { service })).into_response(),
        Err(response) => response,
    }
}

fn build_inline_wasm_submission(
    service: &ProviderServiceRecord,
    input: Value,
) -> Result<crate::wasm::WasmSubmission, Response> {
    let Some(module_bytes_hex) = service.module_bytes_hex.clone() else {
        return Err(error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "service is missing module_bytes_hex binding", "service_id": service.service_id }),
        ));
    };
    let module_bytes = hex::decode(&module_bytes_hex).map_err(|error| {
        error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "service module_bytes_hex is invalid", "details": error.to_string() }),
        )
    })?;
    let workload = crate::wasm::ComputeWasmWorkload::new(&module_bytes, &input)
        .map_err(|error| error_json(StatusCode::BAD_GATEWAY, json!({ "error": error })))?;
    Ok(crate::wasm::WasmSubmission {
        schema_version: crate::wasm::FROGLET_SCHEMA_V1.to_string(),
        submission_type: crate::wasm::WASM_SUBMISSION_TYPE_V1.to_string(),
        workload: crate::wasm::ComputeWasmWorkload {
            abi_version: service.abi_version.clone(),
            requested_capabilities: Vec::new(),
            ..workload
        },
        module_bytes_hex,
        input,
    })
}

fn build_oci_wasm_submission(
    service: &ProviderServiceRecord,
    input: Value,
) -> Result<crate::wasm::OciWasmSubmission, Response> {
    let Some(oci_reference) = service.oci_reference.clone() else {
        return Err(error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "service is missing oci_reference binding", "service_id": service.service_id }),
        ));
    };
    let Some(oci_digest) = service.oci_digest.clone() else {
        return Err(error_json(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "service is missing oci_digest binding", "service_id": service.service_id }),
        ));
    };
    let input_bytes = crate::canonical_json::to_vec(&input)
        .map_err(|error| error_json(StatusCode::BAD_REQUEST, json!({ "error": error.to_string() })))?;
    Ok(crate::wasm::OciWasmSubmission {
        schema_version: crate::wasm::FROGLET_SCHEMA_V1.to_string(),
        submission_type: crate::wasm::WASM_OCI_SUBMISSION_TYPE_V1.to_string(),
        workload: crate::wasm::OciWasmWorkload {
            schema_version: crate::wasm::FROGLET_SCHEMA_V1.to_string(),
            workload_kind: crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_OCI_V1.to_string(),
            abi_version: service.abi_version.clone(),
            module_format: crate::wasm::WASM_MODULE_OCI_FORMAT.to_string(),
            oci_reference,
            oci_digest,
            input_format: crate::wasm::JCS_JSON_FORMAT.to_string(),
            input_hash: crate::crypto::sha256_hex(input_bytes),
            requested_capabilities: Vec::new(),
        },
        input,
    })
}

async fn invoke_or_run_compute(
    state: &OperatorState,
    provider_id: Option<&str>,
    provider_url: Option<&str>,
    offer_id: &str,
    spec: crate::protocol::WorkloadSpec,
    timeout_secs: Option<u64>,
) -> Result<FrogletServiceActionResponse, Response> {
    let runtime_url = runtime_base_url(state)?;
    let response: RuntimeCreateDealResponse = local_json_request(
        state,
        &state.app_state.runtime_auth_token,
        reqwest::Method::POST,
        format!("{runtime_url}/v1/runtime/deals"),
        Some(&RuntimeCreateDealRequest {
            provider: RuntimeProviderRef {
                provider_id: provider_id.map(str::to_string),
                provider_url: provider_url.map(str::to_string),
            },
            offer_id: offer_id.to_string(),
            spec,
            max_price_sats: None,
            idempotency_key: None,
            payment: None,
        }),
    )
    .await?;
    let mut task = response.deal;
    if timeout_secs.unwrap_or(0) == 0 || task_is_terminal(&task.status) {
        return Ok(FrogletServiceActionResponse {
            status: task.status.clone(),
            terminal: task_is_terminal(&task.status),
            result: task.result.clone(),
            task: Some(task),
        });
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs.unwrap_or(15));
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        task = get_runtime_task(state, &task.deal_id).await?;
        if task_is_terminal(&task.status) {
            break;
        }
    }
    Ok(FrogletServiceActionResponse {
        status: task.status.clone(),
        terminal: task_is_terminal(&task.status),
        result: task.result.clone(),
        task: Some(task),
    })
}

async fn froglet_invoke_service(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    Json(payload): Json<InvokeServiceRequest>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    let service = match fetch_provider_service(
        state.as_ref(),
        payload.provider_id.as_deref(),
        payload.provider_url.as_deref(),
        &payload.service_id,
    )
    .await
    {
        Ok(service) => service,
        Err(response) => return response,
    };
    let input = payload.input.unwrap_or(Value::Null);
    let timeout_secs = payload
        .timeout_secs
        .or_else(|| (service.mode == "sync").then_some(15));
    let spec = match service.execution_kind.as_str() {
        "wasm_inline" => match build_inline_wasm_submission(&service, input) {
            Ok(submission) => crate::protocol::WorkloadSpec::Wasm {
                submission: Box::new(submission),
            },
            Err(response) => return response,
        },
        "wasm_oci" => match build_oci_wasm_submission(&service, input) {
            Ok(submission) => crate::protocol::WorkloadSpec::OciWasm {
                submission: Box::new(submission),
            },
            Err(response) => return response,
        },
        _ => {
            return error_json(
                StatusCode::BAD_GATEWAY,
                json!({ "error": "unsupported service execution_kind", "execution_kind": service.execution_kind }),
            )
        }
    };
    match invoke_or_run_compute(
        state.as_ref(),
        payload.provider_id.as_deref().or(Some(service.provider_id.as_str())),
        payload.provider_url.as_deref(),
        &service.offer_id,
        spec,
        timeout_secs,
    )
    .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(response) => response,
    }
}

async fn froglet_run_compute(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    Json(payload): Json<RunComputeRequest>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    let input = payload.input.unwrap_or(Value::Null);
    let execution_kind = payload.execution_kind.unwrap_or_else(|| {
        if payload.oci_reference.is_some() || payload.oci_digest.is_some() {
            "wasm_oci".to_string()
        } else {
            "wasm_inline".to_string()
        }
    });
    let spec = match execution_kind.as_str() {
        "wasm_inline" => {
            let Some(module_bytes_hex) = payload.wasm_module_hex else {
                return error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "wasm_module_hex is required for wasm_inline" }),
                );
            };
            let module_bytes = match hex::decode(&module_bytes_hex) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return error_json(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": format!("invalid wasm_module_hex: {error}") }),
                    );
                }
            };
            let mut workload = match crate::wasm::ComputeWasmWorkload::new(&module_bytes, &input) {
                Ok(workload) => workload,
                Err(error) => return error_json(StatusCode::BAD_REQUEST, json!({ "error": error })),
            };
            if let Some(abi_version) = payload.abi_version {
                workload.abi_version = abi_version;
            }
            crate::protocol::WorkloadSpec::Wasm {
                submission: Box::new(crate::wasm::WasmSubmission {
                    schema_version: crate::wasm::FROGLET_SCHEMA_V1.to_string(),
                    submission_type: crate::wasm::WASM_SUBMISSION_TYPE_V1.to_string(),
                    workload,
                    module_bytes_hex,
                    input,
                }),
            }
        }
        "wasm_oci" => {
            let Some(oci_reference) = payload.oci_reference else {
                return error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "oci_reference is required for wasm_oci" }),
                );
            };
            let Some(oci_digest) = payload.oci_digest else {
                return error_json(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "oci_digest is required for wasm_oci" }),
                );
            };
            let input_bytes = match crate::canonical_json::to_vec(&input) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return error_json(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": error.to_string() }),
                    );
                }
            };
            crate::protocol::WorkloadSpec::OciWasm {
                submission: Box::new(crate::wasm::OciWasmSubmission {
                    schema_version: crate::wasm::FROGLET_SCHEMA_V1.to_string(),
                    submission_type: crate::wasm::WASM_OCI_SUBMISSION_TYPE_V1.to_string(),
                    workload: crate::wasm::OciWasmWorkload {
                        schema_version: crate::wasm::FROGLET_SCHEMA_V1.to_string(),
                        workload_kind: crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_OCI_V1.to_string(),
                        abi_version: payload
                            .abi_version
                            .unwrap_or_else(|| crate::wasm::WASM_RUN_JSON_ABI_V1.to_string()),
                        module_format: crate::wasm::WASM_MODULE_OCI_FORMAT.to_string(),
                        oci_reference,
                        oci_digest,
                        input_format: crate::wasm::JCS_JSON_FORMAT.to_string(),
                        input_hash: crate::crypto::sha256_hex(input_bytes),
                        requested_capabilities: Vec::new(),
                    },
                    input,
                }),
            }
        }
        _ => {
            return error_json(
                StatusCode::BAD_REQUEST,
                json!({ "error": "execution_kind must be wasm_inline or wasm_oci" }),
            )
        }
    };
    match invoke_or_run_compute(
        state.as_ref(),
        payload.provider_id.as_deref(),
        payload.provider_url.as_deref(),
        "execute.wasm",
        spec,
        payload.timeout_secs,
    )
    .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(response) => response,
    }
}

async fn froglet_get_task(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    AxumPath(task_id): AxumPath<String>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    match get_runtime_task(state.as_ref(), &task_id).await {
        Ok(task) => (StatusCode::OK, Json(FrogletTaskResponse { task })).into_response(),
        Err(response) => response,
    }
}

async fn froglet_wait_task(
    headers: HeaderMap,
    State(state): State<Arc<OperatorState>>,
    AxumPath(task_id): AxumPath<String>,
    Json(payload): Json<WaitTaskRequest>,
) -> Response {
    if let Err(response) = require_froglet_auth(&headers, state.as_ref()) {
        return response;
    }
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(payload.timeout_secs.unwrap_or(15));
    let poll_interval = std::time::Duration::from_secs_f64(payload.poll_interval_secs.unwrap_or(0.25));
    loop {
        let task = match get_runtime_task(state.as_ref(), &task_id).await {
            Ok(task) => task,
            Err(response) => return response,
        };
        if task_is_terminal(&task.status) || std::time::Instant::now() >= deadline {
            return (StatusCode::OK, Json(FrogletTaskResponse { task })).into_response();
        }
        tokio::time::sleep(poll_interval).await;
    }
}

pub fn router(state: Arc<OperatorState>) -> Router {
    Router::new()
        .route("/health", get(operator_health))
        .route("/v1/froglet/status", get(froglet_status))
        .route("/v1/froglet/logs", get(froglet_tail_logs))
        .route("/v1/froglet/restart", post(froglet_restart))
        .route("/v1/froglet/projects", get(provider_list_projects).post(provider_create_project))
        .route("/v1/froglet/projects/:project_id", get(provider_get_project))
        .route(
            "/v1/froglet/projects/:project_id/files/*path",
            get(provider_read_file).put(provider_write_file),
        )
        .route("/v1/froglet/projects/:project_id/build", post(provider_build_project))
        .route("/v1/froglet/projects/:project_id/test", post(provider_test_project))
        .route(
            "/v1/froglet/projects/:project_id/publish",
            post(provider_publish_project),
        )
        .route("/v1/froglet/artifacts/publish", post(provider_publish_artifact))
        .route("/v1/froglet/services/local", get(froglet_list_local_services))
        .route(
            "/v1/froglet/services/local/:service_id",
            get(froglet_get_local_service),
        )
        .route("/v1/froglet/services/discover", post(froglet_discover_services))
        .route("/v1/froglet/services/get", post(froglet_get_service))
        .route("/v1/froglet/services/invoke", post(froglet_invoke_service))
        .route("/v1/froglet/compute/run", post(froglet_run_compute))
        .route("/v1/froglet/tasks/:task_id", get(froglet_get_task))
        .route("/v1/froglet/tasks/:task_id/wait", post(froglet_wait_task))
        .with_state(state)
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();
    tls::ensure_rustls_crypto_provider();

    let node_config = NodeConfig::from_env().map_err(std::io::Error::other)?;
    let app_state = state::build_app_state(node_config.clone()).map_err(std::io::Error::other)?;
    let listen_addr = std::env::var("FROGLET_OPERATOR_LISTEN_ADDR")
        .or_else(|_| std::env::var("FROGLET_PROVIDER_CONTROL_LISTEN_ADDR"))
        .unwrap_or_else(|_| DEFAULT_OPERATOR_LISTEN_ADDR.to_string());
    let allow_non_loopback = std::env::var("FROGLET_OPERATOR_ALLOW_NON_LOOPBACK")
        .ok()
        .or_else(|| std::env::var("FROGLET_PROVIDER_CONTROL_ALLOW_NON_LOOPBACK").ok())
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    let projects_root = std::env::var("FROGLET_PROVIDER_PROJECTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| provider_projects::projects_root_from_data_dir(&node_config.storage.data_dir));
    std::fs::create_dir_all(&projects_root)?;
    let consumer_control_auth_token = runtime_auth::load_or_create_local_token(
        &node_config.storage.runtime_dir,
        &node_config.storage.consumer_control_auth_token_path,
        "consumer control auth token",
    )
    .map_err(std::io::Error::other)?;
    if consumer_control_auth_token != app_state.consumer_control_auth_token {
        warn!("Consumer control auth token was reloaded separately from AppState");
    }

    let addr: SocketAddr = listen_addr.parse()?;
    if !addr.ip().is_loopback() && !allow_non_loopback {
        return Err(std::io::Error::other(format!(
            "FROGLET_OPERATOR_LISTEN_ADDR must bind to a loopback address, got {listen_addr}"
        ))
        .into());
    }
    if !addr.ip().is_loopback() {
        warn!(
            "Operator API is binding a non-loopback address ({}) because FROGLET_OPERATOR_ALLOW_NON_LOOPBACK=true; restrict network access to trusted local callers",
            listen_addr
        );
    }

    info!(
        "Consumer control auth token file: {}",
        app_state.consumer_control_auth_token_path.display()
    );
    info!(
        "Froglet control auth token file: {}",
        app_state.provider_control_auth_token_path.display()
    );
    info!("Provider projects root: {}", projects_root.display());

    let state = Arc::new(OperatorState {
        app_state,
        projects_root,
        runtime_log_path: std::env::var("FROGLET_OPERATOR_RUNTIME_LOG_PATH")
            .ok()
            .map(PathBuf::from),
        provider_log_path: std::env::var("FROGLET_OPERATOR_PROVIDER_LOG_PATH")
            .ok()
            .map(PathBuf::from),
        runtime_restart_command: std::env::var("FROGLET_OPERATOR_RUNTIME_RESTART_COMMAND").ok(),
        provider_restart_command: std::env::var("FROGLET_OPERATOR_PROVIDER_RESTART_COMMAND").ok(),
    });
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    println!(" 🎛️  Froglet Operator API: http://{}", bound_addr);
    info!("Froglet operator listening on http://{}", bound_addr);
    axum::serve(listener, router(state)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, header},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::{
        confidential::ConfidentialConfig,
        config::{
            DiscoveryMode, IdentityConfig, LightningConfig, LightningMode, NetworkMode,
            PaymentBackend, PricingConfig, StorageConfig, TorSidecarConfig, WasmConfig,
        },
    };

    static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "froglet-operator-tests-{label}-{}-{unique}-{counter}",
            std::process::id()
        ))
    }

    fn test_operator_state() -> Arc<OperatorState> {
        let temp_dir = unique_temp_dir("state");
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        let node_config = NodeConfig {
            network_mode: NetworkMode::Clearnet,
            listen_addr: "127.0.0.1:0".to_string(),
            public_base_url: None,
            runtime_listen_addr: "127.0.0.1:0".to_string(),
            runtime_allow_non_loopback: false,
            provider_control_listen_addr: "127.0.0.1:0".to_string(),
            provider_control_allow_non_loopback: false,
            http_ca_cert_path: None,
            tor: TorSidecarConfig {
                binary_path: "tor".to_string(),
                backend_listen_addr: "127.0.0.1:0".to_string(),
                startup_timeout_secs: 90,
            },
            discovery_mode: DiscoveryMode::None,
            identity: IdentityConfig {
                auto_generate: true,
            },
            reference_discovery: None,
            pricing: PricingConfig {
                events_query: 0,
                execute_wasm: 0,
            },
            payment_backend: PaymentBackend::None,
            execution_timeout_secs: 5,
            lightning: LightningConfig {
                mode: LightningMode::Mock,
                destination_identity: None,
                base_invoice_expiry_secs: 300,
                success_hold_expiry_secs: 300,
                min_final_cltv_expiry: 18,
                sync_interval_ms: 100,
                lnd_rest: None,
            },
            storage: StorageConfig {
                data_dir: temp_dir.clone(),
                db_path: temp_dir.join("node.db"),
                identity_dir: temp_dir.join("identity"),
                identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
                nostr_publication_seed_path: temp_dir.join("identity/nostr-publication.secp256k1.seed"),
                runtime_dir: temp_dir.join("runtime"),
                runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
                consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
                provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
                tor_dir: temp_dir.join("tor"),
            },
            wasm: WasmConfig {
                policy_path: None,
                policy: None,
            },
            confidential: ConfidentialConfig {
                policy_path: None,
                policy: None,
                session_ttl_secs: 300,
            },
        };
        let app_state = state::build_app_state(node_config).expect("build app state");
        Arc::new(OperatorState {
            projects_root: provider_projects::projects_root_from_data_dir(&app_state.config.storage.data_dir),
            app_state,
            runtime_log_path: None,
            provider_log_path: None,
            runtime_restart_command: None,
            provider_restart_command: None,
        })
    }

    fn operator_request(
        method: Method,
        uri: &str,
        bearer_token: Option<&str>,
        body: Option<Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = bearer_token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = if let Some(payload) = body {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&payload).expect("serialize request body"))
        } else {
            Body::empty()
        };
        builder.body(body).expect("build request")
    }

    async fn response_json<T: serde::de::DeserializeOwned>(
        response: axum::response::Response,
    ) -> (StatusCode, T) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let payload = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "failed to decode JSON response {status}: {error}; body={}",
                String::from_utf8_lossy(&bytes)
            )
        });
        (status, payload)
    }

    #[tokio::test]
    async fn froglet_status_requires_auth() {
        let state = test_operator_state();
        let response = router(state)
            .oneshot(operator_request(
                Method::GET,
                "/v1/froglet/status",
                None,
                None,
            ))
            .await
            .expect("froglet status response");
        let (status, payload): (StatusCode, Value) = response_json(response).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(payload["error"], "missing froglet authorization");
    }

    #[tokio::test]
    async fn provider_project_lifecycle_works_through_operator() {
        let state = test_operator_state();
        let token = state.app_state.provider_control_auth_token.clone();

        let create_response = router(state.clone())
            .oneshot(operator_request(
                Method::POST,
                "/v1/froglet/projects",
                Some(&token),
                Some(json!({
                    "project_id": "hello-world",
                    "service_id": "hello-world",
                    "offer_id": "hello-world",
                    "starter": "hello_world",
                    "summary": "Hello World service",
                    "price_sats": 0,
                    "publication_state": "active"
                })),
            ))
            .await
            .expect("create project response");
        let (create_status, create_payload): (StatusCode, Value) = response_json(create_response).await;
        assert_eq!(create_status, StatusCode::CREATED);
        assert_eq!(create_payload["project"]["project_id"], "hello-world");

        let test_response = router(state.clone())
            .oneshot(operator_request(
                Method::POST,
                "/v1/froglet/projects/hello-world/test",
                Some(&token),
                Some(json!({})),
            ))
            .await
            .expect("test project response");
        let (test_status, test_payload): (StatusCode, Value) = response_json(test_response).await;
        assert_eq!(test_status, StatusCode::OK);
        assert_eq!(test_payload["output"], json!({ "message": "Hello World" }));

        let publish_response = router(state.clone())
            .oneshot(operator_request(
                Method::POST,
                "/v1/froglet/projects/hello-world/publish",
                Some(&token),
                None,
            ))
            .await
            .expect("publish project response");
        let (publish_status, publish_payload): (StatusCode, Value) =
            response_json(publish_response).await;
        assert_eq!(publish_status, StatusCode::CREATED);
        assert_eq!(publish_payload["offer"]["offer"]["payload"]["offer_id"], "hello-world");

        let services_response = router(state)
            .oneshot(operator_request(
                Method::GET,
                "/v1/froglet/services/local",
                Some(&token),
                None,
            ))
            .await
            .expect("list local services response");
        let (services_status, services_payload): (StatusCode, Value) =
            response_json(services_response).await;
        assert_eq!(services_status, StatusCode::OK);
        assert!(
            services_payload["services"]
                .as_array()
                .expect("services array")
                .iter()
                .any(|service| service["service_id"] == "hello-world"),
            "expected hello-world in local services: {services_payload}"
        );
    }

    #[tokio::test]
    async fn create_project_accepts_name_and_result_json() {
        let state = test_operator_state();
        let token = state.app_state.provider_control_auth_token.clone();

        let create_response = router(state.clone())
            .oneshot(operator_request(
                Method::POST,
                "/v1/froglet/projects",
                Some(&token),
                Some(json!({
                    "name": "Lol Service",
                    "summary": "Returns lol",
                    "result_json": "lol",
                    "price_sats": 0,
                    "publication_state": "active"
                })),
            ))
            .await
            .expect("create project response");
        let (create_status, create_payload): (StatusCode, Value) = response_json(create_response).await;
        assert_eq!(create_status, StatusCode::CREATED);
        assert_eq!(create_payload["project"]["project_id"], "lol-service");
        assert_eq!(create_payload["project"]["service_id"], "lol-service");

        let test_response = router(state)
            .oneshot(operator_request(
                Method::POST,
                "/v1/froglet/projects/lol-service/test",
                Some(&token),
                Some(json!({ "input": { "ignored": true } })),
            ))
            .await
            .expect("test project response");
        let (test_status, test_payload): (StatusCode, Value) = response_json(test_response).await;
        assert_eq!(test_status, StatusCode::OK);
        assert_eq!(test_payload["output"], json!("lol"));
    }

    #[tokio::test]
    async fn create_project_infers_static_result_from_summary() {
        let state = test_operator_state();
        let token = state.app_state.provider_control_auth_token.clone();

        let create_response = router(state.clone())
            .oneshot(operator_request(
                Method::POST,
                "/v1/froglet/projects",
                Some(&token),
                Some(json!({
                    "name": "Lol Summary Service",
                    "summary": "A service that returns 'lol-summary' for free.",
                    "price_sats": 0,
                    "publication_state": "active"
                })),
            ))
            .await
            .expect("create project response");
        let (create_status, create_payload): (StatusCode, Value) = response_json(create_response).await;
        assert_eq!(create_status, StatusCode::CREATED);
        assert_eq!(create_payload["project"]["project_id"], "lol-summary-service");

        let test_response = router(state)
            .oneshot(operator_request(
                Method::POST,
                "/v1/froglet/projects/lol-summary-service/test",
                Some(&token),
                Some(json!({ "input": {} })),
            ))
            .await
            .expect("test project response");
        let (test_status, test_payload): (StatusCode, Value) = response_json(test_response).await;
        assert_eq!(test_status, StatusCode::OK);
        assert_eq!(test_payload["output"], json!("lol-summary"));
    }
}
