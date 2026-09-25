//! `froglet-node publish` — the headline subcommand. Reads manifests
//! from the current directory, builds the artifact, calls the engine's
//! end-to-end pipeline, and prints the live marketplace URL.
//!
//! Optional flags:
//! - `--host local|relay|tor|self` — override the manifest's hosting choice
//! - `--marketplace <URL>`     — override the marketplace URL
//! - `--plan`                  — emit the non-mutating consent summary
//! - `--approve-consent HASH`  — approve the exact public plan
//! - `--json`                  — emit machine-readable output

use super::{CliError, pop_flag, pop_kv};
use froglet_publish_engine::{
    DaemonClient, HostingChoice, PublishInput, SourceLocator, plan_publication, publish,
};
use std::{
    io::Read,
    path::{Component, Path},
};
use url::Url;

const MAX_AUTHORED_SOURCE_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    let plan_mode = pop_flag(&mut args, "--plan");
    let approved_consent_hash = pop_kv(&mut args, "--approve-consent");
    let host_override = pop_kv(&mut args, "--host");
    let marketplace_override = pop_kv(&mut args, "--marketplace");

    if !args.is_empty() {
        return Err(CliError::BadArgs(format!(
            "unrecognised args: {args:?}\n\nusage: froglet-node publish [--host local|relay|tor|self] [--marketplace URL] [--plan | --approve-consent HASH] [--json]"
        )));
    }

    let cwd = std::env::current_dir()?;
    let input = load_publish_input(
        &cwd,
        host_override.as_deref(),
        marketplace_override.as_deref(),
        approved_consent_hash,
    )
    .await?;

    execute_publish(input, plan_mode, json_mode).await
}

/// Load one canonical [`PublishInput`] from an authored project directory.
/// CLI and native MCP callers share this path so manifest precedence, source
/// selection, public-URL validation, and hosting policy cannot drift.
pub(crate) async fn load_publish_input(
    project_dir: &Path,
    host_override: Option<&str>,
    marketplace_override: Option<&str>,
    approved_consent_hash: Option<String>,
) -> Result<PublishInput, CliError> {
    super::prepare::check_prepared_source(project_dir)?;
    let (project, _project_path) = super::build::load_project_manifest(project_dir)?;
    let (service, service_path) = super::build::load_service_manifest(project_dir)?;
    let service_dir = service_path
        .parent()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    // Resolve marketplace URL (override > service [marketplace] > project [project.marketplace] > default).
    let marketplace_url =
        resolve_marketplace_url(marketplace_override, &service, project.as_ref()).await?;

    // Read source from the manifest's entrypoint relative to the service dir.
    let source = read_source(&service, &service_dir)?;

    // Convert the optional --host flag into a HostingChoice.
    let hosting_override = match host_override {
        Some("local") => Some(HostingChoice::Local),
        Some("relay") => Some(HostingChoice::Relay),
        Some("tor") => Some(HostingChoice::Tor),
        Some("self") => {
            // Pull the URL from the manifest's [hosting.self] section.
            let url = service
                .hosting
                .as_ref()
                .and_then(|h| h.self_hosted.as_ref())
                .map(|s| s.url.as_str())
                .ok_or_else(|| {
                    CliError::BadArgs(
                        "--host self requires [hosting.self] url in froglet-service.toml"
                            .to_string(),
                    )
                })?;
            let endpoint = crate::provider_resolution::validate_remote_egress_url(
                url,
                "[hosting.self] url",
                false,
                "use a public https:// or .onion self-host URL",
            )
            .await
            .map_err(CliError::BadArgs)?;
            Some(HostingChoice::SelfHosted {
                url: Url::parse(&endpoint.normalized_url).map_err(|e| {
                    CliError::BadArgs(format!("[hosting.self] url is not a valid URL: {e}"))
                })?,
            })
        }
        Some(other) => {
            return Err(CliError::BadArgs(format!(
                "--host {other:?} is not supported; use local | relay | tor | self"
            )));
        }
        None => None,
    };

    let input = PublishInput {
        project,
        service,
        source,
        hosting_override,
        marketplace_url,
        approved_consent_hash,
    };

    Ok(input)
}

pub(crate) async fn execute_publish(
    input: PublishInput,
    plan_mode: bool,
    json_mode: bool,
) -> Result<(), CliError> {
    let daemon = DaemonClient::from_env().map_err(CliError::Engine)?;
    if plan_mode {
        if input.approved_consent_hash.is_some() {
            return Err(CliError::BadArgs(
                "--plan and --approve-consent are mutually exclusive".to_string(),
            ));
        }
        let consent = plan_publication(&input, &daemon)
            .await
            .map_err(CliError::Engine)?;
        if json_mode {
            println!(
                "{}",
                serde_json::to_string_pretty(&consent)
                    .map_err(|error| CliError::Other(format!("serialize consent: {error}")))?
            );
        } else {
            println!("publication consent: {}", consent.consent_hash);
            println!(
                "{}",
                serde_json::to_string_pretty(&consent.summary)
                    .map_err(|error| CliError::Other(format!("serialize consent: {error}")))?
            );
            if consent.status == "approval_required" {
                println!();
                println!(
                    "Approve with: froglet-node publish --approve-consent {}",
                    consent.consent_hash
                );
            }
        }
        return Ok(());
    }

    let output = publish(input, &daemon).await.map_err(CliError::Engine)?;
    remember_publication(&output);

    if json_mode {
        let json = serde_json::to_string_pretty(&output)
            .map_err(|e| CliError::Other(format!("serialize output: {e}")))?;
        println!("{json}");
    } else {
        println!("Publication: {}", output.status);
        if let Some(url) = &output.share_url {
            println!("Share: {url}");
        }
        if output.status == "pending_review" {
            println!(
                "Marketplace activation is not yet verified. Inspect publication status before claiming availability."
            );
        }
        println!();
        println!("provider_id:           {}", output.provider_id);
        println!("public_url:            {}", output.public_url);
        println!("offer_hash:            {}", output.offer_hash);
        if let Some(url) = &output.marketplace_offer_url {
            println!("marketplace_offer_url: {url}");
        }
        if let Some(url) = &output.status_url {
            println!("status_url:            {url}");
        }
        println!();
        println!("Invoke with:");
        println!("  {}", output.invoke_command);
        if !output.warnings.is_empty() {
            println!();
            println!("Warnings:");
            for w in &output.warnings {
                println!("  - {w:?}");
            }
        }
    }
    Ok(())
}

async fn resolve_marketplace_url(
    override_flag: Option<&str>,
    service: &froglet_protocol::manifest::ServiceManifest,
    project: Option<&froglet_protocol::manifest::ProjectManifest>,
) -> Result<Url, CliError> {
    let raw = if let Some(v) = override_flag {
        v.to_string()
    } else if let Some(m) = service.marketplace.as_ref() {
        m.url.clone()
    } else if let Some(p) = project
        && let Some(m) = p.project.marketplace.as_ref()
    {
        m.url.clone()
    } else {
        "https://marketplace.froglet.dev".to_string()
    };
    let endpoint = crate::provider_resolution::validate_remote_egress_url(
        &raw,
        "marketplace URL",
        false,
        "use a public https:// or .onion marketplace URL",
    )
    .await
    .map_err(CliError::BadArgs)?;
    Url::parse(&endpoint.normalized_url)
        .map_err(|e| CliError::BadArgs(format!("marketplace URL {raw:?} is invalid: {e}")))
}

pub(crate) fn read_source(
    service: &froglet_protocol::manifest::ServiceManifest,
    service_dir: &std::path::Path,
) -> Result<SourceLocator, CliError> {
    match (service.runtime.as_str(), service.package_kind.as_str()) {
        ("python", "inline_source") => {
            let entrypoint = service.entrypoint.as_ref().ok_or_else(|| {
                CliError::BadArgs(
                    "service.entrypoint is required for python inline_source".to_string(),
                )
            })?;
            snapshot_project_file(service_dir, entrypoint, "entrypoint")
        }
        ("wasm", "inline_module") => {
            let entrypoint = service.entrypoint.as_ref().ok_or_else(|| {
                CliError::BadArgs(
                    "service.entrypoint is required for wasm inline_module (.wat or .wasm)"
                        .to_string(),
                )
            })?;
            snapshot_project_file(service_dir, entrypoint, "Wasm source")
        }
        ("builtin", "builtin") => {
            let data = service.data.as_ref().ok_or_else(|| {
                CliError::BadArgs(
                    "runtime=builtin package_kind=builtin requires [data] path and format"
                        .to_string(),
                )
            })?;
            snapshot_project_file(service_dir, &data.path, "data source")
        }
        (rt, pk) => Err(CliError::BadArgs(format!(
            "runtime={rt} package_kind={pk} is not supported by the native builder"
        ))),
    }
}

fn snapshot_project_file(
    service_dir: &Path,
    authored_path: &str,
    label: &str,
) -> Result<SourceLocator, CliError> {
    let relative = Path::new(authored_path);
    if authored_path.trim().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CliError::BadArgs(format!(
            "{label} must be a project-relative path without parent traversal"
        )));
    }
    let canonical_root = service_dir.canonicalize().map_err(|error| {
        CliError::BadArgs(format!(
            "service manifest directory {service_dir:?} could not be resolved: {error}"
        ))
    })?;
    let candidate = service_dir.join(relative);
    let authored_metadata = std::fs::symlink_metadata(&candidate).map_err(|error| {
        CliError::BadArgs(format!(
            "{label} {candidate:?} was not found relative to the service manifest: {error}"
        ))
    })?;
    if authored_metadata.file_type().is_symlink() || !authored_metadata.is_file() {
        return Err(CliError::BadArgs(format!(
            "{label} must resolve directly to a regular file, not a symlink"
        )));
    }
    let canonical_path = candidate.canonicalize().map_err(|error| {
        CliError::BadArgs(format!(
            "{label} {candidate:?} could not be resolved: {error}"
        ))
    })?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(CliError::BadArgs(format!(
            "{label} resolves outside the service manifest directory"
        )));
    }
    let file = std::fs::File::open(&canonical_path).map_err(CliError::Io)?;
    let mut bytes = Vec::with_capacity(
        authored_metadata
            .len()
            .min(MAX_AUTHORED_SOURCE_SNAPSHOT_BYTES as u64) as usize,
    );
    file.take(MAX_AUTHORED_SOURCE_SNAPSHOT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(CliError::Io)?;
    if bytes.len() > MAX_AUTHORED_SOURCE_SNAPSHOT_BYTES {
        return Err(CliError::BadArgs(format!(
            "{label} exceeds the {MAX_AUTHORED_SOURCE_SNAPSHOT_BYTES} byte authoring limit"
        )));
    }
    Ok(SourceLocator::FileSnapshot {
        path: canonical_path,
        bytes,
    })
}

/// Private observed progress, not publication policy or an assertion of future
/// availability. A failed write must not turn an already committed publication
/// into a retryable error.
pub(crate) fn remember_publication(output: &froglet_publish_engine::PublishOutput) {
    let root = super::prepare::data_root();
    if !root.exists() {
        return;
    }
    let Some(revision) = &output.publication_revision else {
        return;
    };
    let directory = root.join("publication-observations");
    let write = || -> Result<(), CliError> {
        super::prepare::private_dir(&directory)?;
        let key = crate::crypto::sha256_hex(revision.payload.service_id.as_bytes());
        let observation = serde_json::json!({"service_id":revision.payload.service_id, "revision_hash":revision.revision_hash,
            "observed_at":crate::settlement::current_unix_timestamp(), "status":output.status, "progress":output.progress,
            "hosting":output.consent.summary.hosting, "share_url":output.share_url});
        super::prepare::atomic_private_write(
            &directory.join(format!("{key}.json")),
            &serde_json::to_vec(&observation).map_err(|e| CliError::Other(e.to_string()))?,
        )
    };
    if let Err(error) = write() {
        eprintln!(
            "warning: publication completed but local progress history could not be recorded: {error}; inspect publication_status before retrying"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use froglet_protocol::manifest::ServiceManifest;

    fn python_service(entrypoint: &str) -> ServiceManifest {
        let source = format!(
            r#"
schema_version = "froglet-service/v4"
service_id = "snapshot-test"
runtime = "python"
package_kind = "inline_source"
entrypoint = {entrypoint:?}
[hosting]
default = "local"
[settlement]
method = "none"
"#
        );
        ServiceManifest::from_toml(&source).unwrap().0
    }

    #[test]
    fn source_snapshot_is_not_changed_by_later_file_writes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("handler.py");
        std::fs::write(&path, "print('approved')\n").unwrap();
        let locator = read_source(&python_service("handler.py"), root.path()).unwrap();
        std::fs::write(&path, "print('changed')\n").unwrap();
        let SourceLocator::FileSnapshot { bytes, .. } = locator else {
            panic!("CLI file inputs must be snapshotted");
        };
        assert_eq!(bytes, b"print('approved')\n");
    }

    #[cfg(unix)]
    #[test]
    fn source_symlink_cannot_read_a_sibling_secret() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let secret = parent.path().join("secret.py");
        std::fs::write(&secret, "HOST_SECRET\n").unwrap();
        symlink(&secret, project.join("handler.py")).unwrap();
        let error = read_source(&python_service("handler.py"), &project).unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
    }
}
