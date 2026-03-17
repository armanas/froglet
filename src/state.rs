use crate::{
    config::NodeConfig, db::DbPool, identity::NodeIdentity, lnd::LndRestClient,
    pricing::PricingTable, sandbox::WasmSandbox, wasm_host::WasmHostEnvironment,
};
use serde::Serialize;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::{Mutex as TokioMutex, OnceCell, Semaphore};

fn advertiseable_clearnet_url(addr: SocketAddr) -> Option<String> {
    (!addr.ip().is_unspecified()).then(|| format!("http://{}", addr))
}

fn configured_clearnet_url(config: &NodeConfig) -> Option<String> {
    config.public_base_url.clone().or_else(|| {
        config
            .listen_addr
            .parse::<SocketAddr>()
            .ok()
            .and_then(advertiseable_clearnet_url)
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct TransportStatus {
    pub clearnet_enabled: bool,
    pub clearnet_url: Option<String>,
    pub tor_enabled: bool,
    pub tor_onion_url: Option<String>,
    pub tor_status: String,
}

impl TransportStatus {
    pub fn from_config(config: &NodeConfig) -> Self {
        let clearnet_enabled = config.network_mode.should_start_clearnet();
        Self {
            clearnet_enabled,
            clearnet_url: clearnet_enabled
                .then(|| configured_clearnet_url(config))
                .flatten(),
            tor_enabled: config.network_mode.should_start_tor(),
            tor_onion_url: None,
            tor_status: if config.network_mode.should_start_tor() {
                "starting".to_string()
            } else {
                "disabled".to_string()
            },
        }
    }

    pub fn update_clearnet_bound_addr(
        &mut self,
        config: &NodeConfig,
        bound_addr: SocketAddr,
    ) -> Result<(), String> {
        if !self.clearnet_enabled {
            return Ok(());
        }

        if let Some(public_base_url) = config.public_base_url.clone() {
            self.clearnet_url = Some(public_base_url);
            return Ok(());
        }

        self.clearnet_url = advertiseable_clearnet_url(bound_addr);
        if self.clearnet_url.is_none() {
            return Err(
                "FROGLET_PUBLIC_BASE_URL is required whenever FROGLET_LISTEN_ADDR binds to a wildcard address"
                    .to_string(),
            );
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceStatus {
    pub publish_enabled: bool,
    pub connected: bool,
    pub last_register_at: Option<i64>,
    pub last_heartbeat_at: Option<i64>,
    pub last_error: Option<String>,
}

impl MarketplaceStatus {
    pub fn from_config(config: &NodeConfig) -> Self {
        Self {
            publish_enabled: config
                .marketplace
                .as_ref()
                .map(|marketplace| marketplace.publish)
                .unwrap_or(false),
            connected: false,
            last_register_at: None,
            last_heartbeat_at: None,
            last_error: None,
        }
    }
}

pub struct AppState {
    pub db: DbPool,
    pub transport_status: Arc<TokioMutex<TransportStatus>>,
    pub marketplace_status: Arc<TokioMutex<MarketplaceStatus>>,
    pub wasm_sandbox: Arc<WasmSandbox>,
    pub config: NodeConfig,
    pub identity: Arc<NodeIdentity>,
    pub pricing: PricingTable,
    pub http_client: reqwest::Client,
    pub wasm_host: Option<Arc<WasmHostEnvironment>>,
    pub runtime_auth_token: String,
    pub runtime_auth_token_path: PathBuf,
    pub events_query_semaphore: Arc<Semaphore>,
    pub lnd_rest_client: Option<Arc<LndRestClient>>,
    pub lightning_destination_identity: Arc<OnceCell<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        DiscoveryMode, IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
        PaymentBackend, PricingConfig, StorageConfig, TorSidecarConfig, WasmConfig,
    };

    fn test_config(network_mode: NetworkMode, public_base_url: Option<&str>) -> NodeConfig {
        NodeConfig {
            network_mode,
            listen_addr: "0.0.0.0:8080".to_string(),
            public_base_url: public_base_url.map(str::to_string),
            runtime_listen_addr: "127.0.0.1:8081".to_string(),
            tor: TorSidecarConfig {
                binary_path: "tor".to_string(),
                backend_listen_addr: "127.0.0.1:8082".to_string(),
                startup_timeout_secs: 90,
            },
            discovery_mode: DiscoveryMode::None,
            identity: IdentityConfig {
                auto_generate: true,
            },
            marketplace: None,
            pricing: PricingConfig {
                events_query: 0,
                execute_wasm: 0,
            },
            payment_backend: PaymentBackend::None,
            execution_timeout_secs: 10,
            lightning: LightningConfig {
                mode: LightningMode::Mock,
                destination_identity: None,
                base_invoice_expiry_secs: 300,
                success_hold_expiry_secs: 300,
                min_final_cltv_expiry: 18,
                sync_interval_ms: 1_000,
                lnd_rest: None,
            },
            storage: StorageConfig {
                data_dir: PathBuf::from("./data"),
                db_path: PathBuf::from("./data/node.db"),
                identity_dir: PathBuf::from("./data/identity"),
                identity_seed_path: PathBuf::from("./data/identity/secp256k1.seed"),
                nostr_publication_seed_path: PathBuf::from(
                    "./data/identity/nostr-publication.secp256k1.seed",
                ),
                runtime_dir: PathBuf::from("./data/runtime"),
                runtime_auth_token_path: PathBuf::from("./data/runtime/auth.token"),
                tor_dir: PathBuf::from("./data/tor"),
            },
            wasm: WasmConfig {
                policy_path: None,
                policy: None,
            },
        }
    }

    #[test]
    fn transport_status_uses_public_base_url_override() {
        let status = TransportStatus::from_config(&test_config(
            NetworkMode::Clearnet,
            Some("http://127.0.0.1:8080"),
        ));

        assert_eq!(
            status.clearnet_url.as_deref(),
            Some("http://127.0.0.1:8080")
        );
    }

    #[test]
    fn transport_status_does_not_advertise_clearnet_url_without_clearnet() {
        let status = TransportStatus::from_config(&test_config(
            NetworkMode::Tor,
            Some("http://127.0.0.1:8080"),
        ));

        assert!(!status.clearnet_enabled);
        assert!(status.clearnet_url.is_none());
    }

    #[test]
    fn transport_status_uses_bound_address_when_public_url_is_not_configured() {
        let config = test_config(NetworkMode::Clearnet, None);
        let mut status = TransportStatus::from_config(&config);

        status
            .update_clearnet_bound_addr(
                &config,
                "127.0.0.1:49152".parse().expect("valid socket address"),
            )
            .expect("bound address should be advertiseable");

        assert_eq!(
            status.clearnet_url.as_deref(),
            Some("http://127.0.0.1:49152")
        );
    }

    #[test]
    fn transport_status_keeps_public_url_override_after_binding() {
        let config = test_config(NetworkMode::Clearnet, Some("https://froglet.example"));
        let mut status = TransportStatus::from_config(&config);

        status
            .update_clearnet_bound_addr(
                &config,
                "127.0.0.1:49152".parse().expect("valid socket address"),
            )
            .expect("public base url should remain authoritative");

        assert_eq!(
            status.clearnet_url.as_deref(),
            Some("https://froglet.example")
        );
    }

    #[test]
    fn transport_status_does_not_advertise_wildcard_bind_without_public_url() {
        let status = TransportStatus::from_config(&test_config(NetworkMode::Clearnet, None));
        assert!(status.clearnet_url.is_none());
    }

    #[test]
    fn transport_status_rejects_wildcard_bound_address_without_public_url() {
        let config = test_config(NetworkMode::Clearnet, None);
        let mut status = TransportStatus::from_config(&config);

        let error = status
            .update_clearnet_bound_addr(
                &config,
                "0.0.0.0:49152".parse().expect("valid socket address"),
            )
            .expect_err("wildcard bound address should be rejected");

        assert!(error.contains("FROGLET_PUBLIC_BASE_URL"));
        assert!(status.clearnet_url.is_none());
    }
}
