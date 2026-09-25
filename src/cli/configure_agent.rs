//! Lossless agent configuration changes with a compare-before-write receipt.
//! This is installation metadata, never a signed protocol artifact.
use super::{CliError, pop_flag, pop_kv};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigReceipt {
    pub output: PathBuf,
    pub before_sha256: String,
    pub after_sha256: String,
    pub backup: Option<PathBuf>,
    pub changed: bool,
}

pub fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    if let Some(receipt) = pop_kv(&mut args, "--rollback-receipt") {
        if !args.is_empty() {
            return Err(CliError::BadArgs("unexpected rollback arguments".into()));
        }
        let receipt: ConfigReceipt = serde_json::from_slice(&fs::read(receipt)?)
            .map_err(|e| CliError::BadArgs(format!("invalid config receipt: {e}")))?;
        rollback(&receipt)?;
        println!(
            "{}",
            serde_json::json!({"status":"ok", "restored":receipt.changed})
        );
        return Ok(());
    }
    let target = required(&mut args, "--target")?;
    let generated = PathBuf::from(required(&mut args, "--generated")?);
    let output = PathBuf::from(required(&mut args, "--output")?);
    let expected = pop_kv(&mut args, "--expected-sha256");
    let receipt_path = pop_kv(&mut args, "--receipt").map(PathBuf::from);
    let plan = pop_flag(&mut args, "--plan");
    if !args.is_empty() {
        return Err(CliError::BadArgs(
            "unexpected configure-agent arguments".into(),
        ));
    }
    let before = read_config(&output)?;
    let before_hash = fingerprint(before.as_deref());
    if expected.as_ref().is_some_and(|hash| hash != &before_hash) {
        return Err(CliError::Other("agent_config_changed: configuration changed after approval; prepare a new install plan".into()));
    }
    let after = merge(
        &target,
        before.as_deref().unwrap_or_default(),
        &fs::read(generated)?,
    )?;
    let changed = before.as_deref() != Some(after.as_slice());
    let backup = before.as_ref().filter(|_| changed).map(|_| {
        output.with_file_name(format!(
            "{}.froglet-backup-{before_hash}",
            output.file_name().unwrap_or_default().to_string_lossy()
        ))
    });
    let receipt = ConfigReceipt {
        output,
        before_sha256: before_hash,
        after_sha256: fingerprint(Some(&after)),
        backup,
        changed,
    };
    if !plan && changed {
        apply(&receipt, before.as_deref(), &after, receipt_path.as_deref())?;
    }
    if json_mode || plan {
        println!(
            "{}",
            serde_json::json!({"status":if plan {"planned"} else {"ok"}, "configuration":receipt})
        );
    } else {
        println!(
            "{} Froglet configuration at {}",
            if changed {
                "Merged"
            } else {
                "Already configured:"
            },
            receipt.output.display()
        );
    }
    Ok(())
}

fn required(args: &mut Vec<String>, key: &str) -> Result<String, CliError> {
    pop_kv(args, key).ok_or_else(|| CliError::BadArgs(format!("configure-agent requires {key}")))
}

fn read_config(path: &Path) -> Result<Option<Vec<u8>>, CliError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => Err(CliError::Other(
            "agent config must be a regular file, not a symlink or directory".into(),
        )),
        Ok(meta) if meta.len() > 4 * 1024 * 1024 => {
            Err(CliError::Other("agent config exceeds 4 MiB".into()))
        }
        Ok(_) => Ok(Some(fs::read(path)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn fingerprint(bytes: Option<&[u8]>) -> String {
    bytes
        .map(crate::crypto::sha256_hex)
        .unwrap_or_else(|| "missing".into())
}

pub fn merge(target: &str, before: &[u8], generated: &[u8]) -> Result<Vec<u8>, CliError> {
    let bad = |e: String| {
        CliError::BadArgs(format!(
            "invalid {target} configuration; original file was not changed: {e}"
        ))
    };
    match target {
        "claude-code" => {
            let mut original: Value = if before.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_slice(before).map_err(|e| bad(e.to_string()))?
            };
            let proposed: Value =
                serde_json::from_slice(generated).map_err(|e| bad(e.to_string()))?;
            let entry = proposed
                .pointer("/mcpServers/froglet")
                .filter(|v| v.is_object())
                .ok_or_else(|| bad("missing froglet server".into()))?;
            let root = original
                .as_object_mut()
                .ok_or_else(|| bad("root must be an object".into()))?;
            let servers = root
                .entry("mcpServers")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or_else(|| bad("mcpServers must be an object".into()))?;
            if servers.get("froglet") == Some(entry) {
                return Ok(before.to_vec());
            }
            servers.insert("froglet".into(), entry.clone());
            let mut encoded =
                serde_json::to_vec_pretty(&original).map_err(|e| bad(e.to_string()))?;
            encoded.push(b'\n');
            Ok(encoded)
        }
        "codex" => {
            let original = std::str::from_utf8(before).map_err(|e| bad(e.to_string()))?;
            let proposed = std::str::from_utf8(generated).map_err(|e| bad(e.to_string()))?;
            let mut document = original
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| bad(e.to_string()))?;
            let proposed_document = proposed
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| bad(e.to_string()))?;
            // Compare values first: an identical reinstall must preserve every byte.
            let current_value: toml::Value =
                toml::from_str(original).map_err(|e| bad(e.to_string()))?;
            let proposed_value: toml::Value =
                toml::from_str(proposed).map_err(|e| bad(e.to_string()))?;
            let desired = proposed_value
                .get("mcp_servers")
                .and_then(|v| v.get("froglet"))
                .ok_or_else(|| bad("missing froglet server".into()))?;
            if current_value
                .get("mcp_servers")
                .and_then(|v| v.get("froglet"))
                == Some(desired)
            {
                return Ok(before.to_vec());
            }
            if !document.contains_key("mcp_servers") {
                document["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            let inline = document["mcp_servers"].is_inline_table();
            let table = document["mcp_servers"]
                .as_table_like_mut()
                .ok_or_else(|| bad("mcp_servers must be a table".into()))?;
            let mut replacement = proposed_document["mcp_servers"]["froglet"].clone();
            if inline {
                replacement = toml_edit::Item::Value(
                    replacement
                        .into_value()
                        .map_err(|_| bad("invalid froglet server".into()))?,
                );
            }
            table.insert("froglet", replacement);
            Ok(document.to_string().into_bytes())
        }
        _ => Err(CliError::BadArgs(
            "configuration merging supports claude-code and codex".into(),
        )),
    }
}

/// Advisory locks release on process exit. Keep the lock inode in place: unlinking
/// it would allow a third writer to acquire a different inode while a waiter owns it.
pub(crate) struct MetadataLock {
    _file: fs::File,
}
pub(crate) fn lock_metadata(path: &Path) -> Result<MetadataLock, CliError> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(CliError::Other(
            "metadata lock must be a regular file".into(),
        ));
    }
    file.try_lock().map_err(|e| {
        CliError::Other(format!(
            "operation_in_progress: {}: {e}; retry when the other operation finishes",
            path.display()
        ))
    })?;
    Ok(MetadataLock { _file: file })
}

fn lock(output: &Path) -> Result<MetadataLock, CliError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let path = output.with_file_name(format!(
        "{}.froglet-lock",
        output.file_name().unwrap_or_default().to_string_lossy()
    ));
    lock_metadata(&path)
}

fn private_file(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn replace(path: &Path, bytes: &[u8], expected: &str) -> Result<(), CliError> {
    let temporary = path.with_file_name(format!(
        ".froglet-config-{:016x}.tmp",
        rand::random::<u64>()
    ));
    private_file(&temporary, bytes)?;
    let result = (|| {
        if fingerprint(read_config(path)?.as_deref()) != expected {
            return Err(CliError::Other(
                "agent_config_changed: refusing to replace edited configuration".into(),
            ));
        }
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn apply(
    receipt: &ConfigReceipt,
    before: Option<&[u8]>,
    after: &[u8],
    receipt_path: Option<&Path>,
) -> Result<(), CliError> {
    let _lock = lock(&receipt.output)?;
    if fingerprint(read_config(&receipt.output)?.as_deref()) != receipt.before_sha256 {
        return Err(CliError::Other(
            "agent_config_changed: prepare a new install plan".into(),
        ));
    }
    if let (Some(backup), Some(bytes)) = (&receipt.backup, before) {
        if let Some(existing) = read_config(backup)? {
            if existing != bytes {
                return Err(CliError::Other(
                    "agent config backup has unexpected contents".into(),
                ));
            }
        } else {
            private_file(backup, bytes)?;
        }
    }
    if let Some(path) = receipt_path {
        let bytes =
            serde_json::to_vec_pretty(receipt).map_err(|e| CliError::Other(e.to_string()))?;
        private_file(path, &bytes)?;
    }
    replace(&receipt.output, after, &receipt.before_sha256)
}

pub fn rollback(receipt: &ConfigReceipt) -> Result<(), CliError> {
    if !receipt.changed {
        return Ok(());
    }
    let _lock = lock(&receipt.output)?;
    let current = fingerprint(read_config(&receipt.output)?.as_deref());
    if current == receipt.before_sha256 {
        return Ok(());
    }
    if current != receipt.after_sha256 {
        return Err(CliError::Other(
            "agent_config_changed: keeping edits made after installation; backup retained".into(),
        ));
    }
    if let Some(backup) = &receipt.backup {
        let bytes = read_config(backup)?
            .ok_or_else(|| CliError::Other("configuration backup is missing".into()))?;
        if fingerprint(Some(&bytes)) != receipt.before_sha256 {
            return Err(CliError::Other(
                "configuration backup digest mismatch".into(),
            ));
        }
        replace(&receipt.output, &bytes, &receipt.after_sha256)?;
    } else {
        fs::remove_file(&receipt.output)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_lock_prevents_overlap_and_releases_with_its_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("operation.lock");
        let first = lock_metadata(&path).unwrap();
        assert!(lock_metadata(&path).is_err());
        drop(first);
        let _second = lock_metadata(&path).unwrap();
        assert!(path.is_file());
    }
    #[test]
    fn json_preserves_other_servers_and_reinstall_bytes() {
        let old = br#"{"theme":"dark","mcpServers":{"other":{"command":"other"}}}"#;
        let desired = br#"{"mcpServers":{"froglet":{"command":"froglet-node"}}}"#;
        let merged = merge("claude-code", old, desired).unwrap();
        let value: Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(value["theme"], "dark");
        assert_eq!(value["mcpServers"]["other"]["command"], "other");
        assert_eq!(merge("claude-code", &merged, desired).unwrap(), merged);
        assert!(merge("claude-code", b"broken", desired).is_err());
    }
    #[test]
    fn toml_preserves_comments_and_other_tables() {
        let old = b"# personal configuration\nmodel = 'test' # keep this\n\n[mcp_servers.other]\ncommand = 'other' # also keep\n";
        let desired = b"[mcp_servers.froglet]\ncommand = 'froglet-node'\nargs = ['mcp']\n";
        let merged = merge("codex", old, desired).unwrap();
        let text = String::from_utf8(merged.clone()).unwrap();
        assert!(text.contains("model = 'test' # keep this"));
        assert!(text.contains("command = 'other' # also keep"));
        assert_eq!(merge("codex", &merged, desired).unwrap(), merged);
    }
    #[test]
    fn toml_inline_server_table_is_preserved() {
        let merged = merge(
            "codex",
            b"mcp_servers = { other = { command = 'other' } }\n",
            b"[mcp_servers.froglet]\ncommand = 'froglet-node'\n",
        )
        .unwrap();
        let value: toml::Value = toml::from_str(std::str::from_utf8(&merged).unwrap()).unwrap();
        assert_eq!(
            value["mcp_servers"]["other"]["command"].as_str(),
            Some("other")
        );
        assert_eq!(
            value["mcp_servers"]["froglet"]["command"].as_str(),
            Some("froglet-node")
        );
    }
    #[test]
    fn rollback_restores_only_our_write() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("config.toml");
        let backup = dir.path().join("backup");
        fs::write(&output, b"before").unwrap();
        let receipt = ConfigReceipt {
            output: output.clone(),
            before_sha256: fingerprint(Some(b"before")),
            after_sha256: fingerprint(Some(b"after")),
            backup: Some(backup),
            changed: true,
        };
        apply(&receipt, Some(b"before"), b"after", None).unwrap();
        fs::write(&output, b"user edit").unwrap();
        assert!(rollback(&receipt).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"user edit");
        fs::write(&output, b"after").unwrap();
        rollback(&receipt).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"before");
    }
    #[test]
    fn changed_config_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("config");
        fs::write(&output, b"edited").unwrap();
        let receipt = ConfigReceipt {
            output: output.clone(),
            before_sha256: "missing".into(),
            after_sha256: fingerprint(Some(b"after")),
            backup: None,
            changed: true,
        };
        assert!(apply(&receipt, None, b"after", None).is_err());
        assert_eq!(fs::read(output).unwrap(), b"edited");
    }
}
