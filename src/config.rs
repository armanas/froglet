use serde::Serialize;
use std::{env, fmt, path::PathBuf};

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
    Marketplace,
}

impl fmt::Display for DiscoveryMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiscoveryMode::None => write!(f, "none"),
            DiscoveryMode::Marketplace => write!(f, "marketplace"),
        }
    }
}

impl DiscoveryMode {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "marketplace" => Ok(Self::Marketplace),
            _ => Err(format!(
                "Invalid FROGLET_DISCOVERY_MODE value: '{s}'. Allowed values: none, marketplace"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentBackend {
    None,
    Cashu,
    Lightning,
}

impl fmt::Display for PaymentBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaymentBackend::None => write!(f, "none"),
            PaymentBackend::Cashu => write!(f, "cashu"),
            PaymentBackend::Lightning => write!(f, "lightning"),
        }
    }
}

impl PaymentBackend {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "cashu" => Ok(Self::Cashu),
            "lightning" => Ok(Self::Lightning),
            _ => Err(format!(
                "Invalid FROGLET_PAYMENT_BACKEND value: '{s}'. Allowed values: none, cashu, lightning"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightningMode {
    Mock,
}

impl fmt::Display for LightningMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LightningMode::Mock => write!(f, "mock"),
        }
    }
}

impl LightningMode {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "mock" => Ok(Self::Mock),
            _ => Err(format!(
                "Invalid FROGLET_LIGHTNING_MODE value: '{s}'. Allowed values: mock"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IdentityConfig {
    pub auto_generate: bool,
}

#[derive(Debug, Clone)]
pub struct MarketplaceConfig {
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
pub struct CashuConfig {
    pub mint_allowlist: Vec<String>,
    pub remote_checkstate: bool,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct LightningConfig {
    pub mode: LightningMode,
    pub destination_identity: Option<String>,
    pub base_invoice_expiry_secs: u64,
    pub success_hold_expiry_secs: u64,
    pub min_final_cltv_expiry: u32,
}

#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub identity_dir: PathBuf,
    pub identity_seed_path: PathBuf,
    pub runtime_dir: PathBuf,
    pub runtime_auth_token_path: PathBuf,
    pub tor_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub network_mode: NetworkMode,
    pub listen_addr: String,
    pub discovery_mode: DiscoveryMode,
    pub identity: IdentityConfig,
    pub marketplace: Option<MarketplaceConfig>,
    pub pricing: PricingConfig,
    pub payment_backend: PaymentBackend,
    pub execution_timeout_secs: u64,
    pub cashu: CashuConfig,
    pub lightning: LightningConfig,
    pub storage: StorageConfig,
}

impl NodeConfig {
    pub fn from_env() -> Result<Self, String> {
        let network_mode = match env::var("FROGLET_NETWORK_MODE") {
            Ok(val) => NetworkMode::parse(&val)?,
            Err(_) => NetworkMode::Clearnet,
        };

        let listen_addr =
            env::var("FROGLET_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

        let pricing = PricingConfig {
            events_query: env_u64("FROGLET_PRICE_EVENTS_QUERY", 0)?,
            execute_wasm: env_u64("FROGLET_PRICE_EXEC_WASM", 0)?,
        };

        let marketplace_url = env::var("FROGLET_MARKETPLACE_URL").ok();
        let publish = env_bool("FROGLET_MARKETPLACE_PUBLISH", false)?;
        let required = env_bool("FROGLET_MARKETPLACE_REQUIRED", false)?;
        let discovery_mode = match env::var("FROGLET_DISCOVERY_MODE") {
            Ok(val) => DiscoveryMode::parse(&val)?,
            Err(_) if publish || marketplace_url.is_some() => DiscoveryMode::Marketplace,
            Err(_) => DiscoveryMode::None,
        };

        if required && !publish {
            return Err(
                "FROGLET_MARKETPLACE_REQUIRED=true requires FROGLET_MARKETPLACE_PUBLISH=true"
                    .into(),
            );
        }

        let marketplace = if discovery_mode == DiscoveryMode::Marketplace || publish {
            let url = marketplace_url.ok_or_else(|| {
                "FROGLET_MARKETPLACE_URL is required when marketplace discovery or publishing is enabled"
                    .to_string()
            })?;

            Some(MarketplaceConfig {
                url,
                publish,
                required,
                heartbeat_interval_secs: env_u64(
                    "FROGLET_MARKETPLACE_HEARTBEAT_INTERVAL_SECS",
                    30,
                )?,
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
            return Err(
                "Paid services require FROGLET_PAYMENT_BACKEND to be set to 'lightning' or an explicitly enabled legacy backend such as 'cashu'"
                    .into(),
            );
        }

        let execution_timeout_secs = env_u64("FROGLET_EXECUTION_TIMEOUT_SECS", 10)?.clamp(1, 300);
        let mint_allowlist = env_csv("FROGLET_CASHU_MINT_ALLOWLIST");
        let remote_checkstate = env_bool("FROGLET_CASHU_REMOTE_CHECKSTATE", false)?;
        if remote_checkstate && mint_allowlist.is_empty() {
            return Err(
                "FROGLET_CASHU_REMOTE_CHECKSTATE=true requires FROGLET_CASHU_MINT_ALLOWLIST to avoid untrusted mint callbacks"
                    .into(),
            );
        }
        let cashu = CashuConfig {
            mint_allowlist,
            remote_checkstate,
            request_timeout_secs: env_u64("FROGLET_CASHU_REQUEST_TIMEOUT_SECS", 5)?.clamp(1, 30),
        };
        let lightning = LightningConfig {
            mode: match env::var("FROGLET_LIGHTNING_MODE") {
                Ok(val) => LightningMode::parse(&val)?,
                Err(_) => LightningMode::Mock,
            },
            destination_identity: env::var("FROGLET_LIGHTNING_DESTINATION_IDENTITY").ok(),
            base_invoice_expiry_secs: env_u64("FROGLET_LIGHTNING_BASE_INVOICE_EXPIRY_SECS", 300)?
                .clamp(60, 3600),
            success_hold_expiry_secs: env_u64("FROGLET_LIGHTNING_SUCCESS_HOLD_EXPIRY_SECS", 300)?
                .clamp(60, 3600),
            min_final_cltv_expiry: env_u64("FROGLET_LIGHTNING_MIN_FINAL_CLTV_EXPIRY", 18)?
                .clamp(1, 144) as u32,
        };

        let data_dir =
            PathBuf::from(env::var("FROGLET_DATA_DIR").unwrap_or_else(|_| "./data".to_string()));
        let identity_dir = data_dir.join("identity");
        let identity_seed_path = identity_dir.join("secp256k1.seed");
        let runtime_dir = data_dir.join("runtime");
        let runtime_auth_token_path = runtime_dir.join("auth.token");
        let tor_dir = data_dir.join("tor");
        let db_path = data_dir.join("node.db");

        Ok(Self {
            network_mode,
            listen_addr,
            discovery_mode,
            identity: IdentityConfig {
                auto_generate: env_bool("FROGLET_IDENTITY_AUTO_GENERATE", true)?,
            },
            marketplace,
            pricing,
            payment_backend,
            execution_timeout_secs,
            cashu,
            lightning,
            storage: StorageConfig {
                data_dir,
                db_path,
                identity_dir,
                identity_seed_path,
                runtime_dir,
                runtime_auth_token_path,
                tor_dir,
            },
        })
    }
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

fn env_csv(name: &str) -> Vec<String> {
    match env::var(name) {
        Ok(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Err(_) => Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
