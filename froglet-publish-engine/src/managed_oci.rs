//! Deterministic OCI image planning for a Managed Publication package.
//!
//! Planning reads a platform-specific immutable runner manifest and config,
//! appends one canonical package layer, and computes the exact resulting OCI
//! manifest digest. It does not contact or mutate an output registry. Upload
//! and cross-repository blob mounting are separate activation operations.

use crate::error::PublishError;
use froglet_protocol::{
    canonical_json, crypto,
    managed_deployment::OciImageV1,
    managed_publication::{
        MANAGED_PUBLICATION_RUNNER_CONTRACT_V1, ManagedPublicationPackageRequestV1,
        ManagedPublicationPackageV1,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Cursor};

pub const OCI_IMAGE_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
pub const OCI_IMAGE_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";
pub const OCI_LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";
pub const MANAGED_BUNDLE_IMAGE_PATH: &str = "opt/froglet/managed/bundle.json";
const MANAGED_OCI_BUILDER_ID: &str = "froglet.managed-oci-layout.v1";
const MANAGED_OCI_BUILDER_FINGERPRINT_DOMAIN: &str =
    "froglet.managed-oci-layout.builder-fingerprint.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OciDescriptorV1 {
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub digest: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct OciImageManifestV1 {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "mediaType", default, skip_serializing_if = "Option::is_none")]
    media_type: Option<String>,
    config: OciDescriptorV1,
    layers: Vec<OciDescriptorV1>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    annotations: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlannedManagedOciLayoutV1 {
    pub package: ManagedPublicationPackageV1,
    pub manifest_bytes: Vec<u8>,
    pub manifest_descriptor: OciDescriptorV1,
    pub config_bytes: Vec<u8>,
    pub config_descriptor: OciDescriptorV1,
    pub package_layer_bytes: Vec<u8>,
    pub package_layer_descriptor: OciDescriptorV1,
    /// Immutable base layers that activation must prove available in the
    /// output repository before uploading the new manifest.
    pub base_layers: Vec<OciDescriptorV1>,
}

#[derive(Serialize)]
struct BuilderFingerprintMaterialV1<'a> {
    schema_version: &'static str,
    builder_id: &'static str,
    crate_version: &'static str,
    runner_contract: &'static str,
    base_runner_image: &'a OciImageV1,
    base_config_digest: &'a str,
    package_layer_media_type: &'static str,
    package_path: &'static str,
}

pub fn plan_managed_oci_layout(
    request: &ManagedPublicationPackageRequestV1,
    base_manifest_bytes: &[u8],
    base_config_bytes: &[u8],
) -> Result<PlannedManagedOciLayoutV1, PublishError> {
    request
        .validate()
        .map_err(|error| PublishError::Build(error.to_string()))?;
    verify_digest(
        "base runner manifest",
        &request.base_runner_image.digest,
        base_manifest_bytes,
    )?;
    let base_manifest: OciImageManifestV1 = serde_json::from_slice(base_manifest_bytes)
        .map_err(|error| PublishError::Build(format!("base OCI manifest is invalid: {error}")))?;
    if base_manifest.schema_version != 2
        || base_manifest
            .media_type
            .as_deref()
            .is_some_and(|media_type| {
                media_type != OCI_IMAGE_MANIFEST_MEDIA_TYPE
                    && media_type != "application/vnd.docker.distribution.manifest.v2+json"
            })
    {
        return Err(PublishError::Build(
            "base runner must be a platform-specific OCI or Docker v2 image manifest".to_string(),
        ));
    }
    verify_descriptor("base OCI config", &base_manifest.config, base_config_bytes)?;
    if base_manifest.layers.is_empty() {
        return Err(PublishError::Build(
            "base runner manifest must contain at least one immutable layer".to_string(),
        ));
    }
    for descriptor in &base_manifest.layers {
        validate_descriptor("base OCI layer", descriptor)?;
    }

    let bundle_bytes = request
        .bundle
        .canonical_bytes()
        .map_err(|error| PublishError::Build(error.to_string()))?;
    let package_layer_bytes = deterministic_package_layer(&bundle_bytes)?;
    let package_layer_descriptor = descriptor(OCI_LAYER_MEDIA_TYPE, &package_layer_bytes);

    let config_bytes = append_layer_to_config(base_config_bytes, &package_layer_descriptor.digest)?;
    let config_descriptor = descriptor(OCI_IMAGE_CONFIG_MEDIA_TYPE, &config_bytes);
    let mut layers = base_manifest.layers.clone();
    layers.push(package_layer_descriptor.clone());
    let bundle_manifest_digest = request
        .bundle
        .bundle_manifest_digest()
        .map_err(|error| PublishError::Build(error.to_string()))?;
    let manifest = OciImageManifestV1 {
        schema_version: 2,
        media_type: Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
        config: config_descriptor.clone(),
        layers,
        annotations: BTreeMap::from([
            (
                "dev.froglet.managed.operation-id".to_string(),
                request.operation_id.clone(),
            ),
            (
                "dev.froglet.managed.bundle-digest".to_string(),
                bundle_manifest_digest.clone(),
            ),
            (
                "org.opencontainers.image.base.digest".to_string(),
                request.base_runner_image.digest.clone(),
            ),
        ]),
    };
    let manifest_bytes = canonical_json::to_vec(&manifest)
        .map_err(|error| PublishError::Build(format!("OCI manifest encoding failed: {error}")))?;
    let manifest_descriptor = descriptor(OCI_IMAGE_MANIFEST_MEDIA_TYPE, &manifest_bytes);
    let build_evidence = request
        .bundle
        .intent
        .build_evidence
        .as_ref()
        .ok_or_else(|| PublishError::Build("managed bundle omitted build evidence".to_string()))?;
    let build_evidence_digest = canonical_json::to_vec(build_evidence)
        .map(crypto::sha256_hex)
        .map_err(|error| PublishError::Build(format!("build evidence encoding failed: {error}")))?;
    let builder_fingerprint =
        builder_fingerprint(&request.base_runner_image, &base_manifest.config.digest)?;
    let package = ManagedPublicationPackageV1 {
        schema_version:
            froglet_protocol::managed_publication::MANAGED_PUBLICATION_PACKAGE_SCHEMA_V1.to_string(),
        source_package_digest: request.bundle.source_package_digest.clone(),
        build_evidence_digest,
        bundle_manifest_digest,
        release_bundle_digest: request.release_bundle_digest.clone(),
        base_runner_image: request.base_runner_image.clone(),
        image: OciImageV1 {
            repository: request.output_repository.clone(),
            digest: manifest_descriptor.digest.clone(),
        },
        content_visibility: request.bundle.content_visibility,
        runtime: request.bundle.intent.runtime.clone().unwrap_or_default(),
        package_kind: request
            .bundle
            .intent
            .package_kind
            .clone()
            .unwrap_or_default(),
        builder_id: MANAGED_OCI_BUILDER_ID.to_string(),
        builder_fingerprint,
        runner_contract: MANAGED_PUBLICATION_RUNNER_CONTRACT_V1.to_string(),
    };
    request
        .validate_output(&package)
        .map_err(|error| PublishError::Build(error.to_string()))?;

    Ok(PlannedManagedOciLayoutV1 {
        package,
        manifest_bytes,
        manifest_descriptor,
        config_bytes,
        config_descriptor,
        package_layer_bytes,
        package_layer_descriptor,
        base_layers: base_manifest.layers,
    })
}

fn deterministic_package_layer(bundle_bytes: &[u8]) -> Result<Vec<u8>, PublishError> {
    let mut output = Vec::new();
    {
        let mut archive = tar::Builder::new(&mut output);
        archive.mode(tar::HeaderMode::Deterministic);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(
            u64::try_from(bundle_bytes.len())
                .map_err(|_| PublishError::Build("managed bundle is too large".to_string()))?,
        );
        header.set_mode(0o444);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                MANAGED_BUNDLE_IMAGE_PATH,
                Cursor::new(bundle_bytes),
            )
            .map_err(|error| PublishError::Build(format!("OCI layer creation failed: {error}")))?;
        archive.finish().map_err(|error| {
            PublishError::Build(format!("OCI layer finalization failed: {error}"))
        })?;
    }
    Ok(output)
}

fn append_layer_to_config(
    base_config_bytes: &[u8],
    package_diff_id: &str,
) -> Result<Vec<u8>, PublishError> {
    let mut config: Value = serde_json::from_slice(base_config_bytes)
        .map_err(|error| PublishError::Build(format!("base OCI config is invalid: {error}")))?;
    let object = config
        .as_object_mut()
        .ok_or_else(|| PublishError::Build("base OCI config must be an object".to_string()))?;
    for field in ["architecture", "os"] {
        if object
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(PublishError::Build(format!(
                "base OCI config omitted {field}"
            )));
        }
    }
    let rootfs = object
        .entry("rootfs")
        .or_insert_with(|| json!({"type": "layers", "diff_ids": []}));
    let rootfs = rootfs
        .as_object_mut()
        .ok_or_else(|| PublishError::Build("base OCI rootfs must be an object".to_string()))?;
    if rootfs.get("type").and_then(Value::as_str) != Some("layers") {
        return Err(PublishError::Build(
            "base OCI rootfs type must be layers".to_string(),
        ));
    }
    rootfs
        .entry("diff_ids")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| PublishError::Build("base OCI diff_ids must be an array".to_string()))?
        .push(Value::String(package_diff_id.to_string()));
    object
        .entry("history")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| PublishError::Build("base OCI history must be an array".to_string()))?
        .push(json!({
            "created_by": MANAGED_OCI_BUILDER_ID,
            "comment": "immutable Froglet Managed Publication bundle",
            "empty_layer": false
        }));
    canonical_json::to_vec(&config)
        .map_err(|error| PublishError::Build(format!("OCI config encoding failed: {error}")))
}

fn builder_fingerprint(
    base_runner_image: &OciImageV1,
    base_config_digest: &str,
) -> Result<String, PublishError> {
    let material = BuilderFingerprintMaterialV1 {
        schema_version: MANAGED_OCI_BUILDER_FINGERPRINT_DOMAIN,
        builder_id: MANAGED_OCI_BUILDER_ID,
        crate_version: env!("CARGO_PKG_VERSION"),
        runner_contract: MANAGED_PUBLICATION_RUNNER_CONTRACT_V1,
        base_runner_image,
        base_config_digest,
        package_layer_media_type: OCI_LAYER_MEDIA_TYPE,
        package_path: MANAGED_BUNDLE_IMAGE_PATH,
    };
    let canonical = canonical_json::to_vec(&material)
        .map_err(|error| PublishError::Build(format!("builder fingerprint failed: {error}")))?;
    let mut bytes =
        Vec::with_capacity(MANAGED_OCI_BUILDER_FINGERPRINT_DOMAIN.len() + 1 + canonical.len());
    bytes.extend_from_slice(MANAGED_OCI_BUILDER_FINGERPRINT_DOMAIN.as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(&canonical);
    Ok(crypto::sha256_hex(bytes))
}

fn descriptor(media_type: &str, bytes: &[u8]) -> OciDescriptorV1 {
    OciDescriptorV1 {
        media_type: media_type.to_string(),
        digest: format!("sha256:{}", crypto::sha256_hex(bytes)),
        size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
    }
}

fn verify_descriptor(
    label: &str,
    descriptor: &OciDescriptorV1,
    bytes: &[u8],
) -> Result<(), PublishError> {
    validate_descriptor(label, descriptor)?;
    if descriptor.size
        != u64::try_from(bytes.len())
            .map_err(|_| PublishError::Build(format!("{label} is too large")))?
    {
        return Err(PublishError::Build(format!("{label} size mismatch")));
    }
    verify_digest(label, &descriptor.digest, bytes)
}

fn validate_descriptor(label: &str, descriptor: &OciDescriptorV1) -> Result<(), PublishError> {
    let digest = descriptor
        .digest
        .strip_prefix("sha256:")
        .ok_or_else(|| PublishError::Build(format!("{label} digest must use sha256")))?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || descriptor.size == 0
        || descriptor.media_type.trim().is_empty()
    {
        return Err(PublishError::Build(format!(
            "{label} descriptor is invalid"
        )));
    }
    Ok(())
}

fn verify_digest(label: &str, expected: &str, bytes: &[u8]) -> Result<(), PublishError> {
    let actual = format!("sha256:{}", crypto::sha256_hex(bytes));
    if actual != expected {
        return Err(PublishError::Build(format!("{label} digest mismatch")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use froglet_protocol::managed_publication::{
        MANAGED_PUBLICATION_PACKAGE_REQUEST_SCHEMA_V1, ManagedPublicationBundleManifestV1,
        ManagedPublicationOperationIdentityV1, ManagedPublicationPackageRequestV1,
        managed_publication_operation_id,
    };
    use froglet_protocol::publication::{
        PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1, PUBLICATION_INTENT_SCHEMA_V1,
        PublicationBuildEvidence, PublicationIntent, VerificationFixture,
    };
    use std::io::Read;

    fn hex(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn digest(bytes: &[u8]) -> String {
        format!("sha256:{}", crypto::sha256_hex(bytes))
    }

    fn package_request(base_manifest_digest: String) -> ManagedPublicationPackageRequestV1 {
        let intent = PublicationIntent {
            schema_version: Some(PUBLICATION_INTENT_SCHEMA_V1.to_string()),
            service_id: "analytics".to_string(),
            wasm_module_hex: Some("0061736d01000000".to_string()),
            runtime: Some("wasm".to_string()),
            package_kind: Some("inline_module".to_string()),
            build_evidence: Some(PublicationBuildEvidence {
                schema_version: PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1.to_string(),
                builder: "wat".to_string(),
                builder_version: "1.245.1".to_string(),
                source_digest: hex('1'),
                artifact_digest: hex('2'),
                dependency_mode: "none".to_string(),
                components: Vec::new(),
                hermetic: true,
            }),
            verification: Some(VerificationFixture {
                input: json!({"private": true}),
                expected_output: Some(json!({"ok": true})),
            }),
            ..PublicationIntent::default()
        };
        let bundle = ManagedPublicationBundleManifestV1::from_intent(&intent).unwrap();
        let base_runner_image = OciImageV1 {
            repository: "ghcr.io/example/froglet-runner".to_string(),
            digest: base_manifest_digest,
        };
        let release_bundle_digest = format!("sha256:{}", hex('4'));
        let output_repository = "registry.example.test/private/analytics";
        ManagedPublicationPackageRequestV1 {
            schema_version: MANAGED_PUBLICATION_PACKAGE_REQUEST_SCHEMA_V1.to_string(),
            operation_id: managed_publication_operation_id(
                &ManagedPublicationOperationIdentityV1 {
                    service_id: "analytics",
                    provider_id: &hex('3'),
                    publish_request_digest: &bundle.publish_request_digest,
                    source_package_digest: &hex('2'),
                    base_runner_image: &base_runner_image,
                    release_bundle_digest: &release_bundle_digest,
                    output_repository,
                    target: "regional-container",
                    profile: "small-public",
                },
            )
            .unwrap(),
            bundle,
            output_repository: output_repository.to_string(),
            base_runner_image,
            release_bundle_digest,
        }
    }

    fn base_image() -> (Vec<u8>, Vec<u8>) {
        let config = canonical_json::to_vec(&json!({
            "architecture": "amd64",
            "os": "linux",
            "config": {"Entrypoint": ["/usr/local/bin/docker-entrypoint.sh"]},
            "rootfs": {"type": "layers", "diff_ids": [format!("sha256:{}", hex('5'))]},
            "history": []
        }))
        .unwrap();
        let manifest = OciImageManifestV1 {
            schema_version: 2,
            media_type: Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
            config: descriptor(OCI_IMAGE_CONFIG_MEDIA_TYPE, &config),
            layers: vec![OciDescriptorV1 {
                media_type: OCI_LAYER_MEDIA_TYPE.to_string(),
                digest: format!("sha256:{}", hex('5')),
                size: 123,
            }],
            annotations: BTreeMap::new(),
        };
        (canonical_json::to_vec(&manifest).unwrap(), config)
    }

    #[test]
    fn layout_is_byte_deterministic_and_contains_only_the_runtime_bundle() {
        let (base_manifest, base_config) = base_image();
        let request = package_request(digest(&base_manifest));
        let first = plan_managed_oci_layout(&request, &base_manifest, &base_config).unwrap();
        let second = plan_managed_oci_layout(&request, &base_manifest, &base_config).unwrap();

        assert_eq!(first.manifest_bytes, second.manifest_bytes);
        assert_eq!(first.config_bytes, second.config_bytes);
        assert_eq!(first.package_layer_bytes, second.package_layer_bytes);
        assert_eq!(first.package, second.package);
        assert_eq!(first.package.image.digest, first.manifest_descriptor.digest);
        request.validate_output(&first.package).unwrap();

        let mut archive = tar::Archive::new(Cursor::new(&first.package_layer_bytes));
        let mut entries = archive.entries().unwrap();
        let mut entry = entries.next().unwrap().unwrap();
        assert_eq!(
            entry.path().unwrap().as_ref(),
            std::path::Path::new(MANAGED_BUNDLE_IMAGE_PATH)
        );
        let mut bundled = Vec::new();
        entry.read_to_end(&mut bundled).unwrap();
        assert_eq!(bundled, request.bundle.canonical_bytes().unwrap());
        assert!(entries.next().is_none());
        let text = String::from_utf8(bundled).unwrap();
        assert!(!text.contains("\"private\":"));
        assert!(!text.contains("fixture"));
    }

    #[test]
    fn base_manifest_or_config_drift_fails_before_producing_an_output_digest() {
        let (base_manifest, base_config) = base_image();
        let request = package_request(digest(&base_manifest));

        let mut changed_manifest = base_manifest.clone();
        changed_manifest.push(b' ');
        assert!(plan_managed_oci_layout(&request, &changed_manifest, &base_config).is_err());

        let mut changed_config = base_config;
        changed_config.push(b' ');
        assert!(plan_managed_oci_layout(&request, &base_manifest, &changed_config).is_err());
    }
}
