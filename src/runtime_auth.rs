use crate::config::NodeConfig;
use rand::RngCore;
use std::{fs, path::Path};

#[derive(Debug, Clone)]
pub struct LocalRuntimeAuth {
    pub token: String,
}

pub fn load_or_create_local_runtime_auth(config: &NodeConfig) -> Result<LocalRuntimeAuth, String> {
    let token = load_or_create_token(
        &config.storage.runtime_dir,
        &config.storage.runtime_auth_token_path,
        "runtime auth token",
    )?;

    Ok(LocalRuntimeAuth { token })
}

pub fn load_or_create_local_token(
    dir_path: &Path,
    token_path: &Path,
    label: &str,
) -> Result<String, String> {
    load_or_create_token(dir_path, token_path, label)
}

fn load_or_create_token(dir_path: &Path, token_path: &Path, label: &str) -> Result<String, String> {
    ensure_dir(dir_path, 0o700)?;

    if token_path.exists() {
        load_token(token_path, label)
    } else {
        let token = generate_token();
        persist_token(token_path, &token, label)?;
        Ok(token)
    }
}

fn load_token(path: &Path, label: &str) -> Result<String, String> {
    let token = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {label} {}: {e}", path.display()))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(format!("{label} {} is empty", path.display()));
    }
    Ok(token)
}

fn persist_token(path: &Path, token: &str, label: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| {
                format!("Failed to create {label} {}: {e}", path.display())
            })?;
        file.write_all(token.as_bytes())
            .map_err(|e| format!("Failed to write {label} {}: {e}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, token).map_err(|e| format!("Failed to write {label} {}: {e}", path.display()))?;
    }

    Ok(())
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn ensure_dir(path: &Path, mode: u32) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|e| format!("Failed to create directory {}: {e}", path.display()))?;
    set_mode(path, mode)
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path)
            .map_err(|e| format!("Failed to read metadata for {}: {e}", path.display()))?;
        let mut perms = metadata.permissions();
        perms.set_mode(mode);
        fs::set_permissions(path, perms)
            .map_err(|e| format!("Failed to set permissions on {}: {e}", path.display()))?;
    }

    Ok(())
}
