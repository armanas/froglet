//! Provider-neutral isolated OCI worker seam.
//!
//! The Froglet Node never starts Docker/Podman and never receives or exposes a
//! container-engine socket. It sends a capability-reduced request to an
//! independently isolated worker. The default adapter is disabled/fail-closed;
//! the HTTP adapter authenticates with an operator-owned token that is never
//! serialized into requests or debug output.

use crate::execution::{ExecutionMount, ExecutionWorkload, digest_pinned_oci_image_reference};
pub use froglet_protocol::oci_worker::{
    OCI_WORKER_REQUEST_SCHEMA_V1, OCI_WORKER_RESULT_SCHEMA_V1, OciWorkerEvidence, OciWorkerLimits,
    OciWorkerMount, OciWorkerRequest, OciWorkerResult, OciWorkerSecret, validate_digest_image,
};
use reqwest::redirect::Policy;
use std::{
    collections::HashSet, fmt, future::Future, net::IpAddr, path::Path, pin::Pin, time::Duration,
};
use url::Url;
use zeroize::Zeroizing;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_OVERHEAD_BYTES: usize = 64 * 1024;

/// Reduces a node-private execution workload to the public worker wire
/// contract, intersecting declared access with the exact grants.
pub fn request_from_execution(
    execution: &ExecutionWorkload,
    granted_access: &[String],
    limits: OciWorkerLimits,
) -> Result<OciWorkerRequest, String> {
    let reference = execution
        .oci_reference
        .as_deref()
        .ok_or_else(|| "OCI worker execution requires oci_reference".to_string())?;
    let digest = execution
        .oci_digest
        .as_deref()
        .ok_or_else(|| "OCI worker execution requires oci_digest".to_string())?;
    let image = digest_pinned_oci_image_reference(reference, digest)?;
    let declared = execution
        .requested_access
        .iter()
        .cloned()
        .chain(execution.mounts.iter().map(mount_capability))
        .collect::<HashSet<_>>();
    let granted_set = granted_access.iter().cloned().collect::<HashSet<_>>();
    let mut granted_capabilities = declared
        .intersection(&granted_set)
        .cloned()
        .collect::<Vec<_>>();
    granted_capabilities.sort();

    let mut mounts = execution
        .mounts
        .iter()
        .filter_map(|mount| {
            let capability = mount_capability(mount);
            granted_set.contains(&capability).then(|| OciWorkerMount {
                handle: mount.handle.clone(),
                kind: mount.kind.clone(),
                read_only: mount.read_only,
            })
        })
        .collect::<Vec<_>>();
    mounts.sort_by(|left, right| left.handle.cmp(&right.handle));

    let mut secrets = granted_capabilities
        .iter()
        .filter_map(|capability| capability.strip_prefix("secret."))
        .filter(|handle| !handle.contains('.'))
        .map(|handle| OciWorkerSecret {
            handle: handle.to_string(),
        })
        .collect::<Vec<_>>();
    secrets.sort_by(|left, right| left.handle.cmp(&right.handle));
    secrets.dedup_by(|left, right| left.handle == right.handle);

    let request = OciWorkerRequest {
        schema_version: OCI_WORKER_REQUEST_SCHEMA_V1.to_string(),
        image,
        entrypoint_kind: execution.entrypoint.kind.as_str().to_string(),
        entrypoint: execution.entrypoint.value.clone(),
        input: execution.input.clone(),
        mounts,
        secrets,
        network_egress: granted_capabilities
            .iter()
            .any(|capability| capability == "network.egress"),
        granted_capabilities,
        limits,
    };
    request.normalized()
}

fn mount_capability(mount: &ExecutionMount) -> String {
    format!(
        "mount.{}.{}.{}",
        mount.kind,
        if mount.read_only { "read" } else { "write" },
        mount.handle
    )
}

pub trait OciWorker: Send + Sync {
    fn execute<'a>(
        &'a self,
        request: OciWorkerRequest,
    ) -> Pin<Box<dyn Future<Output = Result<OciWorkerResult, String>> + Send + 'a>>;
}

#[derive(Debug, Clone, Default)]
pub struct DisabledOciWorker;

impl OciWorker for DisabledOciWorker {
    fn execute<'a>(
        &'a self,
        _request: OciWorkerRequest,
    ) -> Pin<Box<dyn Future<Output = Result<OciWorkerResult, String>> + Send + 'a>> {
        Box::pin(async {
            Err(
                "OCI execution is disabled: configure an authenticated isolated worker; the Froglet Node does not access Docker/Podman sockets"
                    .to_string(),
            )
        })
    }
}

#[derive(Clone)]
pub struct HttpOciWorker {
    endpoint: Url,
    bearer_token: Zeroizing<String>,
    http: reqwest::Client,
}

impl fmt::Debug for HttpOciWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpOciWorker")
            .field("endpoint", &self.endpoint)
            .field("bearer_token", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl HttpOciWorker {
    pub fn new(endpoint: Url, bearer_token: String) -> Result<Self, String> {
        validate_worker_endpoint(&endpoint)?;
        if bearer_token.trim().is_empty() {
            return Err("OCI worker bearer token must not be empty".to_string());
        }
        let http = crate::tls::reqwest_client_builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::none())
            .build()
            .map_err(|error| format!("failed to construct OCI worker client: {error}"))?;
        Ok(Self {
            endpoint,
            bearer_token: Zeroizing::new(bearer_token),
            http,
        })
    }

    async fn execute_inner(&self, request: OciWorkerRequest) -> Result<OciWorkerResult, String> {
        request.validate()?;
        let timeout = Duration::from_millis(request.limits.max_runtime_ms)
            .saturating_add(Duration::from_secs(5));
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(self.bearer_token.as_str())
            .timeout(timeout)
            .json(&request)
            .send()
            .await
            .map_err(|error| format!("isolated OCI worker request failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "isolated OCI worker returned HTTP {}",
                response.status()
            ));
        }
        let cap = request
            .limits
            .max_output_bytes
            .checked_add(MAX_RESPONSE_OVERHEAD_BYTES)
            .ok_or_else(|| "OCI worker response byte cap overflow".to_string())?;
        let body = read_body_capped(response, cap).await?;
        let result: OciWorkerResult = serde_json::from_slice(&body)
            .map_err(|error| format!("invalid isolated OCI worker response: {error}"))?;
        result.validate_for(&request)?;
        Ok(result)
    }
}

impl OciWorker for HttpOciWorker {
    fn execute<'a>(
        &'a self,
        request: OciWorkerRequest,
    ) -> Pin<Box<dyn Future<Output = Result<OciWorkerResult, String>> + Send + 'a>> {
        Box::pin(async move { self.execute_inner(request).await })
    }
}

async fn read_body_capped(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("failed reading isolated OCI worker response: {error}"))?
    {
        if body.len().saturating_add(chunk.len()) > cap {
            return Err(
                "isolated OCI worker response exceeds the configured output cap".to_string(),
            );
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_worker_endpoint(endpoint: &Url) -> Result<(), String> {
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        return Err("OCI worker URL must not contain credentials".to_string());
    }
    match endpoint.scheme() {
        "https" => Ok(()),
        "http" if endpoint_is_loopback(endpoint) => Ok(()),
        _ => Err(
            "OCI worker URL must use HTTPS (HTTP is accepted only for a literal loopback sidecar)"
                .to_string(),
        ),
    }
}

fn endpoint_is_loopback(endpoint: &Url) -> bool {
    endpoint
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .is_some_and(|address| address.is_loopback())
}

pub enum ConfiguredOciWorker {
    Disabled(DisabledOciWorker),
    Http(HttpOciWorker),
}

impl ConfiguredOciWorker {
    pub fn from_env() -> Result<Self, String> {
        let Some(endpoint) = std::env::var("FROGLET_OCI_WORKER_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(Self::Disabled(DisabledOciWorker));
        };
        let token = if let Some(path) = std::env::var_os("FROGLET_OCI_WORKER_TOKEN_PATH") {
            read_worker_token_file(Path::new(&path))?
        } else {
            std::env::var("FROGLET_OCI_WORKER_TOKEN").map_err(|_| {
                "FROGLET_OCI_WORKER_TOKEN_PATH (preferred) or FROGLET_OCI_WORKER_TOKEN is required when the OCI worker is enabled"
                    .to_string()
            })?
        };
        let endpoint = Url::parse(&endpoint)
            .map_err(|error| format!("invalid FROGLET_OCI_WORKER_URL: {error}"))?;
        Ok(Self::Http(HttpOciWorker::new(endpoint, token)?))
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Http(_))
    }
}

fn read_worker_token_file(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect OCI worker token file: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("OCI worker token path must be a regular file, not a symlink".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err("OCI worker token file permissions must be 0600".to_string());
        }
    }
    if metadata.len() > 4_096 {
        return Err("OCI worker token file exceeds 4096 bytes".to_string());
    }
    let token = std::fs::read_to_string(path)
        .map_err(|error| format!("could not read OCI worker token file: {error}"))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("OCI worker token file is empty".to_string());
    }
    Ok(token)
}

impl OciWorker for ConfiguredOciWorker {
    fn execute<'a>(
        &'a self,
        request: OciWorkerRequest,
    ) -> Pin<Box<dyn Future<Output = Result<OciWorkerResult, String>> + Send + 'a>> {
        match self {
            Self::Disabled(worker) => worker.execute(request),
            Self::Http(worker) => worker.execute(request),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{ExecutionEntrypointKind, ExecutionRuntime};

    fn limits() -> OciWorkerLimits {
        OciWorkerLimits {
            max_input_bytes: 1024,
            max_runtime_ms: 1_000,
            max_memory_bytes: 64 * 1024 * 1024,
            max_output_bytes: 1024,
            pids_limit: 32,
            cpu_millis: 500,
        }
    }

    fn execution() -> ExecutionWorkload {
        let mut execution = ExecutionWorkload::container_oci(
            ExecutionRuntime::Container,
            "registry.example/froglet/worker:mutable".to_string(),
            "ab".repeat(32),
            ExecutionEntrypointKind::Handler,
            "run".to_string(),
            serde_json::json!({"value": 7}),
        )
        .unwrap();
        execution.requested_access = vec![
            "mount.sqlite.read.dataset".to_string(),
            "network.egress".to_string(),
            "secret.api_key".to_string(),
        ];
        execution.mounts = vec![ExecutionMount {
            handle: "dataset".to_string(),
            kind: "sqlite".to_string(),
            read_only: true,
            binding: Some("/var/run/docker.sock".to_string()),
        }];
        execution
    }

    #[test]
    fn request_contains_only_declared_logical_access() {
        let request = request_from_execution(
            &execution(),
            &[
                "mount.sqlite.read.dataset".to_string(),
                "secret.api_key".to_string(),
                "secret.undeclared".to_string(),
            ],
            limits(),
        )
        .unwrap();
        assert_eq!(request.mounts[0].handle, "dataset");
        assert_eq!(request.secrets[0].handle, "api_key");
        assert!(!request.network_egress);
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(!encoded.contains("docker.sock"));
        assert!(!encoded.contains("undeclared"));
        assert!(!encoded.contains("binding"));
        assert!(!encoded.contains("FROGLET_OCI_WORKER_TOKEN"));
    }

    #[test]
    fn legacy_runtime_mount_requires_legacy_grant_then_crosses_canonically() {
        let mut execution = execution();
        execution.mounts = vec![ExecutionMount {
            handle: "archive".to_string(),
            kind: "s3".to_string(),
            read_only: true,
            binding: None,
        }];
        execution.requested_access = vec!["mount.s3.read.archive".to_string()];

        let denied = request_from_execution(
            &execution,
            &["mount.object_store.read.archive".to_string()],
            limits(),
        )
        .expect("canonical grant must not authorize a legacy runtime mount");
        assert!(denied.mounts.is_empty());
        assert!(denied.granted_capabilities.is_empty());

        let granted =
            request_from_execution(&execution, &["mount.s3.read.archive".to_string()], limits())
                .expect("exact legacy grant");
        assert_eq!(granted.mounts[0].kind, "object_store");
        assert_eq!(
            granted.granted_capabilities,
            ["mount.object_store.read.archive"]
        );
    }

    #[tokio::test]
    async fn disabled_adapter_fails_closed() {
        let request = request_from_execution(&execution(), &[], limits()).unwrap();
        let error = DisabledOciWorker.execute(request).await.unwrap_err();
        assert!(error.contains("does not access Docker/Podman sockets"));
    }

    #[test]
    fn http_adapter_debug_redacts_auth() {
        let worker = HttpOciWorker::new(
            Url::parse("http://127.0.0.1:9090/execute").unwrap(),
            "top-secret-token".to_string(),
        )
        .unwrap();
        let debug = format!("{worker:?}");
        assert!(!debug.contains("top-secret-token"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn cleartext_worker_must_be_literal_loopback() {
        assert!(
            HttpOciWorker::new(
                Url::parse("http://worker.internal/execute").unwrap(),
                "token".to_string(),
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn token_file_requires_0600_and_trims_newline() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker.token");
        std::fs::write(&path, "secret\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_worker_token_file(&path).unwrap(), "secret");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_worker_token_file(&path).unwrap_err().contains("0600"));
    }
}
