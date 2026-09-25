//! Pure wire contract for isolated OCI execution workers.
//!
//! This module is intentionally outside the signed Froglet Kernel artifact
//! model. It contains only serializable request/result types, deterministic
//! validation, and evidence hashes so worker implementations do not depend on
//! the full Froglet Node/runtime graph.

use crate::{
    canonical_json, crypto,
    publication::{OBJECT_STORE_MOUNT_KIND, canonical_mount_capability, canonical_mount_kind},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const OCI_WORKER_REQUEST_SCHEMA_V1: &str = "froglet.oci-worker-request.v1";
pub const OCI_WORKER_RESULT_SCHEMA_V1: &str = "froglet.oci-worker-result.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OciWorkerMount {
    pub handle: String,
    pub kind: String,
    pub read_only: bool,
}

impl OciWorkerMount {
    fn capability(&self) -> String {
        format!(
            "mount.{}.{}.{}",
            self.kind,
            if self.read_only { "read" } else { "write" },
            self.handle
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OciWorkerSecret {
    /// Logical worker-side secret handle. Secret values never cross this seam.
    pub handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OciWorkerLimits {
    pub max_input_bytes: usize,
    pub max_runtime_ms: u64,
    pub max_memory_bytes: u64,
    pub max_output_bytes: usize,
    pub pids_limit: u64,
    /// CPU quota in thousandths of one core (1000 = one core).
    pub cpu_millis: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OciWorkerRequest {
    pub schema_version: String,
    /// Immutable registry/repository@sha256:<digest> reference.
    pub image: String,
    pub entrypoint_kind: String,
    pub entrypoint: String,
    pub input: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<OciWorkerMount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<OciWorkerSecret>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub granted_capabilities: Vec<String>,
    pub network_egress: bool,
    pub limits: OciWorkerLimits,
}

impl OciWorkerRequest {
    /// Converts compatibility aliases at the worker seam. Evidence hashes and
    /// worker-side policy matching therefore use one provider-neutral form,
    /// regardless of whether an older node sent `s3` on the wire.
    pub fn normalized(mut self) -> Result<Self, String> {
        if self
            .granted_capabilities
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
            || self
                .granted_capabilities
                .iter()
                .any(|capability| !valid_capability(capability))
        {
            return Err("OCI worker granted_capabilities must be sorted and unique".to_string());
        }
        for mount in &mut self.mounts {
            mount.kind = canonical_mount_kind(&mount.kind)
                .ok_or_else(|| "OCI worker mount kind is unsupported".to_string())?
                .to_string();
        }
        for capability in &mut self.granted_capabilities {
            *capability = canonical_mount_capability(capability);
        }
        self.granted_capabilities.sort();
        self.granted_capabilities.dedup();
        self.validate_canonical()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.clone().normalized().map(|_| ())
    }

    fn validate_canonical(&self) -> Result<(), String> {
        if self.schema_version != OCI_WORKER_REQUEST_SCHEMA_V1 {
            return Err("unsupported OCI worker request schema_version".to_string());
        }
        validate_digest_image(&self.image)?;
        if !matches!(
            self.entrypoint_kind.as_str(),
            "handler" | "script" | "image"
        ) {
            return Err("OCI worker entrypoint_kind must be handler, script, or image".to_string());
        }
        if self.entrypoint.is_empty()
            || self.entrypoint.len() > 512
            || self.entrypoint.chars().any(char::is_control)
        {
            return Err("OCI worker entrypoint is invalid".to_string());
        }
        if self.limits.max_input_bytes == 0
            || self.limits.max_runtime_ms == 0
            || self.limits.max_memory_bytes == 0
            || self.limits.max_output_bytes == 0
            || self.limits.pids_limit == 0
            || self.limits.cpu_millis == 0
        {
            return Err("OCI worker limits must all be greater than zero".to_string());
        }
        let input_bytes = canonical_json::to_vec(&self.input).map_err(|error| error.to_string())?;
        if input_bytes.len() > self.limits.max_input_bytes {
            return Err("OCI worker input exceeds max_input_bytes".to_string());
        }
        if self
            .granted_capabilities
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
            || self
                .granted_capabilities
                .iter()
                .any(|capability| !valid_capability(capability))
        {
            return Err("OCI worker granted_capabilities must be sorted and unique".to_string());
        }
        if self.network_egress
            != self
                .granted_capabilities
                .iter()
                .any(|capability| capability == "network.egress")
        {
            return Err("OCI worker network_egress must match its explicit grant".to_string());
        }
        let grants = self
            .granted_capabilities
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut mount_handles = HashSet::with_capacity(self.mounts.len());
        for mount in &self.mounts {
            validate_handle("mount", &mount.handle)?;
            if !matches!(
                mount.kind.as_str(),
                "postgres" | "sqlite" | OBJECT_STORE_MOUNT_KIND | "redis"
            ) {
                return Err("OCI worker mount kind is unsupported".to_string());
            }
            if !mount_handles.insert(mount.handle.as_str()) {
                return Err("OCI worker mount handles must be unique".to_string());
            }
            if !grants.contains(mount.capability().as_str()) {
                return Err("OCI worker mount is missing its explicit capability grant".to_string());
            }
        }
        let mut secret_handles = HashSet::with_capacity(self.secrets.len());
        for secret in &self.secrets {
            validate_handle("secret", &secret.handle)?;
            if !secret_handles.insert(secret.handle.as_str()) {
                return Err("OCI worker secret handles must be unique".to_string());
            }
            if !grants.contains(format!("secret.{}", secret.handle).as_str()) {
                return Err(
                    "OCI worker secret is missing its explicit capability grant".to_string()
                );
            }
        }
        Ok(())
    }

    pub fn request_hash(&self) -> Result<String, String> {
        let normalized = self.clone().normalized()?;
        let canonical = canonical_json::to_vec(&normalized).map_err(|error| error.to_string())?;
        Ok(crypto::sha256_hex(canonical))
    }

    pub fn image_digest(&self) -> Result<String, String> {
        validate_digest_image(&self.image)?;
        self.image
            .rsplit_once("@sha256:")
            .map(|(_, digest)| digest.to_string())
            .ok_or_else(|| "OCI worker image digest is missing".to_string())
    }

    pub fn granted_capabilities_hash(&self) -> Result<String, String> {
        let normalized = self.clone().normalized()?;
        let canonical = canonical_json::to_vec(&normalized.granted_capabilities)
            .map_err(|error| error.to_string())?;
        Ok(crypto::sha256_hex(canonical))
    }
}

/// Validates an option-safe, immutable OCI image argument accepted by both the
/// public worker client and reference Docker/Podman workers.
pub fn validate_digest_image(image: &str) -> Result<(), String> {
    let (repository, digest) = image.rsplit_once("@sha256:").ok_or_else(|| {
        "OCI worker image must be digest-only repository@sha256:<hex>".to_string()
    })?;
    let mut components = repository.split('/');
    let registry = components.next().unwrap_or_default();
    let paths = components.collect::<Vec<_>>();
    if image.len() > 576
        || repository.contains('@')
        || !valid_registry_component(registry)
        || paths.is_empty()
        || paths
            .iter()
            .any(|component| !valid_repository_component(component))
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("OCI worker image must be digest-only repository@sha256:<hex>".to_string());
    }
    Ok(())
}

fn valid_registry_component(registry: &str) -> bool {
    if registry.is_empty() || registry.len() > 255 {
        return false;
    }
    if let Some(bracketed) = registry.strip_prefix('[') {
        let Some((address, suffix)) = bracketed.split_once(']') else {
            return false;
        };
        return address.parse::<std::net::Ipv6Addr>().is_ok()
            && (suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_registry_port));
    }
    let (host, port) = registry
        .rsplit_once(':')
        .map_or((registry, None), |(host, port)| (host, Some(port)));
    !host.is_empty()
        && host.split('.').all(|label| {
            label
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                && label
                    .bytes()
                    .last()
                    .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        && port.is_none_or(valid_registry_port)
}

fn valid_registry_port(port: &str) -> bool {
    port.parse::<u16>().ok().is_some_and(|port| port > 0)
}

fn valid_repository_component(component: &str) -> bool {
    component.len() <= 255
        && component
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && component
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && component.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn valid_capability(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                })
        })
}

fn validate_handle(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return Err(format!("OCI worker {label} handle is invalid"));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OciWorkerResult {
    pub schema_version: String,
    pub image: String,
    pub output: Value,
    pub evidence: OciWorkerEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OciWorkerEvidence {
    pub request_hash: String,
    pub image_digest: String,
    pub granted_capabilities_hash: String,
    pub observed_duration_ms: u64,
    pub status: String,
}

impl OciWorkerResult {
    /// Verifies that worker evidence and output are bound to the exact request.
    pub fn validate_for(&self, request: &OciWorkerRequest) -> Result<(), String> {
        if self.schema_version != OCI_WORKER_RESULT_SCHEMA_V1 {
            return Err("unsupported OCI worker result schema_version".to_string());
        }
        if self.image != request.image {
            return Err("OCI worker result image does not match request".to_string());
        }
        let expected_request_hash = request.request_hash()?;
        if self.evidence.request_hash != expected_request_hash {
            return Err("OCI worker evidence request_hash does not match request".to_string());
        }
        let expected_image_digest = request.image_digest()?;
        if self.evidence.image_digest != expected_image_digest {
            return Err("OCI worker evidence image_digest does not match request".to_string());
        }
        let expected_capabilities_hash = request.granted_capabilities_hash()?;
        if self.evidence.granted_capabilities_hash != expected_capabilities_hash {
            return Err(
                "OCI worker evidence granted_capabilities_hash does not match request".to_string(),
            );
        }
        if self.evidence.status != "succeeded" {
            return Err("OCI worker result did not report succeeded status".to_string());
        }
        if self.evidence.observed_duration_ms > request.limits.max_runtime_ms {
            return Err("OCI worker evidence exceeds max_runtime_ms".to_string());
        }
        let bytes = canonical_json::to_vec(&self.output).map_err(|error| error.to_string())?;
        if bytes.len() > request.limits.max_output_bytes {
            return Err("OCI worker output exceeds max_output_bytes".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> OciWorkerRequest {
        OciWorkerRequest {
            schema_version: OCI_WORKER_REQUEST_SCHEMA_V1.to_string(),
            image: format!("registry.example/froglet/worker@sha256:{}", "ab".repeat(32)),
            entrypoint_kind: "image".to_string(),
            entrypoint: "default".to_string(),
            input: json!({"value": 7}),
            mounts: Vec::new(),
            secrets: Vec::new(),
            granted_capabilities: Vec::new(),
            network_egress: false,
            limits: OciWorkerLimits {
                max_input_bytes: 1024,
                max_runtime_ms: 1_000,
                max_memory_bytes: 64 * 1024 * 1024,
                max_output_bytes: 1024,
                pids_limit: 32,
                cpu_millis: 500,
            },
        }
    }

    #[test]
    fn rejects_mutable_option_like_tagged_or_malformed_images() {
        let mut request = request();
        for image in [
            "registry.example/froglet/worker:latest".to_string(),
            format!("--privileged/example@sha256:{}", "ab".repeat(32)),
            format!("registry.example/-option@sha256:{}", "ab".repeat(32)),
            format!("registry.example/example:latest@sha256:{}", "ab".repeat(32)),
            format!("registry.example/example\nnext@sha256:{}", "ab".repeat(32)),
            format!("registry.example//example@sha256:{}", "ab".repeat(32)),
            format!("registry.example:tag/example@sha256:{}", "ab".repeat(32)),
            format!("REGISTRY.example/froglet/worker@sha256:{}", "ab".repeat(32)),
        ] {
            request.image = image;
            assert!(request.validate().unwrap_err().contains("digest-only"));
        }
    }

    #[test]
    fn digest_image_accepts_registry_ports_and_bracketed_ipv6() {
        for repository in [
            "localhost:5000/froglet/worker",
            "[2001:db8::1]:5000/froglet/worker",
        ] {
            let image = format!("{repository}@sha256:{}", "ab".repeat(32));
            validate_digest_image(&image).expect("safe digest-pinned image");
        }
    }

    #[test]
    fn evidence_is_bound_to_the_exact_request() {
        let request = request();
        let result = OciWorkerResult {
            schema_version: OCI_WORKER_RESULT_SCHEMA_V1.to_string(),
            image: request.image.clone(),
            output: json!({"ok": true}),
            evidence: OciWorkerEvidence {
                request_hash: request.request_hash().expect("request hash"),
                image_digest: request.image_digest().expect("image digest"),
                granted_capabilities_hash: request
                    .granted_capabilities_hash()
                    .expect("capabilities hash"),
                observed_duration_ms: 10,
                status: "succeeded".to_string(),
            },
        };
        result.validate_for(&request).expect("matching evidence");

        let mut mismatched = result.clone();
        mismatched.evidence.request_hash = "cd".repeat(32);
        assert!(mismatched.validate_for(&request).is_err());
    }

    #[test]
    fn legacy_s3_worker_requests_normalize_to_object_store_evidence() {
        let mut legacy = request();
        legacy.mounts.push(OciWorkerMount {
            handle: "archive".to_string(),
            kind: "s3".to_string(),
            read_only: true,
        });
        legacy.granted_capabilities = vec!["mount.s3.read.archive".to_string()];

        let normalized = legacy.clone().normalized().expect("legacy request");
        assert_eq!(normalized.mounts[0].kind, OBJECT_STORE_MOUNT_KIND);
        assert_eq!(
            normalized.granted_capabilities,
            ["mount.object_store.read.archive"]
        );

        let mut canonical = legacy.clone();
        canonical.mounts[0].kind = OBJECT_STORE_MOUNT_KIND.to_string();
        canonical.granted_capabilities = vec!["mount.object_store.read.archive".to_string()];
        assert_eq!(
            legacy.request_hash().expect("legacy hash"),
            canonical.request_hash().expect("canonical hash")
        );
        assert_eq!(
            legacy
                .granted_capabilities_hash()
                .expect("legacy capability hash"),
            canonical
                .granted_capabilities_hash()
                .expect("canonical capability hash")
        );
    }
}
