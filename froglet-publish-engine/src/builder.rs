//! Dependency-minimal artifact builders for inline Python, native data, and
//! version-pinned WAT/Wasm publication. OCI packaging remains an adapter seam.
//!
//! The builder reads the source code per [`crate::SourceLocator`] and
//! produces a [`BuiltArtifact`] the engine then signs and publishes.

use crate::{SourceLocator, error::PublishError};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use froglet_protocol::publication::{
    LockedPythonBundleEnvelope, PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1, PublicationBuildEvidence,
    PublicationCsvSchema, PublicationDataFormat, PublicationDataSource,
    PublicationDependencyComponent,
};
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_NATIVE_DATA_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_WAT_SOURCE_BYTES: usize = 512 * 1024;
const MAX_WASM_MODULE_BYTES: usize = 256 * 1024;
pub const EMBEDDED_WAT_BUILDER_VERSION: &str = "1.245.1";
pub const EMBEDDED_WAT_BUILDER_DIGEST: &str =
    "cd48d1679b6858988cb96b154dda0ec5bbb09275b71db46057be37332d5477be";

#[derive(Debug, Clone)]
pub struct BuiltArtifact {
    /// SHA256 hex of the executable package. For locked Python this is the
    /// canonical bundle-envelope digest, not the generated loader bytes.
    pub source_hash: String,
    /// The source bytes themselves, ready to embed as
    /// `inline_source` in the offer artifact.
    pub source_bytes: Vec<u8>,
    /// Human-friendly filename (e.g., "handler.py"). Used for the
    /// offer's `source_path` field.
    pub source_path: String,
    /// Immutable evidence carried into the provider-signed Publication
    /// Revision. This is deliberately outside the Kernel artifact format.
    pub build_evidence: PublicationBuildEvidence,
    /// Present only for locked Python. The provider validates this canonical
    /// envelope and generates the fixed loader locally.
    pub python_bundle: Option<LockedPythonBundleEnvelope>,
}

#[derive(Debug, Clone)]
pub struct BuiltDataSource {
    pub data_source: PublicationDataSource,
    pub package_digest: String,
    pub build_evidence: PublicationBuildEvidence,
}

fn build_evidence(
    builder: &str,
    builder_version: &str,
    source_digest: String,
    artifact_digest: String,
    dependency_mode: &str,
    components: Vec<PublicationDependencyComponent>,
    hermetic: bool,
) -> PublicationBuildEvidence {
    PublicationBuildEvidence {
        schema_version: PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1.to_string(),
        builder: builder.to_string(),
        builder_version: builder_version.to_string(),
        source_digest,
        artifact_digest,
        dependency_mode: dependency_mode.to_string(),
        components,
        hermetic,
    }
}

/// Build an automatically locked, dependency-free Python bundle.
///
/// This compatibility wrapper preserves the existing builder interface. New
/// manifest-aware callers should use [`build_python_locked`] so an explicit
/// local wheel lock can be supplied.
pub async fn build_python_inline(
    locator: &SourceLocator,
    declared_entrypoint: Option<&str>,
) -> Result<BuiltArtifact, PublishError> {
    build_python_locked(locator, declared_entrypoint, None).await
}

pub async fn build_python_locked(
    locator: &SourceLocator,
    declared_entrypoint: Option<&str>,
    lock_relative_path: Option<&str>,
) -> Result<BuiltArtifact, PublishError> {
    let built = crate::python_bundle::build_locked_python_bundle(
        locator,
        declared_entrypoint,
        lock_relative_path,
    )
    .await?;
    let source_bytes = base64::engine::general_purpose::STANDARD
        .decode(&built.envelope.source_base64)
        .map_err(|error| PublishError::Build(format!("decode locked Python source: {error}")))?;
    Ok(BuiltArtifact {
        source_hash: built.package_digest,
        source_bytes,
        source_path: built.source_path,
        build_evidence: built.build_evidence,
        python_bundle: Some(built.envelope),
    })
}

/// Build a pure `froglet.wasm.run_json.v1` inline module without invoking an
/// external compiler. `.wasm` files are snapshotted as-is; `.wat` files and
/// inline text are compiled by the exact `wat` crate embedded in the released
/// Froglet binary.
pub async fn build_wasm_inline(locator: &SourceLocator) -> Result<BuiltArtifact, PublishError> {
    let (source_bytes, source_path, compile_wat) = match locator {
        SourceLocator::Inline(text) => (text.as_bytes().to_vec(), "module.wat".to_string(), true),
        SourceLocator::File(path) => {
            let bytes = tokio::fs::read(path).await.map_err(|error| {
                PublishError::Build(format!("could not read Wasm source {path:?}: {error}"))
            })?;
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .ok_or_else(|| {
                    PublishError::Build(
                        "Wasm inline_module source must end in .wat or .wasm".to_string(),
                    )
                })?;
            if !matches!(extension.as_str(), "wat" | "wasm") {
                return Err(PublishError::Build(
                    "Wasm inline_module source must end in .wat or .wasm".to_string(),
                ));
            }
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(if extension == "wat" {
                    "module.wat"
                } else {
                    "module.wasm"
                })
                .to_string();
            (bytes, file_name, extension == "wat")
        }
        SourceLocator::FileSnapshot { path, bytes } => {
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .ok_or_else(|| {
                    PublishError::Build(
                        "Wasm inline_module source must end in .wat or .wasm".to_string(),
                    )
                })?;
            if !matches!(extension.as_str(), "wat" | "wasm") {
                return Err(PublishError::Build(
                    "Wasm inline_module source must end in .wat or .wasm".to_string(),
                ));
            }
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(if extension == "wat" {
                    "module.wat"
                } else {
                    "module.wasm"
                })
                .to_string();
            (bytes.clone(), file_name, extension == "wat")
        }
        SourceLocator::OciImage { .. } => {
            return Err(PublishError::Build(
                "Wasm inline_module builder cannot consume an OCI image".to_string(),
            ));
        }
    };
    if source_bytes.is_empty() {
        return Err(PublishError::Build("Wasm source is empty".to_string()));
    }
    if compile_wat && source_bytes.len() > MAX_WAT_SOURCE_BYTES {
        return Err(PublishError::Build(format!(
            "WAT source exceeds {MAX_WAT_SOURCE_BYTES} byte limit"
        )));
    }

    let source_hash = hex::encode(Sha256::digest(&source_bytes));
    let module_bytes = if compile_wat {
        wat::parse_bytes(&source_bytes)
            .map_err(|error| PublishError::Build(format!("invalid WAT source: {error}")))?
            .into_owned()
    } else {
        source_bytes.clone()
    };
    if module_bytes.len() > MAX_WASM_MODULE_BYTES {
        return Err(PublishError::Build(format!(
            "Wasm module is {} bytes and exceeds the {MAX_WASM_MODULE_BYTES} byte inline limit",
            module_bytes.len()
        )));
    }
    if !module_bytes.starts_with(b"\0asm\x01\0\0\0") {
        return Err(PublishError::Build(
            "Wasm module does not have the supported version-1 binary header".to_string(),
        ));
    }
    let module_hash = hex::encode(Sha256::digest(&module_bytes));
    let (dependency_mode, components, hermetic, builder, builder_version) = if compile_wat {
        (
            "locked",
            vec![PublicationDependencyComponent {
                role: "builder".to_string(),
                name: "wat".to_string(),
                version: EMBEDDED_WAT_BUILDER_VERSION.to_string(),
                digest: EMBEDDED_WAT_BUILDER_DIGEST.to_string(),
            }],
            true,
            "froglet.embedded-wat",
            EMBEDDED_WAT_BUILDER_VERSION,
        )
    } else {
        (
            "none",
            Vec::new(),
            true,
            "froglet.prebuilt-wasm-snapshot",
            "1",
        )
    };
    let output_path = Path::new(&source_path)
        .with_extension("wasm")
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("module.wasm")
        .to_string();

    Ok(BuiltArtifact {
        source_hash: module_hash.clone(),
        source_bytes: module_bytes,
        source_path: output_path,
        build_evidence: build_evidence(
            builder,
            builder_version,
            source_hash,
            module_hash,
            dependency_mode,
            components,
            hermetic,
        ),
        python_bundle: None,
    })
}

/// Read and encode an immutable native data-query snapshot.
///
/// Binary SQLite sources must use [`SourceLocator::File`]. JSON may be passed
/// inline by MCP adapters, but the CLI also uses a file so both paths bind the
/// exact same bytes. The daemon independently decodes, hashes, and validates
/// the snapshot before persisting any offer.
pub async fn build_data_source(
    locator: &SourceLocator,
    format: PublicationDataFormat,
    csv_schema: Option<PublicationCsvSchema>,
) -> Result<BuiltDataSource, PublishError> {
    let bytes = match locator {
        SourceLocator::Inline(text)
            if matches!(
                format,
                PublicationDataFormat::Json | PublicationDataFormat::Csv
            ) =>
        {
            text.as_bytes().to_vec()
        }
        SourceLocator::Inline(_) => {
            return Err(PublishError::Build(
                "SQLite data sources must be read from a file; inline text is only valid for JSON/CSV"
                    .to_string(),
            ));
        }
        SourceLocator::File(path) => tokio::fs::read(path).await.map_err(|error| {
            PublishError::Build(format!("could not read data source {path:?}: {error}"))
        })?,
        SourceLocator::FileSnapshot { bytes, .. } => bytes.clone(),
        SourceLocator::OciImage { .. } => {
            return Err(PublishError::Build(
                "native data-query sources cannot come from an OCI image".to_string(),
            ));
        }
    };
    if bytes.is_empty() {
        return Err(PublishError::Build("data source is empty".to_string()));
    }
    if bytes.len() > MAX_NATIVE_DATA_SOURCE_BYTES {
        return Err(PublishError::Build(format!(
            "data source is {} bytes and exceeds the native publication limit of {MAX_NATIVE_DATA_SOURCE_BYTES} bytes",
            bytes.len()
        )));
    }
    match (&format, &csv_schema) {
        (PublicationDataFormat::Csv, Some(schema)) => schema.validate().map_err(|error| {
            PublishError::Build(format!("invalid explicit CSV schema: {error}"))
        })?,
        (PublicationDataFormat::Csv, None) => {
            return Err(PublishError::Build(
                "CSV data publication requires an explicit schema".to_string(),
            ));
        }
        (PublicationDataFormat::Json | PublicationDataFormat::Sqlite, Some(_)) => {
            return Err(PublishError::Build(
                "CSV schema is not valid for JSON or SQLite data".to_string(),
            ));
        }
        (PublicationDataFormat::Json | PublicationDataFormat::Sqlite, None) => {}
    }
    let source_hash = hex::encode(Sha256::digest(&bytes));
    let data_source = PublicationDataSource {
        format,
        content_base64: STANDARD.encode(&bytes),
        csv_schema,
    };
    let package_digest = if format == PublicationDataFormat::Csv {
        data_source
            .csv_package_digest()
            .map_err(PublishError::Build)?
    } else {
        source_hash.clone()
    };
    Ok(BuiltDataSource {
        data_source,
        package_digest: package_digest.clone(),
        build_evidence: build_evidence(
            "froglet.native-data-snapshot",
            "1",
            source_hash,
            package_digest,
            "none",
            Vec::new(),
            true,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inline_source_builds_with_hash() {
        let locator = SourceLocator::Inline("def handler(x): return x\n".to_string());
        let artifact = build_python_inline(&locator, None).await.unwrap();
        assert_eq!(artifact.source_path, "handler.py");
        assert_eq!(artifact.source_bytes.len(), 25);
        // Hash is deterministic.
        let again = build_python_inline(&locator, None).await.unwrap();
        assert_eq!(artifact.source_hash, again.source_hash);
    }

    #[tokio::test]
    async fn inline_source_uses_declared_entrypoint() {
        let locator = SourceLocator::Inline("x = 1\n".to_string());
        let artifact = build_python_inline(&locator, Some("main.py"))
            .await
            .unwrap();
        assert_eq!(artifact.source_path, "main.py");
    }

    #[tokio::test]
    async fn rejects_empty_source() {
        let locator = SourceLocator::Inline(String::new());
        let err = build_python_inline(&locator, None).await.unwrap_err();
        assert!(matches!(err, PublishError::Build(_)));
    }

    #[tokio::test]
    async fn rejects_oci_locator() {
        let locator = SourceLocator::OciImage {
            reference: "ghcr.io/x/y:1".to_string(),
            digest: "sha256:abc".to_string(),
        };
        let err = build_python_inline(&locator, None).await.unwrap_err();
        assert!(matches!(err, PublishError::Build(_)));
    }

    #[tokio::test]
    async fn data_source_binds_exact_bytes() {
        let source = SourceLocator::Inline("[{\"id\":1}]".to_string());
        let built = build_data_source(&source, PublicationDataFormat::Json, None)
            .await
            .expect("data source");
        assert_eq!(
            STANDARD.decode(built.data_source.content_base64).unwrap(),
            b"[{\"id\":1}]"
        );
        assert_eq!(
            built.package_digest,
            hex::encode(Sha256::digest(b"[{\"id\":1}]"))
        );
    }

    #[tokio::test]
    async fn rejects_binary_sqlite_as_inline_text() {
        let error = build_data_source(
            &SourceLocator::Inline("not a database".to_string()),
            PublicationDataFormat::Sqlite,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, PublishError::Build(_)));
    }

    #[tokio::test]
    async fn embedded_wat_builder_is_hermetic_and_digest_pinned() {
        let artifact = build_wasm_inline(&SourceLocator::Inline(
            r#"(module
                (memory (export "memory") 1)
                (data (i32.const 0) "42")
                (func (export "alloc") (param i32) (result i32) i32.const 16)
                (func (export "run") (param i32 i32) (result i64) i64.const 2)
            )"#
            .to_string(),
        ))
        .await
        .expect("compile embedded WAT");

        assert!(artifact.source_bytes.starts_with(b"\0asm\x01\0\0\0"));
        assert!(artifact.build_evidence.hermetic);
        assert_eq!(artifact.build_evidence.dependency_mode, "locked");
        assert_eq!(artifact.build_evidence.components.len(), 1);
        assert_eq!(
            artifact.build_evidence.components[0].digest,
            EMBEDDED_WAT_BUILDER_DIGEST
        );
        assert_eq!(
            artifact.build_evidence.artifact_digest,
            artifact.source_hash
        );
    }

    #[tokio::test]
    async fn csv_package_digest_binds_explicit_schema() {
        use froglet_protocol::publication::{PublicationCsvColumn, PublicationCsvColumnType};

        let source = SourceLocator::Inline("id,name\n1,Ada\n".to_string());
        let schema = PublicationCsvSchema {
            collection: "people".to_string(),
            columns: vec![
                PublicationCsvColumn {
                    name: "id".to_string(),
                    column_type: PublicationCsvColumnType::Integer,
                    nullable: false,
                    indexed: true,
                },
                PublicationCsvColumn {
                    name: "name".to_string(),
                    column_type: PublicationCsvColumnType::String,
                    nullable: false,
                    indexed: false,
                },
            ],
        };
        let first = build_data_source(&source, PublicationDataFormat::Csv, Some(schema.clone()))
            .await
            .unwrap();
        let mut changed = schema;
        changed.columns[1].nullable = true;
        let second = build_data_source(&source, PublicationDataFormat::Csv, Some(changed))
            .await
            .unwrap();

        assert_ne!(first.package_digest, second.package_digest);
        assert_eq!(first.build_evidence.artifact_digest, first.package_digest);
    }
}
