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
        config.storage.runtime_dir_mode(),
        0o600,
    )?;

    Ok(LocalRuntimeAuth { token })
}

pub fn load_or_create_local_token(
    dir_path: &Path,
    token_path: &Path,
    label: &str,
    dir_mode: u32,
    file_mode: u32,
) -> Result<String, String> {
    load_or_create_token(dir_path, token_path, label, dir_mode, file_mode)
}

fn load_or_create_token(
    dir_path: &Path,
    token_path: &Path,
    label: &str,
    dir_mode: u32,
    file_mode: u32,
) -> Result<String, String> {
    ensure_dir(dir_path, dir_mode)?;

    if token_path.exists() {
        set_mode(token_path, file_mode)?;
        load_token(token_path, label)
    } else {
        let token = generate_token();
        persist_token(token_path, &token, label, file_mode)?;
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

fn persist_token(path: &Path, token: &str, label: &str, file_mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(file_mode)
            .open(path)
            .map_err(|e| format!("Failed to create {label} {}: {e}", path.display()))?;
        file.write_all(token.as_bytes())
            .map_err(|e| format!("Failed to write {label} {}: {e}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, token)
            .map_err(|e| format!("Failed to write {label} {}: {e}", path.display()))?;
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

#[cfg(test)]
mod tests {
    use super::load_or_create_local_token;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn existing_token_permissions_are_updated_to_requested_mode() {
        let runtime_dir = std::env::temp_dir().join(format!(
            "froglet-runtime-auth-{}-{}",
            std::process::id(),
            super::generate_token()
        ));
        let token_path = runtime_dir.join("froglet-control.token");

        std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");
        std::fs::write(&token_path, "token-value").expect("write token");
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))
            .expect("set initial token permissions");

        let token = load_or_create_local_token(
            &runtime_dir,
            &token_path,
            "provider control auth token",
            0o755,
            0o644,
        )
        .expect("load token");

        let metadata = std::fs::metadata(&token_path).expect("token metadata");
        assert_eq!(token, "token-value");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o644);

        std::fs::remove_dir_all(&runtime_dir).expect("cleanup runtime dir");
    }
}
