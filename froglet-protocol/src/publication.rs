//! Authoring and provider-control publication types.
//!
//! This module is deliberately outside the signed Froglet Kernel protocol. It
//! is the lossless boundary between manifests, agent integrations, the publish
//! engine, and a provider daemon. Only the daemon converts a validated
//! [`PublicationIntent`] into existing signed descriptor and offer payloads.

use crate::manifest::ServiceManifest;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

pub const PUBLICATION_INTENT_SCHEMA_V1: &str = "froglet.publication-intent.v1";
pub const PUBLICATION_REVISION_SCHEMA_V1: &str = "froglet.publication-revision.v1";
pub const PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1: &str = "froglet.publication-build-evidence.v1";
pub const PUBLICATION_CANARY_REQUEST_SCHEMA_V1: &str = "froglet.publication-canary-request.v1";
pub const PUBLICATION_CANARY_RESULT_SCHEMA_V1: &str = "froglet.publication-canary-result.v1";
pub const PYTHON_LOCK_SCHEMA_V1: &str = "froglet.python-lock.v1";
pub const PYTHON_BUNDLE_SCHEMA_V1: &str = "froglet.python-bundle.v1";
const PUBLICATION_REVISION_SIGNATURE_DOMAIN: &[u8] = b"froglet-publication-revision-v1\n";
const PUBLICATION_CANARY_SIGNATURE_DOMAIN: &[u8] = b"froglet-publication-canary-result-v1\n";
const MAX_REVISION_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_REVISION_TEXT_BYTES: usize = 16 * 1024;
const MAX_REVISION_MOUNTS: usize = 64;
const MAX_REVISION_CAPABILITIES: usize = 128;

/// Provider-neutral kind emitted by current publication authoring paths.
pub const OBJECT_STORE_MOUNT_KIND: &str = "object_store";
/// Historical object-store spelling accepted when reading old inputs and
/// already-signed publication revisions.
pub const LEGACY_S3_MOUNT_KIND: &str = "s3";

/// Returns the canonical mount kind for supported authoring/runtime kinds.
///
/// This helper deliberately does not trim or case-fold. Stored signed
/// documents retain exact byte semantics, while current authoring adapters
/// normalize the one supported compatibility alias before signing.
pub fn canonical_mount_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "postgres" => Some("postgres"),
        "sqlite" => Some("sqlite"),
        OBJECT_STORE_MOUNT_KIND | LEGACY_S3_MOUNT_KIND => Some(OBJECT_STORE_MOUNT_KIND),
        "redis" => Some("redis"),
        _ => None,
    }
}

/// Canonicalizes only the historical object-store capability namespace.
/// Other capability strings remain byte-for-byte unchanged after the normal
/// authoring trim/lowercase pass.
pub fn canonical_mount_capability(capability: &str) -> String {
    capability.strip_prefix("mount.s3.").map_or_else(
        || capability.to_string(),
        |suffix| format!("mount.{OBJECT_STORE_MOUNT_KIND}.{suffix}"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicationSettlement {
    None,
    Lightning,
    Stripe,
}

impl FromStr for PublicationSettlement {
    type Err = PublicationIntentError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "lightning" => Ok(Self::Lightning),
            "stripe" => Ok(Self::Stripe),
            other => Err(PublicationIntentError::InvalidSettlement(other.to_string())),
        }
    }
}

impl fmt::Display for PublicationSettlement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::None => "none",
            Self::Lightning => "lightning",
            Self::Stripe => "stripe",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicationCurrency {
    Sat,
    Usd,
}

/// Format of an immutable read-only data snapshot carried from an authoring
/// adapter to the local Froglet daemon. The snapshot is stored outside the
/// signed Kernel; its SHA-256 binding is recorded by the higher-layer
/// Publication Revision and the existing offer execution profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicationDataFormat {
    Json,
    Csv,
    Sqlite,
}

impl fmt::Display for PublicationDataFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Sqlite => "sqlite",
        })
    }
}

/// Scalar conversion applied while a CSV snapshot is imported into Froglet's
/// private, content-addressed SQLite query cache. CSV has no intrinsic type
/// system, so publication requires this schema rather than guessing types
/// from a sample of rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicationCsvColumnType {
    String,
    Integer,
    Number,
    Boolean,
}

impl fmt::Display for PublicationCsvColumnType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationCsvColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub column_type: PublicationCsvColumnType,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub indexed: bool,
}

/// Explicit schema for a single-collection CSV snapshot.
///
/// The first CSV record must match `columns` exactly and in order. Empty cells
/// become JSON `null` only when the corresponding column is nullable. Equality
/// filters over a CSV snapshot must include at least one indexed column, which
/// prevents an apparently small request from silently scanning a large file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationCsvSchema {
    pub collection: String,
    pub columns: Vec<PublicationCsvColumn>,
}

impl PublicationCsvSchema {
    pub fn validate(&self) -> Result<(), String> {
        validate_data_name("CSV collection", &self.collection)?;
        if self.columns.is_empty() || self.columns.len() > 256 {
            return Err("CSV schema must declare between 1 and 256 columns".to_string());
        }
        let mut names = HashSet::with_capacity(self.columns.len());
        let mut indexed = 0usize;
        for column in &self.columns {
            validate_data_name("CSV column", &column.name)?;
            if !names.insert(column.name.as_str()) {
                return Err(format!("duplicate CSV column {:?}", column.name));
            }
            indexed += usize::from(column.indexed);
        }
        if indexed == 0 {
            return Err("CSV schema must declare at least one indexed column".to_string());
        }
        if indexed > 16 {
            return Err("CSV schema cannot declare more than 16 indexed columns".to_string());
        }
        Ok(())
    }
}

fn validate_data_name(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 128 {
        return Err(format!("{label} name must be 1-128 bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{label} name must not contain control characters"));
    }
    Ok(())
}

/// Inline transport representation of a provider-owned data file.
///
/// The authoring adapter reads the file and emits canonical base64 so the
/// daemon never follows a user-controlled host path. The daemon decodes,
/// validates, and stages the bytes under a content-addressed private path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationDataSource {
    pub format: PublicationDataFormat,
    pub content_base64: String,
    /// Required for CSV and rejected for formats that carry their own schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csv_schema: Option<PublicationCsvSchema>,
}

impl PublicationDataSource {
    /// Stable package binding for CSV, whose execution semantics depend on
    /// both raw bytes and the explicit schema. JSON/SQLite retain their
    /// historical raw-byte digest and therefore do not use this helper.
    pub fn csv_package_digest(&self) -> Result<String, String> {
        if self.format != PublicationDataFormat::Csv {
            return Err("csv_package_digest requires format=csv".to_string());
        }
        let canonical = crate::canonical_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(crate::crypto::sha256_hex(canonical))
    }
}

/// Portable Python runtime compatibility declared by a lock manifest.
///
/// This deliberately does not identify a host executable. A build host path
/// or binary hash is not a portable runtime identity. Providers attest their
/// local runtime separately and must match these declared compatibility
/// fields before executing the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonRuntimeLock {
    pub implementation: String,
    pub version: String,
    pub abi: String,
}

/// One locally supplied, hash-pinned pure-Python wheel in a Python lock.
/// Native-extension wheels are excluded from this initial adapter so a bundle
/// can execute without a package resolver, network access, or host cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonLockedArtifact {
    pub name: String,
    pub version: String,
    pub filename: String,
    pub sha256: String,
}

/// Canonical dependency lock for a Python publication.
///
/// Artifact files live next to the lock under `artifacts/<filename>`. The
/// authoring adapter reads only those exact files and verifies every digest;
/// it never invokes pip or a network resolver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonLockManifest {
    pub schema_version: String,
    pub source_sha256: String,
    pub runtime: PythonRuntimeLock,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<PythonLockedArtifact>,
}

impl PythonLockManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != PYTHON_LOCK_SCHEMA_V1 {
            return Err("unsupported Python lock schema_version".to_string());
        }
        if !valid_hash(&self.source_sha256) {
            return Err(
                "Python lock source_sha256 must be 64 lowercase hex characters".to_string(),
            );
        }
        if self.runtime.implementation != "cpython" {
            return Err("Python lock runtime implementation must be cpython".to_string());
        }
        if !valid_python_version(&self.runtime.version) {
            return Err(
                "Python lock runtime version must be an exact major.minor.patch version"
                    .to_string(),
            );
        }
        if self.runtime.abi.is_empty()
            || self.runtime.abi.len() > 32
            || !self.runtime.abi.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | '_')
            })
        {
            return Err("Python lock runtime abi is invalid".to_string());
        }
        if self.artifacts.len() > 256 {
            return Err("Python lock cannot contain more than 256 artifacts".to_string());
        }
        let mut prior: Option<String> = None;
        for artifact in &self.artifacts {
            if artifact.name.is_empty()
                || artifact.name.len() > 128
                || !artifact.name.chars().all(|character| {
                    character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || matches!(character, '-' | '_' | '.')
                })
            {
                return Err("Python lock artifact name is invalid".to_string());
            }
            if artifact.version.is_empty()
                || artifact.version.len() > 128
                || artifact.version.chars().any(char::is_control)
            {
                return Err("Python lock artifact version is invalid".to_string());
            }
            if !valid_python_wheel_filename(&artifact.filename) {
                return Err(format!(
                    "Python lock artifact {:?} must be a basename ending in -none-any.whl",
                    artifact.filename
                ));
            }
            if !valid_hash(&artifact.sha256) {
                return Err(
                    "Python lock artifact sha256 must be 64 lowercase hex characters".to_string(),
                );
            }
            let key = normalize_python_distribution_name(&artifact.name);
            if prior.as_ref().is_some_and(|previous| previous >= &key) {
                return Err("Python lock artifacts must be sorted and unique".to_string());
            }
            prior = Some(key);
        }
        Ok(())
    }

    pub fn lock_sha256(&self) -> Result<String, String> {
        self.validate()?;
        let canonical = crate::canonical_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(crate::crypto::sha256_hex(canonical))
    }
}

/// Canonical comparison form from the Python packaging name-normalization
/// rule: runs of `-`, `_`, and `.` compare as one `-`, case-insensitively.
pub fn normalize_python_distribution_name(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if matches!(character, '-' | '_' | '.') {
            separator = true;
        } else {
            if separator && !output.is_empty() {
                output.push('-');
            }
            separator = false;
            output.push(character);
        }
    }
    output
}

/// Hash-verified wheel bytes carried inside a locked Python bundle. The wheel
/// metadata is repeated from the lock so runtime verification cannot confuse
/// an artifact's bytes with another lock entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPythonBundleArtifact {
    pub locked: PythonLockedArtifact,
    pub content_base64: String,
}

/// Canonical payload embedded in generated Python bundle source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPythonBundleEnvelope {
    pub schema_version: String,
    pub lock: PythonLockManifest,
    pub lock_sha256: String,
    pub source_base64: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<LockedPythonBundleArtifact>,
}

impl LockedPythonBundleEnvelope {
    pub fn validate_metadata(&self) -> Result<(), String> {
        if self.schema_version != PYTHON_BUNDLE_SCHEMA_V1 {
            return Err("unsupported Python bundle schema_version".to_string());
        }
        self.lock.validate()?;
        if self.lock.lock_sha256()? != self.lock_sha256 {
            return Err("Python bundle lock_sha256 does not match canonical lock".to_string());
        }
        if self.artifacts.len() != self.lock.artifacts.len() {
            return Err("Python bundle artifacts do not match lock entries".to_string());
        }
        for (bundled, locked) in self.artifacts.iter().zip(&self.lock.artifacts) {
            if &bundled.locked != locked {
                return Err("Python bundle artifact metadata does not match lock".to_string());
            }
            if bundled.content_base64.is_empty() {
                return Err("Python bundle artifact content must not be empty".to_string());
            }
        }
        if self.source_base64.is_empty() {
            return Err("Python bundle source must not be empty".to_string());
        }
        Ok(())
    }
}

fn valid_python_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}

fn valid_python_wheel_filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains("..")
        && value.ends_with("-none-any.whl")
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// One immutable component used to build or execute a publication package.
/// `digest` is a SHA-256 of the locked component bytes (for example the
/// crates.io checksum), not a mutable URL or package tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationDependencyComponent {
    pub role: String,
    pub name: String,
    pub version: String,
    pub digest: String,
}

/// Provider-signed, non-Kernel evidence describing how immutable package
/// bytes were produced and what dependency information was available.
///
/// `dependency_mode` is deliberately explicit: `none` means the workload has
/// no package dependencies, `locked` requires digest-pinned components, and
/// `unresolved` records a truthful evidence gap instead of implying a lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationBuildEvidence {
    pub schema_version: String,
    pub builder: String,
    pub builder_version: String,
    pub source_digest: String,
    pub artifact_digest: String,
    pub dependency_mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<PublicationDependencyComponent>,
    pub hermetic: bool,
}

impl PublicationBuildEvidence {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1 {
            return Err("unsupported publication build evidence schema_version".to_string());
        }
        for (field, value) in [
            ("builder", self.builder.as_str()),
            ("builder_version", self.builder_version.as_str()),
        ] {
            if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
                return Err(format!(
                    "build evidence {field} must be 1-128 printable bytes"
                ));
            }
        }
        for (field, digest) in [
            ("source_digest", self.source_digest.as_str()),
            ("artifact_digest", self.artifact_digest.as_str()),
        ] {
            if !valid_hash(digest) {
                return Err(format!(
                    "build evidence {field} must be 64 lowercase hex characters"
                ));
            }
        }
        match self.dependency_mode.as_str() {
            "none" if !self.components.is_empty() => {
                return Err("dependency_mode=none cannot include components".to_string());
            }
            "locked" if self.components.is_empty() => {
                return Err("dependency_mode=locked requires at least one component".to_string());
            }
            "unresolved" if !self.components.is_empty() => {
                return Err("dependency_mode=unresolved cannot claim locked components".to_string());
            }
            "none" | "locked" | "unresolved" => {}
            _ => {
                return Err("dependency_mode must be one of: none, locked, unresolved".to_string());
            }
        }
        let mut prior: Option<(&str, &str)> = None;
        for component in &self.components {
            for (field, value) in [
                ("role", component.role.as_str()),
                ("name", component.name.as_str()),
                ("version", component.version.as_str()),
            ] {
                if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
                    return Err(format!(
                        "build dependency {field} must be 1-128 printable bytes"
                    ));
                }
            }
            if !valid_hash(&component.digest) {
                return Err(
                    "build dependency digest must be 64 lowercase hex characters".to_string(),
                );
            }
            let key = (component.role.as_str(), component.name.as_str());
            if prior.is_some_and(|previous| previous >= key) {
                return Err("build dependency components must be sorted and unique".to_string());
            }
            prior = Some(key);
        }
        if self.hermetic && self.dependency_mode == "unresolved" {
            return Err("a hermetic build cannot have unresolved dependencies".to_string());
        }
        Ok(())
    }
}

impl FromStr for PublicationCurrency {
    type Err = PublicationIntentError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "sat" => Ok(Self::Sat),
            "usd" => Ok(Self::Usd),
            other => Err(PublicationIntentError::InvalidCurrency(other.to_string())),
        }
    }
}

impl fmt::Display for PublicationCurrency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Sat => "sat",
            Self::Usd => "usd",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationMount {
    pub handle: String,
    pub kind: String,
    #[serde(default = "default_read_only")]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
}

fn default_read_only() -> bool {
    true
}

impl PublicationMount {
    pub fn validate_authoring(&self) -> Result<(), PublicationIntentError> {
        if self.binding.is_some() {
            return Err(PublicationIntentError::InvalidMount(
                "publication mounts must not include provider-owned binding material".to_string(),
            ));
        }
        if canonical_mount_kind(&self.kind).is_none() {
            return Err(PublicationIntentError::InvalidMount(format!(
                "unsupported mount kind {:?}; allowed: postgres, sqlite, object_store, redis (legacy alias: s3)",
                self.kind
            )));
        }
        if self.handle.is_empty()
            || self.handle.len() > 64
            || !self.handle.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
            })
        {
            return Err(PublicationIntentError::InvalidMount(
                "mount handle must contain 1-64 lowercase ASCII letters, digits, or underscores"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub fn access_capability(&self) -> String {
        format!(
            "mount.{}.{}.{}",
            self.kind,
            if self.read_only { "read" } else { "write" },
            self.handle
        )
    }
}

fn valid_capability(capability: &str) -> bool {
    !capability.is_empty()
        && capability.len() <= 128
        && capability.split('.').all(|segment| {
            !segment.is_empty()
                && segment.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                })
        })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuel_limit: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedPublicationLimits {
    pub max_input_bytes: usize,
    pub max_runtime_ms: u64,
    pub max_memory_bytes: usize,
    pub max_output_bytes: usize,
    pub fuel_limit: u64,
}

/// Provider-control header carrying the exact non-mutating publication
/// precondition approved by the authoring client.
pub const PUBLICATION_PRECONDITION_HEADER: &str = "x-froglet-publication-precondition";
pub const PUBLICATION_PRECONDITION_SCHEMA_V1: &str = "froglet.publication-precondition.v1";
pub const PUBLICATION_IDENTITY_BACKUP_SCHEMA_V1: &str = "froglet.publication-identity-backup.v1";

/// Provider-local identity-backup state disclosed and approval-bound before
/// public publication. Paths and custody key identifiers remain private; the
/// content digest is sufficient to detect backup replacement between plan and
/// mutation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublicationIdentityBackupState {
    Current,
    Missing,
    StaleIdentity,
    BackupMissing,
    BackupDigestMismatch,
    InvalidRecord,
    NotRequiredForPrivateLocalProof,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationIdentityBackup {
    pub schema_version: String,
    pub state: PublicationIdentityBackupState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_sha256: Option<String>,
}

impl PublicationIdentityBackup {
    pub fn new(
        state: PublicationIdentityBackupState,
        backup_sha256: Option<String>,
    ) -> Result<Self, String> {
        let value = Self {
            schema_version: PUBLICATION_IDENTITY_BACKUP_SCHEMA_V1.to_string(),
            state,
            backup_sha256,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn not_required_for_private_local_proof() -> Self {
        Self {
            schema_version: PUBLICATION_IDENTITY_BACKUP_SCHEMA_V1.to_string(),
            state: PublicationIdentityBackupState::NotRequiredForPrivateLocalProof,
            backup_sha256: None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != PUBLICATION_IDENTITY_BACKUP_SCHEMA_V1 {
            return Err(format!(
                "unsupported publication identity-backup schema {:?}",
                self.schema_version
            ));
        }
        if let Some(digest) = self.backup_sha256.as_deref()
            && (digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        {
            return Err(
                "identity backup digest must be 64 lowercase hexadecimal characters".to_string(),
            );
        }
        match self.state {
            PublicationIdentityBackupState::Current if self.backup_sha256.is_none() => {
                Err("current identity backup state requires its exact digest".to_string())
            }
            PublicationIdentityBackupState::Missing
            | PublicationIdentityBackupState::NotRequiredForPrivateLocalProof
                if self.backup_sha256.is_some() =>
            {
                Err("identity backup state must not carry a digest".to_string())
            }
            PublicationIdentityBackupState::Current
            | PublicationIdentityBackupState::Missing
            | PublicationIdentityBackupState::StaleIdentity
            | PublicationIdentityBackupState::BackupMissing
            | PublicationIdentityBackupState::BackupDigestMismatch
            | PublicationIdentityBackupState::InvalidRecord
            | PublicationIdentityBackupState::NotRequiredForPrivateLocalProof => Ok(()),
        }
    }
}

/// Read-only provider resolution for one exact canonical publication intent.
///
/// This is deliberately a higher-layer authoring contract, not a Kernel
/// artifact. The daemon recomputes `precondition_token` immediately before a
/// provider-control publication mutates data, so provider identity or
/// publication-relevant configuration drift fails closed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationPrecondition {
    pub schema_version: String,
    pub provider_id: String,
    pub intent_digest: String,
    pub resolved_limits: ResolvedPublicationLimits,
    pub configuration_token: String,
    pub identity_backup: PublicationIdentityBackup,
    pub precondition_token: String,
}

#[derive(Serialize)]
struct PublicationPreconditionTokenPayload<'a> {
    schema_version: &'a str,
    provider_id: &'a str,
    intent_digest: &'a str,
    resolved_limits: ResolvedPublicationLimits,
    configuration_token: &'a str,
    identity_backup: &'a PublicationIdentityBackup,
}

impl PublicationPrecondition {
    pub fn new(
        provider_id: String,
        intent_digest: String,
        resolved_limits: ResolvedPublicationLimits,
        configuration_token: String,
        identity_backup: PublicationIdentityBackup,
    ) -> Result<Self, String> {
        let mut precondition = Self {
            schema_version: PUBLICATION_PRECONDITION_SCHEMA_V1.to_string(),
            provider_id,
            intent_digest,
            resolved_limits,
            configuration_token,
            identity_backup,
            precondition_token: String::new(),
        };
        precondition.precondition_token = precondition.expected_token()?;
        precondition.validate()?;
        Ok(precondition)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != PUBLICATION_PRECONDITION_SCHEMA_V1 {
            return Err(format!(
                "unsupported publication precondition schema {:?}",
                self.schema_version
            ));
        }
        for (field, value) in [
            ("provider_id", self.provider_id.as_str()),
            ("intent_digest", self.intent_digest.as_str()),
            ("configuration_token", self.configuration_token.as_str()),
            ("precondition_token", self.precondition_token.as_str()),
        ] {
            if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!("{field} must be a 64-hex string"));
            }
        }
        self.identity_backup.validate()?;
        if self.identity_backup.state
            == PublicationIdentityBackupState::NotRequiredForPrivateLocalProof
        {
            return Err(
                "public publication precondition requires observed provider backup state"
                    .to_string(),
            );
        }
        if self.precondition_token != self.expected_token()? {
            return Err("publication precondition token does not match its payload".to_string());
        }
        Ok(())
    }

    fn expected_token(&self) -> Result<String, String> {
        let payload = PublicationPreconditionTokenPayload {
            schema_version: &self.schema_version,
            provider_id: &self.provider_id,
            intent_digest: &self.intent_digest,
            resolved_limits: self.resolved_limits,
            configuration_token: &self.configuration_token,
            identity_backup: &self.identity_backup,
        };
        let canonical =
            crate::canonical_json::to_vec(&payload).map_err(|error| error.to_string())?;
        Ok(crate::crypto::sha256_hex(canonical))
    }
}

impl PublicationLimits {
    pub fn resolve_within(
        &self,
        available: ResolvedPublicationLimits,
    ) -> Result<ResolvedPublicationLimits, PublicationIntentError> {
        let resolved = ResolvedPublicationLimits {
            max_input_bytes: self.max_input_bytes.unwrap_or(available.max_input_bytes),
            max_runtime_ms: self.max_runtime_ms.unwrap_or(available.max_runtime_ms),
            max_memory_bytes: self.max_memory_bytes.unwrap_or(available.max_memory_bytes),
            max_output_bytes: self.max_output_bytes.unwrap_or(available.max_output_bytes),
            fuel_limit: self.fuel_limit.unwrap_or(available.fuel_limit),
        };
        for (field, requested, maximum) in [
            (
                "max_input_bytes",
                resolved.max_input_bytes as u128,
                available.max_input_bytes as u128,
            ),
            (
                "max_runtime_ms",
                resolved.max_runtime_ms as u128,
                available.max_runtime_ms as u128,
            ),
            (
                "max_memory_bytes",
                resolved.max_memory_bytes as u128,
                available.max_memory_bytes as u128,
            ),
            (
                "max_output_bytes",
                resolved.max_output_bytes as u128,
                available.max_output_bytes as u128,
            ),
        ] {
            let invalid = if maximum == 0 {
                requested != 0
            } else {
                requested == 0 || requested > maximum
            };
            if invalid {
                return Err(PublicationIntentError::InvalidLimit {
                    field,
                    requested,
                    maximum,
                });
            }
        }
        if resolved.fuel_limit > available.fuel_limit && available.fuel_limit != 0 {
            return Err(PublicationIntentError::InvalidLimit {
                field: "fuel_limit",
                requested: resolved.fuel_limit as u128,
                maximum: available.fuel_limit as u128,
            });
        }
        Ok(resolved)
    }
}

/// Provider-private invocation fixture. This is authoring metadata, never a
/// field in a signed OfferPayload or public provider/marketplace record.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VerificationFixture {
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_output: Option<Value>,
}

fn deserialize_present_json<'de, D>(deserializer: D) -> Result<Option<Option<Value>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Value>::deserialize(deserializer).map(Some)
}

impl<'de> Deserialize<'de> for VerificationFixture {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireFixture {
            #[serde(default, deserialize_with = "deserialize_present_json")]
            input: Option<Option<Value>>,
            #[serde(default)]
            input_json: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_json")]
            expected_output: Option<Option<Value>>,
            #[serde(default)]
            expected_output_json: Option<String>,
        }

        let wire = WireFixture::deserialize(deserializer)?;
        let input = match (wire.input, wire.input_json) {
            (Some(_), Some(_)) => {
                return Err(D::Error::custom(
                    "verification must use exactly one of input or input_json",
                ));
            }
            (Some(value), None) => value.unwrap_or(Value::Null),
            (None, Some(value)) => serde_json::from_str(&value)
                .map_err(|error| D::Error::custom(format!("invalid input_json: {error}")))?,
            (None, None) => {
                return Err(D::Error::custom(
                    "verification requires input or input_json",
                ));
            }
        };
        let expected_output = match (wire.expected_output, wire.expected_output_json) {
            (Some(_), Some(_)) => {
                return Err(D::Error::custom(
                    "verification must use at most one of expected_output or expected_output_json",
                ));
            }
            (Some(value), None) => Some(value.unwrap_or(Value::Null)),
            (None, Some(value)) => Some(serde_json::from_str(&value).map_err(|error| {
                D::Error::custom(format!("invalid expected_output_json: {error}"))
            })?),
            (None, None) => None,
        };
        Ok(Self {
            input,
            expected_output,
        })
    }
}

/// Evidence returned by the provider-control API after it has executed a
/// publication's private verification fixture. This is operational evidence,
/// not a Kernel artifact and not part of an offer's signed bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalVerificationEvidence {
    /// SHA-256 of the canonical private fixture input. The input itself stays
    /// provider-private; this binding lets an independent canary prove it ran
    /// the same challenge.
    pub input_hash: String,
    pub result_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_output_matched: Option<bool>,
}

/// Currency-aware higher-layer price terms. Amounts are expressed in the
/// currency's minor whole unit: satoshis for `sat`, cents for `usd`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationRevisionPrice {
    pub settlement_method: PublicationSettlement,
    pub currency: PublicationCurrency,
    pub base_amount_minor: u64,
    pub success_amount_minor: u64,
    /// Exact Kernel offer method linked by `offer_hash` (for example
    /// `stripe_mpp.v1` or `lightning.prepaid.v1`).
    pub offer_settlement_method: String,
}

/// Immutable service interface and access declaration attached to a
/// publication revision. Provider-private mount bindings and verification
/// inputs are deliberately excluded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationRevisionService {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    pub source_kind: String,
    pub entrypoint_kind: String,
    pub entrypoint: String,
    pub contract_version: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<PublicationMount>,
    /// Effective sorted access set, including capabilities derived from
    /// mounts. This mirrors the capabilities in the signed Kernel offer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

impl PublicationRevisionService {
    pub fn validate(&self) -> Result<(), PublicationRevisionError> {
        for (field, value) in [
            ("service.source_kind", self.source_kind.as_str()),
            ("service.entrypoint_kind", self.entrypoint_kind.as_str()),
            ("service.entrypoint", self.entrypoint.as_str()),
            ("service.contract_version", self.contract_version.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(PublicationRevisionError::InvalidPayload(format!(
                    "{field} must not be empty"
                )));
            }
        }
        if !matches!(self.mode.as_str(), "sync" | "async") {
            return Err(PublicationRevisionError::InvalidPayload(
                "service.mode must be sync or async".to_string(),
            ));
        }
        if self
            .project_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(PublicationRevisionError::InvalidPayload(
                "service.project_id must not be empty when present".to_string(),
            ));
        }
        for (field, value) in [
            ("service.summary", self.summary.as_deref()),
            ("service.starter", self.starter.as_deref()),
        ] {
            if value.is_some_and(|value| value.len() > MAX_REVISION_TEXT_BYTES) {
                return Err(PublicationRevisionError::InvalidPayload(format!(
                    "{field} exceeds {MAX_REVISION_TEXT_BYTES} bytes"
                )));
            }
        }
        if self.mounts.len() > MAX_REVISION_MOUNTS {
            return Err(PublicationRevisionError::InvalidPayload(format!(
                "service.mounts exceeds {MAX_REVISION_MOUNTS} entries"
            )));
        }
        let mut mount_handles = HashSet::with_capacity(self.mounts.len());
        for mount in &self.mounts {
            mount
                .validate_authoring()
                .map_err(|error| PublicationRevisionError::InvalidPayload(error.to_string()))?;
            if !mount_handles.insert(mount.handle.as_str()) {
                return Err(PublicationRevisionError::InvalidPayload(format!(
                    "duplicate service mount handle {:?}",
                    mount.handle
                )));
            }
            if !self.capabilities.contains(&mount.access_capability()) {
                return Err(PublicationRevisionError::InvalidPayload(format!(
                    "service capabilities omit access declaration for mount {:?}",
                    mount.handle
                )));
            }
        }
        if self.capabilities.len() > MAX_REVISION_CAPABILITIES {
            return Err(PublicationRevisionError::InvalidPayload(format!(
                "service.capabilities exceeds {MAX_REVISION_CAPABILITIES} entries"
            )));
        }
        if self
            .capabilities
            .iter()
            .any(|capability| !valid_capability(capability))
        {
            return Err(PublicationRevisionError::InvalidPayload(
                "service.capabilities contains an invalid capability".to_string(),
            ));
        }
        if self.capabilities.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(PublicationRevisionError::InvalidPayload(
                "service.capabilities must be sorted and unique".to_string(),
            ));
        }
        for (field, schema) in [
            ("service.input_schema", self.input_schema.as_ref()),
            ("service.output_schema", self.output_schema.as_ref()),
        ] {
            if let Some(schema) = schema {
                let encoded = crate::canonical_json::to_vec(schema)
                    .map_err(|error| PublicationRevisionError::Canonical(error.to_string()))?;
                if encoded.len() > MAX_REVISION_SCHEMA_BYTES {
                    return Err(PublicationRevisionError::InvalidPayload(format!(
                        "{field} exceeds {MAX_REVISION_SCHEMA_BYTES} canonical JSON bytes"
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_canonical_for_signing(&self) -> Result<(), PublicationRevisionError> {
        if self
            .mounts
            .iter()
            .any(|mount| mount.kind == LEGACY_S3_MOUNT_KIND)
            || self
                .capabilities
                .iter()
                .any(|capability| capability.starts_with("mount.s3."))
        {
            return Err(PublicationRevisionError::InvalidPayload(
                "new publication revisions must use object_store, not the legacy s3 alias"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Immutable provider-signed statement that binds the existing Kernel offer
/// to executable bytes, currency-aware price intent, resolved limits, and the
/// successful private local verification. It is deliberately non-Kernel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationRevisionPayload {
    pub schema_version: String,
    pub provider_id: String,
    pub service_id: String,
    pub offer_id: String,
    pub offer_hash: String,
    pub binding_hash: String,
    pub package_digest: String,
    pub runtime: String,
    pub package_kind: String,
    /// Immutable, provider-signed build and dependency evidence. Optional only
    /// when reading pre-evidence revisions; every current publication path
    /// populates it before signing a new revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_evidence: Option<PublicationBuildEvidence>,
    pub service: PublicationRevisionService,
    pub limits: ResolvedPublicationLimits,
    pub price: PublicationRevisionPrice,
    pub local_verification: LocalVerificationEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignedPublicationRevision {
    /// Stable identity of the immutable revision: SHA-256 of canonical
    /// `payload` bytes. This is independent of signature encoding.
    pub revision_hash: String,
    pub signer_pubkey: String,
    pub signature: String,
    pub payload: PublicationRevisionPayload,
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl PublicationRevisionPayload {
    pub fn validate(&self) -> Result<(), PublicationRevisionError> {
        if self.schema_version != PUBLICATION_REVISION_SCHEMA_V1 {
            return Err(PublicationRevisionError::InvalidPayload(
                "unsupported schema_version".to_string(),
            ));
        }
        for (field, value) in [
            ("provider_id", self.provider_id.as_str()),
            ("offer_hash", self.offer_hash.as_str()),
            ("binding_hash", self.binding_hash.as_str()),
            ("package_digest", self.package_digest.as_str()),
            (
                "local_verification.input_hash",
                self.local_verification.input_hash.as_str(),
            ),
            (
                "local_verification.result_hash",
                self.local_verification.result_hash.as_str(),
            ),
        ] {
            if !valid_hash(value) {
                return Err(PublicationRevisionError::InvalidPayload(format!(
                    "{field} must be 64 lowercase hex characters"
                )));
            }
        }
        for (field, value) in [
            ("service_id", self.service_id.as_str()),
            ("offer_id", self.offer_id.as_str()),
            ("runtime", self.runtime.as_str()),
            ("package_kind", self.package_kind.as_str()),
            (
                "price.offer_settlement_method",
                self.price.offer_settlement_method.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(PublicationRevisionError::InvalidPayload(format!(
                    "{field} must not be empty"
                )));
            }
        }
        if self.local_verification.expected_output_matched == Some(false) {
            return Err(PublicationRevisionError::InvalidPayload(
                "a failed local verification cannot be signed as a revision".to_string(),
            ));
        }
        if let Some(build_evidence) = &self.build_evidence {
            build_evidence
                .validate()
                .map_err(PublicationRevisionError::InvalidPayload)?;
            if build_evidence.artifact_digest != self.package_digest {
                return Err(PublicationRevisionError::InvalidPayload(
                    "build evidence artifact_digest must equal package_digest".to_string(),
                ));
            }
        }
        self.service.validate()?;
        for (field, value) in [
            (
                "limits.max_input_bytes",
                self.limits.max_input_bytes as u128,
            ),
            ("limits.max_runtime_ms", self.limits.max_runtime_ms as u128),
            (
                "limits.max_output_bytes",
                self.limits.max_output_bytes as u128,
            ),
        ] {
            if value == 0 {
                return Err(PublicationRevisionError::InvalidPayload(format!(
                    "{field} must be greater than zero"
                )));
            }
        }
        if self.runtime != "builtin" && self.limits.max_memory_bytes == 0 {
            return Err(PublicationRevisionError::InvalidPayload(
                "limits.max_memory_bytes must be greater than zero for non-builtin runtimes"
                    .to_string(),
            ));
        }
        let total = self
            .price
            .base_amount_minor
            .checked_add(self.price.success_amount_minor)
            .ok_or_else(|| {
                PublicationRevisionError::InvalidPayload("price total overflow".to_string())
            })?;
        match self.price.settlement_method {
            PublicationSettlement::None
                if total != 0 || self.price.offer_settlement_method != "none" =>
            {
                Err(PublicationRevisionError::InvalidPayload(
                    "free revision requires zero amounts and offer settlement none".to_string(),
                ))
            }
            PublicationSettlement::Lightning
                if self.price.currency != PublicationCurrency::Sat
                    || !self.price.offer_settlement_method.starts_with("lightning.") =>
            {
                Err(PublicationRevisionError::InvalidPayload(
                    "lightning revision requires sat currency and a Lightning offer method"
                        .to_string(),
                ))
            }
            PublicationSettlement::Stripe
                if self.price.currency != PublicationCurrency::Usd
                    || self.price.offer_settlement_method != "stripe_mpp.v1" =>
            {
                Err(PublicationRevisionError::InvalidPayload(
                    "Stripe revision requires usd currency and stripe_mpp.v1".to_string(),
                ))
            }
            PublicationSettlement::None
            | PublicationSettlement::Lightning
            | PublicationSettlement::Stripe => Ok(()),
        }
    }

    fn validate_canonical_for_signing(&self) -> Result<(), PublicationRevisionError> {
        self.validate()?;
        self.service.validate_canonical_for_signing()
    }
}

fn publication_revision_signing_bytes(
    payload: &PublicationRevisionPayload,
) -> Result<Vec<u8>, PublicationRevisionError> {
    let canonical = crate::canonical_json::to_vec(payload)
        .map_err(|error| PublicationRevisionError::Canonical(error.to_string()))?;
    let mut bytes =
        Vec::with_capacity(PUBLICATION_REVISION_SIGNATURE_DOMAIN.len() + canonical.len());
    bytes.extend_from_slice(PUBLICATION_REVISION_SIGNATURE_DOMAIN);
    bytes.extend_from_slice(&canonical);
    Ok(bytes)
}

pub fn sign_publication_revision(
    payload: PublicationRevisionPayload,
    sign_message_hex: impl Fn(&[u8]) -> String,
) -> Result<SignedPublicationRevision, PublicationRevisionError> {
    payload.validate_canonical_for_signing()?;
    let canonical = crate::canonical_json::to_vec(&payload)
        .map_err(|error| PublicationRevisionError::Canonical(error.to_string()))?;
    let revision_hash = crate::crypto::sha256_hex(&canonical);
    let signature = sign_message_hex(&publication_revision_signing_bytes(&payload)?);
    Ok(SignedPublicationRevision {
        revision_hash,
        signer_pubkey: payload.provider_id.clone(),
        signature,
        payload,
    })
}

impl SignedPublicationRevision {
    pub fn verify(&self) -> Result<(), PublicationRevisionError> {
        self.payload.validate()?;
        if self.signer_pubkey != self.payload.provider_id {
            return Err(PublicationRevisionError::InvalidPayload(
                "signer_pubkey must equal payload.provider_id".to_string(),
            ));
        }
        let canonical = crate::canonical_json::to_vec(&self.payload)
            .map_err(|error| PublicationRevisionError::Canonical(error.to_string()))?;
        let expected_hash = crate::crypto::sha256_hex(&canonical);
        if self.revision_hash != expected_hash {
            return Err(PublicationRevisionError::HashMismatch);
        }
        if !crate::crypto::verify_message(
            &self.signer_pubkey,
            &self.signature,
            &publication_revision_signing_bytes(&self.payload)?,
        ) {
            return Err(PublicationRevisionError::SignatureInvalid);
        }
        Ok(())
    }
}

/// Fresh marketplace challenge for an exact immutable publication revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublicationCanaryRequest {
    pub schema_version: String,
    pub revision_hash: String,
    pub offer_hash: String,
    pub challenge: String,
    pub input: Value,
}

impl PublicationCanaryRequest {
    pub fn validate(&self) -> Result<(), PublicationCanaryError> {
        if self.schema_version != PUBLICATION_CANARY_REQUEST_SCHEMA_V1 {
            return Err(PublicationCanaryError::InvalidPayload(
                "unsupported request schema_version".to_string(),
            ));
        }
        for (field, value) in [
            ("revision_hash", self.revision_hash.as_str()),
            ("offer_hash", self.offer_hash.as_str()),
            ("challenge", self.challenge.as_str()),
        ] {
            if !valid_hash(value) {
                return Err(PublicationCanaryError::InvalidPayload(format!(
                    "{field} must be 64 lowercase hex characters"
                )));
            }
        }
        crate::canonical_json::to_vec(&self.input)
            .map_err(|error| PublicationCanaryError::Canonical(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationCanaryResultPayload {
    pub schema_version: String,
    pub provider_id: String,
    pub revision_hash: String,
    pub offer_hash: String,
    pub challenge: String,
    pub input_hash: String,
    pub result_hash: String,
    pub status: String,
}

impl PublicationCanaryResultPayload {
    pub fn validate(&self) -> Result<(), PublicationCanaryError> {
        if self.schema_version != PUBLICATION_CANARY_RESULT_SCHEMA_V1 {
            return Err(PublicationCanaryError::InvalidPayload(
                "unsupported result schema_version".to_string(),
            ));
        }
        for (field, value) in [
            ("provider_id", self.provider_id.as_str()),
            ("revision_hash", self.revision_hash.as_str()),
            ("offer_hash", self.offer_hash.as_str()),
            ("challenge", self.challenge.as_str()),
            ("input_hash", self.input_hash.as_str()),
            ("result_hash", self.result_hash.as_str()),
        ] {
            if !valid_hash(value) {
                return Err(PublicationCanaryError::InvalidPayload(format!(
                    "{field} must be 64 lowercase hex characters"
                )));
            }
        }
        if self.status != "succeeded" {
            return Err(PublicationCanaryError::InvalidPayload(
                "status must be succeeded".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignedPublicationCanaryResult {
    pub payload_hash: String,
    pub signer_pubkey: String,
    pub signature: String,
    pub payload: PublicationCanaryResultPayload,
}

fn publication_canary_signing_bytes(
    payload: &PublicationCanaryResultPayload,
) -> Result<Vec<u8>, PublicationCanaryError> {
    let canonical = crate::canonical_json::to_vec(payload)
        .map_err(|error| PublicationCanaryError::Canonical(error.to_string()))?;
    let mut bytes = Vec::with_capacity(PUBLICATION_CANARY_SIGNATURE_DOMAIN.len() + canonical.len());
    bytes.extend_from_slice(PUBLICATION_CANARY_SIGNATURE_DOMAIN);
    bytes.extend_from_slice(&canonical);
    Ok(bytes)
}

pub fn sign_publication_canary_result(
    payload: PublicationCanaryResultPayload,
    sign_message_hex: impl Fn(&[u8]) -> String,
) -> Result<SignedPublicationCanaryResult, PublicationCanaryError> {
    payload.validate()?;
    let canonical = crate::canonical_json::to_vec(&payload)
        .map_err(|error| PublicationCanaryError::Canonical(error.to_string()))?;
    let payload_hash = crate::crypto::sha256_hex(canonical);
    let signature = sign_message_hex(&publication_canary_signing_bytes(&payload)?);
    Ok(SignedPublicationCanaryResult {
        payload_hash,
        signer_pubkey: payload.provider_id.clone(),
        signature,
        payload,
    })
}

impl SignedPublicationCanaryResult {
    pub fn verify(&self) -> Result<(), PublicationCanaryError> {
        self.payload.validate()?;
        if self.signer_pubkey != self.payload.provider_id {
            return Err(PublicationCanaryError::InvalidPayload(
                "signer_pubkey must equal payload.provider_id".to_string(),
            ));
        }
        let canonical = crate::canonical_json::to_vec(&self.payload)
            .map_err(|error| PublicationCanaryError::Canonical(error.to_string()))?;
        if self.payload_hash != crate::crypto::sha256_hex(canonical) {
            return Err(PublicationCanaryError::HashMismatch);
        }
        if !crate::crypto::verify_message(
            &self.signer_pubkey,
            &self.signature,
            &publication_canary_signing_bytes(&self.payload)?,
        ) {
            return Err(PublicationCanaryError::SignatureInvalid);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PublicationCanaryError {
    #[error("invalid publication canary payload: {0}")]
    InvalidPayload(String),
    #[error("publication canary canonicalization failed: {0}")]
    Canonical(String),
    #[error("publication canary hash mismatch")]
    HashMismatch,
    #[error("publication canary signature verification failed")]
    SignatureInvalid,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PublicationRevisionError {
    #[error("invalid publication revision payload: {0}")]
    InvalidPayload(String),
    #[error("publication revision canonicalization failed: {0}")]
    Canonical(String),
    #[error("publication revision hash mismatch")]
    HashMismatch,
    #[error("publication revision signature verification failed")]
    SignatureInvalid,
}

/// Canonical, non-Kernel publication request shared by every authoring path.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublicationIntent {
    /// Omitted only for compatibility with pre-contract provider-control
    /// clients. Every current authoring adapter emits v1 explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    pub service_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm_module_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounts: Option<Vec<PublicationMount>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_source: Option<String>,
    /// Canonical, resolver-free Python package. Current authoring adapters
    /// populate this instead of raw `inline_source`; the provider validates
    /// every embedded wheel and generates the fixed runtime loader locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_bundle: Option<LockedPythonBundleEnvelope>,
    /// Immutable JSON or SQLite bytes for the native read-only data-query
    /// lane. Mutually exclusive with executable artifact fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<PublicationDataSource>,
    /// Build/dependency evidence generated by the local package builder. The
    /// provider validates its artifact digest before carrying it into a signed
    /// Publication Revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_evidence: Option<PublicationBuildEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    pub price_sats: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_fee_msat: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_fee_msat: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_method: Option<PublicationSettlement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_currency: Option<PublicationCurrency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<PublicationLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationFixture>,
}

impl PublicationIntent {
    pub fn from_service_manifest(
        manifest: &ServiceManifest,
    ) -> Result<Self, PublicationIntentError> {
        let price = manifest.price.as_ref();
        let intent = Self {
            schema_version: Some(PUBLICATION_INTENT_SCHEMA_V1.to_string()),
            service_id: manifest.service_id.clone(),
            offer_id: manifest.offer_id.clone(),
            project_id: manifest.project_id.clone(),
            oci_reference: manifest.oci.as_ref().map(|value| value.reference.clone()),
            oci_digest: manifest.oci.as_ref().map(|value| value.digest.clone()),
            runtime: Some(manifest.runtime.clone()),
            package_kind: Some(manifest.package_kind.clone()),
            entrypoint_kind: manifest.entrypoint_kind.clone(),
            entrypoint: manifest.entrypoint.clone(),
            contract_version: manifest.contract_version.clone(),
            mounts: (!manifest.mounts.is_empty()).then(|| manifest.mounts.clone()),
            capabilities: (!manifest.capabilities.is_empty())
                .then(|| manifest.capabilities.clone()),
            source_kind: manifest.source_kind.clone(),
            summary: manifest.summary.clone(),
            starter: manifest.starter.clone(),
            mode: manifest.mode.clone(),
            price_sats: price.and_then(|value| value.sats).unwrap_or(0),
            base_fee_msat: price.and_then(|value| value.base_fee_msat),
            success_fee_msat: price.and_then(|value| value.success_fee_msat),
            settlement_method: manifest
                .settlement
                .as_ref()
                .map(|value| PublicationSettlement::from_str(&value.method))
                .transpose()?,
            price_currency: price
                .and_then(|value| value.currency.as_deref())
                .map(PublicationCurrency::from_str)
                .transpose()?,
            limits: manifest.limits.clone(),
            publication_state: manifest.publication_state.clone(),
            input_schema: manifest.input_schema.clone(),
            output_schema: manifest.output_schema.clone(),
            verification: manifest.verification.clone(),
            ..Self::default()
        };
        intent.normalized()
    }

    pub fn normalized(mut self) -> Result<Self, PublicationIntentError> {
        if let Some(mounts) = &mut self.mounts {
            for mount in mounts {
                mount.kind = canonical_mount_kind(&mount.kind)
                    .ok_or_else(|| {
                        PublicationIntentError::InvalidMount(format!(
                            "unsupported mount kind {:?}; allowed: postgres, sqlite, object_store, redis (legacy alias: s3)",
                            mount.kind
                        ))
                    })?
                    .to_string();
            }
        }
        if let Some(capabilities) = &mut self.capabilities {
            for capability in capabilities.iter_mut() {
                *capability = capability.trim().to_ascii_lowercase();
                *capability = canonical_mount_capability(capability);
                if !valid_capability(capability) {
                    return Err(PublicationIntentError::InvalidCapability(
                        capability.clone(),
                    ));
                }
            }
            capabilities.sort();
            capabilities.dedup();
            if capabilities.is_empty() {
                self.capabilities = None;
            }
        }
        self.validate_authoring()?;
        Ok(self)
    }

    pub fn validate_authoring(&self) -> Result<(), PublicationIntentError> {
        if let Some(schema_version) = self.schema_version.as_deref()
            && schema_version != PUBLICATION_INTENT_SCHEMA_V1
        {
            return Err(PublicationIntentError::InvalidSchemaVersion(
                schema_version.to_string(),
            ));
        }
        if let Some(mounts) = &self.mounts {
            let mut handles = HashSet::with_capacity(mounts.len());
            for mount in mounts {
                mount.validate_authoring()?;
                if !handles.insert(mount.handle.as_str()) {
                    return Err(PublicationIntentError::InvalidMount(format!(
                        "duplicate mount handle {:?}",
                        mount.handle
                    )));
                }
            }
        }
        if let Some(capabilities) = &self.capabilities
            && capabilities
                .iter()
                .any(|capability| capability.trim().is_empty())
        {
            return Err(PublicationIntentError::InvalidCapability(String::new()));
        }
        if let Some(data_source) = &self.data_source {
            if data_source.content_base64.trim().is_empty() {
                return Err(PublicationIntentError::InvalidDataSource(
                    "content_base64 must be non-empty".to_string(),
                ));
            }
            if self.runtime.as_deref() != Some("builtin")
                || self.package_kind.as_deref() != Some("builtin")
            {
                return Err(PublicationIntentError::InvalidDataSource(
                    "data_source requires runtime=builtin and package_kind=builtin".to_string(),
                ));
            }
            if self.artifact_path.is_some()
                || self.wasm_module_hex.is_some()
                || self.inline_source.is_some()
                || self.python_bundle.is_some()
                || self.oci_reference.is_some()
                || self.oci_digest.is_some()
            {
                return Err(PublicationIntentError::InvalidDataSource(
                    "data_source is mutually exclusive with executable artifact fields".to_string(),
                ));
            }
            if self
                .mounts
                .as_ref()
                .is_some_and(|mounts| !mounts.is_empty())
                || self
                    .capabilities
                    .as_ref()
                    .is_some_and(|capabilities| !capabilities.is_empty())
            {
                return Err(PublicationIntentError::InvalidDataSource(
                    "the v1 read-only data lane does not accept mounts or capabilities".to_string(),
                ));
            }
            match (&data_source.format, &data_source.csv_schema) {
                (PublicationDataFormat::Csv, Some(schema)) => schema
                    .validate()
                    .map_err(PublicationIntentError::InvalidDataSource)?,
                (PublicationDataFormat::Csv, None) => {
                    return Err(PublicationIntentError::InvalidDataSource(
                        "CSV data_source requires csv_schema".to_string(),
                    ));
                }
                (PublicationDataFormat::Json | PublicationDataFormat::Sqlite, Some(_)) => {
                    return Err(PublicationIntentError::InvalidDataSource(
                        "csv_schema is only valid for CSV data sources".to_string(),
                    ));
                }
                (PublicationDataFormat::Json | PublicationDataFormat::Sqlite, None) => {}
            }
        }
        if let Some(python_bundle) = &self.python_bundle {
            python_bundle
                .validate_metadata()
                .map_err(PublicationIntentError::InvalidBuildEvidence)?;
            if self.runtime.as_deref() != Some("python")
                || self.package_kind.as_deref() != Some("inline_source")
            {
                return Err(PublicationIntentError::InvalidBuildEvidence(
                    "python_bundle requires runtime=python and package_kind=inline_source"
                        .to_string(),
                ));
            }
            if self.inline_source.is_some()
                || self.artifact_path.is_some()
                || self.wasm_module_hex.is_some()
                || self.data_source.is_some()
                || self.oci_reference.is_some()
                || self.oci_digest.is_some()
            {
                return Err(PublicationIntentError::InvalidBuildEvidence(
                    "python_bundle is mutually exclusive with other executable artifact fields"
                        .to_string(),
                ));
            }
        }
        if let Some(build_evidence) = &self.build_evidence {
            build_evidence
                .validate()
                .map_err(PublicationIntentError::InvalidBuildEvidence)?;
        }
        let (base_fee_msat, success_fee_msat) = self.requested_price_schedule()?;
        let total_msat = base_fee_msat
            .checked_add(success_fee_msat)
            .ok_or(PublicationIntentError::PriceScheduleOverflow)?;
        if !base_fee_msat.is_multiple_of(1_000) || !success_fee_msat.is_multiple_of(1_000) {
            return Err(PublicationIntentError::FractionalPriceLeg {
                base_fee_msat,
                success_fee_msat,
            });
        }
        if let Some(explicit_success_fee_msat) = self.success_fee_msat {
            let legacy_success_fee_msat = self
                .price_sats
                .checked_mul(1_000)
                .ok_or(PublicationIntentError::PriceScheduleOverflow)?;
            if explicit_success_fee_msat != legacy_success_fee_msat {
                return Err(PublicationIntentError::AmbiguousPricing {
                    price_sats: self.price_sats,
                    success_fee_msat: explicit_success_fee_msat,
                });
            }
        }
        if total_msat != 0 && self.settlement_method.is_none() {
            return Err(PublicationIntentError::IncompatiblePayment(
                "paid publication requires an explicit settlement method".to_string(),
            ));
        }
        if self.settlement_method == Some(PublicationSettlement::None)
            && (self.price_sats != 0
                || self.base_fee_msat.unwrap_or(0) != 0
                || self.success_fee_msat.unwrap_or(0) != 0)
        {
            return Err(PublicationIntentError::IncompatiblePayment(
                "settlement method none requires a zero price schedule".to_string(),
            ));
        }
        match (self.settlement_method, self.price_currency) {
            (Some(PublicationSettlement::Lightning), Some(PublicationCurrency::Usd)) => {
                Err(PublicationIntentError::IncompatiblePayment(
                    "lightning settlement requires sat currency".to_string(),
                ))
            }
            (Some(PublicationSettlement::Stripe), currency)
                if currency != Some(PublicationCurrency::Usd) =>
            {
                Err(PublicationIntentError::IncompatiblePayment(
                    "stripe settlement requires explicit usd currency".to_string(),
                ))
            }
            _ => Ok(()),
        }
    }

    /// Resolve the requested schedule without choosing a concrete settlement
    /// rail implementation. Explicit `none` is always free. For legacy callers
    /// that omit settlement, existing daemon inference remains possible.
    pub fn requested_price_schedule(&self) -> Result<(u64, u64), PublicationIntentError> {
        if self.settlement_method == Some(PublicationSettlement::None) {
            return Ok((0, 0));
        }
        Ok((
            self.base_fee_msat.unwrap_or(0),
            match self.success_fee_msat {
                Some(value) => value,
                None => self
                    .price_sats
                    .checked_mul(1_000)
                    .ok_or(PublicationIntentError::PriceScheduleOverflow)?,
            },
        ))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PublicationIntentError {
    #[error("unsupported publication intent schema version {0:?}")]
    InvalidSchemaVersion(String),
    #[error("unsupported publication settlement method {0:?}")]
    InvalidSettlement(String),
    #[error("unsupported publication price currency {0:?}")]
    InvalidCurrency(String),
    #[error("invalid publication mount: {0}")]
    InvalidMount(String),
    #[error("invalid publication capability {0:?}")]
    InvalidCapability(String),
    #[error("invalid publication data source: {0}")]
    InvalidDataSource(String),
    #[error("invalid publication build evidence: {0}")]
    InvalidBuildEvidence(String),
    #[error("incompatible publication payment intent: {0}")]
    IncompatiblePayment(String),
    #[error(
        "ambiguous publication pricing: price_sats={price_sats} conflicts with explicit success_fee_msat={success_fee_msat}"
    )]
    AmbiguousPricing {
        price_sats: u64,
        success_fee_msat: u64,
    },
    #[error("publication price schedule overflows u64 millisatoshis")]
    PriceScheduleOverflow,
    #[error(
        "publication price legs ({base_fee_msat}, {success_fee_msat}) msat must each be representable as whole currency minor units"
    )]
    FractionalPriceLeg {
        base_fee_msat: u64,
        success_fee_msat: u64,
    },
    #[error("publication limit {field}={requested} exceeds available maximum {maximum}")]
    InvalidLimit {
        field: &'static str,
        requested: u128,
        maximum: u128,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_mount_defaults_to_read_only() {
        let mount: PublicationMount = serde_json::from_value(serde_json::json!({
            "handle": "warehouse",
            "kind": "postgres"
        }))
        .unwrap();
        assert!(mount.read_only);
    }

    #[test]
    fn publication_intent_rejects_unknown_fields() {
        let error = serde_json::from_value::<PublicationIntent>(serde_json::json!({
            "service_id": "analytics",
            "price_sats": 0,
            "settlemnt_method": "none"
        }))
        .unwrap_err();
        assert!(error.to_string().contains("settlemnt_method"));
    }

    #[test]
    fn publication_intent_rejects_conflicting_price_fields() {
        let error = PublicationIntent {
            service_id: "analytics".to_string(),
            price_sats: 5,
            success_fee_msat: Some(6_000),
            settlement_method: Some(PublicationSettlement::Lightning),
            price_currency: Some(PublicationCurrency::Sat),
            ..PublicationIntent::default()
        }
        .normalized()
        .unwrap_err();
        assert!(matches!(
            error,
            PublicationIntentError::AmbiguousPricing { .. }
        ));
    }

    #[test]
    fn publication_intent_rejects_duplicate_mount_handles() {
        let error = PublicationIntent {
            service_id: "analytics".to_string(),
            mounts: Some(vec![
                PublicationMount {
                    handle: "warehouse".to_string(),
                    kind: "postgres".to_string(),
                    read_only: true,
                    binding: None,
                },
                PublicationMount {
                    handle: "warehouse".to_string(),
                    kind: "sqlite".to_string(),
                    read_only: true,
                    binding: None,
                },
            ]),
            ..PublicationIntent::default()
        }
        .normalized()
        .unwrap_err();

        assert!(
            matches!(error, PublicationIntentError::InvalidMount(message) if message.contains("duplicate"))
        );
    }

    #[test]
    fn publication_intent_canonicalizes_legacy_s3_mounts_and_capabilities() {
        let intent = PublicationIntent {
            service_id: "backups".to_string(),
            mounts: Some(vec![PublicationMount {
                handle: "archive".to_string(),
                kind: LEGACY_S3_MOUNT_KIND.to_string(),
                read_only: true,
                binding: None,
            }]),
            capabilities: Some(vec![" mount.s3.read.archive ".to_string()]),
            ..PublicationIntent::default()
        }
        .normalized()
        .expect("legacy authoring alias");

        assert_eq!(
            intent.mounts.as_ref().expect("mounts")[0].kind,
            OBJECT_STORE_MOUNT_KIND
        );
        assert_eq!(
            intent.capabilities,
            Some(vec!["mount.object_store.read.archive".to_string()])
        );
    }

    #[test]
    fn publication_intent_rejects_legacy_price_overflow() {
        let error = PublicationIntent {
            service_id: "analytics".to_string(),
            price_sats: u64::MAX,
            settlement_method: Some(PublicationSettlement::Lightning),
            price_currency: Some(PublicationCurrency::Sat),
            ..PublicationIntent::default()
        }
        .normalized()
        .unwrap_err();

        assert_eq!(error, PublicationIntentError::PriceScheduleOverflow);
    }

    #[test]
    fn paid_publication_requires_an_explicit_settlement_rail() {
        let error = PublicationIntent {
            service_id: "analytics".to_string(),
            price_sats: 5,
            price_currency: Some(PublicationCurrency::Usd),
            ..PublicationIntent::default()
        }
        .normalized()
        .unwrap_err();

        assert!(matches!(
            error,
            PublicationIntentError::IncompatiblePayment(message)
                if message.contains("explicit settlement method")
        ));
    }

    #[test]
    fn fractional_whole_unit_price_is_rejected_before_downstream_rounding() {
        let error = PublicationIntent {
            service_id: "analytics".to_string(),
            base_fee_msat: Some(1),
            settlement_method: Some(PublicationSettlement::Lightning),
            price_currency: Some(PublicationCurrency::Sat),
            ..PublicationIntent::default()
        }
        .normalized()
        .unwrap_err();

        assert_eq!(
            error,
            PublicationIntentError::FractionalPriceLeg {
                base_fee_msat: 1,
                success_fee_msat: 0,
            }
        );
    }

    #[test]
    fn offsetting_fractional_price_legs_are_rejected() {
        let error = PublicationIntent {
            service_id: "analytics".to_string(),
            base_fee_msat: Some(500),
            success_fee_msat: Some(500),
            settlement_method: Some(PublicationSettlement::Lightning),
            price_currency: Some(PublicationCurrency::Sat),
            ..PublicationIntent::default()
        }
        .normalized()
        .unwrap_err();

        assert!(matches!(
            error,
            PublicationIntentError::FractionalPriceLeg {
                base_fee_msat: 500,
                success_fee_msat: 500
            }
        ));
    }

    fn revision_payload(provider_id: String) -> PublicationRevisionPayload {
        PublicationRevisionPayload {
            schema_version: PUBLICATION_REVISION_SCHEMA_V1.to_string(),
            provider_id,
            service_id: "analytics".to_string(),
            offer_id: "analytics-v1".to_string(),
            offer_hash: "11".repeat(32),
            binding_hash: "22".repeat(32),
            package_digest: "22".repeat(32),
            runtime: "wasm".to_string(),
            package_kind: "inline_module".to_string(),
            build_evidence: None,
            service: PublicationRevisionService {
                project_id: Some("analytics".to_string()),
                summary: Some("Read analytics".to_string()),
                starter: Some(r#"{"query":"select 1"}"#.to_string()),
                source_kind: "artifact".to_string(),
                entrypoint_kind: "handler".to_string(),
                entrypoint: "run".to_string(),
                contract_version: "froglet.wasm.run_json.v1".to_string(),
                mode: "sync".to_string(),
                mounts: Vec::new(),
                capabilities: Vec::new(),
                input_schema: Some(serde_json::json!({"type": "object"})),
                output_schema: Some(serde_json::json!({"type": "object"})),
            },
            limits: ResolvedPublicationLimits {
                max_input_bytes: 4096,
                max_runtime_ms: 2500,
                max_memory_bytes: 8 * 1024 * 1024,
                max_output_bytes: 2048,
                fuel_limit: 50_000,
            },
            price: PublicationRevisionPrice {
                settlement_method: PublicationSettlement::None,
                currency: PublicationCurrency::Sat,
                base_amount_minor: 0,
                success_amount_minor: 0,
                offer_settlement_method: "none".to_string(),
            },
            local_verification: LocalVerificationEvidence {
                input_hash: "22".repeat(32),
                result_hash: "33".repeat(32),
                expected_output_matched: Some(true),
            },
        }
    }

    #[test]
    fn publication_revision_signs_verifies_and_detects_tampering() {
        let signing_key = crate::crypto::generate_signing_key();
        let provider_id = crate::crypto::public_key_hex(&signing_key);
        let revision = sign_publication_revision(revision_payload(provider_id), |message| {
            crate::crypto::sign_message_hex(&signing_key, message)
        })
        .unwrap();

        revision.verify().unwrap();
        let mut tampered = revision;
        tampered.payload.binding_hash = "44".repeat(32);
        assert_eq!(
            tampered.verify().unwrap_err(),
            PublicationRevisionError::HashMismatch
        );
    }

    #[test]
    fn new_revision_signing_rejects_the_legacy_s3_alias() {
        let signing_key = crate::crypto::generate_signing_key();
        let provider_id = crate::crypto::public_key_hex(&signing_key);
        let mut payload = revision_payload(provider_id);
        payload.service.mounts.push(PublicationMount {
            handle: "archive".to_string(),
            kind: LEGACY_S3_MOUNT_KIND.to_string(),
            read_only: true,
            binding: None,
        });
        payload
            .service
            .capabilities
            .push("mount.s3.read.archive".to_string());

        let error = sign_publication_revision(payload, |message| {
            crate::crypto::sign_message_hex(&signing_key, message)
        })
        .expect_err("new signatures must use the canonical kind");
        assert!(
            matches!(error, PublicationRevisionError::InvalidPayload(message) if message.contains("object_store"))
        );
    }

    #[test]
    fn already_signed_legacy_s3_revision_remains_verifiable() {
        let signing_key = crate::crypto::generate_signing_key();
        let provider_id = crate::crypto::public_key_hex(&signing_key);
        let mut payload = revision_payload(provider_id.clone());
        payload.service.mounts.push(PublicationMount {
            handle: "archive".to_string(),
            kind: LEGACY_S3_MOUNT_KIND.to_string(),
            read_only: true,
            binding: None,
        });
        payload
            .service
            .capabilities
            .push("mount.s3.read.archive".to_string());
        let canonical = crate::canonical_json::to_vec(&payload).expect("canonical payload");
        let revision = SignedPublicationRevision {
            revision_hash: crate::crypto::sha256_hex(&canonical),
            signer_pubkey: provider_id,
            signature: crate::crypto::sign_message_hex(
                &signing_key,
                &publication_revision_signing_bytes(&payload).expect("signing bytes"),
            ),
            payload,
        };

        revision.verify().expect("legacy signed revision");
    }

    #[test]
    fn publication_canary_signs_exact_fresh_challenge() {
        let signing_key = crate::crypto::generate_signing_key();
        let provider_id = crate::crypto::public_key_hex(&signing_key);
        let request = PublicationCanaryRequest {
            schema_version: PUBLICATION_CANARY_REQUEST_SCHEMA_V1.to_string(),
            revision_hash: "11".repeat(32),
            offer_hash: "22".repeat(32),
            challenge: "33".repeat(32),
            input: serde_json::json!({"op": "describe"}),
        };
        request.validate().expect("valid canary request");
        let input_hash = crate::crypto::sha256_hex(
            crate::canonical_json::to_vec(&request.input).expect("canonical input"),
        );
        let signed = sign_publication_canary_result(
            PublicationCanaryResultPayload {
                schema_version: PUBLICATION_CANARY_RESULT_SCHEMA_V1.to_string(),
                provider_id,
                revision_hash: request.revision_hash.clone(),
                offer_hash: request.offer_hash.clone(),
                challenge: request.challenge.clone(),
                input_hash,
                result_hash: "44".repeat(32),
                status: "succeeded".to_string(),
            },
            |message| crate::crypto::sign_message_hex(&signing_key, message),
        )
        .expect("sign canary result");
        signed.verify().expect("verify canary result");

        let mut replayed = signed;
        replayed.payload.challenge = "55".repeat(32);
        assert_eq!(replayed.verify(), Err(PublicationCanaryError::HashMismatch));
    }

    #[test]
    fn data_source_requires_the_narrow_native_profile() {
        let error = PublicationIntent {
            service_id: "catalog".to_string(),
            runtime: Some("python".to_string()),
            package_kind: Some("inline_source".to_string()),
            data_source: Some(PublicationDataSource {
                format: PublicationDataFormat::Json,
                content_base64: "W10=".to_string(),
                csv_schema: None,
            }),
            settlement_method: Some(PublicationSettlement::None),
            ..PublicationIntent::default()
        }
        .normalized()
        .expect_err("data source must not masquerade as Python");
        assert!(matches!(
            error,
            PublicationIntentError::InvalidDataSource(_)
        ));
    }

    #[test]
    fn csv_data_requires_explicit_indexed_schema() {
        let error = PublicationIntent {
            service_id: "catalog".to_string(),
            runtime: Some("builtin".to_string()),
            package_kind: Some("builtin".to_string()),
            data_source: Some(PublicationDataSource {
                format: PublicationDataFormat::Csv,
                content_base64: "aWQKMQo=".to_string(),
                csv_schema: None,
            }),
            settlement_method: Some(PublicationSettlement::None),
            ..PublicationIntent::default()
        }
        .normalized()
        .expect_err("CSV without a schema must fail closed");
        assert!(matches!(
            error,
            PublicationIntentError::InvalidDataSource(message)
                if message.contains("csv_schema")
        ));
    }

    #[test]
    fn revision_build_evidence_must_bind_package_digest() {
        let signing_key = crate::crypto::generate_signing_key();
        let provider_id = crate::crypto::public_key_hex(&signing_key);
        let mut payload = revision_payload(provider_id);
        payload.build_evidence = Some(PublicationBuildEvidence {
            schema_version: PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1.to_string(),
            builder: "froglet.embedded-wat".to_string(),
            builder_version: "1.245.1".to_string(),
            source_digest: "44".repeat(32),
            artifact_digest: "55".repeat(32),
            dependency_mode: "locked".to_string(),
            components: vec![PublicationDependencyComponent {
                role: "builder".to_string(),
                name: "wat".to_string(),
                version: "1.245.1".to_string(),
                digest: "66".repeat(32),
            }],
            hermetic: true,
        });

        let error = sign_publication_revision(payload, |message| {
            crate::crypto::sign_message_hex(&signing_key, message)
        })
        .expect_err("provenance cannot bind different artifact bytes");
        assert!(matches!(
            error,
            PublicationRevisionError::InvalidPayload(message)
                if message.contains("artifact_digest")
        ));
    }

    #[test]
    fn publication_revision_binds_currency_minor_units_and_offer_method() {
        let signing_key = crate::crypto::generate_signing_key();
        let provider_id = crate::crypto::public_key_hex(&signing_key);
        let mut payload = revision_payload(provider_id);
        payload.price = PublicationRevisionPrice {
            settlement_method: PublicationSettlement::Stripe,
            currency: PublicationCurrency::Usd,
            base_amount_minor: 25,
            success_amount_minor: 500,
            offer_settlement_method: "stripe_mpp.v1".to_string(),
        };

        let revision = sign_publication_revision(payload, |message| {
            crate::crypto::sign_message_hex(&signing_key, message)
        })
        .unwrap();
        revision.verify().unwrap();
        assert_eq!(revision.payload.price.currency, PublicationCurrency::Usd);
        assert_eq!(revision.payload.price.success_amount_minor, 500);
    }

    #[test]
    fn publication_precondition_token_binds_identity_intent_limits_and_configuration() {
        let limits = ResolvedPublicationLimits {
            max_input_bytes: 4_096,
            max_runtime_ms: 2_500,
            max_memory_bytes: 8 * 1024 * 1024,
            max_output_bytes: 2_048,
            fuel_limit: 50_000,
        };
        let identity_backup = PublicationIdentityBackup::new(
            PublicationIdentityBackupState::Current,
            Some("44".repeat(32)),
        )
        .expect("valid identity backup binding");
        let precondition = PublicationPrecondition::new(
            "11".repeat(32),
            "22".repeat(32),
            limits,
            "33".repeat(32),
            identity_backup,
        )
        .expect("valid publication precondition");
        precondition.validate().expect("precondition validates");

        for tampered in [
            {
                let mut value = precondition.clone();
                value.provider_id = "44".repeat(32);
                value
            },
            {
                let mut value = precondition.clone();
                value.intent_digest = "55".repeat(32);
                value
            },
            {
                let mut value = precondition.clone();
                value.resolved_limits.max_runtime_ms += 1;
                value
            },
            {
                let mut value = precondition.clone();
                value.configuration_token = "66".repeat(32);
                value
            },
            {
                let mut value = precondition.clone();
                value.identity_backup.backup_sha256 = Some("77".repeat(32));
                value
            },
        ] {
            assert!(tampered.validate().is_err());
        }

        assert!(
            PublicationIdentityBackup::new(PublicationIdentityBackupState::Current, None).is_err()
        );
        assert!(
            PublicationIdentityBackup::new(
                PublicationIdentityBackupState::Missing,
                Some("88".repeat(32)),
            )
            .is_err()
        );
        assert!(
            PublicationPrecondition::new(
                "11".repeat(32),
                "22".repeat(32),
                limits,
                "33".repeat(32),
                PublicationIdentityBackup::not_required_for_private_local_proof(),
            )
            .is_err(),
            "a public precondition cannot claim the private-local exemption"
        );
    }
}
