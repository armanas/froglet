use crate::{
    builtins::DataQueryHandlerCache, confidential::ConfidentialPolicy, config::NodeConfig, db,
    db::DbPool, execution::BuiltinServiceHandler, identity::NodeIdentity, lnd::LndRestClient,
    pricing::PricingTable, public_quota::IdentityQuota, runtime_auth, sandbox::WasmSandbox,
    settlement::SettlementRegistry, tls, wasm_host::WasmHostEnvironment,
};
use serde::Serialize;
use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::{Mutex as TokioMutex, OnceCell, RwLock, Semaphore, watch};

fn advertiseable_clearnet_url(addr: SocketAddr) -> Option<String> {
    (!addr.ip().is_unspecified()).then(|| format!("http://{}", addr))
}

/// Map a bound listener address to one this same process can dial:
/// wildcard binds (`0.0.0.0` / `::`) accept loopback connections, so
/// substitute the matching loopback address and keep the bound port.
fn self_dial_addr(bound_addr: SocketAddr) -> SocketAddr {
    if !bound_addr.ip().is_unspecified() {
        return bound_addr;
    }
    let loopback: std::net::IpAddr = match bound_addr.ip() {
        std::net::IpAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
        std::net::IpAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
    };
    SocketAddr::new(loopback, bound_addr.port())
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
    pub relay_enabled: bool,
    pub relay_url: Option<String>,
    pub relay_status: String,
    /// Desired connection state derived only from durable exact publication
    /// grants. Skipped from capability serialization; callers see the
    /// resulting `reserved`/`starting`/`up`/`down` status instead.
    #[serde(skip)]
    relay_activation_tx: watch::Sender<bool>,
    /// Linearizes relay-origin request admission against exact grant and
    /// lifecycle changes. Request middleware holds a read guard through the
    /// complete response; revocation/publish/activation holds a write guard
    /// through database commit and activation-gate synchronization.
    #[serde(skip)]
    relay_publication_gate: Arc<RwLock<()>>,
    /// Self-dialable address of this node's own provider (public router)
    /// listener, recorded at bind time. Lets the runtime half of a dual
    /// node resolve itself as a provider without the operator setting
    /// `FROGLET_RUNTIME_PROVIDER_BASE_URL`. Stays `None` on runtime-only
    /// nodes so loopback provider URLs remain fail-closed there. Internal
    /// routing detail — never serialized into transport-status responses.
    #[serde(skip)]
    pub local_provider_bound_addr: Option<SocketAddr>,
}

impl TransportStatus {
    pub fn from_config(config: &NodeConfig) -> Self {
        let clearnet_enabled = config.network_mode.should_start_clearnet();
        let (relay_activation_tx, _relay_activation_rx) = watch::channel(false);
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
            relay_enabled: config.relay.enabled,
            relay_url: None,
            relay_status: if config.relay.enabled {
                "reserved".to_string()
            } else {
                "disabled".to_string()
            },
            relay_activation_tx,
            relay_publication_gate: Arc::new(RwLock::new(())),
            local_provider_bound_addr: None,
        }
    }

    /// Bind relay planning to the current provider identity. This derives the
    /// exact public URL locally and opens no network connection.
    pub fn configure_relay_planning(
        &mut self,
        config: &NodeConfig,
        provider_id: &str,
        has_matching_grant: bool,
    ) -> Result<(), String> {
        self.relay_url = config.relay.planned_public_url(provider_id)?;
        if !self.relay_enabled {
            self.relay_status = "disabled".to_string();
            self.relay_activation_tx.send_replace(false);
            return Ok(());
        }
        self.relay_status = if has_matching_grant {
            "starting".to_string()
        } else {
            "reserved".to_string()
        };
        self.relay_activation_tx.send_replace(has_matching_grant);
        Ok(())
    }

    pub(crate) fn relay_activation_receiver(&self) -> watch::Receiver<bool> {
        self.relay_activation_tx.subscribe()
    }

    pub(crate) fn set_relay_activation_desired(&self, active: bool) {
        self.relay_activation_tx.send_replace(active);
    }

    pub(crate) fn relay_publication_gate(&self) -> Arc<RwLock<()>> {
        self.relay_publication_gate.clone()
    }

    pub fn update_clearnet_bound_addr(
        &mut self,
        config: &NodeConfig,
        bound_addr: SocketAddr,
    ) -> Result<(), String> {
        if !self.clearnet_enabled {
            return Ok(());
        }

        // The provider listener is bound; remember a self-dialable form of
        // it (wildcard binds are reachable via loopback) so the runtime
        // half of a dual node can resolve itself as a provider.
        self.local_provider_bound_addr = Some(self_dial_addr(bound_addr));

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
    pub process_execution_semaphore: Arc<Semaphore>,
    /// Lazily validated native data-query handlers, isolated to this node and
    /// keyed by the signed contract plus content/package digest.
    pub native_data_query_handlers: DataQueryHandlerCache,
    /// Serializes native-data publication through verification and lifecycle
    /// commit. A failed request must never remove a content-addressed file
    /// adopted by another successful request on the same Froglet Node.
    pub native_data_publication_lock: TokioMutex<()>,
    pub hosted_trial_deal_quota: Option<Arc<IdentityQuota>>,
    pub hosted_trial_session_quota: Arc<IdentityQuota>,
    pub event_publish_quota: Arc<IdentityQuota>,
    pub quote_create_quota: Arc<IdentityQuota>,
    pub confidential_session_quota: Arc<IdentityQuota>,
    pub lnd_rest_client: Option<Arc<LndRestClient>>,
    /// Concrete phoenixd client used by the prepaid (`lightning.prepaid.v1`)
    /// settlement flow.  `Some` only when `LightningMode::Phoenixd` is
    /// configured.  The prepaid flow reaches phoenixd's prepaid-specific
    /// methods (create_invoice / get_incoming_payment / pay_invoice) through
    /// this concrete handle, not the `LightningWallet` trait.
    pub phoenixd_client: Option<Arc<crate::settlement::phoenixd::PhoenixdClient>>,
    /// Backend-neutral Lightning wallet used by `settlement/lightning.rs`.
    /// Populated from `phoenixd_client` (prepaid) or `lnd_rest_client`
    /// (escrow) when configured; `None` in Mock mode and in test fixtures
    /// that set both clients to `None`.
    pub lightning_wallet: Option<crate::settlement::wallet::ArcLightningWallet>,
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
    // Identity recovery deliberately precedes general directory creation:
    // a journaled restore owns the not-yet-installed identity directory and
    // must be completed before `load_or_create` is allowed to generate keys.
    let identity = Arc::new(NodeIdentity::load_or_create(&config)?);
    ensure_storage_dirs(&config)?;

    let wasm_sandbox = Arc::new(WasmSandbox::from_env()?);
    wasm_sandbox.warm_up();

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
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
        .as_secs()
        .try_into()
        .map_err(|_| "system clock exceeds SQLite timestamp range".to_string())?;
    let paused_publications = db_pool
        .pause_publications_for_provider_identity(identity.node_id(), now)
        .map_err(|error| format!("failed to reconcile publication identity: {error}"))?;
    if !paused_publications.is_empty() {
        tracing::warn!(
            provider_id = %identity.node_id(),
            services = ?paused_publications,
            "paused active publications signed by a different provider identity; republish is required"
        );
    }
    let planned_relay_url = config.relay.planned_public_url(identity.node_id())?;
    let expected_relay = planned_relay_url
        .as_deref()
        .zip(config.relay.url.as_deref());
    let relay_grants = db_pool
        .reconcile_publication_transport_grants(expected_relay)
        .map_err(|error| format!("failed to reconcile publication transport grants: {error}"))?;
    let has_matching_relay_grant = !relay_grants.is_empty();
    let mut transport_status = TransportStatus::from_config(&config);
    transport_status.configure_relay_planning(
        &config,
        identity.node_id(),
        has_matching_relay_grant,
    )?;
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

    let phoenixd_client = config
        .lightning
        .phoenixd
        .as_ref()
        .map(crate::settlement::phoenixd::PhoenixdClient::from_config)
        .transpose()
        .map_err(|error| format!("failed to initialize phoenixd client: {error}"))?
        .map(Arc::new);

    // Build the backend-neutral wallet trait object.  phoenixd (prepaid) takes
    // precedence when configured, otherwise the LND REST client (escrow).  At
    // most one Lightning backend is active per node (enforced by
    // `LightningMode`).  Casts are explicit so the concrete Arc is proven to
    // satisfy LightningWallet before it is erased.
    let lightning_wallet: Option<crate::settlement::wallet::ArcLightningWallet> =
        if let Some(client) = phoenixd_client.as_ref() {
            Some(Arc::clone(client) as crate::settlement::wallet::ArcLightningWallet)
        } else {
            lnd_rest_client
                .as_ref()
                .map(|client| Arc::clone(client) as crate::settlement::wallet::ArcLightningWallet)
        };

    let settlement_registry = SettlementRegistry::new(&config)
        .map_err(|error| format!("failed to initialize settlement drivers: {error}"))?;

    let session_pool = if config.session_pool.enabled {
        Some(crate::session_pool::SessionPool::new(
            config.session_pool.size,
            std::time::Duration::from_secs(config.session_pool.ttl_secs),
        ))
    } else {
        None
    };
    let hosted_trial_deal_quota = config.session_pool.enabled.then(|| {
        Arc::new(IdentityQuota::new(
            config.public_quota.hosted_trial_deals_per_identity,
            std::time::Duration::from_secs(config.public_quota.hosted_trial_window_secs),
        ))
    });
    let hosted_trial_session_quota = Arc::new(IdentityQuota::new(
        config.public_quota.hosted_trial_sessions_per_identity,
        std::time::Duration::from_secs(config.public_quota.hosted_trial_window_secs),
    ));
    let public_write_quota_window =
        std::time::Duration::from_secs(config.public_quota.public_write_window_secs);
    let event_publish_quota = Arc::new(IdentityQuota::new(
        config.public_quota.event_publishes_per_identity,
        public_write_quota_window,
    ));
    let quote_create_quota = Arc::new(IdentityQuota::new(
        config.public_quota.quotes_per_identity,
        public_write_quota_window,
    ));
    let confidential_session_quota = Arc::new(IdentityQuota::new(
        config.public_quota.confidential_sessions_per_identity,
        public_write_quota_window,
    ));

    Ok(Arc::new(AppState {
        db: db_pool,
        transport_status: Arc::new(TokioMutex::new(transport_status)),
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
        process_execution_semaphore: Arc::new(Semaphore::new(config.process_limits.concurrency)),
        native_data_query_handlers: DataQueryHandlerCache::default(),
        native_data_publication_lock: TokioMutex::const_new(()),
        hosted_trial_deal_quota,
        hosted_trial_session_quota,
        event_publish_quota,
        quote_create_quota,
        confidential_session_quota,
        lnd_rest_client,
        phoenixd_client,
        lightning_wallet,
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
            relay: crate::config::RelayConfig::default(),
            identity: IdentityConfig {
                auto_generate: true,
            },
            pricing: PricingConfig {
                events_query: 0,
                execute_wasm: 0,
            },
            payment_backends: vec![PaymentBackend::None],
            execution_timeout_secs: 10,
            process_limits: Default::default(),
            public_quota: Default::default(),
            lightning: LightningConfig {
                mode: LightningMode::Mock,
                destination_identity: None,
                base_invoice_expiry_secs: 300,
                success_hold_expiry_secs: 300,
                min_final_cltv_expiry: 18,
                sync_interval_ms: 1_000,
                lnd_rest: None,
                phoenixd: None,
            },
            x402: None,
            stripe: None,
            buyer_stripe: None,
            buyer_phoenixd: None,
            requester_spend: Default::default(),
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
            gpu: Default::default(),
            confidential: crate::confidential::ConfidentialConfig {
                policy_path: None,
                policy: None,
                session_ttl_secs: 300,
            },
            marketplace_url: None,
            marketplace_allow_local: false,
            provider_artifact_root: None,
            postgres_mounts: std::collections::BTreeMap::new(),
            session_pool: Default::default(),
            hosted_trial_origin_secret: None,
        }
    }

    fn signed_test_publication_revision(
        identity: &NodeIdentity,
    ) -> froglet_protocol::publication::SignedPublicationRevision {
        use froglet_protocol::publication::{
            LocalVerificationEvidence, PUBLICATION_REVISION_SCHEMA_V1, PublicationCurrency,
            PublicationRevisionPayload, PublicationRevisionPrice, PublicationRevisionService,
            PublicationSettlement, ResolvedPublicationLimits, sign_publication_revision,
        };

        sign_publication_revision(
            PublicationRevisionPayload {
                schema_version: PUBLICATION_REVISION_SCHEMA_V1.to_string(),
                provider_id: identity.node_id().to_string(),
                service_id: "identity-restart-service".to_string(),
                offer_id: "identity-restart-offer".to_string(),
                offer_hash: "11".repeat(32),
                binding_hash: "22".repeat(32),
                package_digest: "33".repeat(32),
                runtime: "wasm".to_string(),
                package_kind: "inline_module".to_string(),
                build_evidence: None,
                service: PublicationRevisionService {
                    project_id: None,
                    summary: None,
                    starter: None,
                    source_kind: "artifact".to_string(),
                    entrypoint_kind: "handler".to_string(),
                    entrypoint: "run".to_string(),
                    contract_version: "froglet.wasm.run_json.v1".to_string(),
                    mode: "sync".to_string(),
                    mounts: Vec::new(),
                    capabilities: Vec::new(),
                    input_schema: None,
                    output_schema: None,
                },
                limits: ResolvedPublicationLimits {
                    max_input_bytes: 4096,
                    max_runtime_ms: 1000,
                    max_memory_bytes: 8 * 1024 * 1024,
                    max_output_bytes: 4096,
                    fuel_limit: 50_000,
                },
                price: PublicationRevisionPrice {
                    settlement_method: PublicationSettlement::None,
                    currency: PublicationCurrency::Sat,
                    base_amount_minor: 0,
                    success_amount_minor: 0,
                    offer_settlement_method: "none".to_string(),
                },
                local_verification: LocalVerificationEvidence {
                    input_hash: "44".repeat(32),
                    result_hash: "55".repeat(32),
                    expected_output_matched: Some(true),
                },
            },
            |message| identity.sign_message_hex(message),
        )
        .expect("valid test publication revision")
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
    fn restart_after_identity_rotation_pauses_old_active_publication() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("valid time")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!(
            "froglet-state-identity-restart-{}-{unique}",
            std::process::id()
        ));
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
            host_readable_control_token: false,
        };

        let first = build_app_state(config.clone()).expect("first startup");
        let old_provider_id = first.identity.node_id().to_string();
        let signed_revision = signed_test_publication_revision(&first.identity);
        let revision_hash = signed_revision.revision_hash.clone();
        drop(first);

        let conn = db::initialize_db(&config.storage.db_path).expect("open publication db");
        db::persist_and_activate_publication_revision(
            &conn,
            &db::NewPublicationRevision {
                revision_hash: revision_hash.clone(),
                service_id: "identity-restart-service".to_string(),
                offer_id: "identity-restart-offer".to_string(),
                offer_hash: signed_revision.payload.offer_hash.clone(),
                binding_hash: signed_revision.payload.binding_hash.clone(),
                signed_revision_json: serde_json::to_string(&signed_revision)
                    .expect("signed revision JSON"),
                definition_json: serde_json::json!({
                    "offer_id": "identity-restart-offer",
                    "service_id": "identity-restart-service",
                })
                .to_string(),
            },
            "active",
            r#"{"operation":"publish"}"#,
            1,
        )
        .expect("persist old-identity publication");
        drop(conn);

        let paths = crate::identity_custody::IdentityPaths::from(&config);
        let backup_path = temp_dir.join("identity-backup.json");
        let recovery_key_path = temp_dir.join("identity-recovery.key");
        let adapter = crate::identity_custody::RecoveryKeyFileAdapter::create(&recovery_key_path)
            .expect("recovery key");
        crate::identity_custody::create_backup(&paths, &backup_path, &adapter, None)
            .expect("current backup");
        let rotation = crate::identity_custody::rotate_node_identity(
            &paths,
            &adapter,
            None,
            "restart reconciliation test",
        )
        .expect("rotate identity");
        assert_eq!(rotation.old_node_id, old_provider_id);

        let second = build_app_state(config.clone()).expect("restart after rotation");
        assert_eq!(second.identity.node_id(), rotation.new_node_id);
        drop(second);

        let conn = db::initialize_db(&config.storage.db_path).expect("reopen publication db");
        let lifecycle = db::get_publication_lifecycle(&conn, "identity-restart-service")
            .expect("lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.status, "paused");
        assert!(lifecycle.active_revision_hash.is_none());
        assert_eq!(lifecycle.selected_revision_hash, revision_hash);
        assert_eq!(lifecycle.revision_count, 1);
        let revision = db::get_publication_revision(
            &conn,
            "identity-restart-service",
            &lifecycle.selected_revision_hash,
        )
        .expect("revision lookup")
        .expect("revision remains");
        assert_eq!(
            revision.signed_revision["payload"]["provider_id"],
            old_provider_id
        );
        let operations = db::list_publication_operations(&conn, "identity-restart-service", 10)
            .expect("operation log");
        assert!(operations.iter().any(|operation| {
            operation.operation == "pause"
                && operation.evidence["reason"] == "provider_identity_changed"
                && operation.evidence["required_action"] == "republish"
        }));
        drop(conn);
        std::fs::remove_dir_all(&temp_dir).expect("clean test directory");
    }

    #[test]
    fn restart_after_relay_control_url_change_removes_old_grant() {
        let temp_dir = tempfile::tempdir().expect("temporary directory");
        let mut config = test_config(NetworkMode::Clearnet, None);
        config.storage = StorageConfig {
            data_dir: temp_dir.path().to_path_buf(),
            db_path: temp_dir.path().join("node.db"),
            identity_dir: temp_dir.path().join("identity"),
            identity_seed_path: temp_dir.path().join("identity/secp256k1.seed"),
            nostr_publication_seed_path: temp_dir
                .path()
                .join("identity/nostr-publication.secp256k1.seed"),
            runtime_dir: temp_dir.path().join("runtime"),
            runtime_auth_token_path: temp_dir.path().join("runtime/auth.token"),
            consumer_control_auth_token_path: temp_dir.path().join("runtime/consumerctl.token"),
            provider_control_auth_token_path: temp_dir.path().join("runtime/froglet-control.token"),
            tor_dir: temp_dir.path().join("tor"),
            host_readable_control_token: false,
        };
        config.relay = crate::config::RelayConfig {
            url: Some("wss://control-a.relay.example/v1/tunnel".to_string()),
            public_suffix: Some("relay.example".to_string()),
            enabled: true,
        };

        let first = build_app_state(config.clone()).expect("first startup");
        let public_url = config
            .relay
            .planned_public_url(first.identity.node_id())
            .expect("planned relay URL")
            .expect("enabled relay URL");
        let signed_revision = signed_test_publication_revision(&first.identity);
        let revision_hash = signed_revision.revision_hash.clone();
        drop(first);

        let conn = db::initialize_db(&config.storage.db_path).expect("open publication db");
        let lifecycle = db::persist_and_activate_publication_revision(
            &conn,
            &db::NewPublicationRevision {
                revision_hash: revision_hash.clone(),
                service_id: "identity-restart-service".to_string(),
                offer_id: "identity-restart-offer".to_string(),
                offer_hash: signed_revision.payload.offer_hash.clone(),
                binding_hash: signed_revision.payload.binding_hash.clone(),
                signed_revision_json: serde_json::to_string(&signed_revision)
                    .expect("signed revision JSON"),
                definition_json: serde_json::json!({
                    "offer_id": "identity-restart-offer",
                    "service_id": "identity-restart-service",
                })
                .to_string(),
            },
            "active",
            r#"{"operation":"publish"}"#,
            1,
        )
        .expect("persist publication");
        db::persist_publication_transport_grant(
            &conn,
            &db::NewPublicationTransportGrant {
                transport: "relay",
                service_id: "identity-restart-service",
                revision_hash: &revision_hash,
                activation_token: &lifecycle.activation_token,
                public_url: &public_url,
                relay_control_url: "wss://control-a.relay.example/v1/tunnel",
                now: 2,
            },
        )
        .expect("persist relay grant");
        drop(conn);

        let matching = build_app_state(config.clone()).expect("matching restart");
        {
            let transport = matching
                .transport_status
                .try_lock()
                .expect("uncontended transport status");
            assert_eq!(transport.relay_status, "starting");
            let desired = transport.relay_activation_receiver();
            assert!(*desired.borrow());
        }
        drop(matching);

        config.relay.url = Some("wss://control-b.relay.example/v1/tunnel".to_string());
        let changed = build_app_state(config.clone()).expect("changed-control restart");
        {
            let transport = changed
                .transport_status
                .try_lock()
                .expect("uncontended transport status");
            assert_eq!(transport.relay_status, "reserved");
            assert_eq!(transport.relay_url.as_deref(), Some(public_url.as_str()));
            let desired = transport.relay_activation_receiver();
            assert!(!*desired.borrow());
        }
        drop(changed);

        let conn = db::initialize_db(&config.storage.db_path).expect("reopen publication db");
        assert!(
            db::list_publication_transport_grants(&conn, "relay")
                .expect("relay grants")
                .is_empty(),
            "a grant for the old WSS endpoint must not reconnect after restart"
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
        let database_mode =
            std::fs::metadata(&state.config.storage.db_path).expect("database metadata");
        let mut wal_path = state.config.storage.db_path.as_os_str().to_os_string();
        wal_path.push("-wal");
        let wal_mode =
            std::fs::metadata(std::path::PathBuf::from(wal_path)).expect("database WAL metadata");
        let mut shm_path = state.config.storage.db_path.as_os_str().to_os_string();
        shm_path.push("-shm");
        let shm_mode =
            std::fs::metadata(std::path::PathBuf::from(shm_path)).expect("database SHM metadata");

        assert_eq!(data_mode.permissions().mode() & 0o777, 0o755);
        assert_eq!(runtime_mode.permissions().mode() & 0o777, 0o755);
        assert_eq!(provider_token_mode.permissions().mode() & 0o777, 0o644);
        assert_eq!(runtime_token_mode.permissions().mode() & 0o777, 0o600);
        assert_eq!(database_mode.permissions().mode() & 0o777, 0o600);
        assert_eq!(wal_mode.permissions().mode() & 0o777, 0o600);
        assert_eq!(shm_mode.permissions().mode() & 0o777, 0o600);
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
    fn transport_status_records_local_provider_bound_addr() {
        let config = test_config(NetworkMode::Clearnet, None);
        let mut status = TransportStatus::from_config(&config);
        assert!(status.local_provider_bound_addr.is_none());

        status
            .update_clearnet_bound_addr(
                &config,
                "127.0.0.1:49152".parse().expect("valid socket address"),
            )
            .expect("bound address should be advertiseable");

        assert_eq!(
            status.local_provider_bound_addr,
            Some("127.0.0.1:49152".parse().expect("valid socket address"))
        );
    }

    #[test]
    fn transport_status_records_wildcard_bind_as_loopback_self_dial() {
        // Wildcard binds accept loopback connections; the recorded
        // self-dial target must be the loopback form, not 0.0.0.0.
        let config = test_config(NetworkMode::Clearnet, Some("https://froglet.example"));
        let mut status = TransportStatus::from_config(&config);

        status
            .update_clearnet_bound_addr(
                &config,
                "0.0.0.0:49152".parse().expect("valid socket address"),
            )
            .expect("public base url keeps wildcard binds valid");

        assert_eq!(
            status.local_provider_bound_addr,
            Some("127.0.0.1:49152".parse().expect("valid socket address"))
        );
    }

    #[test]
    fn transport_status_does_not_record_provider_addr_when_clearnet_disabled() {
        let config = test_config(NetworkMode::Tor, None);
        let mut status = TransportStatus::from_config(&config);

        status
            .update_clearnet_bound_addr(
                &config,
                "127.0.0.1:49152".parse().expect("valid socket address"),
            )
            .expect("disabled clearnet is a no-op");

        assert!(status.local_provider_bound_addr.is_none());
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
