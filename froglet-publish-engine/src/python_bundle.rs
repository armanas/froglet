//! Resolver-free, locked Python publication bundles.
//!
//! A bundle contains the exact source plus locally supplied pure-Python wheel
//! bytes. The adapter never invokes pip, reads a package cache, or accesses the
//! network. Runtime compatibility is the declared CPython version/ABI, not a
//! build-host executable path or hash.

use crate::{SourceLocator, error::PublishError};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use froglet_protocol::{
    canonical_json, crypto,
    publication::{
        LockedPythonBundleArtifact, LockedPythonBundleEnvelope,
        PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1, PYTHON_BUNDLE_SCHEMA_V1, PYTHON_LOCK_SCHEMA_V1,
        PublicationBuildEvidence, PublicationDependencyComponent, PythonLockManifest,
        PythonLockedArtifact, PythonRuntimeLock, normalize_python_distribution_name,
    },
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::Command,
};
use tokio::io::AsyncReadExt;
use zip::ZipArchive;

pub const LOCKED_PYTHON_BUILDER_VERSION: &str = "1";
pub const LOCKED_PYTHON_WHEEL_PARSER_VERSION: &str = "2.4.2";
pub const LOCKED_PYTHON_WHEEL_PARSER_DIGEST: &str =
    "fabe6324e908f85a1c52063ce7aa26b68dcb7eb6dbc83a2d148403c9bc3eba50";
const MAX_SOURCE_BYTES: usize = 512 * 1024;
const MAX_LOCK_BYTES: usize = 512 * 1024;
const MAX_WHEEL_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOTAL_WHEEL_BYTES: usize = 12 * 1024 * 1024;
const MAX_WHEEL_MEMBERS: usize = 4_096;
const MAX_WHEEL_MEMBER_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TOTAL_UNCOMPRESSED_WHEEL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct BuiltPythonBundle {
    pub package_digest: String,
    pub source_digest: String,
    pub source_path: String,
    pub envelope: LockedPythonBundleEnvelope,
    pub build_evidence: PublicationBuildEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonRuntimeAttestation {
    pub runtime: PythonRuntimeLock,
    pub compatible: bool,
}

/// Private verified bytes ready for one invocation. This type intentionally
/// does not implement `Debug` so wheel/source contents cannot be logged by an
/// adapter accidentally.
pub struct VerifiedPythonBundle {
    pub package_digest: String,
    pub source: Vec<u8>,
    pub runtime: PythonRuntimeLock,
    pub wheels: Vec<VerifiedPythonWheel>,
}

pub struct VerifiedPythonWheel {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// Build a locked Python bundle. With no explicit lock, Froglet generates an
/// exact dependency-free lock for the detected runtime. An explicit lock is
/// resolved relative to the source file's directory and may load wheels only
/// from the lock file's adjacent `artifacts/` directory.
pub async fn build_locked_python_bundle(
    locator: &SourceLocator,
    declared_entrypoint: Option<&str>,
    lock_relative_path: Option<&str>,
) -> Result<BuiltPythonBundle, PublishError> {
    let (source, source_path, source_file) = load_source(locator, declared_entrypoint).await?;
    let runtime = inspect_default_runtime().map_err(PublishError::Build)?;
    build_with_runtime(
        source,
        source_path,
        source_file,
        lock_relative_path,
        runtime,
    )
    .await
}

async fn load_source(
    locator: &SourceLocator,
    declared_entrypoint: Option<&str>,
) -> Result<(Vec<u8>, String, Option<PathBuf>), PublishError> {
    let (source, source_path, source_file) = match locator {
        SourceLocator::Inline(text) => (
            text.as_bytes().to_vec(),
            declared_entrypoint.unwrap_or("handler.py").to_string(),
            None,
        ),
        SourceLocator::File(path) => {
            let bytes = read_file_bounded(path, "Python source", MAX_SOURCE_BYTES).await?;
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("handler.py")
                .to_string();
            (bytes, filename, Some(path.clone()))
        }
        SourceLocator::FileSnapshot { path, bytes } => {
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("handler.py")
                .to_string();
            (bytes.clone(), filename, Some(path.clone()))
        }
        SourceLocator::OciImage { .. } => {
            return Err(PublishError::Build(
                "locked Python bundle cannot consume an OCI image".to_string(),
            ));
        }
    };
    if source.is_empty() {
        return Err(PublishError::Build("Python source is empty".to_string()));
    }
    if source.len() > MAX_SOURCE_BYTES {
        return Err(PublishError::Build(format!(
            "Python source exceeds {MAX_SOURCE_BYTES} bytes"
        )));
    }
    std::str::from_utf8(&source)
        .map_err(|_| PublishError::Build("Python source must be valid UTF-8".to_string()))?;
    Ok((source, source_path, source_file))
}

async fn read_file_bounded(
    path: &Path,
    label: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, PublishError> {
    let file = tokio::fs::File::open(path).await.map_err(|error| {
        PublishError::Build(format!("could not open {label} {path:?}: {error}"))
    })?;
    let mut reader = file.take(max_bytes.saturating_add(1) as u64);
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    reader.read_to_end(&mut bytes).await.map_err(|error| {
        PublishError::Build(format!("could not read {label} {path:?}: {error}"))
    })?;
    if bytes.len() > max_bytes {
        return Err(PublishError::Build(format!(
            "{label} {path:?} exceeds {max_bytes} bytes"
        )));
    }
    Ok(bytes)
}

async fn build_with_runtime(
    source: Vec<u8>,
    source_path: String,
    source_file: Option<PathBuf>,
    lock_relative_path: Option<&str>,
    runtime: PythonRuntimeLock,
) -> Result<BuiltPythonBundle, PublishError> {
    let source_digest = crypto::sha256_hex(&source);
    let (lock, artifacts) = match lock_relative_path {
        Some(relative) => {
            let source_file = source_file.as_ref().ok_or_else(|| {
                PublishError::Build(
                    "an explicit Python lock requires a filesystem source file".to_string(),
                )
            })?;
            let lock_path = resolve_lock_path(source_file, relative)?;
            load_explicit_lock(&lock_path, &source_digest, &runtime).await?
        }
        None => (
            PythonLockManifest {
                schema_version: PYTHON_LOCK_SCHEMA_V1.to_string(),
                source_sha256: source_digest.clone(),
                runtime: runtime.clone(),
                artifacts: Vec::new(),
            },
            Vec::new(),
        ),
    };
    lock.validate().map_err(PublishError::Build)?;
    let lock_sha256 = lock.lock_sha256().map_err(PublishError::Build)?;
    let envelope = LockedPythonBundleEnvelope {
        schema_version: PYTHON_BUNDLE_SCHEMA_V1.to_string(),
        lock: lock.clone(),
        lock_sha256: lock_sha256.clone(),
        source_base64: STANDARD.encode(&source),
        artifacts,
    };
    validate_bundle_contents(&envelope).map_err(PublishError::Build)?;
    let canonical_envelope = canonical_json::to_vec(&envelope)
        .map_err(|error| PublishError::Build(format!("canonicalize Python bundle: {error}")))?;
    let package_digest = crypto::sha256_hex(&canonical_envelope);

    let runtime_digest =
        crypto::sha256_hex(canonical_json::to_vec(&runtime).map_err(|error| {
            PublishError::Build(format!("canonicalize Python runtime: {error}"))
        })?);
    let mut components = vec![
        PublicationDependencyComponent {
            role: "builder".to_string(),
            name: "zip".to_string(),
            version: LOCKED_PYTHON_WHEEL_PARSER_VERSION.to_string(),
            digest: LOCKED_PYTHON_WHEEL_PARSER_DIGEST.to_string(),
        },
        PublicationDependencyComponent {
            role: "lock".to_string(),
            name: "froglet.python-lock".to_string(),
            version: "1".to_string(),
            digest: lock_sha256,
        },
        PublicationDependencyComponent {
            role: "runtime".to_string(),
            name: runtime.implementation.clone(),
            version: format!("{}+{}", runtime.version, runtime.abi),
            digest: runtime_digest,
        },
    ];
    components.extend(
        lock.artifacts
            .iter()
            .map(|artifact| PublicationDependencyComponent {
                role: "dependency".to_string(),
                name: artifact.name.clone(),
                version: artifact.version.clone(),
                digest: artifact.sha256.clone(),
            }),
    );
    components.sort_by(|left, right| (&left.role, &left.name).cmp(&(&right.role, &right.name)));

    Ok(BuiltPythonBundle {
        package_digest: package_digest.clone(),
        source_digest: source_digest.clone(),
        source_path,
        envelope,
        build_evidence: PublicationBuildEvidence {
            schema_version: PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1.to_string(),
            builder: "froglet.locked-python-bundle".to_string(),
            builder_version: LOCKED_PYTHON_BUILDER_VERSION.to_string(),
            source_digest,
            artifact_digest: package_digest,
            dependency_mode: "locked".to_string(),
            components,
            hermetic: true,
        },
    })
}

fn resolve_lock_path(source_file: &Path, relative: &str) -> Result<PathBuf, PublishError> {
    use std::path::Component;

    let relative_path = Path::new(relative);
    if relative.trim().is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(PublishError::Build(
            "Python lock must be a relative path without parent traversal".to_string(),
        ));
    }
    let source_dir = source_file.parent().unwrap_or_else(|| Path::new("."));
    let canonical_source_dir = source_dir.canonicalize().map_err(|error| {
        PublishError::Build(format!(
            "could not resolve Python source directory: {error}"
        ))
    })?;
    let candidate = source_dir.join(relative_path);
    let metadata = std::fs::symlink_metadata(&candidate).map_err(|error| {
        PublishError::Build(format!(
            "could not inspect Python lock {candidate:?}: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PublishError::Build(
            "Python lock must be a regular file, not a symlink".to_string(),
        ));
    }
    let canonical = candidate.canonicalize().map_err(|error| {
        PublishError::Build(format!(
            "could not resolve Python lock {candidate:?}: {error}"
        ))
    })?;
    if !canonical.starts_with(&canonical_source_dir) {
        return Err(PublishError::Build(
            "Python lock resolves outside the source directory".to_string(),
        ));
    }
    Ok(canonical)
}

async fn load_explicit_lock(
    lock_path: &Path,
    source_digest: &str,
    runtime: &PythonRuntimeLock,
) -> Result<(PythonLockManifest, Vec<LockedPythonBundleArtifact>), PublishError> {
    let encoded = read_file_bounded(lock_path, "Python lock", MAX_LOCK_BYTES).await?;
    let lock: PythonLockManifest = serde_json::from_slice(&encoded).map_err(|error| {
        PublishError::Build(format!("invalid Python lock {lock_path:?}: {error}"))
    })?;
    lock.validate().map_err(PublishError::Build)?;
    if lock.source_sha256 != source_digest {
        return Err(PublishError::Build(
            "Python lock source_sha256 does not match source bytes".to_string(),
        ));
    }
    if &lock.runtime != runtime {
        return Err(PublishError::Build(format!(
            "Python lock runtime {:?} does not match local build runtime {:?}",
            lock.runtime, runtime
        )));
    }
    let artifacts_dir = lock_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("artifacts");
    if !lock.artifacts.is_empty() {
        let metadata = std::fs::symlink_metadata(&artifacts_dir).map_err(|error| {
            PublishError::Build(format!(
                "could not inspect Python lock artifact directory {artifacts_dir:?}: {error}"
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(PublishError::Build(
                "Python lock artifacts path must be a real directory, not a symlink".to_string(),
            ));
        }
    }
    let canonical_artifacts_dir = artifacts_dir
        .canonicalize()
        .unwrap_or(artifacts_dir.clone());
    let mut total_bytes = 0usize;
    let mut total_uncompressed = 0u64;
    let mut artifacts = Vec::with_capacity(lock.artifacts.len());
    for locked in &lock.artifacts {
        let path = artifacts_dir.join(&locked.filename);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            PublishError::Build(format!("could not inspect locked wheel {path:?}: {error}"))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PublishError::Build(format!(
                "locked wheel {:?} must be a regular file, not a symlink",
                locked.filename
            )));
        }
        let canonical_path = path.canonicalize().map_err(|error| {
            PublishError::Build(format!("could not resolve locked wheel {path:?}: {error}"))
        })?;
        if !canonical_path.starts_with(&canonical_artifacts_dir) {
            return Err(PublishError::Build(format!(
                "locked wheel {:?} resolves outside the artifacts directory",
                locked.filename
            )));
        }
        let bytes = read_file_bounded(&canonical_path, "locked wheel", MAX_WHEEL_BYTES).await?;
        if bytes.is_empty() || bytes.len() > MAX_WHEEL_BYTES {
            return Err(PublishError::Build(format!(
                "locked wheel {:?} must contain 1-{MAX_WHEEL_BYTES} bytes",
                locked.filename
            )));
        }
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| PublishError::Build("Python wheel byte total overflow".to_string()))?;
        if total_bytes > MAX_TOTAL_WHEEL_BYTES {
            return Err(PublishError::Build(format!(
                "locked wheels exceed {MAX_TOTAL_WHEEL_BYTES} total bytes"
            )));
        }
        let digest = hex::encode(Sha256::digest(&bytes));
        if digest != locked.sha256 {
            return Err(PublishError::Build(format!(
                "locked wheel {:?} sha256 mismatch",
                locked.filename
            )));
        }
        validate_pure_python_wheel(&bytes, locked, runtime, &mut total_uncompressed)
            .map_err(PublishError::Build)?;
        artifacts.push(LockedPythonBundleArtifact {
            locked: locked.clone(),
            content_base64: STANDARD.encode(bytes),
        });
    }
    Ok((lock, artifacts))
}

/// Validate hashes and wheel contents without consulting host state.
pub fn verify_bundle(
    envelope: &LockedPythonBundleEnvelope,
) -> Result<VerifiedPythonBundle, String> {
    envelope.validate_metadata()?;
    let source = STANDARD
        .decode(&envelope.source_base64)
        .map_err(|error| format!("invalid Python bundle source_base64: {error}"))?;
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "Python bundle source must contain 1-{MAX_SOURCE_BYTES} bytes"
        ));
    }
    std::str::from_utf8(&source).map_err(|_| "Python bundle source is not UTF-8".to_string())?;
    if crypto::sha256_hex(&source) != envelope.lock.source_sha256 {
        return Err("Python bundle source hash does not match lock".to_string());
    }
    let mut total_bytes = 0usize;
    let mut total_uncompressed = 0u64;
    let mut wheels = Vec::with_capacity(envelope.artifacts.len());
    for artifact in &envelope.artifacts {
        let bytes = STANDARD
            .decode(&artifact.content_base64)
            .map_err(|error| format!("invalid locked wheel base64: {error}"))?;
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| "Python wheel byte total overflow".to_string())?;
        if bytes.is_empty() || bytes.len() > MAX_WHEEL_BYTES || total_bytes > MAX_TOTAL_WHEEL_BYTES
        {
            return Err("Python bundle wheel byte limit exceeded".to_string());
        }
        if crypto::sha256_hex(&bytes) != artifact.locked.sha256 {
            return Err(format!(
                "Python bundle wheel {:?} hash mismatch",
                artifact.locked.filename
            ));
        }
        validate_pure_python_wheel(
            &bytes,
            &artifact.locked,
            &envelope.lock.runtime,
            &mut total_uncompressed,
        )?;
        wheels.push(VerifiedPythonWheel {
            filename: artifact.locked.filename.clone(),
            bytes,
        });
    }
    let canonical = canonical_json::to_vec(envelope).map_err(|error| error.to_string())?;
    Ok(VerifiedPythonBundle {
        package_digest: crypto::sha256_hex(canonical),
        source,
        runtime: envelope.lock.runtime.clone(),
        wheels,
    })
}

pub fn validate_bundle_contents(envelope: &LockedPythonBundleEnvelope) -> Result<(), String> {
    verify_bundle(envelope).map(|_| ())
}

pub fn package_digest(envelope: &LockedPythonBundleEnvelope) -> Result<String, String> {
    validate_bundle_contents(envelope)?;
    let canonical = canonical_json::to_vec(envelope).map_err(|error| error.to_string())?;
    Ok(crypto::sha256_hex(canonical))
}

pub fn validate_build_evidence(
    envelope: &LockedPythonBundleEnvelope,
    evidence: &PublicationBuildEvidence,
) -> Result<(), String> {
    let verified = verify_bundle(envelope)?;
    let runtime_digest = crypto::sha256_hex(
        canonical_json::to_vec(&envelope.lock.runtime).map_err(|error| error.to_string())?,
    );
    let mut expected_components = vec![
        PublicationDependencyComponent {
            role: "builder".to_string(),
            name: "zip".to_string(),
            version: LOCKED_PYTHON_WHEEL_PARSER_VERSION.to_string(),
            digest: LOCKED_PYTHON_WHEEL_PARSER_DIGEST.to_string(),
        },
        PublicationDependencyComponent {
            role: "lock".to_string(),
            name: "froglet.python-lock".to_string(),
            version: "1".to_string(),
            digest: envelope.lock_sha256.clone(),
        },
        PublicationDependencyComponent {
            role: "runtime".to_string(),
            name: envelope.lock.runtime.implementation.clone(),
            version: format!(
                "{}+{}",
                envelope.lock.runtime.version, envelope.lock.runtime.abi
            ),
            digest: runtime_digest,
        },
    ];
    expected_components.extend(envelope.lock.artifacts.iter().map(|artifact| {
        PublicationDependencyComponent {
            role: "dependency".to_string(),
            name: artifact.name.clone(),
            version: artifact.version.clone(),
            digest: artifact.sha256.clone(),
        }
    }));
    expected_components
        .sort_by(|left, right| (&left.role, &left.name).cmp(&(&right.role, &right.name)));
    evidence.validate()?;
    if evidence.builder != "froglet.locked-python-bundle"
        || evidence.builder_version != LOCKED_PYTHON_BUILDER_VERSION
        || evidence.source_digest != envelope.lock.source_sha256
        || evidence.artifact_digest != verified.package_digest
        || evidence.dependency_mode != "locked"
        || !evidence.hermetic
        || evidence.components != expected_components
    {
        return Err(
            "locked Python build evidence does not exactly match the canonical bundle".to_string(),
        );
    }
    Ok(())
}

pub fn attest_runtime(
    expected: &PythonRuntimeLock,
    executable: &Path,
) -> Result<PythonRuntimeAttestation, String> {
    let runtime = inspect_runtime(executable)?;
    Ok(PythonRuntimeAttestation {
        compatible: &runtime == expected,
        runtime,
    })
}

pub fn resolve_python_executable() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("FROGLET_PYTHON3") {
        return resolve_executable(PathBuf::from(path));
    }
    for candidate in ["/usr/bin/python3", "/usr/local/bin/python3", "/bin/python3"] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return resolve_executable(path);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join("python3");
            if candidate.is_file() {
                return resolve_executable(candidate);
            }
        }
    }
    Err("python3 executable not found for locked Python bundle".to_string())
}

fn resolve_executable(path: PathBuf) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|error| format!("could not resolve Python executable {path:?}: {error}"))
}

fn inspect_default_runtime() -> Result<PythonRuntimeLock, String> {
    inspect_runtime(&resolve_python_executable()?)
}

fn inspect_runtime(executable: &Path) -> Result<PythonRuntimeLock, String> {
    #[derive(Deserialize)]
    struct RuntimeOutput {
        implementation: String,
        version: String,
        abi: String,
    }

    let output = Command::new(executable)
        .arg("-I")
        .arg("-S")
        .arg("-c")
        .arg(
            "import json,sys; print(json.dumps({'implementation':sys.implementation.name,'version':'.'.join(map(str,sys.version_info[:3])),'abi':sys.implementation.cache_tag},separators=(',',':')))",
        )
        .env_clear()
        .env("LANG", "C.UTF-8")
        .output()
        .map_err(|error| format!("failed to inspect Python runtime: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Python runtime inspection failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let parsed: RuntimeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid Python runtime inspection output: {error}"))?;
    let runtime = PythonRuntimeLock {
        implementation: parsed.implementation,
        version: parsed.version,
        abi: parsed.abi,
    };
    PythonLockManifest {
        schema_version: PYTHON_LOCK_SCHEMA_V1.to_string(),
        source_sha256: "0".repeat(64),
        runtime: runtime.clone(),
        artifacts: Vec::new(),
    }
    .validate()?;
    Ok(runtime)
}

fn validate_pure_python_wheel(
    bytes: &[u8],
    locked: &PythonLockedArtifact,
    runtime: &PythonRuntimeLock,
    total_uncompressed: &mut u64,
) -> Result<(), String> {
    let filename = &locked.filename;
    let stem = filename
        .strip_suffix(".whl")
        .ok_or_else(|| format!("wheel {filename:?} has an invalid filename"))?;
    let mut tags = stem.rsplitn(4, '-');
    let platform_tag = tags.next().unwrap_or_default();
    let abi_tag = tags.next().unwrap_or_default();
    let python_tag = tags.next().unwrap_or_default();
    let distribution_and_version = tags.next().unwrap_or_default();
    if platform_tag != "any"
        || abi_tag != "none"
        || python_tag.is_empty()
        || distribution_and_version.is_empty()
        || !python_tag
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '.')
    {
        return Err(format!(
            "wheel {filename:?} is not tagged as a pure-Python <python>-none-any wheel"
        ));
    }
    let runtime_release = parse_numeric_release(&runtime.version)
        .ok_or_else(|| "locked Python runtime version is invalid".to_string())?;
    let exact_cpython_tag = format!("cp{}{}", runtime_release[0], runtime_release[1]);
    if python_tag != "py3" && python_tag != exact_cpython_tag {
        return Err(format!(
            "wheel {filename:?} tag {python_tag:?} is incompatible with locked runtime {} {}",
            runtime.implementation, runtime.version
        ));
    }
    let filename_parts = distribution_and_version.split('-').collect::<Vec<_>>();
    if !matches!(filename_parts.len(), 2 | 3)
        || normalize_python_distribution_name(filename_parts[0])
            != normalize_python_distribution_name(&locked.name)
        || filename_parts[1].to_ascii_lowercase()
            != normalize_wheel_filename_component(&locked.version)
        || filename_parts.get(2).is_some_and(|build| {
            !build.starts_with(|character: char| character.is_ascii_digit())
                || !build.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '.')
                })
        })
    {
        return Err(format!(
            "wheel {filename:?} distribution/version does not match locked {} {}",
            locked.name, locked.version
        ));
    }
    let expected_tag = format!("{python_tag}-none-any");
    let expected_dist_info = format!("{}-{}.dist-info", filename_parts[0], filename_parts[1]);
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("wheel {filename:?} is not a valid ZIP archive: {error}"))?;
    if archive.is_empty() || archive.len() > MAX_WHEEL_MEMBERS {
        return Err(format!(
            "wheel {filename:?} must contain 1-{MAX_WHEEL_MEMBERS} members"
        ));
    }
    let mut paths = HashSet::with_capacity(archive.len());
    let mut wheel_metadata = None;
    let mut package_metadata = None;
    for index in 0..archive.len() {
        let mut member = archive
            .by_index(index)
            .map_err(|error| format!("invalid wheel member: {error}"))?;
        let name = member.name().to_string();
        let declared_size = member.size();
        validate_wheel_member_path(&name)?;
        if !paths.insert(name.clone()) {
            return Err(format!(
                "wheel {filename:?} contains duplicate path {name:?}"
            ));
        }
        if member
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!(
                "wheel {filename:?} contains symlink member {name:?}"
            ));
        }
        if declared_size > MAX_WHEEL_MEMBER_BYTES {
            return Err(format!("wheel member {name:?} exceeds the size limit"));
        }
        *total_uncompressed = total_uncompressed
            .checked_add(declared_size)
            .ok_or_else(|| "wheel uncompressed size overflow".to_string())?;
        if *total_uncompressed > MAX_TOTAL_UNCOMPRESSED_WHEEL_BYTES {
            return Err("locked wheels exceed the global uncompressed size limit".to_string());
        }
        let lower = name.to_ascii_lowercase();
        if name
            .split('/')
            .any(|component| component.to_ascii_lowercase().ends_with(".data"))
        {
            return Err(format!(
                "wheel {filename:?} uses an unsupported .data relocation layout"
            ));
        }
        if [".so", ".pyd", ".dll", ".dylib", ".a", ".exe"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
        {
            return Err(format!(
                "wheel {filename:?} contains native-library member {name:?}"
            ));
        }
        if name.ends_with(".dist-info/WHEEL") {
            if name != format!("{expected_dist_info}/WHEEL") {
                return Err(format!(
                    "wheel {filename:?} WHEEL path does not match its filename"
                ));
            }
            if wheel_metadata.is_some() {
                return Err(format!(
                    "wheel {filename:?} contains multiple WHEEL metadata files"
                ));
            }
            if member.size() > 64 * 1024 {
                return Err(format!("wheel {filename:?} WHEEL metadata is too large"));
            }
            let mut metadata = String::new();
            member
                .read_to_string(&mut metadata)
                .map_err(|error| format!("could not read WHEEL metadata: {error}"))?;
            wheel_metadata = Some(metadata);
        } else if name.ends_with(".dist-info/METADATA") {
            if name != format!("{expected_dist_info}/METADATA") {
                return Err(format!(
                    "wheel {filename:?} METADATA path does not match its filename"
                ));
            }
            if package_metadata.is_some() {
                return Err(format!(
                    "wheel {filename:?} contains multiple METADATA files"
                ));
            }
            if member.size() > 256 * 1024 {
                return Err(format!("wheel {filename:?} METADATA is too large"));
            }
            let mut metadata = String::new();
            member
                .read_to_string(&mut metadata)
                .map_err(|error| format!("could not read package METADATA: {error}"))?;
            package_metadata = Some(metadata);
        } else {
            // Reading every member to EOF verifies the compressed stream and
            // CRC rather than trusting only central-directory sizes. Discard
            // bytes instead of extracting them; execution imports the exact
            // verified wheel archive directly.
            let read = std::io::copy(&mut member, &mut std::io::sink())
                .map_err(|error| format!("could not verify wheel member {name:?}: {error}"))?;
            if read != declared_size {
                return Err(format!(
                    "wheel member {name:?} size does not match its ZIP metadata"
                ));
            }
        }
    }
    let metadata = wheel_metadata
        .ok_or_else(|| format!("wheel {filename:?} has no .dist-info/WHEEL metadata"))?;
    let purelib = metadata.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("Root-Is-Purelib")
            .then(|| value.trim())
    });
    if !purelib.is_some_and(|value| value.eq_ignore_ascii_case("true")) {
        return Err(format!(
            "wheel {filename:?} must declare Root-Is-Purelib: true"
        ));
    }
    let tags = metadata.lines().filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("Tag").then(|| value.trim())
    });
    if !tags.into_iter().any(|tag| tag == expected_tag) {
        return Err(format!(
            "wheel {filename:?} WHEEL metadata does not declare filename tag {expected_tag:?}"
        ));
    }
    let package_metadata =
        package_metadata.ok_or_else(|| format!("wheel {filename:?} has no .dist-info/METADATA"))?;
    let metadata_name = metadata_header(&package_metadata, "Name")
        .ok_or_else(|| format!("wheel {filename:?} METADATA has no Name"))?;
    let metadata_version = metadata_header(&package_metadata, "Version")
        .ok_or_else(|| format!("wheel {filename:?} METADATA has no Version"))?;
    if normalize_python_distribution_name(metadata_name)
        != normalize_python_distribution_name(&locked.name)
    {
        return Err(format!(
            "wheel {filename:?} METADATA Name does not match lock"
        ));
    }
    if metadata_version != locked.version {
        return Err(format!(
            "wheel {filename:?} METADATA Version does not match lock"
        ));
    }
    if let Some(requires_python) = metadata_header(&package_metadata, "Requires-Python")
        && !requires_python_allows(requires_python, &runtime.version)?
    {
        return Err(format!(
            "wheel {filename:?} Requires-Python {requires_python:?} excludes locked runtime {}",
            runtime.version
        ));
    }
    if package_metadata.lines().any(|line| {
        line.split_once(':')
            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("Requires-Dist"))
    }) {
        return Err(format!(
            "wheel {filename:?} declares Requires-Dist; dependency-bearing wheels are unsupported by the v1 resolver-free adapter"
        ));
    }
    Ok(())
}

fn metadata_header<'a>(metadata: &'a str, expected: &str) -> Option<&'a str> {
    metadata.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(expected).then(|| value.trim())
    })
}

fn normalize_wheel_filename_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() || character == '.' {
            if separator && !output.is_empty() {
                output.push('_');
            }
            separator = false;
            output.push(character);
        } else {
            separator = true;
        }
    }
    output
}

fn parse_numeric_release(value: &str) -> Option<Vec<u64>> {
    let release = value
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!release.is_empty()).then_some(release)
}

fn compare_releases(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let width = left.len().max(right.len());
    (0..width)
        .map(|index| {
            left.get(index)
                .copied()
                .unwrap_or(0)
                .cmp(&right.get(index).copied().unwrap_or(0))
        })
        .find(|ordering| !ordering.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn requires_python_allows(specifier: &str, runtime: &str) -> Result<bool, String> {
    use std::cmp::Ordering;

    let observed = parse_numeric_release(runtime)
        .ok_or_else(|| "locked Python runtime version is invalid".to_string())?;
    for raw_constraint in specifier.split(',') {
        let constraint = raw_constraint.trim();
        let (operator, expected_text) = [">=", "<=", "==", "!=", "~=", ">", "<"]
            .into_iter()
            .find_map(|operator| {
                constraint
                    .strip_prefix(operator)
                    .map(|rest| (operator, rest))
            })
            .ok_or_else(|| format!("unsupported Requires-Python constraint {constraint:?}"))?;
        let wildcard = expected_text.strip_suffix(".*");
        let expected = parse_numeric_release(wildcard.unwrap_or(expected_text.trim()))
            .ok_or_else(|| format!("unsupported Requires-Python version {expected_text:?}"))?;
        let ordering = compare_releases(&observed, &expected);
        let matches = match operator {
            ">=" => matches!(ordering, Ordering::Equal | Ordering::Greater),
            "<=" => matches!(ordering, Ordering::Equal | Ordering::Less),
            ">" => ordering == Ordering::Greater,
            "<" => ordering == Ordering::Less,
            "==" if wildcard.is_some() => observed.starts_with(&expected),
            "!=" if wildcard.is_some() => !observed.starts_with(&expected),
            "==" => ordering == Ordering::Equal,
            "!=" => ordering != Ordering::Equal,
            "~=" => {
                if expected.len() < 2 {
                    return Err(format!(
                        "compatible Requires-Python constraint {constraint:?} needs at least two release components"
                    ));
                }
                let prefix = &expected[..expected.len() - 1];
                matches!(ordering, Ordering::Equal | Ordering::Greater)
                    && observed.starts_with(prefix)
            }
            _ => unreachable!("operator allowlist"),
        };
        if !matches {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_wheel_member_path(name: &str) -> Result<(), String> {
    let normalized = name.trim_end_matches('/');
    if normalized.is_empty()
        || normalized.starts_with('/')
        || name.contains('\\')
        || name.contains('\0')
        || normalized
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(format!("wheel contains unsafe member path {name:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    fn runtime() -> PythonRuntimeLock {
        PythonRuntimeLock {
            implementation: "cpython".to_string(),
            version: "3.12.4".to_string(),
            abi: "cpython-312".to_string(),
        }
    }

    fn wheel(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut output);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            for (name, bytes) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn pure_wheel() -> Vec<u8> {
        wheel(&[
            ("demo/__init__.py", b"VALUE = 7\n"),
            (
                "demo-1.0.dist-info/METADATA",
                b"Metadata-Version: 2.1\nName: demo\nVersion: 1.0\n",
            ),
            (
                "demo-1.0.dist-info/WHEEL",
                b"Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
            ),
        ])
    }

    #[tokio::test]
    async fn dependency_free_bundle_is_deterministic_and_locked() {
        let source = b"def handler(event, context): return event\n".to_vec();
        let first = build_with_runtime(
            source.clone(),
            "handler.py".to_string(),
            None,
            None,
            runtime(),
        )
        .await
        .unwrap();
        let second = build_with_runtime(source, "handler.py".to_string(), None, None, runtime())
            .await
            .unwrap();
        assert_eq!(first.package_digest, second.package_digest);
        assert!(first.envelope.artifacts.is_empty());
        assert!(first.build_evidence.hermetic);
        assert_eq!(first.build_evidence.dependency_mode, "locked");
        assert_eq!(
            package_digest(&first.envelope).unwrap(),
            first.package_digest
        );
    }

    #[test]
    fn accepts_pure_python_wheel() {
        let locked = PythonLockedArtifact {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            filename: "demo-1.0-py3-none-any.whl".to_string(),
            sha256: "0".repeat(64),
        };
        let mut total = 0;
        validate_pure_python_wheel(&pure_wheel(), &locked, &runtime(), &mut total).unwrap();
    }

    #[test]
    fn rejects_native_and_traversal_wheels() {
        let native = wheel(&[
            ("demo/native.so", b"native"),
            ("demo-1.0.dist-info/METADATA", b"Name: demo\nVersion: 1.0\n"),
            (
                "demo-1.0.dist-info/WHEEL",
                b"Root-Is-Purelib: true\nTag: py3-none-any\n",
            ),
        ]);
        let locked = PythonLockedArtifact {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            filename: "demo-1.0-py3-none-any.whl".to_string(),
            sha256: "0".repeat(64),
        };
        let mut native_total = 0;
        assert!(
            validate_pure_python_wheel(&native, &locked, &runtime(), &mut native_total)
                .unwrap_err()
                .contains("native-library")
        );
        let traversal = wheel(&[
            ("../owned.py", b"x"),
            ("demo-1.0.dist-info/METADATA", b"Name: demo\nVersion: 1.0\n"),
            (
                "demo-1.0.dist-info/WHEEL",
                b"Root-Is-Purelib: true\nTag: py3-none-any\n",
            ),
        ]);
        let mut traversal_total = 0;
        assert!(
            validate_pure_python_wheel(&traversal, &locked, &runtime(), &mut traversal_total)
                .unwrap_err()
                .contains("unsafe")
        );
    }

    #[tokio::test]
    async fn explicit_lock_reproduces_from_only_local_pinned_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("handler.py");
        let lock_path = directory.path().join("froglet-python.lock.json");
        let artifacts_dir = directory.path().join("artifacts");
        std::fs::create_dir(&artifacts_dir).unwrap();
        let source = b"from demo import VALUE\ndef handler(event, context): return VALUE\n";
        std::fs::write(&source_path, source).unwrap();
        let wheel = pure_wheel();
        let filename = "demo-1.0-py3-none-any.whl";
        std::fs::write(artifacts_dir.join(filename), &wheel).unwrap();
        let lock = PythonLockManifest {
            schema_version: PYTHON_LOCK_SCHEMA_V1.to_string(),
            source_sha256: crypto::sha256_hex(source),
            runtime: runtime(),
            artifacts: vec![PythonLockedArtifact {
                name: "demo".to_string(),
                version: "1.0".to_string(),
                filename: filename.to_string(),
                sha256: crypto::sha256_hex(&wheel),
            }],
        };
        std::fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();

        let built = build_with_runtime(
            source.to_vec(),
            "handler.py".to_string(),
            Some(source_path),
            Some("froglet-python.lock.json"),
            runtime(),
        )
        .await
        .unwrap();

        let verified = verify_bundle(&built.envelope).unwrap();
        assert_eq!(verified.source, source);
        assert_eq!(verified.wheels.len(), 1);
        assert_eq!(verified.wheels[0].bytes, wheel);
        assert_eq!(verified.package_digest, built.package_digest);
        assert!(built.build_evidence.hermetic);
        assert_eq!(built.build_evidence.components.len(), 4);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn explicit_lock_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("handler.py");
        let lock_target = outside.path().join("lock.json");
        std::fs::write(&source_path, b"x = 1\n").unwrap();
        std::fs::write(&lock_target, b"{}").unwrap();
        symlink(&lock_target, directory.path().join("lock.json")).unwrap();

        let error = resolve_lock_path(&source_path, "lock.json").unwrap_err();
        assert!(error.to_string().contains("not a symlink"));
    }
}
