//! LocalBackend — private development hosting. Binds 127.0.0.1, does
//! NOT register with the marketplace. The engine returns the local
//! provider URL so the caller (CLI/MCP) can echo it back.

use super::{HostingBackend, PreparedHosting};
use crate::error::PublishError;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct LocalBackend {
    /// Local provider URL the engine reports back. Caller is responsible
    /// for ensuring a `froglet-node` is running at this address (the
    /// `init`/`build` subcommands handle that).
    pub provider_url: String,
}

impl LocalBackend {
    /// Build with the default `FROGLET_LISTEN_ADDR` (`127.0.0.1:8080`).
    pub fn default_url() -> Self {
        Self {
            provider_url: "http://127.0.0.1:8080".to_string(),
        }
    }

    /// Build with a caller-supplied URL (e.g., from
    /// `FROGLET_PUBLIC_BASE_URL`).
    pub fn with_url(url: impl Into<String>) -> Self {
        Self {
            provider_url: url.into(),
        }
    }
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::default_url()
    }
}

#[async_trait]
impl HostingBackend for LocalBackend {
    fn name(&self) -> &'static str {
        "local"
    }

    async fn prepare(&self) -> Result<PreparedHosting, PublishError> {
        Ok(PreparedHosting {
            public_url: self.provider_url.clone(),
            register_with_marketplace: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_backend_returns_provider_url() {
        let backend = LocalBackend::default();
        let prepared = backend.prepare().await.unwrap();
        assert_eq!(prepared.public_url, "http://127.0.0.1:8080");
        assert!(!prepared.register_with_marketplace);
    }

    #[tokio::test]
    async fn local_backend_with_custom_url() {
        let backend = LocalBackend::with_url("http://127.0.0.1:9090");
        let prepared = backend.prepare().await.unwrap();
        assert_eq!(prepared.public_url, "http://127.0.0.1:9090");
    }
}
