use crate::config::NodeConfig;
use axum::http::{HeaderMap, StatusCode, header};
use rand::RngCore;
use serde_json::json;
use std::{fs, path::Path};
use subtle::ConstantTimeEq;

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

pub fn require_bearer_token(
    headers: &HeaderMap,
    expected_token: &str,
    scope: &str,
) -> Result<(), (StatusCode, serde_json::Value)> {
    let Some(header_value) = headers.get(header::AUTHORIZATION) else {
        return Err((
            StatusCode::UNAUTHORIZED,
            json!({ "error": format!("missing {scope} authorization") }),
        ));
    };

    let authorization = header_value.to_str().map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            json!({ "error": format!("invalid {scope} authorization header") }),
        )
    })?;

    let Some(token) = authorization.strip_prefix("Bearer ") else {
        return Err((
            StatusCode::UNAUTHORIZED,
            json!({ "error": format!("invalid {scope} authorization scheme") }),
        ));
    };

    let valid = token
        .as_bytes()
        .ct_eq(expected_token.as_bytes())
        .unwrap_u8()
        == 1;
    if !valid {
        return Err((
            StatusCode::UNAUTHORIZED,
            json!({ "error": format!("invalid {scope} authorization token") }),
        ));
    }

    Ok(())
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
    use super::{load_or_create_local_token, require_bearer_token};
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
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

    #[test]
    fn bearer_token_validation_accepts_matching_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer test-token"),
        );

        let result = require_bearer_token(&headers, "test-token", "runtime");

        assert!(result.is_ok());
    }

    #[test]
    fn bearer_token_validation_rejects_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic test-token"),
        );

        let error = require_bearer_token(&headers, "test-token", "runtime")
            .expect_err("basic auth should be rejected");

        assert_eq!(error.0, StatusCode::UNAUTHORIZED);
        assert_eq!(error.1["error"], "invalid runtime authorization scheme");
    }
}
