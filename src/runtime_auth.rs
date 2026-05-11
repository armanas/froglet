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
    // The runtime auth token stays 0o600 even when
    // FROGLET_HOST_READABLE_CONTROL_TOKEN is set: the host-readable flag
    // exists so the operator can view provider-control state from the host,
    // not so that other host users can steal the runtime bearer token
    // (which would grant arbitrary authenticated calls). Keeping this tight
    // is a deliberate security invariant; see
    // `state::tests::build_app_state_keeps_provider_control_token_host_readable_when_enabled`
    // and the private hosted-rotation runbook.
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
    require_bearer_token_or_alt_header(headers, expected_token, scope, None)
}

/// Same gate as [`require_bearer_token`] but additionally accepts the token
/// in a caller-specified alternative header (e.g. `x-froglet-admin-token`).
/// The Authorization header is checked first; on miss, the alt header is
/// consulted. Both comparisons use [`subtle::ConstantTimeEq`] so neither path
/// leaks the expected token via timing.
///
/// Pass `alt_header = None` for "Authorization-only" semantics, identical to
/// [`require_bearer_token`]. Pass `Some("x-froglet-admin-token")` for
/// services that historically accept either header (e.g. the marketplace
/// arbiter — without this, sister implementations drift and one was found
/// using `==` instead of constant-time compare).
pub fn require_bearer_token_or_alt_header(
    headers: &HeaderMap,
    expected_token: &str,
    scope: &str,
    alt_header: Option<&'static str>,
) -> Result<(), (StatusCode, serde_json::Value)> {
    let expected_bytes = expected_token.as_bytes();

    if let Some(header_value) = headers.get(header::AUTHORIZATION) {
        let authorization = header_value.to_str().map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                json!({ "error": format!("invalid {scope} authorization header") }),
            )
        })?;

        if let Some(token) = authorization.strip_prefix("Bearer ") {
            if token.as_bytes().ct_eq(expected_bytes).unwrap_u8() == 1 {
                return Ok(());
            }
            // Authorization header present and well-formed but token did not
            // match. If an alt header is configured, fall through to check
            // it; otherwise reject with the historical message so existing
            // tests that match on this string keep passing.
            if alt_header.is_none() {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    json!({ "error": format!("invalid {scope} authorization token") }),
                ));
            }
        } else if alt_header.is_none() {
            return Err((
                StatusCode::UNAUTHORIZED,
                json!({ "error": format!("invalid {scope} authorization scheme") }),
            ));
        }
    } else if alt_header.is_none() {
        return Err((
            StatusCode::UNAUTHORIZED,
            json!({ "error": format!("missing {scope} authorization") }),
        ));
    }

    if let Some(name) = alt_header
        && let Some(value) = headers.get(name)
        && let Ok(value) = value.to_str()
        && value.trim().as_bytes().ct_eq(expected_bytes).unwrap_u8() == 1
    {
        return Ok(());
    }

    Err((
        StatusCode::UNAUTHORIZED,
        json!({ "error": format!("invalid {scope} authorization token") }),
    ))
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
    use super::{
        load_or_create_local_token, require_bearer_token, require_bearer_token_or_alt_header,
    };
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

    #[test]
    fn alt_header_path_accepts_matching_token_when_authorization_is_absent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-froglet-admin-token",
            HeaderValue::from_static("admin-secret"),
        );

        let result = require_bearer_token_or_alt_header(
            &headers,
            "admin-secret",
            "admin",
            Some("x-froglet-admin-token"),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn alt_header_path_accepts_matching_token_when_authorization_mismatches() {
        // Drift-protection regression: an attacker cannot steal admin
        // access by sending a non-matching Authorization that previously
        // would short-circuit before the alt-header check.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong-token"),
        );
        headers.insert(
            "x-froglet-admin-token",
            HeaderValue::from_static("admin-secret"),
        );

        let result = require_bearer_token_or_alt_header(
            &headers,
            "admin-secret",
            "admin",
            Some("x-froglet-admin-token"),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn alt_header_path_rejects_when_neither_header_matches() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong"),
        );
        headers.insert(
            "x-froglet-admin-token",
            HeaderValue::from_static("also-wrong"),
        );

        let error = require_bearer_token_or_alt_header(
            &headers,
            "admin-secret",
            "admin",
            Some("x-froglet-admin-token"),
        )
        .expect_err("both headers wrong must reject");

        assert_eq!(error.0, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn alt_header_path_rejects_when_no_headers_present() {
        let headers = HeaderMap::new();

        let error = require_bearer_token_or_alt_header(
            &headers,
            "admin-secret",
            "admin",
            Some("x-froglet-admin-token"),
        )
        .expect_err("missing headers must reject");

        assert_eq!(error.0, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn alt_header_disabled_preserves_authorization_only_behavior() {
        // Sanity: with alt_header=None, behavior is byte-identical to
        // require_bearer_token. An x-froglet-admin-token header is ignored.
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-froglet-admin-token",
            HeaderValue::from_static("test-token"),
        );

        let error = require_bearer_token_or_alt_header(&headers, "test-token", "runtime", None)
            .expect_err("alt header must not be honored when alt_header=None");

        assert_eq!(error.0, StatusCode::UNAUTHORIZED);
        assert_eq!(error.1["error"], "missing runtime authorization");
    }
}
