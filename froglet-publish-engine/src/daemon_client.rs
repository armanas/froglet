//! HTTP client for the local `froglet-node` daemon.
//!
//! The engine talks to the daemon over its existing
//! provider-control HTTP API instead of reaching into the daemon's
//! Rust internals. This keeps the daemon ↔ engine boundary clean and
//! lets the engine be used against ANY froglet-node (local or remote)
//! that an operator controls.
//!
//! Two endpoints matter for Phase 1A:
//!
//! - `GET /v1/node/capabilities` — read identity + transport URLs.
//! - `POST /v1/provider/artifacts/publish` — sign + persist an offer
//!   to the daemon's `/v1/feed`. Requires the provider-control Bearer
//!   token.

use crate::error::PublishError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:8080";
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(30);
const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(5);

/// Where to find the daemon's provider-control auth token.
#[derive(Debug, Clone)]
pub enum ControlAuth {
    /// Token value directly.
    Value(String),
    /// Path to a file containing the token (default daemon convention,
    /// `<data_dir>/runtime/froglet-control.token`).
    File(PathBuf),
}

impl ControlAuth {
    /// Resolve the literal token string.
    pub async fn resolve(&self) -> Result<String, PublishError> {
        match self {
            Self::Value(v) => Ok(v.trim().to_string()),
            Self::File(path) => {
                let content = tokio::fs::read_to_string(path).await.map_err(|e| {
                    PublishError::InvalidInput {
                        field: "control_auth.file",
                        reason: format!("could not read token file {path:?}: {e}"),
                    }
                })?;
                Ok(content.trim().to_string())
            }
        }
    }
}

/// Mirrors `froglet::api::types::ProviderControlPublishArtifactRequest`.
/// Field names + types must stay in lockstep with that struct.
#[derive(Debug, Clone, Serialize, Default)]
pub struct PublishArtifactRequest {
    pub service_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_module_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oci_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oci_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    pub price_sats: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publication_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

/// Trimmed view of the daemon's publish response, matching the fields
/// the engine needs to build [`crate::PublishOutput`]. The daemon
/// returns a richer envelope; we only deserialize what we use.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishArtifactResponse {
    #[serde(default)]
    pub status: Option<String>,
    pub evidence: PublishEvidence,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PublishEvidence {
    pub provider_id: String,
    pub descriptor_hash: String,
    pub offer_hash: String,
    pub offer_id: String,
    #[serde(default)]
    pub service_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Capabilities {
    pub identity: CapabilitiesIdentity,
    #[serde(default)]
    pub transports: CapabilitiesTransports,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilitiesIdentity {
    pub node_id: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CapabilitiesTransports {
    #[serde(default)]
    pub clearnet: Option<TransportEntry>,
    #[serde(default)]
    pub tor: Option<TransportEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransportEntry {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub onion_url: Option<String>,
}

/// Thin client. Cheap to clone; one `reqwest::Client` shared across
/// requests for connection reuse.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    pub daemon_url: Url,
    pub control_auth: ControlAuth,
    http: reqwest::Client,
}

impl DaemonClient {
    pub fn new(daemon_url: Url, control_auth: ControlAuth) -> Result<Self, PublishError> {
        let http = reqwest::Client::builder()
            .timeout(PUBLISH_TIMEOUT)
            .build()?;
        Ok(Self {
            daemon_url,
            control_auth,
            http,
        })
    }

    /// Build a client from environment, mirroring the daemon's defaults.
    ///
    /// - `FROGLET_DAEMON_URL` → daemon URL (default `http://127.0.0.1:8080`)
    /// - `FROGLET_PROVIDER_CONTROL_TOKEN` → literal token (preferred)
    /// - `FROGLET_PROVIDER_CONTROL_TOKEN_PATH` → token file path
    ///   (default `~/.froglet/runtime/froglet-control.token` if neither
    ///   env var is set)
    pub fn from_env() -> Result<Self, PublishError> {
        let daemon_url_str =
            std::env::var("FROGLET_DAEMON_URL").unwrap_or_else(|_| DEFAULT_DAEMON_URL.to_string());
        let daemon_url = Url::parse(&daemon_url_str).map_err(|e| PublishError::InvalidInput {
            field: "FROGLET_DAEMON_URL",
            reason: format!("not a valid URL: {e}"),
        })?;

        let control_auth = if let Ok(token) = std::env::var("FROGLET_PROVIDER_CONTROL_TOKEN") {
            ControlAuth::Value(token)
        } else if let Ok(path) = std::env::var("FROGLET_PROVIDER_CONTROL_TOKEN_PATH") {
            ControlAuth::File(PathBuf::from(path))
        } else {
            // Fall back to the daemon's default token path under FROGLET_DATA_DIR.
            let data_dir = std::env::var("FROGLET_DATA_DIR")
                .map(PathBuf::from)
                .or_else(|_| {
                    dirs_home()
                        .map(|h| h.join(".froglet"))
                        .ok_or_else(|| PublishError::InvalidInput {
                            field: "FROGLET_DATA_DIR",
                            reason:
                                "no FROGLET_DATA_DIR or HOME set; cannot find provider-control token"
                                    .to_string(),
                        })
                })?;
            ControlAuth::File(data_dir.join("runtime/froglet-control.token"))
        };

        Self::new(daemon_url, control_auth)
    }

    /// `GET /v1/node/capabilities` — read identity + transports.
    pub async fn capabilities(&self) -> Result<Capabilities, PublishError> {
        let mut url = self.daemon_url.clone();
        url.set_path("/v1/node/capabilities");
        let response = self
            .http
            .get(url.clone())
            .timeout(CAPABILITIES_TIMEOUT)
            .send()
            .await
            .map_err(|e| PublishError::Http(format!("GET {url} failed: {e}")))?;
        if !response.status().is_success() {
            return Err(PublishError::Http(format!(
                "GET {url} returned HTTP {}: is froglet-node running?",
                response.status()
            )));
        }
        let capabilities: Capabilities = response
            .json()
            .await
            .map_err(|e| PublishError::Http(format!("capabilities JSON parse failed: {e}")))?;
        Ok(capabilities)
    }

    /// `POST /v1/provider/artifacts/publish` — sign + persist an offer.
    pub async fn publish_artifact(
        &self,
        request: &PublishArtifactRequest,
    ) -> Result<PublishArtifactResponse, PublishError> {
        let token = self.control_auth.resolve().await?;
        if token.is_empty() {
            return Err(PublishError::InvalidInput {
                field: "provider_control_token",
                reason: "provider-control token is empty; check the daemon's runtime/froglet-control.token"
                    .to_string(),
            });
        }

        let mut url = self.daemon_url.clone();
        url.set_path("/v1/provider/artifacts/publish");

        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .json(request)
            .send()
            .await
            .map_err(|e| PublishError::Http(format!("POST {url} failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "daemon",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let parsed: PublishArtifactResponse =
            response.json().await.map_err(|e| PublishError::Hosting {
                backend: "daemon",
                reason: format!("publish response JSON parse failed: {e}"),
            })?;
        Ok(parsed)
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn control_auth_value_resolves() {
        let auth = ControlAuth::Value("  my-token  ".to_string());
        assert_eq!(auth.resolve().await.unwrap(), "my-token");
    }

    #[tokio::test]
    async fn control_auth_file_resolves() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(tmp.path(), "file-token\n").await.unwrap();
        let auth = ControlAuth::File(tmp.path().to_path_buf());
        assert_eq!(auth.resolve().await.unwrap(), "file-token");
    }

    #[tokio::test]
    async fn control_auth_file_missing_errors() {
        let auth = ControlAuth::File(PathBuf::from("/definitely/not/here"));
        let err = auth.resolve().await.unwrap_err();
        assert!(matches!(err, PublishError::InvalidInput { .. }));
    }

    #[test]
    fn from_env_with_value_token() {
        let _g = TestEnv::set_var("FROGLET_PROVIDER_CONTROL_TOKEN", "test-tok");
        let client = DaemonClient::from_env().unwrap();
        assert!(matches!(client.control_auth, ControlAuth::Value(ref v) if v == "test-tok"));
    }

    /// RAII guard that sets an env var for the duration of one test and
    /// restores the previous value on drop. Avoids cross-test pollution
    /// that single-threaded test runners would otherwise hit. Single-
    /// threaded tests can shrug at this; multi-threaded does not.
    struct TestEnv {
        key: &'static str,
        previous: Option<String>,
    }

    impl TestEnv {
        fn set_var(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: tests are single-threaded by convention for env vars.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
