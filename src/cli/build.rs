//! `froglet-node build` — validate manifests + build the artifact, but
//! do not publish. Useful as a quick sanity check before `publish`.

use super::{CliError, pop_flag};
use froglet_protocol::manifest::{ManifestWarning, ProjectManifest, ServiceManifest};
use froglet_publish_engine::builder::build_python_inline;
use std::io::Write;
use std::path::{Path, PathBuf};

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    let cwd = std::env::current_dir().map_err(CliError::Io)?;

    let (project, project_path) = load_project_manifest(&cwd)?;
    let (service, service_path) = load_service_manifest(&cwd)?;

    if !json_mode {
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
        let service_dir = service_path.parent().unwrap_or_else(|| Path::new("."));
        let source = super::publish::read_source(&service, service_dir)?;
        let artifact = build_python_inline(&source, Some(&entrypoint))
            .await
            .map_err(CliError::Engine)?;
        if json_mode {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "built",
                    "project_manifest": project_path.as_ref().map(|path| path.display().to_string()),
                    "service_manifest": service_path.display().to_string(),
                    "service_id": service.service_id,
                    "source_path": artifact.source_path,
                    "source_hash": artifact.source_hash,
                    "source_bytes": artifact.source_bytes.len(),
                }))
                .map_err(|error| CliError::Other(format!("serialize build output: {error}")))?
            );
        } else {
            println!("✓ artifact built");
            println!("  source path:  {}", artifact.source_path);
            println!("  source hash:  {}", artifact.source_hash);
            println!("  source bytes: {}", artifact.source_bytes.len());
        }
    } else if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "manifests-ok",
                "project_manifest": project_path.as_ref().map(|path| path.display().to_string()),
                "service_manifest": service_path.display().to_string(),
                "service_id": service.service_id,
                "runtime": service.runtime,
                "package_kind": service.package_kind,
            }))
            .map_err(|error| CliError::Other(format!("serialize build output: {error}")))?
        );
    } else {
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
    let (manifest, warnings) = ServiceManifest::from_toml(&toml_str)?;
    // Warnings go only to stderr so `build --json`, native MCP stdio, and
    // agent parsers retain a clean machine-readable stdout stream.
    let _ = write_manifest_warnings(std::io::stderr().lock(), &warnings);
    Ok((manifest, path))
}

fn write_manifest_warnings(
    mut writer: impl Write,
    warnings: &[ManifestWarning],
) -> std::io::Result<()> {
    for warning in warnings {
        let (code, message) = match warning {
            ManifestWarning::LegacyV2Service { missing_section } => (
                "legacy_v2_service",
                format!(
                    "service manifest v2 omitted {missing_section}; compatibility defaults were applied"
                ),
            ),
            ManifestWarning::DeprecatedFlyHosting => (
                "deprecated_fly_hosting",
                "legacy v3 Fly hosting is readable for migration only; import it through a deployment adapter, then author hosting.managed target/profile"
                    .to_string(),
            ),
        };
        writeln!(writer, "froglet-warning {code}: {message}")?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_warnings_are_stable_single_line_stderr_records() {
        let warnings = [
            ManifestWarning::DeprecatedFlyHosting,
            ManifestWarning::LegacyV2Service {
                missing_section: "[hosting]",
            },
        ];
        let mut output = Vec::new();
        write_manifest_warnings(&mut output, &warnings).unwrap();
        let output = String::from_utf8(output).unwrap();
        let lines = output.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "froglet-warning deprecated_fly_hosting: legacy v3 Fly hosting is readable for migration only; import it through a deployment adapter, then author hosting.managed target/profile"
        );
        assert_eq!(
            lines[1],
            "froglet-warning legacy_v2_service: service manifest v2 omitted [hosting]; compatibility defaults were applied"
        );
    }
}
