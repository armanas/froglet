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

const ONION_V3_LABEL_LEN: usize = 56;

#[derive(Debug, Clone)]
pub struct TorBackend {
    /// Where to reach the local daemon for its capabilities.
    pub daemon_url: Url,
}

impl TorBackend {
    pub fn new(daemon_url: Url) -> Self {
        Self { daemon_url }
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

pub(crate) fn validated_public_tor_url(value: &str) -> Result<String, PublishError> {
    let url = Url::parse(value).map_err(|error| PublishError::Hosting {
        backend: "tor",
        reason: format!("daemon advertised an invalid Tor URL: {error}"),
    })?;
    let host = url.host_str().ok_or_else(|| PublishError::Hosting {
        backend: "tor",
        reason: "daemon Tor URL has no onion host".to_string(),
    })?;
    let onion_label = host.strip_suffix(".onion").unwrap_or_default();
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || onion_label.len() != ONION_V3_LABEL_LEN
        || !onion_label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
    {
        return Err(PublishError::Hosting {
            backend: "tor",
            reason: "daemon Tor URL must be a credential-free http://<56-char-v3>.onion origin"
                .to_string(),
        });
    }
    Ok(format!("http://{host}"))
}

#[async_trait]
impl HostingBackend for TorBackend {
    fn name(&self) -> &'static str {
        "tor"
    }

    async fn prepare(&self) -> Result<PreparedHosting, PublishError> {
        let mut capabilities_url = self.daemon_url.clone();
        capabilities_url.set_path("/v1/node/capabilities");

        let response = crate::http_client_builder()
            .build()?
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
        let onion = tor
            .url
            .or(tor.onion_url)
            .ok_or_else(|| PublishError::Hosting {
                backend: "tor",
                reason: "daemon Tor transport enabled but no onion URL published yet; check froglet-node logs for hidden service bootstrap".to_string(),
            })?;

        Ok(PreparedHosting {
            public_url: validated_public_tor_url(&onion)?,
            register_with_marketplace: true,
            publication_revision: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn v3_onion_url() -> String {
        format!("http://{}.onion", "a".repeat(ONION_V3_LABEL_LEN))
    }

    async fn capabilities_daemon(onion_url: String) -> (Url, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            let body = format!(
                r#"{{"transports":{{"tor":{{"enabled":true,"onion_url":"{onion_url}"}}}}}}"#
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        (Url::parse(&format!("http://{address}")).unwrap(), server)
    }

    #[test]
    fn accepts_an_exact_v3_onion_origin() {
        assert_eq!(
            validated_public_tor_url(&format!("{}/", v3_onion_url())).unwrap(),
            v3_onion_url()
        );
    }

    #[test]
    fn rejects_malformed_or_non_origin_tor_urls() {
        let valid_host = format!("{}.onion", "a".repeat(ONION_V3_LABEL_LEN));
        for invalid in [
            "not-a-url".to_string(),
            format!("https://{valid_host}"),
            format!("http://user@{valid_host}"),
            format!("http://{valid_host}:8080"),
            format!("http://{}.onion", "0".repeat(ONION_V3_LABEL_LEN)),
            format!("http://{valid_host}#fragment"),
        ] {
            assert!(
                validated_public_tor_url(&invalid).is_err(),
                "unexpected valid Tor URL: {invalid}"
            );
        }
    }

    #[test]
    fn rejects_v2_path_and_query_urls() {
        assert!(validated_public_tor_url("http://abcdefghijklmnop.onion").is_err());
        assert!(validated_public_tor_url(&format!("{}/service", v3_onion_url())).is_err());
        assert!(validated_public_tor_url(&format!("{}?token=x", v3_onion_url())).is_err());
    }

    #[tokio::test]
    async fn prepare_applies_the_exact_origin_validator() {
        let (daemon_url, server) = capabilities_daemon(format!("{}/service", v3_onion_url())).await;
        let error = TorBackend::new(daemon_url)
            .prepare()
            .await
            .expect_err("Tor prepare must reject a path-bearing endpoint");
        assert!(error.to_string().contains("56-char-v3"));
        server.await.unwrap();
    }
}
