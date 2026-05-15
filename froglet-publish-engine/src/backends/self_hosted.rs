//! SelfHostedBackend — trust the URL the user supplied, validate basic
//! reachability, and let the marketplace's `/v1/registrations` endpoint
//! do the heavy validation (signed descriptor in `/v1/feed`,
//! capabilities match, etc.).

use super::{HostingBackend, PreparedHosting};
use crate::error::PublishError;
use async_trait::async_trait;
use url::Url;

#[derive(Debug, Clone)]
pub struct SelfHostedBackend {
    pub url: Url,
}

impl SelfHostedBackend {
    pub fn new(url: Url) -> Self {
        Self { url }
    }
}

#[async_trait]
impl HostingBackend for SelfHostedBackend {
    fn name(&self) -> &'static str {
        "self_hosted"
    }

    async fn prepare(&self) -> Result<PreparedHosting, PublishError> {
        // The marketplace validates the URL shape + reachability when
        // /v1/registrations is hit. The engine only refuses obvious
        // local addresses here so the operator gets a fast error
        // instead of a 422 from the marketplace.
        let host = self.url.host_str().ok_or_else(|| PublishError::Hosting {
            backend: "self_hosted",
            reason: format!("URL {:?} has no host", self.url.as_str()),
        })?;
        let host_lower = host.to_ascii_lowercase();
        if host_lower == "localhost"
            || host_lower.ends_with(".localhost")
            || host_lower.ends_with(".local")
            || host_lower.ends_with(".internal")
        {
            return Err(PublishError::Hosting {
                backend: "self_hosted",
                reason: format!("URL host {host:?} is not publicly reachable"),
            });
        }
        Ok(PreparedHosting {
            public_url: self.url.as_str().trim_end_matches('/').to_string(),
            register_with_marketplace: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepts_public_https_url() {
        let backend = SelfHostedBackend::new(Url::parse("https://my-host.fly.dev").unwrap());
        let prepared = backend.prepare().await.unwrap();
        assert_eq!(prepared.public_url, "https://my-host.fly.dev");
        assert!(prepared.register_with_marketplace);
    }

    #[tokio::test]
    async fn rejects_localhost_url() {
        let backend = SelfHostedBackend::new(Url::parse("http://localhost:8080").unwrap());
        let err = backend.prepare().await.unwrap_err();
        assert!(matches!(
            err,
            PublishError::Hosting {
                backend: "self_hosted",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn rejects_dot_internal_url() {
        let backend = SelfHostedBackend::new(Url::parse("https://node.internal").unwrap());
        let err = backend.prepare().await.unwrap_err();
        assert!(matches!(
            err,
            PublishError::Hosting {
                backend: "self_hosted",
                ..
            }
        ));
    }
}
