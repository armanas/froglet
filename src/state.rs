use crate::{
    confidential::ConfidentialPolicy, config::NodeConfig, db, db::DbPool,
    execution::BuiltinServiceHandler, identity::NodeIdentity, lnd::LndRestClient,
    pricing::PricingTable, runtime_auth, sandbox::WasmSandbox, settlement::SettlementRegistry, tls,
    wasm_host::WasmHostEnvironment,
};
use serde::Serialize;
use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};
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

pub struct AppState {
    pub db: DbPool,
    pub transport_status: Arc<TokioMutex<TransportStatus>>,
    pub wasm_sandbox: Arc<WasmSandbox>,
    pub config: NodeConfig,
    pub identity: Arc<NodeIdentity>,
    pub pricing: PricingTable,
    pub http_client: reqwest::Client,
    pub wasm_host: Option<Arc<WasmHostEnvironment>>,
    pub confidential_policy: Option<Arc<ConfidentialPolicy>>,
    pub runtime_auth_token: String,
    pub runtime_auth_token_path: PathBuf,
    pub consumer_control_auth_token: String,
    pub consumer_control_auth_token_path: PathBuf,
    pub provider_control_auth_token: String,
    pub provider_control_auth_token_path: PathBuf,
    pub events_query_semaphore: Arc<Semaphore>,
    pub lnd_rest_client: Option<Arc<LndRestClient>>,
    pub lightning_destination_identity: Arc<OnceCell<String>>,
    pub event_batch_writer: Option<db::EventBatchWriter>,
    pub builtin_services: HashMap<String, Arc<dyn BuiltinServiceHandler>>,
    pub settlement_registry: SettlementRegistry,
    /// Short-lived session-token pool. `Some` only when
    /// `FROGLET_SESSION_POOL_ENABLED=1`. See `src/session_pool.rs` and
    /// `docs/SYSTEM_DESIGN.md §8`.
    pub session_pool: Option<crate::session_pool::SessionPool>,
}

pub fn ensure_storage_dirs(config: &NodeConfig) -> Result<(), String> {
    for path in [
        &config.storage.data_dir,
        &config.storage.runtime_dir,
        &config.storage.tor_dir,
        &config.storage.identity_dir,
    ] {
        std::fs::create_dir_all(path)
            .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    }
    Ok(())
}

pub fn build_app_state(config: NodeConfig) -> Result<Arc<AppState>, String> {
    tls::ensure_rustls_crypto_provider();
    ensure_storage_dirs(&config)?;

    let wasm_sandbox = Arc::new(WasmSandbox::from_env()?);
    wasm_sandbox.warm_up();

    let identity = Arc::new(NodeIdentity::load_or_create(&config)?);
    let runtime_auth = runtime_auth::load_or_create_local_runtime_auth(&config)?;
    let consumer_control_auth_token = runtime_auth::load_or_create_local_token(
        &config.storage.runtime_dir,
        &config.storage.consumer_control_auth_token_path,
        "consumer control auth token",
        config.storage.runtime_dir_mode(),
        0o600,
    )?;
    let provider_control_auth_token = runtime_auth::load_or_create_local_token(
        &config.storage.runtime_dir,
        &config.storage.provider_control_auth_token_path,
        "provider control auth token",
        config.storage.runtime_dir_mode(),
        config.storage.provider_control_token_mode(),
    )?;
    let db_pool = DbPool::open(&config.storage.db_path)
        .map_err(|error| format!("failed to initialize SQLite DB pool: {error}"))?;
    let events_query_capacity = db_pool.read_connection_count().max(1);
    let http_client = tls::build_reqwest_client(config.http_ca_cert_path.as_deref())
        .map_err(|error| format!("failed to initialize shared HTTP client: {error}"))?;
    let wasm_host = config
        .wasm
        .policy
        .clone()
        .map(WasmHostEnvironment::from_policy)
        .transpose()
        .map(|environment| environment.map(Arc::new))?;
    let lnd_rest_client = config
        .lightning
        .lnd_rest
        .as_ref()
        .map(LndRestClient::from_config)
        .transpose()
        .map_err(|error| format!("failed to initialize cached LND REST client: {error}"))?
        .map(Arc::new);

    let settlement_registry = SettlementRegistry::new(&config);

    let session_pool = if config.session_pool.enabled {
        Some(crate::session_pool::SessionPool::new(
            config.session_pool.size,
            std::time::Duration::from_secs(config.session_pool.ttl_secs),
        ))
    } else {
        None
    };

    Ok(Arc::new(AppState {
        db: db_pool,
        transport_status: Arc::new(TokioMutex::new(TransportStatus::from_config(&config))),
        wasm_sandbox,
        pricing: PricingTable::from_config(config.pricing),
        identity,
        config: config.clone(),
        http_client,
        wasm_host,
        confidential_policy: config.confidential.policy.clone().map(Arc::new),
        runtime_auth_token: runtime_auth.token,
        runtime_auth_token_path: config.storage.runtime_auth_token_path.clone(),
        consumer_control_auth_token,
        consumer_control_auth_token_path: config.storage.consumer_control_auth_token_path.clone(),
        provider_control_auth_token,
        provider_control_auth_token_path: config.storage.provider_control_auth_token_path.clone(),
        events_query_semaphore: Arc::new(Semaphore::new(events_query_capacity)),
        lnd_rest_client,
        lightning_destination_identity: Arc::new(OnceCell::new()),
        event_batch_writer: None,
        builtin_services: HashMap::new(),
        settlement_registry,
        session_pool,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig, PaymentBackend,
        PricingConfig, StorageConfig, TorSidecarConfig, WasmConfig,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_config(network_mode: NetworkMode, public_base_url: Option<&str>) -> NodeConfig {
        NodeConfig {
            network_mode,
            listen_addr: "0.0.0.0:8080".to_string(),
            public_base_url: public_base_url.map(str::to_string),
            runtime_listen_addr: "127.0.0.1:8081".to_string(),
            runtime_allow_non_loopback: false,
            http_ca_cert_path: None,
            tor: TorSidecarConfig {
                binary_path: "tor".to_string(),
                backend_listen_addr: "127.0.0.1:8082".to_string(),
                startup_timeout_secs: 90,
            },
            identity: IdentityConfig {
                auto_generate: true,
            },
            pricing: PricingConfig {
                events_query: 0,
                execute_wasm: 0,
            },
            payment_backends: vec![PaymentBackend::None],
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
            x402: None,
            stripe: None,
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
                consumer_control_auth_token_path: PathBuf::from("./data/runtime/consumerctl.token"),
                provider_control_auth_token_path: PathBuf::from(
                    "./data/runtime/froglet-control.token",
                ),
                tor_dir: PathBuf::from("./data/tor"),
                host_readable_control_token: false,
            },
            wasm: WasmConfig {
                policy_path: None,
                policy: None,
            },
            confidential: crate::confidential::ConfidentialConfig {
                policy_path: None,
                policy: None,
                session_ttl_secs: 300,
            },
            marketplace_url: None,
            postgres_mounts: std::collections::BTreeMap::new(),
            session_pool: Default::default(),
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

    #[cfg(unix)]
    #[test]
    fn build_app_state_keeps_provider_control_token_host_readable_when_enabled() {
        use std::os::unix::fs::PermissionsExt;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("valid time")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("froglet-state-host-readable-{unique}"));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");

        let mut config = test_config(NetworkMode::Clearnet, None);
        config.storage = StorageConfig {
            data_dir: temp_dir.clone(),
            db_path: temp_dir.join("node.db"),
            identity_dir: temp_dir.join("identity"),
            identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
            nostr_publication_seed_path: temp_dir.join("identity/nostr-publication.secp256k1.seed"),
            runtime_dir: temp_dir.join("runtime"),
            runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
            consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
            provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
            tor_dir: temp_dir.join("tor"),
            host_readable_control_token: true,
        };

        let state = build_app_state(config).expect("app state");
        let data_mode =
            std::fs::metadata(&state.config.storage.data_dir).expect("data dir metadata");
        let runtime_mode =
            std::fs::metadata(&state.config.storage.runtime_dir).expect("runtime dir metadata");
        let provider_token_mode =
            std::fs::metadata(&state.config.storage.provider_control_auth_token_path)
                .expect("provider control token metadata");
        let runtime_token_mode = std::fs::metadata(&state.config.storage.runtime_auth_token_path)
            .expect("runtime token metadata");

        assert_eq!(data_mode.permissions().mode() & 0o777, 0o755);
        assert_eq!(runtime_mode.permissions().mode() & 0o777, 0o755);
        assert_eq!(provider_token_mode.permissions().mode() & 0o777, 0o644);
        assert_eq!(runtime_token_mode.permissions().mode() & 0o777, 0o600);
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
