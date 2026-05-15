//! TorBackend — queries the running `froglet-node` daemon for its Tor
//! `transports.tor.url` and uses that as the public URL.
//!
//! Architectural note: the engine does NOT start its own Tor process.
//! The daemon is in charge of runtime lifecycle (it has
//! `src/tor.rs::start_hidden_service()` wired into its supervisor);
//! the engine is in charge of *publishing*. If the daemon isn't
//! configured for Tor (`FROGLET_NETWORK_MODE=tor` or `dual`), the
//! engine surfaces a clear error pointing at the env var the operator
//! needs to set.
//!
//! For Phase 1A.3 this is a thin HTTP probe of
//! `<daemon>/v1/node/capabilities`. Real reachability validation
//! through Tor happens at marketplace registration time.

use super::{HostingBackend, PreparedHosting};
use crate::error::PublishError;
use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

#[derive(Debug, Clone)]
pub struct TorBackend {
    /// Where to reach the local daemon for its capabilities. Default
    /// `http://127.0.0.1:8080`.
    pub daemon_url: Url,
}

impl TorBackend {
    pub fn new(daemon_url: Url) -> Self {
        Self { daemon_url }
    }

    pub fn local() -> Self {
        Self {
            daemon_url: Url::parse("http://127.0.0.1:8080").expect("static url parses"),
        }
    }
}

impl Default for TorBackend {
    fn default() -> Self {
        Self::local()
    }
}

#[derive(Debug, Deserialize)]
struct Capabilities {
    transports: Transports,
}

#[derive(Debug, Deserialize)]
struct Transports {
    #[serde(default)]
    tor: Option<TorTransport>,
}

#[derive(Debug, Deserialize)]
struct TorTransport {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    onion_url: Option<String>,
}

#[async_trait]
impl HostingBackend for TorBackend {
    fn name(&self) -> &'static str {
        "tor"
    }

    async fn prepare(&self) -> Result<PreparedHosting, PublishError> {
        let mut capabilities_url = self.daemon_url.clone();
        capabilities_url.set_path("/v1/node/capabilities");

        let response = reqwest::Client::new()
            .get(capabilities_url.clone())
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| PublishError::Hosting {
                backend: "tor",
                reason: format!("could not reach daemon at {capabilities_url}: {e}"),
            })?;
        if !response.status().is_success() {
            return Err(PublishError::Hosting {
                backend: "tor",
                reason: format!(
                    "daemon at {capabilities_url} returned HTTP {}: is froglet-node running?",
                    response.status()
                ),
            });
        }
        let capabilities: Capabilities =
            response.json().await.map_err(|e| PublishError::Hosting {
                backend: "tor",
                reason: format!("daemon capabilities JSON parse failed: {e}"),
            })?;

        let tor = capabilities
            .transports
            .tor
            .ok_or_else(|| PublishError::Hosting {
                backend: "tor",
                reason: "daemon does not advertise a Tor transport; set FROGLET_NETWORK_MODE=tor (or dual) and restart froglet-node".to_string(),
            })?;
        if !tor.enabled {
            return Err(PublishError::Hosting {
                backend: "tor",
                reason: "daemon Tor transport is disabled; set FROGLET_NETWORK_MODE=tor (or dual) and restart froglet-node".to_string(),
            });
        }
        let onion = tor.url.or(tor.onion_url).ok_or_else(|| PublishError::Hosting {
            backend: "tor",
            reason: "daemon Tor transport enabled but no onion URL published yet; check froglet-node logs for hidden service bootstrap".to_string(),
        })?;

        Ok(PreparedHosting {
            public_url: onion,
            register_with_marketplace: true,
        })
    }
}

#[cfg(test)]
mod tests {
    // Real integration tests live in Phase 1A.8 (depends on a running
    // daemon configured for Tor). Unit-testing the JSON shape would
    // duplicate the protocol's existing tests; skipped here to keep
    // the engine crate light.
}
