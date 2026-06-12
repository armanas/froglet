//! Artifact builders. Phase 1A ships Python `inline_source` only.
//! WASM `inline_module` + OCI builders land in Phase 1B.
//!
//! The builder reads the source code per [`crate::SourceLocator`] and
//! produces a [`BuiltArtifact`] the engine then signs and publishes.

use crate::{SourceLocator, error::PublishError};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct BuiltArtifact {
    /// SHA256 hex of the source bytes. Used as the offer's
    /// `module_hash` field and the offer's content addressing.
    pub source_hash: String,
    /// The source bytes themselves, ready to embed as
    /// `inline_source` in the offer artifact.
    pub source_bytes: Vec<u8>,
    /// Human-friendly filename (e.g., "handler.py"). Used for the
    /// offer's `source_path` field.
    pub source_path: String,
}

/// Build an artifact for a Python `inline_source` service.
///
/// Phase 1A only supports Python text source. WASM and OCI are 1B.
pub async fn build_python_inline(
    locator: &SourceLocator,
    declared_entrypoint: Option<&str>,
) -> Result<BuiltArtifact, PublishError> {
    let (source_bytes, source_path) = match locator {
        SourceLocator::Inline(text) => {
            let filename = declared_entrypoint.unwrap_or("handler.py").to_string();
            (text.as_bytes().to_vec(), filename)
        }
        SourceLocator::File(path) => {
            let bytes = tokio::fs::read(path).await.map_err(|e| {
                PublishError::Build(format!("could not read source file {path:?}: {e}"))
            })?;
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from)
                .unwrap_or_else(|| "handler.py".to_string());
            (bytes, filename)
        }
        SourceLocator::OciImage { .. } => {
            return Err(PublishError::Build(
                "Python inline_source builder cannot consume an OCI image; \
                 use the OCI runtime instead (Phase 1B)"
                    .to_string(),
            ));
        }
    };

    if source_bytes.is_empty() {
        return Err(PublishError::Build("source is empty".to_string()));
    }

    // Basic sanity: must be valid UTF-8 (Python source).
    if std::str::from_utf8(&source_bytes).is_err() {
        return Err(PublishError::Build(
            "Python inline_source must be valid UTF-8".to_string(),
        ));
    }

    let mut hasher = Sha256::new();
    hasher.update(&source_bytes);
    let source_hash = hex::encode(hasher.finalize());

    tracing::debug!(
        bytes = source_bytes.len(),
        hash = %source_hash,
        path = %source_path,
        "built Python inline_source artifact",
    );

    Ok(BuiltArtifact {
        source_hash,
        source_bytes,
        source_path,
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
}
