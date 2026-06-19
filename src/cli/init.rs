//! `froglet-node init <name>` — scaffold a new Froglet service project.
//!
//! Creates four files in a new directory under the current working
//! directory:
//!
//! - `<name>/froglet.toml`            (project manifest)
//! - `<name>/froglet-service.toml`    (service manifest, v3)
//! - `<name>/handler.py`              (Python entrypoint skeleton)
//! - `<name>/.gitignore`              (sensible defaults)
//!
//! After init, the user (or an LLM) can run `froglet-node publish` in
//! the new directory and end up with a live service.

use super::{CliError, pop_flag};
use std::path::Path;

const HANDLER_PY: &str = r#"# Froglet Python handler. The default contract is
# froglet.python.handler_json.v1, which means:
#
# - Input arrives as the `event` JSON object
# - `context` carries runtime metadata such as granted mounts
# - Return one JSON-serializable value
#
# Edit `handler()` to do real work, then run:
#   froglet-node publish


def handler(event: dict, context: dict) -> dict:
    """Service entry point. Return the JSON result."""
    return {"echo": event, "mounts": context.get("mounts", {})}
"#;

const GITIGNORE: &str =
    "# Local Froglet runtime state\n.froglet/\n_tmp/\n\n# Python\n__pycache__/\n*.pyc\n.venv/\n";

fn project_toml(name: &str) -> String {
    format!(
        r#"schema_version = "froglet/v1"

[project]
name = "{name}"

[project.marketplace]
url = "https://marketplace.froglet.dev"

[project.defaults]
runtime = "python"
hosting = "tor"
settlement = "none"
"#
    )
}

fn service_toml(name: &str) -> String {
    format!(
        r#"schema_version = "froglet-service/v3"

project_id = "{name}"
service_id = "{name}"
summary = "Froglet service {name}"

runtime = "python"
package_kind = "inline_source"
entrypoint_kind = "handler"
entrypoint = "handler.py"
contract_version = "froglet.python.handler_json.v1"

[hosting]
default = "tor"

[settlement]
method = "none"

[price]
sats = 0
"#
    )
}

pub fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");

    let name = match args.first() {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return Err(CliError::BadArgs(
                "usage: froglet-node init <name> [--json]".to_string(),
            ));
        }
    };

    // Validate identifier shape early (mirrors the manifest validator).
    validate_name(&name)?;

    let dir = Path::new(&name);
    if dir.exists() {
        return Err(CliError::BadArgs(format!(
            "directory {dir:?} already exists; refusing to overwrite"
        )));
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("froglet.toml"), project_toml(&name))?;
    std::fs::write(dir.join("froglet-service.toml"), service_toml(&name))?;
    std::fs::write(dir.join("handler.py"), HANDLER_PY)?;
    std::fs::write(dir.join(".gitignore"), GITIGNORE)?;

    if json_mode {
        println!(
            "{{\"status\":\"ok\",\"path\":\"{}\",\"files\":[\"froglet.toml\",\"froglet-service.toml\",\"handler.py\",\".gitignore\"]}}",
            dir.display()
        );
    } else {
        println!("Scaffolded Froglet service at {}:", dir.display());
        println!("  - froglet.toml          (project manifest)");
        println!("  - froglet-service.toml  (service manifest, v3)");
        println!("  - handler.py            (Python entrypoint)");
        println!("  - .gitignore");
        println!();
        println!("Next:");
        println!("  cd {name}");
        println!("  # edit handler.py");
        println!("  froglet-node publish");
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), CliError> {
    if name.is_empty() || name.len() > 63 {
        return Err(CliError::BadArgs(format!(
            "name {name:?}: must be 1-63 characters"
        )));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(CliError::BadArgs(format!(
            "name {name:?}: must not start or end with a hyphen"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(CliError::BadArgs(format!(
            "name {name:?}: must be lowercase ASCII letters, digits, or interior hyphens"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_uses_handler_abi() {
        let service = service_toml("echoer");
        assert!(service.contains("entrypoint_kind = \"handler\""));
        assert!(service.contains("entrypoint = \"handler.py\""));
        assert!(service.contains("contract_version = \"froglet.python.handler_json.v1\""));
        assert!(HANDLER_PY.contains("def handler(event: dict, context: dict) -> dict:"));
    }

    #[test]
    fn validates_name_shape() {
        assert!(validate_name("translator").is_ok());
        assert!(validate_name("my-service-1").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("-leading").is_err());
        assert!(validate_name("UPPERCASE").is_err());
        assert!(validate_name("with_underscore").is_err());
    }
}
