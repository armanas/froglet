use crate::config::NodeConfig;
use rand::RngCore;
use std::{fs, path::Path};

#[derive(Debug, Clone)]
pub struct LocalRuntimeAuth {
    pub token: String,
}

pub fn load_or_create_local_runtime_auth(config: &NodeConfig) -> Result<LocalRuntimeAuth, String> {
    ensure_dir(&config.storage.runtime_dir, 0o700)?;

    let token = if config.storage.runtime_auth_token_path.exists() {
        load_token(&config.storage.runtime_auth_token_path)?
    } else {
        let token = generate_token();
        persist_token(&config.storage.runtime_auth_token_path, &token)?;
        token
    };

    Ok(LocalRuntimeAuth { token })
}

fn load_token(path: &Path) -> Result<String, String> {
    let token = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read runtime auth token {}: {e}", path.display()))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(format!("Runtime auth token {} is empty", path.display()));
    }
    Ok(token)
}

fn persist_token(path: &Path, token: &str) -> Result<(), String> {
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
                format!(
                    "Failed to create runtime auth token {}: {e}",
                    path.display()
                )
            })?;
        file.write_all(token.as_bytes())
            .map_err(|e| format!("Failed to write runtime auth token {}: {e}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, token)
            .map_err(|e| format!("Failed to write runtime auth token {}: {e}", path.display()))?;
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
