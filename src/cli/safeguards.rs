//! Authenticated operator controls. Invitation credentials go to private files,
//! never the agent transcript; revocation does not erase usage or existing deals.
use super::{CliError, pop_flag, pop_kv};
use serde_json::{Map, Value, json};
use std::{io::Write, path::PathBuf};

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    pop_flag(&mut args, "--json");
    let mut arguments = Map::new();
    for (flag, key) in [
        ("--reason", "reason"),
        ("--name", "name"),
        ("--token-file", "token_file"),
        ("--invite-id", "invite_id"),
    ] {
        if let Some(value) = pop_kv(&mut args, flag) {
            arguments.insert(key.into(), json!(value));
        }
    }
    for (flag, key) in [
        ("--expires-at", "expires_at"),
        ("--max-requests", "max_requests"),
    ] {
        if let Some(value) = pop_kv(&mut args, flag) {
            arguments.insert(
                key.into(),
                json!(value.parse::<u64>().map_err(|_| CliError::BadArgs(format!(
                    "{flag} requires a non-negative integer"
                )))?),
            );
        }
    }
    let operation = match args.as_slice() {
        [] => "safeguards_status",
        [op] => match op.as_str() {
            "status" => "safeguards_status",
            "pause" => "safeguards_pause",
            "resume" => "safeguards_resume",
            "prune-cache" => "prune_cache",
            "invite-create" => "invite_create",
            "invite-list" => "invite_list",
            "invite-revoke" => "invite_revoke",
            _ => return Err(usage()),
        },
        _ => return Err(usage()),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&action(operation, &arguments).await?)
            .map_err(|e| CliError::Other(e.to_string()))?
    );
    Ok(())
}
fn usage() -> CliError {
    CliError::BadArgs("safeguards status|pause|resume|prune-cache|invite-list|invite-create|invite-revoke [--json]; invite-create requires --name, --expires-at, --max-requests, --token-file; invite-revoke requires --invite-id".into())
}
fn string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, CliError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CliError::BadArgs(format!("{key} is required")))
}

pub async fn action(action: &str, args: &Map<String, Value>) -> Result<Value, CliError> {
    let (path, payload) = match action {
        "safeguards_status" => ("/v1/provider/usage".into(), None),
        "safeguards_pause" | "safeguards_resume" => (
            "/v1/provider/control".into(),
            Some(json!({"paused":action=="safeguards_pause","reason":args.get("reason")})),
        ),
        "prune_cache" => ("/v1/provider/maintenance".into(), Some(json!({}))),
        "invite_list" => ("/v1/provider/invites".into(), None),
        "invite_create" => (
            "/v1/provider/invites".into(),
            Some(
                json!({"name":string(args,"name")?,"expires_at":args.get("expires_at"),"max_requests":args.get("max_requests")}),
            ),
        ),
        "invite_revoke" => {
            let id = string(args, "invite_id")?;
            if id.len() != 64
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(CliError::BadArgs("invalid invite_id".into()));
            }
            (format!("/v1/provider/invites/{id}/revoke"), Some(json!({})))
        }
        _ => return Err(usage()),
    };
    // Reserve the output path before issuing a credential, so an existing file
    // or invalid destination cannot leave an undisclosed usable invitation.
    let mut token_file = if action == "invite_create" {
        let path = PathBuf::from(string(args, "token_file")?);
        if !path.is_absolute() {
            return Err(CliError::BadArgs("token_file must be absolute".into()));
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        Some((path.clone(), options.open(&path)?))
    } else {
        None
    };
    let result = request(&path, payload.as_ref()).await;
    let mut body = match result {
        Ok(body) => body,
        Err(error) => {
            if let Some((path, _)) = &token_file {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
    };
    if let Some((path, file)) = token_file.as_mut() {
        let Some(token) = body
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(|s| zeroize::Zeroizing::new(s.to_string()))
        else {
            revoke_failed_invite(&body).await;
            let _ = std::fs::remove_file(path);
            return Err(CliError::Daemon("invitation response omitted a valid credential; inspect invite_list and revoke the undelivered invitation".into()));
        };
        body.as_object_mut()
            .ok_or_else(|| CliError::Daemon("invalid invitation response".into()))?
            .remove("access_token");
        if let Err(error) = file
            .write_all(token.as_bytes())
            .and_then(|_| file.sync_all())
        {
            revoke_failed_invite(&body).await;
            let _ = std::fs::remove_file(path);
            return Err(CliError::Io(error));
        }
        body["token_file"] = json!(path);
        body["next_action"] = json!(
            "Deliver this private credential file to the intended recipient through your trusted channel. Invoke with access_token_file; never include it in a URL or QR code."
        );
    }
    Ok(body)
}

async fn revoke_failed_invite(body: &Value) {
    if let Some(id) = body["id"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        let _ = request(
            &format!("/v1/provider/invites/{id}/revoke"),
            Some(&json!({})),
        )
        .await;
    }
}

async fn request(path: &str, payload: Option<&Value>) -> Result<Value, CliError> {
    let daemon = froglet_publish_engine::DaemonClient::from_env().map_err(CliError::Engine)?;
    let token = daemon
        .control_auth
        .resolve()
        .await
        .map_err(CliError::Engine)?;
    let client = crate::tls::reqwest_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| CliError::Other(e.to_string()))?;
    let url = daemon
        .daemon_url
        .join(path)
        .map_err(|e| CliError::Other(e.to_string()))?;
    let builder = match payload {
        Some(body) => client.post(url).json(body),
        None => client.get(url),
    };
    let response = builder
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| CliError::Other(e.to_string()))?;
    let status = response.status();
    let body = crate::http_body::read_json_response_limited(response, 256 * 1024, "safeguards")
        .await
        .map_err(CliError::Other)?;
    if !status.is_success() {
        return Err(CliError::Daemon(format!(
            "safeguards request refused ({status}): {body}"
        )));
    }
    Ok(body)
}
