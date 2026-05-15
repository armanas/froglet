//! `froglet-node whoami` — print identity + daemon info. Useful for
//! "is the daemon up and which provider_id am I publishing as".

use super::{CliError, pop_flag};
use froglet_publish_engine::DaemonClient;

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");

    let daemon = DaemonClient::from_env().map_err(CliError::Engine)?;
    let caps = daemon.capabilities().await.map_err(CliError::Engine)?;

    let clearnet_url = caps
        .transports
        .clearnet
        .as_ref()
        .and_then(|t| t.url.as_deref());
    let tor_url = caps
        .transports
        .tor
        .as_ref()
        .and_then(|t| t.url.as_deref().or(t.onion_url.as_deref()));

    if json_mode {
        let payload = serde_json::json!({
            "daemon_url": daemon.daemon_url.as_str(),
            "provider_id": caps.identity.node_id,
            "clearnet_url": clearnet_url,
            "tor_url": tor_url,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    } else {
        println!("Daemon:      {}", daemon.daemon_url);
        println!("provider_id: {}", caps.identity.node_id);
        if let Some(u) = clearnet_url {
            println!("clearnet:    {u}");
        } else {
            println!("clearnet:    (disabled — set FROGLET_PUBLIC_BASE_URL to expose publicly)");
        }
        if let Some(u) = tor_url {
            println!("tor:         {u}");
        } else {
            println!("tor:         (disabled — set FROGLET_NETWORK_MODE=tor for hidden service)");
        }
    }
    Ok(())
}
