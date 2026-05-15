//! Build → host → sign → register pipeline for publishing Froglet
//! services. The engine takes a parsed manifest + source code and turns
//! it into a live marketplace offer in one call.
//!
//! See `docs/MANIFEST.md` for the contract this engine consumes, and
//! the plan file at
//! `/Users/armanas/.claude/plans/review-codebase-and-froglet-services-lexical-scott.md`
//! for the design rationale.
//!
//! ## Surfaces
//!
//! - **CLI**: `froglet-node publish` (Phase 2) calls into [`publish()`]
//!   with manifests read from disk.
//! - **MCP**: `marketplace_publish` MCP tool (Phase 3) calls
//!   [`publish()`] with manifests reconstructed from structured input.
//!
//! Both surfaces wrap the same entry point so there is one source of
//! truth for the publish flow.
//!
//! ## Phase 1A scope
//!
//! - Backends: [`HostingChoice::Local`], [`HostingChoice::Tor`],
//!   [`HostingChoice::SelfHosted`].
//! - Runtime: Python `inline_source` only. WASM + OCI builders ship
//!   in Phase 1B alongside Managed + Fly backends.

pub mod backends;
pub mod builder;
pub mod error;
pub mod registration;

pub use error::{PublishError, PublishWarning};

use froglet_protocol::manifest::{ProjectManifest, ServiceManifest};
use std::path::PathBuf;
use url::Url;

/// Input to [`publish()`].
#[derive(Debug, Clone)]
pub struct PublishInput {
    /// Project-level manifest (`froglet.toml`). Optional — when absent,
    /// engine defaults apply.
    pub project: Option<ProjectManifest>,
    /// Per-service manifest (`froglet-service.toml` v3).
    pub service: ServiceManifest,
    /// Where the service source code lives.
    pub source: SourceLocator,
    /// Override the manifest's hosting choice. Used by CLI/MCP flags
    /// like `--host tor` that ignore the manifest's `[hosting] default`.
    pub hosting_override: Option<HostingChoice>,
    /// Marketplace URL to register against. Overrides any value in the
    /// manifests.
    pub marketplace_url: Url,
}

/// Where the service source code can be loaded from.
#[derive(Debug, Clone)]
pub enum SourceLocator {
    /// Inline source text, e.g., the Python script body.
    Inline(String),
    /// Filesystem path to the source file. Engine reads + hashes it.
    File(PathBuf),
    /// OCI image reference + digest. Engine does not pull; it trusts
    /// the digest. Phase 1B + OCI runtimes only.
    OciImage { reference: String, digest: String },
}

/// Hosting backend selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostingChoice {
    /// Private development; binds 127.0.0.1, does NOT register with
    /// marketplace.
    Local,
    /// Tor v3 hidden service spawned in-process by the engine. Reuses
    /// `froglet::tor::start_hidden_service()`.
    Tor,
    /// User-supplied public URL. Engine validates reachability + asks
    /// marketplace to register.
    SelfHosted { url: Url },
    /// Managed `<slug>.providers.<suffix>` (Phase 1B).
    #[allow(dead_code)] // Phase 1B
    Managed { slug: Option<String> },
    /// Deploy to Fly.io (Phase 1B).
    #[allow(dead_code)] // Phase 1B
    Fly { app: String, region: String },
}

/// Successful publish result. The caller uses these fields to tell the
/// user where the service is live and how to invoke it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PublishOutput {
    /// The provider's signing-key pubkey (64-hex BIP340).
    pub provider_id: String,
    /// The public URL the service is reachable at. For `Local` this is
    /// `http://127.0.0.1:<port>`; for `Tor` it's the `.onion` URL; for
    /// `SelfHosted` it's the user-supplied URL.
    pub public_url: String,
    /// SHA256 hash of the published offer artifact.
    pub offer_hash: String,
    /// Marketplace URL where the offer is queryable. `None` for
    /// `Local` (no marketplace registration).
    pub marketplace_offer_url: Option<String>,
    /// Shell-friendly command that demonstrates invoking the service.
    pub invoke_command: String,
    /// Status URL the caller can poll for indexer eventual-consistency
    /// updates. `None` for `Local`.
    pub status_url: Option<String>,
    /// Soft warnings from manifest validation or backend choice.
    pub warnings: Vec<PublishWarning>,
}

/// Run the full build → host → sign → register pipeline.
///
/// Phase 1A.1 stub. Subsequent sub-phases (1A.2 .. 1A.7) flesh out the
/// pipeline. Returns [`PublishError::NotImplemented`] in Phase 1A.1.
pub async fn publish(input: PublishInput) -> Result<PublishOutput, PublishError> {
    // Resolve the effective hosting choice: override > manifest > project default > "local".
    let hosting = resolve_hosting_choice(&input)?;

    tracing::info!(
        service_id = %input.service.service_id,
        runtime = %input.service.runtime,
        hosting = ?hosting,
        marketplace = %input.marketplace_url,
        "froglet-publish-engine: pipeline start",
    );

    // Phase 1A.1: skeleton only. Real backends land in 1A.2 .. 1A.7.
    Err(PublishError::NotImplemented {
        what: format!("backend dispatch for {:?}", hosting),
    })
}

fn resolve_hosting_choice(input: &PublishInput) -> Result<HostingChoice, PublishError> {
    // Override wins.
    if let Some(choice) = &input.hosting_override {
        return Ok(choice.clone());
    }
    // Then the service manifest's [hosting] default.
    if let Some(hosting) = &input.service.hosting {
        return parse_hosting_default(&hosting.default, input);
    }
    // Then the project manifest's [project.defaults] hosting.
    if let Some(project) = &input.project
        && let Some(defaults) = &project.project.defaults
        && let Some(default) = &defaults.hosting
    {
        return parse_hosting_default(default, input);
    }
    // Final fallback: local (private).
    Ok(HostingChoice::Local)
}

fn parse_hosting_default(name: &str, input: &PublishInput) -> Result<HostingChoice, PublishError> {
    match name {
        "local" => Ok(HostingChoice::Local),
        "tor" => Ok(HostingChoice::Tor),
        "self" => {
            // Pull the URL from the service manifest's [hosting.self] section.
            let url_str = input
                .service
                .hosting
                .as_ref()
                .and_then(|h| h.self_hosted.as_ref())
                .map(|s| s.url.as_str())
                .ok_or_else(|| PublishError::InvalidInput {
                    field: "hosting.self.url",
                    reason: "self-hosted backend requires hosting.self.url".to_string(),
                })?;
            let url = Url::parse(url_str).map_err(|e| PublishError::InvalidInput {
                field: "hosting.self.url",
                reason: format!("not a valid URL: {e}"),
            })?;
            Ok(HostingChoice::SelfHosted { url })
        }
        "managed" => Err(PublishError::NotImplemented {
            what: "Managed backend (Phase 1B)".to_string(),
        }),
        "fly" => Err(PublishError::NotImplemented {
            what: "Fly backend (Phase 1B)".to_string(),
        }),
        other => Err(PublishError::InvalidInput {
            field: "hosting.default",
            reason: format!("unknown hosting backend {other:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use froglet_protocol::manifest::{
        HostingSection, SelfHostingConfig, ServiceManifest, SettlementSection,
    };

    fn minimal_service_manifest(hosting_default: &str) -> ServiceManifest {
        let toml = format!(
            r#"
            schema_version = "froglet-service/v3"
            service_id = "test-service"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "{}"
            [settlement]
            method = "none"
            "#,
            hosting_default
        );
        ServiceManifest::from_toml(&toml).unwrap().0
    }

    #[tokio::test]
    async fn publish_returns_not_implemented_in_phase_1a1() {
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("local"),
            source: SourceLocator::Inline("print('hi')".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
        };
        let err = publish(input).await.unwrap_err();
        assert!(matches!(err, PublishError::NotImplemented { .. }));
    }

    #[test]
    fn resolves_hosting_from_manifest_default() {
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("tor"),
            source: SourceLocator::Inline("x".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
        };
        assert_eq!(resolve_hosting_choice(&input).unwrap(), HostingChoice::Tor);
    }

    #[test]
    fn resolves_hosting_override_wins() {
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("tor"),
            source: SourceLocator::Inline("x".to_string()),
            hosting_override: Some(HostingChoice::Local),
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
        };
        assert_eq!(
            resolve_hosting_choice(&input).unwrap(),
            HostingChoice::Local
        );
    }

    #[test]
    fn resolves_self_hosting_pulls_url_from_manifest() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "test-service"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "self"
            [hosting.self]
            url = "https://my-host.example.com"
            [settlement]
            method = "none"
        "#;
        let service = ServiceManifest::from_toml(toml).unwrap().0;
        let input = PublishInput {
            project: None,
            service,
            source: SourceLocator::Inline("x".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
        };
        match resolve_hosting_choice(&input).unwrap() {
            HostingChoice::SelfHosted { url } => {
                assert_eq!(url.as_str(), "https://my-host.example.com/")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn _silence_unused_warning() {
        // Keep these types reachable for Phase 1B without unused-import noise.
        let _ = HostingSection {
            default: "self".to_string(),
            local: None,
            tor: None,
            self_hosted: Some(SelfHostingConfig {
                url: "https://x".to_string(),
            }),
            managed: None,
            fly: None,
        };
        let _ = SettlementSection {
            method: "none".to_string(),
        };
    }
}
