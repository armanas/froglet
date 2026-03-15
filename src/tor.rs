use std::{
    collections::VecDeque,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use tokio::{
    fs,
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, Command},
    sync::mpsc,
};

pub struct TorHiddenService {
    pub onion_url: String,
    child: Child,
}

impl TorHiddenService {
    pub async fn wait(mut self) -> Result<ExitStatus, String> {
        self.child
            .wait()
            .await
            .map_err(|error| format!("failed waiting on Tor sidecar process: {error}"))
    }
}

pub async fn start_hidden_service(
    binary_path: &str,
    tor_dir: PathBuf,
    target_addr: SocketAddr,
    startup_timeout: Duration,
) -> Result<TorHiddenService, String> {
    tracing::info!("Starting external Tor sidecar...");

    let data_dir = tor_dir.join("data");
    let hidden_service_dir = tor_dir.join("hidden_service");
    ensure_secure_dir(&tor_dir)?;
    ensure_secure_dir(&data_dir)?;
    ensure_secure_dir(&hidden_service_dir)?;

    let torrc_path = tor_dir.join("torrc");
    let torrc = build_torrc(&data_dir, &hidden_service_dir, target_addr);
    std::fs::write(&torrc_path, torrc)
        .map_err(|error| format!("failed to write Tor config {torrc_path:?}: {error}"))?;
    set_mode(&torrc_path, 0o600)?;

    let mut command = Command::new(binary_path);
    command
        .arg("-f")
        .arg(&torrc_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn Tor sidecar binary '{binary_path}': {error}"))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture Tor sidecar stderr".to_string())?;
    let (log_tx, mut log_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        forward_tor_logs(stderr, log_tx).await;
    });

    let onion_url = wait_for_tor_startup(
        &mut child,
        &hidden_service_dir,
        startup_timeout,
        &mut log_rx,
    )
    .await?;

    Ok(TorHiddenService { onion_url, child })
}

fn build_torrc(data_dir: &Path, hidden_service_dir: &Path, target_addr: SocketAddr) -> String {
    format!(
        "DataDirectory {}\n\
         SocksPort 0\n\
         HiddenServiceDir {}\n\
         HiddenServiceVersion 3\n\
         HiddenServicePort 80 {}\n\
         Log notice stderr\n",
        torrc_quote_path(data_dir),
        torrc_quote_path(hidden_service_dir),
        target_addr
    )
}

fn torrc_quote_path(path: &Path) -> String {
    let escaped = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{escaped}\"")
}

async fn wait_for_tor_startup(
    child: &mut Child,
    hidden_service_dir: &Path,
    startup_timeout: Duration,
    log_rx: &mut mpsc::UnboundedReceiver<String>,
) -> Result<String, String> {
    let hostname_path = hidden_service_dir.join("hostname");
    let deadline = Instant::now() + startup_timeout;
    let mut recent_logs = VecDeque::with_capacity(20);
    let mut bootstrapped = false;
    let mut onion_url: Option<String> = None;

    loop {
        while let Ok(line) = log_rx.try_recv() {
            remember_log_line(&mut recent_logs, line.clone());
            if line.contains("Bootstrapped 100%") {
                bootstrapped = true;
                tracing::info!("Tor sidecar bootstrapped successfully");
            }
        }

        if onion_url.is_none()
            && let Ok(hostname_contents) = fs::read_to_string(&hostname_path).await
        {
            onion_url = Some(parse_onion_url(&hostname_contents)?);
        }

        if bootstrapped && let Some(onion_url) = onion_url.clone() {
            return Ok(onion_url);
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed checking Tor sidecar status: {error}"))?
        {
            return Err(format!(
                "Tor sidecar exited before startup completed with status {status}{}",
                format_recent_logs(&recent_logs)
            ));
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for Tor sidecar startup after {}s{}",
                startup_timeout.as_secs(),
                format_recent_logs(&recent_logs)
            ));
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn forward_tor_logs(stderr: ChildStderr, log_tx: mpsc::UnboundedSender<String>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.contains("[err]") {
            tracing::error!("tor sidecar: {line}");
        } else if line.contains("[warn]") {
            tracing::warn!("tor sidecar: {line}");
        } else if line.contains("Bootstrapped 100%") {
            tracing::info!("tor sidecar: {line}");
        } else {
            tracing::debug!("tor sidecar: {line}");
        }
        let _ = log_tx.send(line);
    }
}

fn parse_onion_url(hostname_contents: &str) -> Result<String, String> {
    let hostname = hostname_contents.trim();
    if hostname.is_empty() {
        return Err("Tor sidecar wrote an empty hidden-service hostname".to_string());
    }
    if !hostname.ends_with(".onion") {
        return Err(format!(
            "Tor sidecar returned an invalid hidden-service hostname: {hostname}"
        ));
    }
    Ok(format!("http://{hostname}"))
}

fn remember_log_line(recent_logs: &mut VecDeque<String>, line: String) {
    if recent_logs.len() == 20 {
        recent_logs.pop_front();
    }
    recent_logs.push_back(line);
}

fn format_recent_logs(recent_logs: &VecDeque<String>) -> String {
    if recent_logs.is_empty() {
        String::new()
    } else {
        format!(
            "\nRecent Tor logs:\n{}",
            recent_logs.iter().cloned().collect::<Vec<_>>().join("\n")
        )
    }
}

fn ensure_secure_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|error| format!("failed to create Tor directory {path:?}: {error}"))?;
    set_mode(path, 0o700)
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("failed to read metadata for {path:?}: {error}"))?;
        let mut perms = metadata.permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms).map_err(|error| {
            format!("failed to set permissions on {path:?} to {mode:o}: {error}")
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn build_torrc_configures_hidden_service_port_mapping() {
        let data_dir = PathBuf::from("/tmp/froglet-tor/data");
        let hidden_service_dir = PathBuf::from("/tmp/froglet-tor/hidden");
        let target_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 18123);
        let torrc = build_torrc(&data_dir, &hidden_service_dir, target_addr);

        assert!(torrc.contains("SocksPort 0"));
        assert!(torrc.contains("HiddenServiceVersion 3"));
        assert!(torrc.contains("HiddenServicePort 80 127.0.0.1:18123"));
        assert!(torrc.contains("DataDirectory \"/tmp/froglet-tor/data\""));
    }

    #[test]
    fn parse_onion_url_requires_valid_hostname() {
        assert_eq!(
            parse_onion_url("exampleonionaddress.onion\n").unwrap(),
            "http://exampleonionaddress.onion"
        );
        assert!(parse_onion_url("").is_err());
        assert!(parse_onion_url("not-an-onion.example").is_err());
    }
}
