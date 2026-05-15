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
pub mod daemon_client;
pub mod error;
pub mod registration;

pub use daemon_client::{ControlAuth, DaemonClient};
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

/// Run the full build → host → sign → register pipeline against a
/// running `froglet-node` daemon.
///
/// Phase 1A.7 wiring: composes the 5 sub-phase pieces (LocalBackend,
/// TorBackend, SelfHostedBackend, Python builder, DaemonClient,
/// registration helpers) into one coherent flow.
///
/// Caller is responsible for ensuring the daemon is reachable (the
/// CLI's `init` subcommand handles that for the operator path).
pub async fn publish(
    input: PublishInput,
    daemon: &DaemonClient,
) -> Result<PublishOutput, PublishError> {
    // 1. Resolve hosting choice (override > service manifest > project default > local).
    let hosting = resolve_hosting_choice(&input)?;

    tracing::info!(
        service_id = %input.service.service_id,
        runtime = %input.service.runtime,
        hosting = ?hosting,
        marketplace = %input.marketplace_url,
        "froglet-publish-engine: pipeline start",
    );

    // 2. Bring up the public surface.
    let prepared = pipeline::prepare_hosting(&hosting, daemon).await?;

    // 3. Build the artifact from source.
    let runtime = input.service.runtime.as_str();
    let package_kind = input.service.package_kind.as_str();
    let publish_request = pipeline::build_publish_request(&input, runtime, package_kind).await?;

    // 4. Publish via the daemon (signs + persists to /v1/feed).
    let response = daemon.publish_artifact(&publish_request).await?;
    let evidence = response.evidence;

    // 5. Register with marketplace if the backend says we should.
    let (marketplace_offer_url, status_url, indexer_warning) = if prepared.register_with_marketplace
    {
        let transport_hint = hosting_transport_hint(&hosting);
        registration::register_with_marketplace(
            &input.marketplace_url,
            &prepared.public_url,
            transport_hint,
        )
        .await?;

        let offer_url =
            registration::marketplace_offer_url(&input.marketplace_url, &evidence.offer_hash);
        let provider_status_url = format!(
            "{}/v1/providers/{}",
            input.marketplace_url.as_str().trim_end_matches('/'),
            evidence.provider_id,
        );

        // 6. Wait for indexer projection (eventually consistent).
        let warning =
            match registration::wait_for_indexer(&input.marketplace_url, &evidence.provider_id)
                .await
            {
                Ok(_) => None,
                Err(PublishError::Verification { tries: _, .. }) => {
                    Some(PublishWarning::IndexerLag { seconds_waited: 90 })
                }
                Err(other) => return Err(other),
            };
        (Some(offer_url), Some(provider_status_url), warning)
    } else {
        (None, None, None)
    };

    let mut warnings: Vec<PublishWarning> = Vec::new();
    if !prepared.register_with_marketplace {
        warnings.push(PublishWarning::NotRegistered {
            reason: format!("hosting backend {:?} is private", hosting),
        });
    }
    if let Some(w) = indexer_warning {
        warnings.push(w);
    }

    Ok(PublishOutput {
        provider_id: evidence.provider_id,
        public_url: prepared.public_url,
        offer_hash: evidence.offer_hash,
        marketplace_offer_url,
        invoke_command: format!("froglet-node invoke {} '{{}}'", evidence.offer_id),
        status_url,
        warnings,
    })
}

fn hosting_transport_hint(hosting: &HostingChoice) -> Option<&'static str> {
    match hosting {
        HostingChoice::Tor => Some("tor"),
        _ => Some("clearnet"),
    }
}

/// Pipeline helpers — kept private to encourage callers to use [`publish`].
mod pipeline {
    use super::*;
    use crate::backends::{HostingBackend, PreparedHosting};
    use crate::backends::{local::LocalBackend, self_hosted::SelfHostedBackend, tor::TorBackend};

    pub(super) async fn prepare_hosting(
        choice: &HostingChoice,
        daemon: &DaemonClient,
    ) -> Result<PreparedHosting, PublishError> {
        match choice {
            HostingChoice::Local => {
                LocalBackend::with_url(daemon.daemon_url.as_str())
                    .prepare()
                    .await
            }
            HostingChoice::Tor => TorBackend::new(daemon.daemon_url.clone()).prepare().await,
            HostingChoice::SelfHosted { url } => {
                SelfHostedBackend::new(url.clone()).prepare().await
            }
            HostingChoice::Managed { .. } => Err(PublishError::NotImplemented {
                what: "Managed backend (Phase 1B)".to_string(),
            }),
            HostingChoice::Fly { .. } => Err(PublishError::NotImplemented {
                what: "Fly backend (Phase 1B)".to_string(),
            }),
        }
    }

    pub(super) async fn build_publish_request(
        input: &PublishInput,
        runtime: &str,
        package_kind: &str,
    ) -> Result<daemon_client::PublishArtifactRequest, PublishError> {
        let mut request = daemon_client::PublishArtifactRequest {
            service_id: input.service.service_id.clone(),
            offer_id: input.service.offer_id.clone(),
            runtime: Some(runtime.to_string()),
            package_kind: Some(package_kind.to_string()),
            entrypoint_kind: input.service.entrypoint_kind.clone(),
            entrypoint: input.service.entrypoint.clone(),
            contract_version: input.service.contract_version.clone(),
            summary: input.service.summary.clone(),
            mode: input.service.mode.clone(),
            price_sats: input
                .service
                .price
                .as_ref()
                .and_then(|p| p.sats)
                .unwrap_or(0),
            publication_state: input.service.publication_state.clone(),
            input_schema: input.service.input_schema.clone(),
            output_schema: input.service.output_schema.clone(),
            ..Default::default()
        };

        match (runtime, package_kind) {
            ("python", "inline_source") => {
                let artifact = crate::builder::build_python_inline(
                    &input.source,
                    input.service.entrypoint.as_deref(),
                )
                .await?;
                request.inline_source =
                    Some(String::from_utf8(artifact.source_bytes).map_err(|_| {
                        PublishError::Build("Python inline_source must be valid UTF-8".to_string())
                    })?);
            }
            (rt, pk) => {
                return Err(PublishError::NotImplemented {
                    what: format!("builder for runtime={rt} package_kind={pk} (Phase 1B)"),
                });
            }
        }

        Ok(request)
    }
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

    // Note: an end-to-end `publish()` test requires a running daemon +
    // marketplace, and lives in Phase 1A.8 as an `#[ignore]`d integration
    // test. The pure-function tests below cover the hosting-resolution
    // logic without a daemon.

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
