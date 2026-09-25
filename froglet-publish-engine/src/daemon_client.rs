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
use crate::managed_oci::PlannedManagedOciLayoutV1;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use froglet_protocol::managed_publication::{
    ManagedPublicationBundleManifestV1, ManagedPublicationCapsuleV1, ManagedPublicationOperationV1,
    ManagedPublicationPackageRequestV1, ManagedPublicationPackageV1, ManagedPublicationPlanV1,
    ManagedPublicationRegistrationV1,
};
use froglet_protocol::publication::{
    LocalVerificationEvidence, PUBLICATION_PRECONDITION_HEADER, PublicationPrecondition,
    SignedPublicationRevision,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};
use url::Url;

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:8080";
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(320);
const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(5);
const PRECONDITION_TIMEOUT: Duration = Duration::from_secs(10);
/// Must remain aligned with the provider upload route's decoded-body limit.
pub const MAX_MANAGED_PUBLICATION_UPLOAD_BODY_BYTES: usize = 24 * 1024 * 1024;

struct BoundedJsonBody {
    bytes: Vec<u8>,
    limit: usize,
}

impl BoundedJsonBody {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
        }
    }
}

impl Write for BoundedJsonBody {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(
                "managed publication upload exceeds its encoded body limit",
            ));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialize_json_body_bounded<T: Serialize>(
    value: &T,
    limit: usize,
) -> Result<Vec<u8>, PublishError> {
    let mut output = BoundedJsonBody::new(limit);
    serde_json::to_writer(&mut output, value).map_err(|error| PublishError::InvalidInput {
        field: "managed_publication_upload",
        reason: format!("encoded request exceeds {limit} bytes or is invalid: {error}"),
    })?;
    Ok(output.bytes)
}

pub(crate) fn is_activation_token(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

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

/// The publish engine and provider daemon deliberately share this one
/// authoring DTO. Adding a publication field anywhere else cannot create a
/// second hand-maintained transport shape.
pub use froglet_protocol::publication::PublicationIntent as PublishArtifactRequest;

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
    #[serde(default)]
    pub local_verification: Option<LocalVerificationEvidence>,
    #[serde(default)]
    pub publication_revision: Option<SignedPublicationRevision>,
    pub activation_token: String,
    #[serde(default)]
    pub previous_publication: Option<PublicationLifecycleSnapshot>,
    #[serde(default)]
    pub previous_transport_grants: Vec<PublicationTransportGrantSnapshot>,
}

#[derive(Debug, Serialize)]
struct ExactPauseRequest<'a> {
    activation_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_publication: Option<&'a PublicationLifecycleSnapshot>,
    previous_transport_grants: &'a [PublicationTransportGrantSnapshot],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicationLifecycleSnapshot {
    pub service_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_revision_hash: Option<String>,
    pub selected_revision_hash: String,
    pub activation_token: String,
    pub revision_count: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicationTransportGrantSnapshot {
    pub transport: String,
    pub service_id: String,
    pub revision_hash: String,
    pub offer_id: String,
    pub offer_hash: String,
    pub activation_token: String,
    pub public_url: String,
    pub relay_control_url: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
struct RelayActivationRequest<'a> {
    service_id: &'a str,
    revision_hash: &'a str,
    activation_token: &'a str,
    public_url: &'a str,
    relay_control_url: &'a str,
}

#[derive(Debug, Serialize)]
struct ManagedPublicationCapsuleRequest<'a> {
    plan: &'a ManagedPublicationPlanV1,
    consent_hash: &'a str,
    revision_hash: &'a str,
}

#[derive(Debug, Serialize)]
struct ManagedPublicationPackageProfileRequest<'a> {
    bundle: &'a ManagedPublicationBundleManifestV1,
    target: &'a str,
    profile: &'a str,
}

#[derive(Debug, Deserialize)]
struct ManagedPublicationPackageProfileResponse {
    request: ManagedPublicationPackageRequestV1,
    base_manifest_base64: String,
    base_config_base64: String,
}

#[derive(Debug, Serialize)]
struct ManagedPublicationPlanRequest<'a> {
    bundle: &'a ManagedPublicationBundleManifestV1,
    package: &'a ManagedPublicationPackageV1,
    target: &'a str,
    profile: &'a str,
}

#[derive(Debug, Serialize)]
struct ManagedPublicationActivationRequest<'a> {
    bundle: &'a ManagedPublicationBundleManifestV1,
    capsule: &'a ManagedPublicationCapsuleV1,
}

#[derive(Debug, Serialize)]
struct ManagedPublicationUploadRequest<'a> {
    plan: &'a ManagedPublicationPlanV1,
    layout: &'a PlannedManagedOciLayoutV1,
}

#[derive(Debug, Serialize)]
struct ManagedPublicationPrepareRequest<'a> {
    plan: &'a ManagedPublicationPlanV1,
    consent_hash: &'a str,
}

#[derive(Debug, Deserialize)]
struct ManagedPublicationUploadResponse {
    operation_id: String,
    image: froglet_protocol::managed_deployment::OciImageV1,
    verified_manifest_digest: String,
}

#[derive(Debug, Deserialize)]
struct ManagedPublicationActivationResponse {
    operation: ManagedPublicationOperationV1,
}

#[derive(Debug, Deserialize)]
struct RelayActivationResponse {
    status: String,
    public_url: String,
}

#[derive(Debug, Deserialize)]
struct PausePublicationResponse {
    publication: PausedPublication,
    #[serde(default)]
    relay_withdrawal: Option<RelayWithdrawalResponse>,
}

#[derive(Debug, Deserialize)]
struct RelayWithdrawalResponse {
    status: String,
    remaining_grants: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PauseCompensationOutcome {
    pub relay_withdrawal_status: String,
    pub remaining_relay_grants: usize,
}

#[derive(Debug, Deserialize)]
struct PausedPublication {
    status: String,
    selected_revision_hash: String,
    activation_token: String,
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
    #[serde(default)]
    pub relay: Option<TransportEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransportEntry {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub onion_url: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub control_url: Option<String>,
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
        let http = crate::http_client_builder()
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
    /// - otherwise `FROGLET_DATA_DIR/runtime/froglet-control.token`, and with
    ///   no data dir set, the first existing of the daemon layout
    ///   (`~/.froglet/runtime/`) and the agent-bootstrap layout
    ///   (`~/.froglet/data/runtime/`)
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
        } else if let Ok(data_dir) = std::env::var("FROGLET_DATA_DIR") {
            ControlAuth::File(PathBuf::from(data_dir).join("runtime/froglet-control.token"))
        } else {
            let home = dirs_home().ok_or_else(|| PublishError::InvalidInput {
                field: "FROGLET_DATA_DIR",
                reason: "no FROGLET_DATA_DIR or HOME set; cannot find provider-control token"
                    .to_string(),
            })?;
            ControlAuth::File(default_control_token_path(&home))
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

    /// Resolve provider identity and exact effective limits for this immutable
    /// intent without staging data or persisting publication state.
    pub async fn publication_precondition(
        &self,
        request: &PublishArtifactRequest,
    ) -> Result<PublicationPrecondition, PublishError> {
        let token = self.control_auth.resolve().await?;
        if token.is_empty() {
            return Err(PublishError::InvalidInput {
                field: "provider_control_token",
                reason: "provider-control token is empty; check the daemon's runtime/froglet-control.token"
                    .to_string(),
            });
        }

        let mut url = self.daemon_url.clone();
        url.set_path("/v1/provider/artifacts/preflight");
        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PRECONDITION_TIMEOUT)
            .json(request)
            .send()
            .await
            .map_err(|error| PublishError::Http(format!("POST {url} failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "daemon-preflight",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        response
            .json()
            .await
            .map_err(|error| PublishError::Hosting {
                backend: "daemon-preflight",
                reason: format!("preflight response JSON parse failed: {error}"),
            })
    }

    /// Resolve immutable packaging inputs for one provider-private target
    /// profile. The daemon returns raw base OCI bytes so digest verification is
    /// performed by the deterministic packager, not trusted from configuration.
    pub async fn managed_publication_package_profile(
        &self,
        bundle: &ManagedPublicationBundleManifestV1,
        target: &str,
        profile: &str,
    ) -> Result<(ManagedPublicationPackageRequestV1, Vec<u8>, Vec<u8>), PublishError> {
        bundle
            .validate()
            .map_err(|error| PublishError::InvalidInput {
                field: "managed_publication_bundle",
                reason: error.to_string(),
            })?;
        let token = self.control_auth.resolve().await?;
        if token.is_empty() {
            return Err(PublishError::InvalidInput {
                field: "provider_control_token",
                reason: "provider-control token is empty".to_string(),
            });
        }
        let mut url = self.daemon_url.clone();
        url.set_path("/v1/provider/managed-publications/package-request");
        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PRECONDITION_TIMEOUT)
            .json(&ManagedPublicationPackageProfileRequest {
                bundle,
                target,
                profile,
            })
            .send()
            .await
            .map_err(|error| PublishError::Http(format!("POST {url} failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "managed-publication-package-profile",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let response: ManagedPublicationPackageProfileResponse =
            response
                .json()
                .await
                .map_err(|error| PublishError::Hosting {
                    backend: "managed-publication-package-profile",
                    reason: format!("package profile response JSON parse failed: {error}"),
                })?;
        response
            .request
            .validate()
            .map_err(|error| PublishError::Hosting {
                backend: "managed-publication-package-profile",
                reason: error.to_string(),
            })?;
        if response.request.bundle != *bundle {
            return Err(PublishError::Hosting {
                backend: "managed-publication-package-profile",
                reason: "daemon returned a package request for a different bundle".to_string(),
            });
        }
        let manifest = STANDARD
            .decode(response.base_manifest_base64)
            .map_err(|_| PublishError::Hosting {
                backend: "managed-publication-package-profile",
                reason: "daemon returned invalid base manifest base64".to_string(),
            })?;
        let config =
            STANDARD
                .decode(response.base_config_base64)
                .map_err(|_| PublishError::Hosting {
                    backend: "managed-publication-package-profile",
                    reason: "daemon returned invalid base config base64".to_string(),
                })?;
        Ok((response.request, manifest, config))
    }

    pub async fn managed_publication_plan(
        &self,
        bundle: &ManagedPublicationBundleManifestV1,
        package: &ManagedPublicationPackageV1,
        target: &str,
        profile: &str,
    ) -> Result<ManagedPublicationPlanV1, PublishError> {
        let token = self.control_auth.resolve().await?;
        if token.is_empty() {
            return Err(PublishError::InvalidInput {
                field: "provider_control_token",
                reason: "provider-control token is empty".to_string(),
            });
        }
        let mut url = self.daemon_url.clone();
        url.set_path("/v1/provider/managed-publications/plan");
        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PUBLISH_TIMEOUT)
            .json(&ManagedPublicationPlanRequest {
                bundle,
                package,
                target,
                profile,
            })
            .send()
            .await
            .map_err(|error| PublishError::Http(format!("POST {url} failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "managed-publication-plan",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let plan: ManagedPublicationPlanV1 =
            response
                .json()
                .await
                .map_err(|error| PublishError::Hosting {
                    backend: "managed-publication-plan",
                    reason: format!("managed plan response JSON parse failed: {error}"),
                })?;
        plan.validate().map_err(|error| PublishError::Hosting {
            backend: "managed-publication-plan",
            reason: error.to_string(),
        })?;
        if plan.payload.target != target
            || plan.payload.profile != profile
            || plan.payload.package != *package
            || plan.payload.publish_request_digest != bundle.publish_request_digest
        {
            return Err(PublishError::Hosting {
                backend: "managed-publication-plan",
                reason: "daemon returned a plan for different immutable inputs".to_string(),
            });
        }
        Ok(plan)
    }

    pub async fn upload_managed_publication_image(
        &self,
        plan: &ManagedPublicationPlanV1,
        layout: &PlannedManagedOciLayoutV1,
    ) -> Result<(), PublishError> {
        plan.validate()
            .map_err(|error| PublishError::InvalidInput {
                field: "managed_publication_plan",
                reason: error.to_string(),
            })?;
        if layout.package != plan.payload.package {
            return Err(PublishError::InvalidInput {
                field: "managed_oci_layout",
                reason: "layout package differs from approved plan".to_string(),
            });
        }
        // Encode through a capped writer before resolving credentials or
        // contacting the daemon. This bounds both allocation and upload size;
        // reqwest's `.json(...)` would otherwise materialize an unbounded body.
        let body = serialize_json_body_bounded(
            &ManagedPublicationUploadRequest { plan, layout },
            MAX_MANAGED_PUBLICATION_UPLOAD_BODY_BYTES,
        )?;
        let token = self.control_auth.resolve().await?;
        let mut url = self.daemon_url.clone();
        url.set_path("/v1/provider/managed-publications/upload");
        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PUBLISH_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| PublishError::Http(format!("POST {url} failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "managed-publication-upload",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let uploaded: ManagedPublicationUploadResponse =
            response
                .json()
                .await
                .map_err(|error| PublishError::Hosting {
                    backend: "managed-publication-upload",
                    reason: format!("managed upload response JSON parse failed: {error}"),
                })?;
        if uploaded.operation_id != plan.payload.operation_id
            || uploaded.image != plan.payload.package.image
            || uploaded.verified_manifest_digest != plan.payload.package.image.digest
        {
            return Err(PublishError::Hosting {
                backend: "managed-publication-upload",
                reason: "daemon upload proof differs from the approved package".to_string(),
            });
        }
        Ok(())
    }

    /// Persist the exact approved plan and consent before the first registry
    /// or operator mutation. Repeating the same pair returns its current
    /// durable phase so interrupted or already-active work can resume;
    /// immutable drift is a conflict.
    pub async fn prepare_managed_publication(
        &self,
        plan: &ManagedPublicationPlanV1,
        consent_hash: &str,
    ) -> Result<ManagedPublicationOperationV1, PublishError> {
        plan.validate()
            .map_err(|error| PublishError::InvalidInput {
                field: "managed_publication_plan",
                reason: error.to_string(),
            })?;
        let token = self.control_auth.resolve().await?;
        let mut url = self.daemon_url.clone();
        url.set_path("/v1/provider/managed-publications/prepare");
        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PRECONDITION_TIMEOUT)
            .json(&ManagedPublicationPrepareRequest { plan, consent_hash })
            .send()
            .await
            .map_err(|error| PublishError::Http(format!("POST {url} failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "managed-publication-prepare",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let response: ManagedPublicationActivationResponse =
            response
                .json()
                .await
                .map_err(|error| PublishError::Hosting {
                    backend: "managed-publication-prepare",
                    reason: format!("managed prepare response JSON parse failed: {error}"),
                })?;
        response
            .operation
            .validate_against(plan)
            .map_err(|error| PublishError::Hosting {
                backend: "managed-publication-prepare",
                reason: error.to_string(),
            })?;
        if response.operation.consent_hash != consent_hash {
            return Err(PublishError::Hosting {
                backend: "managed-publication-prepare",
                reason: "daemon prepared a different consent".to_string(),
            });
        }
        Ok(response.operation)
    }

    pub async fn activate_managed_publication(
        &self,
        bundle: &ManagedPublicationBundleManifestV1,
        capsule: &ManagedPublicationCapsuleV1,
    ) -> Result<ManagedPublicationOperationV1, PublishError> {
        let token = self.control_auth.resolve().await?;
        if token.is_empty() {
            return Err(PublishError::InvalidInput {
                field: "provider_control_token",
                reason: "provider-control token is empty".to_string(),
            });
        }
        let mut url = self.daemon_url.clone();
        url.set_path("/v1/provider/managed-publications/activate");
        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PUBLISH_TIMEOUT)
            .json(&ManagedPublicationActivationRequest { bundle, capsule })
            .send()
            .await
            .map_err(|error| PublishError::Http(format!("POST {url} failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "managed-publication-activate",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let response: ManagedPublicationActivationResponse =
            response
                .json()
                .await
                .map_err(|error| PublishError::Hosting {
                    backend: "managed-publication-activate",
                    reason: format!("managed activation response JSON parse failed: {error}"),
                })?;
        response
            .operation
            .validate_against(&capsule.plan)
            .map_err(|error| PublishError::Hosting {
                backend: "managed-publication-activate",
                reason: error.to_string(),
            })?;
        Ok(response.operation)
    }

    pub async fn compensate_managed_publication(
        &self,
        operation_id: &str,
    ) -> Result<ManagedPublicationOperationV1, PublishError> {
        let token = self.control_auth.resolve().await?;
        let mut url = self.daemon_url.clone();
        url.set_path(&format!(
            "/v1/provider/managed-publications/operations/{operation_id}/compensate"
        ));
        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PUBLISH_TIMEOUT)
            .send()
            .await
            .map_err(|error| PublishError::Http(format!("POST {url} failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "managed-publication-compensate",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let response: ManagedPublicationActivationResponse =
            response
                .json()
                .await
                .map_err(|error| PublishError::Hosting {
                    backend: "managed-publication-compensate",
                    reason: format!("managed compensation response JSON parse failed: {error}"),
                })?;
        Ok(response.operation)
    }

    pub async fn reconcile_managed_publication(
        &self,
        operation_id: &str,
    ) -> Result<ManagedPublicationOperationV1, PublishError> {
        self.managed_publication_operation_post(operation_id, "reconcile", None)
            .await
    }

    pub async fn begin_managed_publication_registration(
        &self,
        operation_id: &str,
    ) -> Result<ManagedPublicationOperationV1, PublishError> {
        self.managed_publication_operation_post(operation_id, "registering", None)
            .await
    }

    pub async fn complete_managed_publication(
        &self,
        operation_id: &str,
        registration: &ManagedPublicationRegistrationV1,
    ) -> Result<ManagedPublicationOperationV1, PublishError> {
        self.managed_publication_operation_post(operation_id, "complete", Some(registration))
            .await
    }

    async fn managed_publication_operation_post(
        &self,
        operation_id: &str,
        action: &'static str,
        registration: Option<&ManagedPublicationRegistrationV1>,
    ) -> Result<ManagedPublicationOperationV1, PublishError> {
        let token = self.control_auth.resolve().await?;
        if token.is_empty() {
            return Err(PublishError::InvalidInput {
                field: "provider_control_token",
                reason: "provider-control token is empty".to_string(),
            });
        }
        let mut url = self.daemon_url.clone();
        url.set_path(&format!(
            "/v1/provider/managed-publications/operations/{operation_id}/{action}"
        ));
        let mut request = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PUBLISH_TIMEOUT);
        if let Some(registration) = registration {
            request = request.json(registration);
        }
        let response = request
            .send()
            .await
            .map_err(|error| PublishError::Http(format!("POST {url} failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "managed-publication-operation",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let response: ManagedPublicationActivationResponse =
            response
                .json()
                .await
                .map_err(|error| PublishError::Hosting {
                    backend: "managed-publication-operation",
                    reason: format!("managed operation response JSON parse failed: {error}"),
                })?;
        Ok(response.operation)
    }

    /// Build the self-contained, exact Descriptor/Offer/Revision handoff for a
    /// validated Managed Publication plan. This is read-only: operator or
    /// registry mutation remains a separate durable activation step.
    pub async fn managed_publication_capsule(
        &self,
        plan: &ManagedPublicationPlanV1,
        consent_hash: &str,
        revision_hash: &str,
    ) -> Result<ManagedPublicationCapsuleV1, PublishError> {
        plan.validate()
            .map_err(|error| PublishError::InvalidInput {
                field: "managed_publication_plan",
                reason: error.to_string(),
            })?;
        for (field, value) in [
            ("managed_publication_consent_hash", consent_hash),
            ("managed_publication_revision_hash", revision_hash),
        ] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            {
                return Err(PublishError::InvalidInput {
                    field,
                    reason: "must be 64 lowercase hexadecimal characters".to_string(),
                });
            }
        }
        let token = self.control_auth.resolve().await?;
        if token.is_empty() {
            return Err(PublishError::InvalidInput {
                field: "provider_control_token",
                reason: "provider-control token is empty".to_string(),
            });
        }
        let mut url = self.daemon_url.clone();
        url.set_path("/v1/provider/managed-publications/capsule");
        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PRECONDITION_TIMEOUT)
            .json(&ManagedPublicationCapsuleRequest {
                plan,
                consent_hash,
                revision_hash,
            })
            .send()
            .await
            .map_err(|error| PublishError::Http(format!("POST {url} failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "managed-publication-capsule",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let capsule: ManagedPublicationCapsuleV1 =
            response
                .json()
                .await
                .map_err(|error| PublishError::Hosting {
                    backend: "managed-publication-capsule",
                    reason: format!("capsule response JSON parse failed: {error}"),
                })?;
        capsule
            .validate_against(plan)
            .map_err(|error| PublishError::Hosting {
                backend: "managed-publication-capsule",
                reason: error.to_string(),
            })?;
        if capsule.consent_hash != consent_hash || capsule.source_revision_hash != revision_hash {
            return Err(PublishError::Hosting {
                backend: "managed-publication-capsule",
                reason: "daemon returned a capsule for a different approval or source revision"
                    .to_string(),
            });
        }
        Ok(capsule)
    }

    /// `POST /v1/provider/artifacts/publish` — sign + persist an offer.
    pub async fn publish_artifact(
        &self,
        request: &PublishArtifactRequest,
        approved_precondition: Option<&str>,
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

        if let Some(approved_precondition) = approved_precondition
            && (approved_precondition.len() != 64
                || !approved_precondition
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(PublishError::InvalidInput {
                field: "publication_precondition",
                reason: "approved publication precondition must be a 64-hex token".to_string(),
            });
        }

        let mut request_builder = self.http.post(url.clone()).bearer_auth(&token);
        if let Some(approved_precondition) = approved_precondition {
            request_builder =
                request_builder.header(PUBLICATION_PRECONDITION_HEADER, approved_precondition);
        }
        let response = match request_builder.json(request).send().await {
            Ok(response) => response,
            Err(error) if approved_precondition.is_some() => {
                return Err(PublishError::PartialState {
                    service_id: request.service_id.clone(),
                    revision_hash: "unknown".to_string(),
                    completion_error: format!("POST {url} did not return a response: {error}"),
                    compensation_error: "the daemon may have persisted a revision, but no exact revision hash was returned for safe compensation"
                        .to_string(),
                });
            }
            Err(error) => return Err(PublishError::Http(format!("POST {url} failed: {error}"))),
        };
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "daemon",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let parsed: PublishArtifactResponse = match response.json().await {
            Ok(parsed) => parsed,
            Err(error) if approved_precondition.is_some() => {
                return Err(PublishError::PartialState {
                    service_id: request.service_id.clone(),
                    revision_hash: "unknown".to_string(),
                    completion_error: format!("publish response JSON parse failed: {error}"),
                    compensation_error: "the daemon returned success without a readable exact revision hash for safe compensation"
                        .to_string(),
                });
            }
            Err(error) => {
                return Err(PublishError::Hosting {
                    backend: "daemon",
                    reason: format!("publish response JSON parse failed: {error}"),
                });
            }
        };
        Ok(parsed)
    }

    /// Persist and activate one exact relay transport grant. The daemon
    /// validates the active revision/token and waits for the approval-bound
    /// endpoint to be live before returning.
    pub async fn activate_relay_transport(
        &self,
        service_id: &str,
        revision_hash: &str,
        activation_token: &str,
        public_url: &str,
        relay_control_url: &str,
    ) -> Result<(), PublishError> {
        if !is_activation_token(activation_token) {
            return Err(PublishError::InvalidInput {
                field: "activation_token",
                reason: "relay activation token must be 64 lowercase hexadecimal characters"
                    .to_string(),
            });
        }
        let token = self.control_auth.resolve().await?;
        if token.is_empty() {
            return Err(PublishError::InvalidInput {
                field: "provider_control_token",
                reason: "provider-control token is empty".to_string(),
            });
        }
        let mut url = self.daemon_url.clone();
        url.set_path("/v1/provider/transports/relay/activate");
        let body = RelayActivationRequest {
            service_id,
            revision_hash,
            activation_token,
            public_url,
            relay_control_url,
        };

        // A lost response is ambiguous because the durable grant may already
        // exist. Replay the exact idempotent request once before reporting
        // partial state to the caller.
        let mut last_send_error = None;
        let response = loop {
            match self
                .http
                .post(url.clone())
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
            {
                Ok(response) => break response,
                Err(error) if last_send_error.is_none() => {
                    last_send_error = Some(error.to_string())
                }
                Err(error) => {
                    return Err(PublishError::PartialState {
                        service_id: service_id.to_string(),
                        revision_hash: revision_hash.to_string(),
                        completion_error: format!(
                            "relay activation response was lost twice: first={:?}; second={error}",
                            last_send_error
                        ),
                        compensation_error:
                            "the exact grant may be durable; exact pause compensation is required"
                                .to_string(),
                    });
                }
            }
        };
        let status = response.status();
        if !status.is_success() {
            let response_body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "relay",
                reason: format!("POST {url} returned HTTP {status}: {response_body}"),
            });
        }
        let activated: RelayActivationResponse =
            response
                .json()
                .await
                .map_err(|error| PublishError::PartialState {
                    service_id: service_id.to_string(),
                    revision_hash: revision_hash.to_string(),
                    completion_error: format!(
                        "relay activation response JSON was invalid: {error}"
                    ),
                    compensation_error:
                        "the exact grant may be durable; exact pause compensation is required"
                            .to_string(),
                })?;
        if activated.status != "up"
            || activated.public_url.trim_end_matches('/') != public_url.trim_end_matches('/')
        {
            return Err(PublishError::Hosting {
                backend: "relay",
                reason: format!(
                    "daemon activated relay status {:?} at {:?}, expected up at {:?}",
                    activated.status, activated.public_url, public_url
                ),
            });
        }
        Ok(())
    }

    /// Pause only the exact activation created by this publish attempt. The
    /// daemon compares both revision and activation token, so delayed
    /// compensation cannot pause a later activation of the same revision.
    pub async fn pause_publication_revision(
        &self,
        service_id: &str,
        revision_hash: &str,
        activation_token: &str,
        previous_publication: Option<&PublicationLifecycleSnapshot>,
        previous_transport_grants: &[PublicationTransportGrantSnapshot],
    ) -> Result<PauseCompensationOutcome, PublishError> {
        if service_id.trim().is_empty() || service_id.len() > 128 {
            return Err(PublishError::InvalidInput {
                field: "service_id",
                reason: "compensation service_id must contain 1-128 bytes".to_string(),
            });
        }
        if revision_hash.len() != 64 || !revision_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(PublishError::InvalidInput {
                field: "revision_hash",
                reason: "compensation revision_hash must be 64 hexadecimal characters".to_string(),
            });
        }
        if !is_activation_token(activation_token) {
            return Err(PublishError::InvalidInput {
                field: "activation_token",
                reason: "compensation activation_token must be 64 lowercase hexadecimal characters"
                    .to_string(),
            });
        }
        let token = self.control_auth.resolve().await?;
        if token.is_empty() {
            return Err(PublishError::InvalidInput {
                field: "provider_control_token",
                reason: "provider-control token is empty; check the daemon's runtime/froglet-control.token"
                    .to_string(),
            });
        }

        let mut url = self.daemon_url.clone();
        url.set_query(None);
        url.set_fragment(None);
        {
            let mut segments = url.path_segments_mut().map_err(|_| PublishError::Hosting {
                backend: "daemon-compensation",
                reason: "daemon URL cannot carry provider-control path segments".to_string(),
            })?;
            segments.clear();
            for segment in [
                "v1",
                "provider",
                "publications",
                service_id,
                "revisions",
                revision_hash,
                "pause",
            ] {
                segments.push(segment);
            }
        }
        let response = self
            .http
            .post(url.clone())
            .bearer_auth(&token)
            .timeout(PRECONDITION_TIMEOUT)
            .json(&ExactPauseRequest {
                activation_token,
                previous_publication,
                previous_transport_grants,
            })
            .send()
            .await
            .map_err(|error| PublishError::Hosting {
                backend: "daemon-compensation",
                reason: format!("POST {url} failed: {error}"),
            })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PublishError::Hosting {
                backend: "daemon-compensation",
                reason: format!("POST {url} returned HTTP {status}: {body}"),
            });
        }
        let paused: PausePublicationResponse =
            response
                .json()
                .await
                .map_err(|error| PublishError::Hosting {
                    backend: "daemon-compensation",
                    reason: format!("exact pause response JSON parse failed: {error}"),
                })?;
        let (expected_status, expected_revision_hash) =
            previous_publication.map_or(("paused", revision_hash), |previous| {
                (
                    previous.status.as_str(),
                    previous.selected_revision_hash.as_str(),
                )
            });
        if paused.publication.status != expected_status
            || paused.publication.selected_revision_hash != expected_revision_hash
            || !is_activation_token(&paused.publication.activation_token)
        {
            return Err(PublishError::Hosting {
                backend: "daemon-compensation",
                reason: format!(
                    "exact compensation returned status {:?}, revision {:?}, and activation token {:?}; expected status {:?} and revision {:?}",
                    paused.publication.status,
                    paused.publication.selected_revision_hash,
                    paused.publication.activation_token,
                    expected_status,
                    expected_revision_hash,
                ),
            });
        }
        let relay_withdrawal = paused
            .relay_withdrawal
            .ok_or_else(|| PublishError::Hosting {
                backend: "daemon-compensation",
                reason: "exact pause response omitted relay withdrawal evidence".to_string(),
            })?;
        if !matches!(
            relay_withdrawal.status.as_str(),
            "reserved" | "shared_active"
        ) {
            return Err(PublishError::Hosting {
                backend: "daemon-compensation",
                reason: format!(
                    "exact pause returned unknown relay withdrawal status {:?}",
                    relay_withdrawal.status
                ),
            });
        }
        Ok(PauseCompensationOutcome {
            relay_withdrawal_status: relay_withdrawal.status,
            remaining_relay_grants: relay_withdrawal.remaining_grants,
        })
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The daemon's own data dir defaults to `~/.froglet` while the agent
/// bootstrap (scripts/agent-bootstrap.sh) provisions `~/.froglet/data`, so a
/// fresh shell must probe both token layouts. When neither file exists yet,
/// return the daemon layout so error messages point at the canonical path.
fn default_control_token_path(home: &std::path::Path) -> PathBuf {
    let candidates = [
        home.join(".froglet/runtime/froglet-control.token"),
        home.join(".froglet/data/runtime/froglet-control.token"),
    ];
    for candidate in &candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }
    candidates[0].clone()
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
    fn managed_upload_encoding_is_bounded_before_network_io() {
        let oversized = vec![0_u8; MAX_MANAGED_PUBLICATION_UPLOAD_BODY_BYTES / 2 + 1];
        let error =
            serialize_json_body_bounded(&oversized, MAX_MANAGED_PUBLICATION_UPLOAD_BODY_BYTES)
                .expect_err("JSON integer-array expansion must hit the body cap");
        assert!(matches!(
            error,
            PublishError::InvalidInput {
                field: "managed_publication_upload",
                ..
            }
        ));
    }

    #[test]
    fn from_env_with_value_token() {
        let _g = TestEnv::set_var("FROGLET_PROVIDER_CONTROL_TOKEN", "test-tok");
        let client = DaemonClient::from_env().unwrap();
        assert!(matches!(client.control_auth, ControlAuth::Value(ref v) if v == "test-tok"));
    }

    #[test]
    fn default_token_path_prefers_daemon_layout_when_present() {
        let home = tempfile::tempdir().unwrap();
        let daemon = home.path().join(".froglet/runtime/froglet-control.token");
        let bootstrap = home
            .path()
            .join(".froglet/data/runtime/froglet-control.token");
        for p in [&daemon, &bootstrap] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "tok").unwrap();
        }
        assert_eq!(default_control_token_path(home.path()), daemon);
    }

    #[test]
    fn default_token_path_probes_bootstrap_layout() {
        let home = tempfile::tempdir().unwrap();
        let bootstrap = home
            .path()
            .join(".froglet/data/runtime/froglet-control.token");
        std::fs::create_dir_all(bootstrap.parent().unwrap()).unwrap();
        std::fs::write(&bootstrap, "tok").unwrap();
        assert_eq!(default_control_token_path(home.path()), bootstrap);
    }

    #[test]
    fn default_token_path_falls_back_to_daemon_layout() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            default_control_token_path(home.path()),
            home.path().join(".froglet/runtime/froglet-control.token")
        );
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
