//! `froglet-node build` — validate manifests + build the artifact, but
//! do not publish. Useful as a quick sanity check before `publish`.

use super::{CliError, pop_flag};
use froglet_protocol::manifest::{ProjectManifest, ServiceManifest};
use froglet_publish_engine::{SourceLocator, builder::build_python_inline};
use std::path::{Path, PathBuf};

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    let cwd = std::env::current_dir().map_err(CliError::Io)?;

    let (project, project_path) = load_project_manifest(&cwd)?;
    let (service, service_path) = load_service_manifest(&cwd)?;

    if json_mode {
        println!(
            "{{\"status\":\"manifests-ok\",\"project_manifest\":\"{}\",\"service_manifest\":\"{}\",\"service_id\":\"{}\"}}",
            project_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            service_path.display(),
            service.service_id,
        );
    } else {
        println!("✓ service manifest {} is valid", service_path.display());
        if let Some(p) = &project_path {
            println!("✓ project manifest {} is valid", p.display());
        }
        println!();
        println!("Service:  {}", service.service_id);
        println!("Runtime:  {} ({})", service.runtime, service.package_kind);
        if let Some(project) = &project {
            println!("Project:  {}", project.project.name);
        }
    }

    // For Python inline_source, validate the entrypoint file exists + builds.
    if service.runtime == "python" && service.package_kind == "inline_source" {
        let entrypoint = service
            .entrypoint
            .clone()
            .ok_or_else(|| CliError::Other("service.entrypoint is required".to_string()))?;
        let path = service_path
            .parent()
            .map(|p| p.join(&entrypoint))
            .unwrap_or_else(|| PathBuf::from(&entrypoint));
        if !path.exists() {
            return Err(CliError::Other(format!(
                "entrypoint {path:?} not found relative to service manifest"
            )));
        }
        let artifact = build_python_inline(&SourceLocator::File(path.clone()), Some(&entrypoint))
            .await
            .map_err(CliError::Engine)?;
        if json_mode {
            println!(
                "{{\"status\":\"built\",\"source_path\":\"{}\",\"source_hash\":\"{}\",\"source_bytes\":{}}}",
                path.display(),
                artifact.source_hash,
                artifact.source_bytes.len(),
            );
        } else {
            println!("✓ artifact built");
            println!("  source path:  {}", path.display());
            println!("  source hash:  {}", artifact.source_hash);
            println!("  source bytes: {}", artifact.source_bytes.len());
        }
    } else if !json_mode {
        println!(
            "(skipping artifact build: runtime={} package_kind={} is Phase 1B)",
            service.runtime, service.package_kind
        );
    }
    Ok(())
}

/// Look for `froglet-service.toml` starting at `cwd`. Errors if not found.
pub fn load_service_manifest(cwd: &Path) -> Result<(ServiceManifest, PathBuf), CliError> {
    let path = cwd.join("froglet-service.toml");
    if !path.exists() {
        return Err(CliError::BadArgs(format!(
            "froglet-service.toml not found in {cwd:?}; run `froglet-node init <name>` first"
        )));
    }
    let toml_str = std::fs::read_to_string(&path)?;
    let (manifest, _warnings) = ServiceManifest::from_toml(&toml_str)?;
    Ok((manifest, path))
}

/// Walk upward from `cwd` looking for `froglet.toml`. Returns `(None, None)`
/// if not found (project manifest is optional).
pub fn load_project_manifest(
    cwd: &Path,
) -> Result<(Option<ProjectManifest>, Option<PathBuf>), CliError> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let candidate = d.join("froglet.toml");
        if candidate.exists() {
            let toml_str = std::fs::read_to_string(&candidate)?;
            let (manifest, _warnings) = ProjectManifest::from_toml(&toml_str)?;
            return Ok((Some(manifest), Some(candidate)));
        }
        dir = d.parent();
    }
    Ok((None, None))
}
