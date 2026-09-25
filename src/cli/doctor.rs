//! Read-only diagnosis. A failed component does not erase healthy evidence.
use super::{CliError, pop_flag, prepare};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    let preflight = pop_flag(&mut args, "--preflight");
    if !args.is_empty() {
        return Err(CliError::BadArgs(
            "usage: froglet-node doctor [--json]".into(),
        ));
    }
    let report = if preflight {
        installation_preflight().await?
    } else {
        snapshot(&prepare::data_root()).await
    };
    if json_mode {
        println!("{report}");
    } else {
        println!(
            "Froglet: {}",
            report["status"].as_str().unwrap_or("unknown")
        );
        for issue in report["issues"].as_array().into_iter().flatten() {
            println!(
                "{}: {}\n  {}",
                issue["code"].as_str().unwrap_or("unknown"),
                issue["message"].as_str().unwrap_or(""),
                issue["next_action"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

fn url(name: &str, default: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| default.into())
        .trim_end_matches('/')
        .to_string()
}

async fn get(
    client: &reqwest::Client,
    url: String,
    token: Option<String>,
) -> Result<Value, String> {
    let mut last = String::new();
    // Retry only reads. Never turn diagnosis into repair or a replayed deal.
    for attempt in 0..2 {
        let mut request = client.get(&url);
        if let Some(token) = &token {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                return crate::http_body::read_json_response_limited(
                    response,
                    1024 * 1024,
                    "diagnostic response",
                )
                .await;
            }
            Ok(response) => {
                last = format!("HTTP {}", response.status());
                if response.status().is_client_error() {
                    break;
                }
            }
            Err(error) => last = error.to_string(),
        }
        if attempt == 0 {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    Err(last)
}

pub async fn snapshot(root: &Path) -> Value {
    let provider = url("FROGLET_PROVIDER_URL", "http://127.0.0.1:8080");
    let runtime = url("FROGLET_RUNTIME_URL", "http://127.0.0.1:8081");
    snapshot_at(root, &provider, &runtime).await
}

pub async fn snapshot_at(root: &Path, provider: &str, runtime: &str) -> Value {
    let client = match crate::tls::reqwest_client_builder()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return json!({"status":"error", "code":"http_client_unavailable", "error":error.to_string()});
        }
    };
    let (capabilities, runtime_health) = tokio::join!(
        get(&client, format!("{provider}/v1/node/capabilities"), None),
        get(&client, format!("{runtime}/health"), None)
    );
    let mut issues = Vec::new();
    let issue = |code: &str, message: &str, action: &str, retryable: bool| json!({"code":code, "message":message, "next_action":action, "retryable":retryable});
    let installation_present = root.join("identity").exists();
    if capabilities.is_err() {
        issues.push(issue(if installation_present {"node_unreachable"} else {"installation_not_detected"}, "The local provider did not respond.", "Check the installation and its service-manager status. Do not start a second node over the same state directory.", true));
    }
    if runtime_health.is_err() {
        issues.push(issue(
            "runtime_unreachable",
            "The requester runtime did not respond.",
            "Check the configured runtime address and restart the installed service if it stopped.",
            true,
        ));
    }
    let capabilities = capabilities.unwrap_or(Value::Null);
    let relay = capabilities
        .pointer("/transports/relay")
        .cloned()
        .unwrap_or(Value::Null);
    if relay["status"].as_str() == Some("down") {
        issues.push(issue("relay_unavailable", "The public connection is offline; local services may still work.", "Keep Froglet running and restore network access. It reconnects while an approved publication remains active.", true));
    }
    let control_path = std::env::var_os("FROGLET_PROVIDER_CONTROL_TOKEN_PATH")
        .or_else(|| std::env::var_os("FROGLET_PROVIDER_AUTH_TOKEN_PATH"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("runtime/froglet-control.token"));
    let token = std::fs::read_to_string(control_path)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let publications = if let Some(token) = token {
        match get(
            &client,
            format!("{provider}/v1/provider/publications"),
            Some(token),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                issues.push(issue("publication_status_unavailable", &format!("Could not read publication status: {error}"), "Check the provider-control token path and node version; do not paste token contents into chat.", true));
                Value::Null
            }
        }
    } else {
        issues.push(issue(
            "control_token_missing",
            "Publication status cannot be authenticated.",
            "Start the installed node and check its configured token-file path.",
            false,
        ));
        Value::Null
    };
    let mut publication_progress = Vec::new();
    for publication in publications["publications"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(service_id) = publication["service_id"].as_str() else {
            continue;
        };
        let key = crate::crypto::sha256_hex(service_id.as_bytes());
        let observation = std::fs::read(
            root.join("publication-observations")
                .join(format!("{key}.json")),
        )
        .ok()
        .filter(|bytes| bytes.len() <= 64 * 1024)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let Some(mut observation) = observation else {
            continue;
        };
        let selected = publication["selected_revision_hash"].as_str();
        observation["matches_selected_revision"] =
            json!(selected.is_some() && selected == observation["revision_hash"].as_str());
        observation["current_availability_verified"] = json!(false);
        if observation["status"] == "pending_review"
            && observation["matches_selected_revision"] == true
        {
            issues.push(issue("activation_pending", &format!("{service_id}: the last publication did not verify marketplace activation."), "Inspect publication status and marketplace availability. Do not republish merely to retry admission.", true));
        }
        publication_progress.push(observation);
    }
    let updates = prepare::check_updates(root).unwrap_or_else(
        |e| json!({"status":"error", "code":"update_check_failed", "error":e.to_string()}),
    );
    if updates["status"] == "error" {
        issues.push(issue("update_check_failed", "Registered source changes could not be checked.", "Check access to the private preparation registry. Existing published revisions remain unchanged.", true));
    }
    let paths = crate::identity_custody::IdentityPaths {
        data_dir: root.to_path_buf(),
        identity_dir: root.join("identity"),
        node_seed_path: root.join("identity/secp256k1.seed"),
        nostr_publication_seed_path: root.join("identity/nostr-publication.secp256k1.seed"),
    };
    let backup = crate::identity_custody::backup_status(&paths);
    if backup.state != crate::identity_custody::BackupStatusState::Current {
        issues.push(issue("identity_backup_needs_attention", "A current encrypted identity backup has not been verified.", "Ask the agent to run identity backup-status and help create or recover a backup. Keep its recovery key separately; never paste either into chat.", false));
    }
    let agent = std::fs::read(root.join("agent-session.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .unwrap_or(Value::Null);
    if agent.is_null() {
        issues.push(issue("agent_not_observed", "No initialized agent session has called Froglet yet.", "Restart your agent if necessary and ask it to call Froglet status. A shell probe does not establish agent attachment.", false));
    }
    let mut agent_connection = "not_proven";
    #[cfg(unix)]
    if let Some(pid) = agent["process_id"]
        .as_u64()
        .filter(|pid| *pid > 0 && *pid <= i32::MAX as u64)
    {
        // Signal zero only checks existence; it never signals or changes a process.
        if unsafe { libc::kill(pid as i32, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            agent_connection = "disconnected";
            issues.push(issue("agent_disconnected", "The last observed agent connection has ended.", "Open or restart the agent and ask it to call Froglet status. Existing services keep running independently.", false));
        }
    }
    json!({"status":if issues.is_empty() {"ok"} else {"needs_attention"}, "stage":"diagnosis", "checked_at":crate::settlement::current_unix_timestamp(),
        "release":env!("CARGO_PKG_VERSION"), "provider_running":!capabilities.is_null(), "runtime_running":runtime_health.is_ok(), "relay":relay,
        "provider_id":capabilities.pointer("/identity/node_id"), "publications":publications, "publication_progress":publication_progress, "updates":updates, "identity_backup":backup,
        "agent_session":agent, "agent_current_connection":agent_connection, "issues":issues, "state_path":root,
        "availability_note":"Services on this computer are unavailable while it sleeps or is offline."})
}

async fn installation_preflight() -> Result<Value, CliError> {
    let mut listeners = Vec::new();
    let mut ports = Vec::new();
    for (name, default) in [
        ("FROGLET_PROVIDER_URL", "http://127.0.0.1:8080"),
        ("FROGLET_RUNTIME_URL", "http://127.0.0.1:8081"),
    ] {
        let endpoint = reqwest::Url::parse(&url(name, default))
            .map_err(|e| CliError::BadArgs(e.to_string()))?;
        let address: std::net::IpAddr = endpoint
            .host_str()
            .and_then(|host| host.parse().ok())
            .filter(std::net::IpAddr::is_loopback)
            .ok_or_else(|| {
                CliError::BadArgs(
                    "native setup requires literal loopback provider and runtime addresses".into(),
                )
            })?;
        let socket = std::net::SocketAddr::new(
            address,
            endpoint
                .port_or_known_default()
                .ok_or_else(|| CliError::BadArgs("local endpoint has no port".into()))?,
        );
        let listener = tokio::net::TcpListener::bind(socket).await.map_err(|e| CliError::Other(format!("port_unavailable: {socket}: {e}; inspect the existing process or choose distinct unused loopback ports and prepare a new installation plan")))?;
        ports.push(socket.to_string());
        listeners.push(listener);
    }
    Ok(
        json!({"status":"ok", "stage":"ports_available", "ports":ports, "evidence":"both addresses could be bound; node startup rechecks them"}),
    )
}

pub fn error_report(error: &CliError) -> Value {
    if let CliError::Structured { report, .. } = error {
        return report.clone();
    }
    let text = error.to_string();
    let (code, action, retryable) = if text.contains("consent")
        && (text.contains("match") || text.contains("changed"))
    {
        (
            "stale_approval",
            "Prepare a new publication plan and obtain approval for its exact hash.",
            false,
        )
    } else if text.contains("agent_config_changed") {
        (
            "agent_config_changed",
            "Preserve the current settings and prepare a new installation plan.",
            false,
        )
    } else if text.contains("port_unavailable") {
        (
            "port_unavailable",
            "Inspect the existing process or select unused local ports and prepare a new installation plan.",
            false,
        )
    } else if text.contains("provider_unavailable") {
        (
            "provider_unavailable",
            "Check the shared service status and ask the publisher to keep their computer online.",
            true,
        )
    } else if text.contains("receipt_verification_failed") || text.contains("receipt_missing") {
        (
            "receipt_not_verified",
            "Inspect the existing deal and its artifacts; do not repeat the operation as a new call.",
            false,
        )
    } else if text.contains("source_changed") {
        (
            "source_changed",
            "Prepare the updated source, review its preview, and obtain a new publication approval.",
            false,
        )
    } else if text.contains("payment_required") || text.contains("402") {
        (
            "payment_required",
            "Choose a free service or explicitly configure the advanced paid workflow.",
            false,
        )
    } else if text.contains("token") || text.contains("401") {
        (
            "authentication_required",
            "Check the local token-file paths; never paste token contents into chat.",
            false,
        )
    } else if matches!(error, CliError::BadArgs(_) | CliError::Manifest(_)) {
        (
            "invalid_input",
            "Correct the named input and retry; no automatic publication was authorized.",
            false,
        )
    } else {
        (
            "operation_failed",
            "Call doctor and inspect the operation's status before retrying a mutation.",
            false,
        )
    };
    json!({"status":"error", "code":code, "error":text, "retryable":retryable, "next_action":action})
}
