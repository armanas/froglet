//! SelfHostedBackend — trust the URL the user supplied, validate basic
//! reachability, and let the marketplace's `/v1/registrations` endpoint
//! do the heavy validation (signed descriptor in `/v1/feed`,
//! capabilities match, etc.).

use super::{HostingBackend, PreparedHosting};
use crate::error::PublishError;
use async_trait::async_trait;
use std::net::IpAddr;
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

fn publicly_routable_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_multicast()
                || ip.is_unspecified()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 198 && (18..=19).contains(&octets[1])))
        }
        IpAddr::V6(ip) => {
            let octets = ip.octets();
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return publicly_routable_ip(IpAddr::V4(mapped));
            }
            !(ip.is_loopback()
                || ip.is_multicast()
                || ip.is_unspecified()
                || (octets[0] & 0xfe) == 0xfc
                || (octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80)
                || octets[0..4] == [0x20, 0x01, 0x0d, 0xb8])
        }
    }
}

pub(crate) fn validated_public_self_hosted_url(value: &str) -> Result<String, PublishError> {
    let url = Url::parse(value).map_err(|error| PublishError::Hosting {
        backend: "self_hosted",
        reason: format!("invalid self-hosted URL: {error}"),
    })?;
    let host = url.host_str().ok_or_else(|| PublishError::Hosting {
        backend: "self_hosted",
        reason: "self-hosted URL has no host".to_string(),
    })?;
    let normalized_host = host
        .trim_matches(|character| character == '[' || character == ']')
        .to_ascii_lowercase();
    let reserved_name = normalized_host == "localhost"
        || normalized_host.ends_with(".localhost")
        || normalized_host.ends_with(".local")
        || normalized_host.ends_with(".internal")
        || normalized_host.ends_with(".onion");
    let non_public_literal = normalized_host
        .parse::<IpAddr>()
        .is_ok_and(|ip| !publicly_routable_ip(ip));
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || reserved_name
        || non_public_literal
    {
        return Err(PublishError::Hosting {
            backend: "self_hosted",
            reason: "self-hosted URL must be a credential-free public HTTPS root origin"
                .to_string(),
        });
    }
    Ok(url.to_string().trim_end_matches('/').to_string())
}

#[async_trait]
impl HostingBackend for SelfHostedBackend {
    fn name(&self) -> &'static str {
        "self_hosted"
    }

    async fn prepare(&self) -> Result<PreparedHosting, PublishError> {
        Ok(PreparedHosting {
            public_url: validated_public_self_hosted_url(self.url.as_str())?,
            register_with_marketplace: true,
            publication_revision: None,
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

    #[test]
    fn public_url_must_be_a_public_https_root_origin() {
        assert!(validated_public_self_hosted_url("https://provider.example/").is_ok());
        assert!(validated_public_self_hosted_url("https://8.8.8.8").is_ok());
        assert!(validated_public_self_hosted_url("https://[2606:4700:4700::1111]").is_ok());
        for invalid in [
            "http://provider.example",
            "https://user@provider.example",
            "https://provider.example/api",
            "https://provider.example?token=x",
            "https://provider.example#fragment",
            "https://localhost",
            "https://node.local",
            "https://node.internal",
            "https://abcdefghijklmnop.onion",
            "https://127.0.0.1",
            "https://10.0.0.1",
            "https://169.254.169.254",
            "https://192.168.1.1",
            "https://[::1]",
            "https://[fc00::1]",
            "https://[fe80::1]",
        ] {
            assert!(
                validated_public_self_hosted_url(invalid).is_err(),
                "unexpected valid self-hosted URL: {invalid}"
            );
        }
    }

    #[tokio::test]
    async fn prepare_applies_the_root_origin_validator() {
        let backend = SelfHostedBackend::new(
            Url::parse("https://provider.example/private").expect("test URL"),
        );
        let error = backend
            .prepare()
            .await
            .expect_err("prepare must reject a path-bearing endpoint");
        assert!(error.to_string().contains("root origin"));
    }
}
