use std::time::Duration;

#[derive(Debug, Clone)]
pub struct MarketplaceConfig {
    pub database_url: String,
    pub feed_sources: Vec<String>,
    pub poll_interval: Duration,
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

        let poll_interval_secs: u64 = std::env::var("MARKETPLACE_POLL_INTERVAL_SECS")
            .unwrap_or_else(|_| "30".to_string())
            .parse()
            .map_err(|_| "MARKETPLACE_POLL_INTERVAL_SECS must be a positive integer".to_string())?;

        Ok(Self {
            database_url,
            feed_sources,
            poll_interval: Duration::from_secs(poll_interval_secs),
        })
    }
}
