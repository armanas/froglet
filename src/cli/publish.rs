//! `froglet-node publish` — the headline subcommand. Reads manifests
//! from the current directory, builds the artifact, calls the engine's
//! end-to-end pipeline, and prints the live marketplace URL.
//!
//! Optional flags:
//! - `--host local|tor|self`  — override the manifest's hosting choice
//! - `--marketplace <URL>`     — override the marketplace URL
//! - `--json`                  — emit machine-readable output

use super::{CliError, pop_flag, pop_kv};
use froglet_publish_engine::{DaemonClient, HostingChoice, PublishInput, SourceLocator, publish};
use url::Url;

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    let host_override = pop_kv(&mut args, "--host");
    let marketplace_override = pop_kv(&mut args, "--marketplace");

    if !args.is_empty() {
        return Err(CliError::BadArgs(format!(
            "unrecognised args: {args:?}\n\nusage: froglet-node publish [--host local|tor|self] [--marketplace URL] [--json]"
        )));
    }

    let cwd = std::env::current_dir()?;
    let (project, _project_path) = super::build::load_project_manifest(&cwd)?;
    let (service, service_path) = super::build::load_service_manifest(&cwd)?;
    let service_dir = service_path
        .parent()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    // Resolve marketplace URL (override > service [marketplace] > project [project.marketplace] > default).
    let marketplace_url =
        resolve_marketplace_url(marketplace_override.as_deref(), &service, project.as_ref())?;

    // Read source from the manifest's entrypoint relative to the service dir.
    let source = read_source(&service, &service_dir)?;

    // Convert the optional --host flag into a HostingChoice.
    let hosting_override = match host_override.as_deref() {
        Some("local") => Some(HostingChoice::Local),
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
            Some(HostingChoice::SelfHosted {
                url: Url::parse(url).map_err(|e| {
                    CliError::BadArgs(format!("[hosting.self] url is not a valid URL: {e}"))
                })?,
            })
        }
        Some(other) => {
            return Err(CliError::BadArgs(format!(
                "--host {other:?} is not supported; use local | tor | self"
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
    };

    let daemon = DaemonClient::from_env().map_err(CliError::Engine)?;
    let output = publish(input, &daemon).await.map_err(CliError::Engine)?;

    if json_mode {
        let json = serde_json::to_string_pretty(&output)
            .map_err(|e| CliError::Other(format!("serialize output: {e}")))?;
        println!("{json}");
    } else {
        println!("✓ published");
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

fn resolve_marketplace_url(
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
    Url::parse(&raw)
        .map_err(|e| CliError::BadArgs(format!("marketplace URL {raw:?} is invalid: {e}")))
}

fn read_source(
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
            let path = service_dir.join(entrypoint);
            if !path.exists() {
                return Err(CliError::BadArgs(format!(
                    "entrypoint {path:?} not found relative to service manifest"
                )));
            }
            Ok(SourceLocator::File(path))
        }
        (rt, pk) => Err(CliError::BadArgs(format!(
            "runtime={rt} package_kind={pk} is not supported in Phase 1A; only python+inline_source"
        ))),
    }
}
