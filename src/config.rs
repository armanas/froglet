use crate::confidential::{self, ConfidentialConfig, ConfidentialPolicy};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fmt, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    Clearnet,
    Tor,
    Dual,
}

impl fmt::Display for NetworkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetworkMode::Clearnet => write!(f, "clearnet"),
            NetworkMode::Tor => write!(f, "tor"),
            NetworkMode::Dual => write!(f, "dual"),
        }
    }
}

impl NetworkMode {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "clearnet" => Ok(Self::Clearnet),
            "tor" => Ok(Self::Tor),
            "dual" => Ok(Self::Dual),
            _ => Err(format!(
                "Invalid FROGLET_NETWORK_MODE value: '{s}'. Allowed values: clearnet, tor, dual"
            )),
        }
    }

    pub fn should_start_clearnet(&self) -> bool {
        matches!(self, Self::Clearnet | Self::Dual)
    }

    pub fn should_start_tor(&self) -> bool {
        matches!(self, Self::Tor | Self::Dual)
    }

    pub fn tor_required(&self) -> bool {
        matches!(self, Self::Tor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryMode {
    None,
    Reference,
}

impl fmt::Display for DiscoveryMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiscoveryMode::None => write!(f, "none"),
            DiscoveryMode::Reference => write!(f, "reference"),
        }
    }
}

impl DiscoveryMode {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "reference" => Ok(Self::Reference),
            _ => Err(format!(
                "Invalid FROGLET_DISCOVERY_MODE value: '{s}'. Allowed values: none, reference"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentBackend {
    None,
    Lightning,
}

impl fmt::Display for PaymentBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaymentBackend::None => write!(f, "none"),
            PaymentBackend::Lightning => write!(f, "lightning"),
        }
    }
}

impl PaymentBackend {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "lightning" => Ok(Self::Lightning),
            _ => Err(format!(
                "Invalid FROGLET_PAYMENT_BACKEND value: '{s}'. Allowed values: none, lightning"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightningMode {
    Mock,
    LndRest,
}

impl fmt::Display for LightningMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LightningMode::Mock => write!(f, "mock"),
            LightningMode::LndRest => write!(f, "lnd_rest"),
        }
    }
}

impl LightningMode {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "mock" => Ok(Self::Mock),
            "lnd_rest" => Ok(Self::LndRest),
            _ => Err(format!(
                "Invalid FROGLET_LIGHTNING_MODE value: '{s}'. Allowed values: mock, lnd_rest"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IdentityConfig {
    pub auto_generate: bool,
}

#[derive(Debug, Clone)]
pub struct ReferenceDiscoveryConfig {
    pub url: String,
    pub publish: bool,
    pub required: bool,
    pub heartbeat_interval_secs: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PricingConfig {
    pub events_query: u64,
    pub execute_wasm: u64,
}

impl PricingConfig {
    pub fn has_paid_services(&self) -> bool {
        self.events_query > 0 || self.execute_wasm > 0
    }
}

#[derive(Debug, Clone)]
pub struct LightningConfig {
    pub mode: LightningMode,
    pub destination_identity: Option<String>,
    pub base_invoice_expiry_secs: u64,
    pub success_hold_expiry_secs: u64,
    pub min_final_cltv_expiry: u32,
    pub sync_interval_ms: u64,
    pub lnd_rest: Option<LightningLndRestConfig>,
}

#[derive(Debug, Clone)]
pub struct LightningLndRestConfig {
    pub rest_url: String,
    pub tls_cert_path: Option<PathBuf>,
    pub macaroon_path: PathBuf,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub identity_dir: PathBuf,
    pub identity_seed_path: PathBuf,
    pub nostr_publication_seed_path: PathBuf,
    pub runtime_dir: PathBuf,
    pub runtime_auth_token_path: PathBuf,
    pub tor_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct TorSidecarConfig {
    pub binary_path: String,
    pub backend_listen_addr: String,
    pub startup_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_path: Option<PathBuf>,
    #[serde(skip_serializing, skip_deserializing)]
    pub policy: Option<WasmPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<WasmHttpPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sqlite: Option<WasmSqlitePolicy>,
}

impl WasmPolicy {
    pub fn advertised_capabilities(&self) -> Vec<String> {
        let mut capabilities = Vec::new();

        if let Some(http) = &self.http {
            capabilities.push(crate::wasm::WASM_CAPABILITY_HTTP_FETCH.to_string());
            for profile in http.auth_profiles.keys() {
                capabilities.push(format!(
                    "{}{}",
                    crate::wasm::WASM_CAPABILITY_HTTP_FETCH_AUTH_PREFIX,
                    profile
                ));
            }
        }

        if let Some(sqlite) = &self.sqlite {
            for handle in sqlite.handles.keys() {
                capabilities.push(format!(
                    "{}{}",
                    crate::wasm::WASM_CAPABILITY_SQLITE_QUERY_READ_PREFIX,
                    handle
                ));
            }
        }

        capabilities.sort();
        capabilities.dedup();
        capabilities
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmHttpPolicy {
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub allow_private_networks: bool,
    #[serde(default = "default_http_max_calls_per_execution")]
    pub max_calls_per_execution: u32,
    #[serde(default = "default_http_max_timeout_ms")]
    pub max_timeout_ms: u64,
    #[serde(default = "default_http_max_request_body_bytes")]
    pub max_request_body_bytes: usize,
    #[serde(default = "default_http_max_response_body_bytes")]
    pub max_response_body_bytes: usize,
    #[serde(default = "default_http_max_redirects")]
    pub max_redirects: usize,
    #[serde(default)]
    pub auth_profiles: BTreeMap<String, WasmHttpAuthProfile>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WasmHttpAuthProfile {
    pub header_name: String,
    #[serde(skip_serializing)]
    pub header_value: String,
}

impl fmt::Debug for WasmHttpAuthProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmHttpAuthProfile")
            .field("header_name", &self.header_name)
            .field("header_value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmSqlitePolicy {
    #[serde(default = "default_sqlite_max_queries_per_execution")]
    pub max_queries_per_execution: u32,
    #[serde(default = "default_sqlite_max_rows_per_query")]
    pub max_rows_per_query: usize,
    #[serde(default = "default_sqlite_max_result_bytes")]
    pub max_result_bytes: usize,
    #[serde(default)]
    pub handles: BTreeMap<String, WasmSqliteHandleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmSqliteHandleConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub network_mode: NetworkMode,
    pub listen_addr: String,
    pub public_base_url: Option<String>,
    pub runtime_listen_addr: String,
    pub runtime_allow_non_loopback: bool,
    pub http_ca_cert_path: Option<PathBuf>,
    pub tor: TorSidecarConfig,
    pub discovery_mode: DiscoveryMode,
    pub identity: IdentityConfig,
    pub reference_discovery: Option<ReferenceDiscoveryConfig>,
    pub pricing: PricingConfig,
    pub payment_backend: PaymentBackend,
    pub execution_timeout_secs: u64,
    pub lightning: LightningConfig,
    pub storage: StorageConfig,
    pub wasm: WasmConfig,
    pub confidential: ConfidentialConfig,
}

impl NodeConfig {
    pub fn from_env() -> Result<Self, String> {
        let network_mode = match env::var("FROGLET_NETWORK_MODE") {
            Ok(val) => NetworkMode::parse(&val)?,
            Err(_) => NetworkMode::Clearnet,
        };

        let listen_addr =
            env::var("FROGLET_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let public_base_url = match env::var("FROGLET_PUBLIC_BASE_URL") {
            Ok(value) => Some(normalize_public_base_url(&value)?),
            Err(_) => None,
        };
        let runtime_listen_addr = env::var("FROGLET_RUNTIME_LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8081".to_string());
        let runtime_allow_non_loopback = env_bool("FROGLET_RUNTIME_ALLOW_NON_LOOPBACK", false)?;
        let http_ca_cert_path = env::var("FROGLET_HTTP_CA_CERT_PATH")
            .ok()
            .map(PathBuf::from);
        let tor = TorSidecarConfig {
            binary_path: env::var("FROGLET_TOR_BINARY").unwrap_or_else(|_| "tor".to_string()),
            backend_listen_addr: env::var("FROGLET_TOR_BACKEND_LISTEN_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8082".to_string()),
            startup_timeout_secs: env_u64("FROGLET_TOR_STARTUP_TIMEOUT_SECS", 90)?.clamp(5, 300),
        };

        let pricing = PricingConfig {
            events_query: env_u64("FROGLET_PRICE_EVENTS_QUERY", 0)?,
            execute_wasm: env_u64("FROGLET_PRICE_EXEC_WASM", 0)?,
        };

        let discovery_url = env::var("FROGLET_DISCOVERY_URL").ok();
        let publish = env_bool("FROGLET_DISCOVERY_PUBLISH", false)?;
        let required = env_bool("FROGLET_DISCOVERY_REQUIRED", false)?;
        let discovery_mode = match env::var("FROGLET_DISCOVERY_MODE") {
            Ok(val) => DiscoveryMode::parse(&val)?,
            Err(_) if publish || discovery_url.is_some() => DiscoveryMode::Reference,
            Err(_) => DiscoveryMode::None,
        };

        if required && !publish {
            return Err(
                "FROGLET_DISCOVERY_REQUIRED=true requires FROGLET_DISCOVERY_PUBLISH=true".into(),
            );
        }

        let reference_discovery = if discovery_mode == DiscoveryMode::Reference || publish {
            let url = discovery_url.ok_or_else(|| {
                "FROGLET_DISCOVERY_URL is required when reference discovery or publishing is enabled"
                    .to_string()
            })?;

            Some(ReferenceDiscoveryConfig {
                url,
                publish,
                required,
                heartbeat_interval_secs: env_u64("FROGLET_DISCOVERY_HEARTBEAT_INTERVAL_SECS", 30)?,
            })
        } else {
            None
        };

        let payment_backend = match env::var("FROGLET_PAYMENT_BACKEND") {
            Ok(val) => PaymentBackend::parse(&val)?,
            Err(_) if pricing.has_paid_services() => PaymentBackend::Lightning,
            Err(_) => PaymentBackend::None,
        };

        if pricing.has_paid_services() && matches!(payment_backend, PaymentBackend::None) {
            return Err("Paid services require FROGLET_PAYMENT_BACKEND=lightning".into());
        }

        let execution_timeout_secs = env_u64("FROGLET_EXECUTION_TIMEOUT_SECS", 10)?.clamp(1, 300);
        let lightning_required = matches!(payment_backend, PaymentBackend::Lightning);
        let lightning_mode = match env::var("FROGLET_LIGHTNING_MODE") {
            Ok(val) => LightningMode::parse(&val)?,
            Err(_) if lightning_required => {
                return Err(
                    "FROGLET_LIGHTNING_MODE is required whenever Lightning payments are active"
                        .into(),
                );
            }
            Err(_) => LightningMode::Mock,
        };
        let lnd_rest_url = env::var("FROGLET_LIGHTNING_REST_URL").ok();
        let lnd_tls_cert_path = env::var("FROGLET_LIGHTNING_TLS_CERT_PATH")
            .ok()
            .map(PathBuf::from);
        let lnd_macaroon_path = env::var("FROGLET_LIGHTNING_MACAROON_PATH")
            .ok()
            .map(PathBuf::from);
        let lnd_request_timeout_secs =
            env_u64("FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS", 5)?.clamp(1, 30);

        if matches!(lightning_mode, LightningMode::LndRest) {
            let Some(rest_url) = lnd_rest_url.as_ref() else {
                return Err(
                    "FROGLET_LIGHTNING_MODE=lnd_rest requires FROGLET_LIGHTNING_REST_URL".into(),
                );
            };
            let plaintext_loopback = lnd_rest_url_allows_plaintext_loopback(rest_url)
                .map_err(|error| error.to_string())?;
            if rest_url.starts_with("https://") && lnd_tls_cert_path.is_none() {
                return Err(
                    "FROGLET_LIGHTNING_TLS_CERT_PATH is required for https LND REST endpoints"
                        .into(),
                );
            }
            if rest_url.starts_with("http://") && !plaintext_loopback {
                return Err(
                    "FROGLET_LIGHTNING_REST_URL must use https:// unless it points to a loopback-only http:// endpoint".into(),
                );
            }
            if lnd_macaroon_path.is_none() {
                return Err(
                    "FROGLET_LIGHTNING_MODE=lnd_rest requires FROGLET_LIGHTNING_MACAROON_PATH"
                        .into(),
                );
            }
        }

        let lightning = LightningConfig {
            mode: lightning_mode,
            destination_identity: env::var("FROGLET_LIGHTNING_DESTINATION_IDENTITY").ok(),
            base_invoice_expiry_secs: env_u64("FROGLET_LIGHTNING_BASE_INVOICE_EXPIRY_SECS", 300)?
                .clamp(60, 3600),
            success_hold_expiry_secs: env_u64("FROGLET_LIGHTNING_SUCCESS_HOLD_EXPIRY_SECS", 300)?
                .clamp(60, 3600),
            min_final_cltv_expiry: env_u64("FROGLET_LIGHTNING_MIN_FINAL_CLTV_EXPIRY", 18)?
                .clamp(1, 144) as u32,
            sync_interval_ms: env_u64("FROGLET_LIGHTNING_SYNC_INTERVAL_MS", 1_000)?
                .clamp(100, 60_000),
            lnd_rest: matches!(lightning_mode, LightningMode::LndRest).then(|| {
                LightningLndRestConfig {
                    rest_url: lnd_rest_url.expect("validated lnd rest url"),
                    tls_cert_path: lnd_tls_cert_path,
                    macaroon_path: lnd_macaroon_path.expect("validated lnd macaroon path"),
                    request_timeout_secs: lnd_request_timeout_secs,
                }
            }),
        };

        let data_dir =
            PathBuf::from(env::var("FROGLET_DATA_DIR").unwrap_or_else(|_| "./data".to_string()));
        let identity_dir = data_dir.join("identity");
        let identity_seed_path = identity_dir.join("secp256k1.seed");
        let nostr_publication_seed_path = identity_dir.join("nostr-publication.secp256k1.seed");
        let runtime_dir = data_dir.join("runtime");
        let runtime_auth_token_path = runtime_dir.join("auth.token");
        let tor_dir = data_dir.join("tor");
        let db_path = data_dir.join("node.db");
        let wasm_policy_path = env::var("FROGLET_WASM_POLICY_PATH").ok().map(PathBuf::from);
        let wasm_policy = match wasm_policy_path.as_ref() {
            Some(path) => Some(load_wasm_policy(path, &db_path)?),
            None => None,
        };
        let confidential_policy_path = env::var("FROGLET_CONFIDENTIAL_POLICY_PATH")
            .ok()
            .map(PathBuf::from);
        let confidential_policy = match confidential_policy_path.as_ref() {
            Some(path) => Some(load_confidential_policy(path, &db_path)?),
            None => None,
        };

        Ok(Self {
            network_mode,
            listen_addr,
            public_base_url,
            runtime_listen_addr,
            runtime_allow_non_loopback,
            http_ca_cert_path,
            tor,
            discovery_mode,
            identity: IdentityConfig {
                auto_generate: env_bool("FROGLET_IDENTITY_AUTO_GENERATE", true)?,
            },
            reference_discovery,
            pricing,
            payment_backend,
            execution_timeout_secs,
            lightning,
            storage: StorageConfig {
                data_dir,
                db_path,
                identity_dir,
                identity_seed_path,
                nostr_publication_seed_path,
                runtime_dir,
                runtime_auth_token_path,
                tor_dir,
            },
            wasm: WasmConfig {
                policy_path: wasm_policy_path,
                policy: wasm_policy,
            },
            confidential: ConfidentialConfig {
                policy_path: confidential_policy_path,
                policy: confidential_policy,
                session_ttl_secs: env_u64("FROGLET_CONFIDENTIAL_SESSION_TTL_SECS", 300)?
                    .clamp(30, 3600),
            },
        })
    }
}

fn default_http_max_calls_per_execution() -> u32 {
    16
}

fn default_http_max_timeout_ms() -> u64 {
    10_000
}

fn default_http_max_request_body_bytes() -> usize {
    256 * 1024
}

fn default_http_max_response_body_bytes() -> usize {
    2 * 1024 * 1024
}

fn default_http_max_redirects() -> usize {
    5
}

fn default_sqlite_max_queries_per_execution() -> u32 {
    16
}

fn default_sqlite_max_rows_per_query() -> usize {
    256
}

fn default_sqlite_max_result_bytes() -> usize {
    256 * 1024
}

fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env::var(name) {
        Ok(value) => match value.to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(format!(
                "Invalid {name} value: '{value}'. Allowed values: true/false"
            )),
        },
        Err(_) => Ok(default),
    }
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    match env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|_| format!("Invalid {name} value: '{value}'. Expected unsigned integer")),
        Err(_) => Ok(default),
    }
}

fn lnd_rest_url_allows_plaintext_loopback(rest_url: &str) -> Result<bool, String> {
    let parsed = Url::parse(rest_url)
        .map_err(|error| format!("invalid FROGLET_LIGHTNING_REST_URL: {error}"))?;
    match parsed.scheme() {
        "https" => Ok(false),
        "http" => Ok(matches!(
            parsed.host_str(),
            Some("127.0.0.1" | "localhost" | "::1")
        )),
        scheme => Err(format!(
            "FROGLET_LIGHTNING_REST_URL must use https:// or loopback http://; got scheme '{scheme}'"
        )),
    }
}

fn normalize_public_base_url(url: &str) -> Result<String, String> {
    let parsed =
        Url::parse(url).map_err(|error| format!("Invalid FROGLET_PUBLIC_BASE_URL: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed.to_string().trim_end_matches('/').to_string()),
        other => Err(format!(
            "Invalid FROGLET_PUBLIC_BASE_URL scheme '{other}'. Allowed schemes: http, https"
        )),
    }
}

fn load_wasm_policy(path: &Path, internal_db_path: &Path) -> Result<WasmPolicy, String> {
    let document = fs::read_to_string(path).map_err(|error| {
        format!(
            "Failed to read FROGLET_WASM_POLICY_PATH {}: {error}",
            path.display()
        )
    })?;
    let policy: WasmPolicy = toml::from_str(&document).map_err(|error| {
        format!(
            "Failed to parse FROGLET_WASM_POLICY_PATH {}: {error}",
            path.display()
        )
    })?;
    validate_wasm_policy(&policy, internal_db_path)?;
    Ok(policy)
}

fn load_confidential_policy(
    path: &Path,
    internal_db_path: &Path,
) -> Result<ConfidentialPolicy, String> {
    confidential::load_policy(path, internal_db_path)
}

/// Validates a Wasm policy at load time.
///
/// NOTE: The SQLite path protection check uses `fs::canonicalize` which resolves
/// symlinks at the time of this call. If a symlink is created after this check
/// but before the database is opened, the protection could be bypassed. This is
/// acceptable because the filesystem is operator-controlled, but operators should
/// be aware this is a startup-time check, not a runtime enforcement.
fn validate_wasm_policy(policy: &WasmPolicy, internal_db_path: &Path) -> Result<(), String> {
    if let Some(http) = &policy.http {
        if http.max_calls_per_execution == 0 {
            return Err(
                "Wasm HTTP policy max_calls_per_execution must be greater than zero".into(),
            );
        }
        if http.max_timeout_ms == 0 {
            return Err("Wasm HTTP policy max_timeout_ms must be greater than zero".into());
        }
        if http.max_request_body_bytes == 0 {
            return Err("Wasm HTTP policy max_request_body_bytes must be greater than zero".into());
        }
        if http.max_response_body_bytes == 0 {
            return Err(
                "Wasm HTTP policy max_response_body_bytes must be greater than zero".into(),
            );
        }

        for host in &http.allowed_hosts {
            if host.trim().is_empty() {
                return Err("Wasm HTTP policy allowed_hosts must not contain empty values".into());
            }
        }

        for (profile, auth_profile) in &http.auth_profiles {
            validate_wasm_policy_name("http auth profile", profile)?;
            if auth_profile.header_name.trim().is_empty() {
                return Err(format!(
                    "Wasm HTTP auth profile '{profile}' header_name must not be empty"
                ));
            }
        }
    }

    if let Some(sqlite) = &policy.sqlite {
        if sqlite.max_queries_per_execution == 0 {
            return Err(
                "Wasm SQLite policy max_queries_per_execution must be greater than zero".into(),
            );
        }
        if sqlite.max_rows_per_query == 0 {
            return Err("Wasm SQLite policy max_rows_per_query must be greater than zero".into());
        }
        if sqlite.max_result_bytes == 0 {
            return Err("Wasm SQLite policy max_result_bytes must be greater than zero".into());
        }
        if sqlite.handles.is_empty() {
            return Err("Wasm SQLite policy must define at least one named handle".into());
        }

        let normalized_internal_db_path = normalize_path_for_comparison(internal_db_path)?;
        for (handle, handle_config) in &sqlite.handles {
            validate_wasm_policy_name("sqlite handle", handle)?;
            if !handle_config.path.exists() {
                return Err(format!(
                    "Wasm SQLite handle '{handle}' path does not exist: {}",
                    handle_config.path.display()
                ));
            }
            let normalized_handle_path = normalize_path_for_comparison(&handle_config.path)?;
            if normalized_handle_path == normalized_internal_db_path {
                return Err(format!(
                    "Wasm SQLite handle '{handle}' must not reference Froglet's internal database"
                ));
            }
        }
    }

    Ok(())
}

fn validate_wasm_policy_name(kind: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
    {
        return Err(format!(
            "Invalid {kind} name '{value}'. Allowed characters: lowercase ascii letters, digits, '-' and '_'"
        ));
    }
    Ok(())
}

fn normalize_path_for_comparison(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| format!("failed to resolve current working directory: {error}"))?
            .join(path)
    };

    let mut existing_prefix = absolute.as_path();
    let mut missing_components = Vec::new();
    while !existing_prefix.exists() {
        let component = existing_prefix.file_name().ok_or_else(|| {
            format!(
                "failed to normalize path '{}': no existing parent directory found",
                absolute.display()
            )
        })?;
        missing_components.push(component.to_os_string());
        existing_prefix = existing_prefix.parent().ok_or_else(|| {
            format!(
                "failed to normalize path '{}': no existing parent directory found",
                absolute.display()
            )
        })?;
    }

    let mut normalized = fs::canonicalize(existing_prefix).map_err(|error| {
        format!(
            "failed to canonicalize path prefix '{}': {error}",
            existing_prefix.display()
        )
    })?;
    while let Some(component) = missing_components.pop() {
        normalized.push(component);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "froglet-config-tests-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn test_parse_valid_modes() {
        assert_eq!(
            NetworkMode::parse("clearnet").unwrap(),
            NetworkMode::Clearnet
        );
        assert_eq!(NetworkMode::parse("tor").unwrap(), NetworkMode::Tor);
        assert_eq!(NetworkMode::parse("dual").unwrap(), NetworkMode::Dual);
        assert_eq!(
            NetworkMode::parse("CLEARNET").unwrap(),
            NetworkMode::Clearnet
        );
    }

    #[test]
    fn test_parse_invalid_mode() {
        assert!(NetworkMode::parse("invalid").is_err());
        assert!(DiscoveryMode::parse("relay").is_err());
        assert!(PaymentBackend::parse("wallet").is_err());
    }

    #[test]
    fn test_parse_lnd_rest_lightning_mode() {
        assert_eq!(
            LightningMode::parse("lnd_rest").unwrap(),
            LightningMode::LndRest
        );
        assert_eq!(
            LightningMode::parse("LND_REST").unwrap(),
            LightningMode::LndRest
        );
    }

    #[test]
    fn test_paid_services_detection() {
        let pricing = PricingConfig {
            events_query: 0,
            execute_wasm: 0,
        };
        assert!(!pricing.has_paid_services());

        let pricing = PricingConfig {
            events_query: 0,
            execute_wasm: 10,
        };
        assert!(pricing.has_paid_services());
    }

    #[test]
    fn test_lnd_rest_url_rejects_non_loopback_http() {
        assert!(!lnd_rest_url_allows_plaintext_loopback("https://lnd.example.com").unwrap());
        assert!(lnd_rest_url_allows_plaintext_loopback("http://127.0.0.1:8080").unwrap());
        assert!(lnd_rest_url_allows_plaintext_loopback("http://localhost:8080").unwrap());
        assert!(!lnd_rest_url_allows_plaintext_loopback("http://10.0.0.5:8080").unwrap());
    }

    #[test]
    fn test_normalize_public_base_url_accepts_http_and_https() {
        assert_eq!(
            normalize_public_base_url("http://127.0.0.1:8080/").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normalize_public_base_url("https://froglet.example.com").unwrap(),
            "https://froglet.example.com"
        );
    }

    #[test]
    fn test_normalize_public_base_url_rejects_non_http_schemes() {
        assert!(normalize_public_base_url("ftp://froglet.example.com").is_err());
    }

    #[test]
    fn test_load_wasm_policy_accepts_valid_policy_file() {
        let temp_dir = unique_temp_dir("wasm-policy-valid");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let internal_db_path = temp_dir.join("node.db");
        let sqlite_db_path = temp_dir.join("workload.db");
        std::fs::write(&internal_db_path, "").unwrap();
        std::fs::write(&sqlite_db_path, "").unwrap();

        let policy_path = temp_dir.join("wasm-policy.toml");
        std::fs::write(
            &policy_path,
            format!(
                r#"
[http]
allowed_hosts = ["api.example.com"]
max_calls_per_execution = 4
max_timeout_ms = 5000
max_request_body_bytes = 1024
max_response_body_bytes = 2048
max_redirects = 2

[http.auth_profiles.github]
header_name = "authorization"
header_value = "Bearer token"

[sqlite]
max_queries_per_execution = 3
max_rows_per_query = 10
max_result_bytes = 4096

[sqlite.handles.main]
path = "{}"
"#,
                sqlite_db_path.display()
            ),
        )
        .unwrap();

        let policy = load_wasm_policy(&policy_path, &internal_db_path).expect("policy should load");
        assert_eq!(
            policy.advertised_capabilities(),
            vec![
                "db.sqlite.query.read.main".to_string(),
                "net.http.fetch".to_string(),
                "net.http.fetch.auth.github".to_string(),
            ]
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_load_wasm_policy_rejects_internal_db_handle() {
        let temp_dir = unique_temp_dir("wasm-policy-internal-db");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let internal_db_path = temp_dir.join("node.db");
        std::fs::write(&internal_db_path, "").unwrap();

        let policy_path = temp_dir.join("wasm-policy.toml");
        std::fs::write(
            &policy_path,
            format!(
                r#"
[sqlite]
max_queries_per_execution = 1
max_rows_per_query = 10
max_result_bytes = 1024

[sqlite.handles.main]
path = "{}"
"#,
                internal_db_path.display()
            ),
        )
        .unwrap();

        let error = load_wasm_policy(&policy_path, &internal_db_path)
            .expect_err("policy should reject internal db handle");
        assert!(error.contains("must not reference Froglet's internal database"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
