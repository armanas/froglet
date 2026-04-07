use std::time::Duration;

pub const DEFAULT_MAX_DYNAMIC_SOURCES: usize = 200;

#[derive(Debug, Clone)]
pub struct MarketplaceConfig {
    pub database_url: String,
    pub feed_sources: Vec<String>,
    pub discovery_url: Option<String>,
    pub poll_interval: Duration,
    pub max_dynamic_sources: usize,
}

impl MarketplaceConfig {
    pub fn from_env() -> Result<Self, String> {
        let database_url = std::env::var("MARKETPLACE_DATABASE_URL")
            .map_err(|_| "MARKETPLACE_DATABASE_URL is required".to_string())?;

        let feed_sources: Vec<String> = std::env::var("MARKETPLACE_FEED_SOURCES")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let discovery_url = std::env::var("MARKETPLACE_DISCOVERY_URL").ok();

        let poll_interval_secs: u64 = std::env::var("MARKETPLACE_POLL_INTERVAL_SECS")
            .unwrap_or_else(|_| "30".to_string())
            .parse()
            .map_err(|_| "MARKETPLACE_POLL_INTERVAL_SECS must be a positive integer".to_string())?;

        let max_dynamic_sources: usize = std::env::var("MARKETPLACE_MAX_DYNAMIC_SOURCES")
            .unwrap_or_else(|_| DEFAULT_MAX_DYNAMIC_SOURCES.to_string())
            .parse()
            .map_err(|_| "MARKETPLACE_MAX_DYNAMIC_SOURCES must be a positive integer".to_string())?;

        Ok(Self {
            database_url,
            feed_sources,
            discovery_url,
            poll_interval: Duration::from_secs(poll_interval_secs),
            max_dynamic_sources,
        })
    }
}
