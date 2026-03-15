use crate::{
    config::NodeConfig, db::DbPool, identity::NodeIdentity, pricing::PricingTable,
    sandbox::WasmSandbox,
};
use serde::Serialize;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex as TokioMutex;

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
        Self {
            clearnet_enabled: config.network_mode.should_start_clearnet(),
            clearnet_url: config
                .network_mode
                .should_start_clearnet()
                .then(|| format!("http://{}", config.listen_addr)),
            tor_enabled: config.network_mode.should_start_tor(),
            tor_onion_url: None,
            tor_status: if config.network_mode.should_start_tor() {
                "starting".to_string()
            } else {
                "disabled".to_string()
            },
        }
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
    pub runtime_auth_token: String,
    pub runtime_auth_token_path: PathBuf,
}
