use axum::http::StatusCode;
use serde_json::json;
use std::net::IpAddr;

use crate::state::AppState;

pub type ResolutionFailure = (StatusCode, serde_json::Value);

const LOOPBACK_HOSTS: &[&str] = &[
    "127.0.0.1",
    "localhost",
    "localhost.",
    "localhost.localdomain",
    "::1",
    "[::1]",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteEndpointReachability {
    Public,
    Onion,
    LocalOnly,
}

#[derive(Debug, Clone)]
pub struct ValidatedRemoteEndpoint {
    pub normalized_url: String,
    pub reachability: RemoteEndpointReachability,
    pub pinned_public_addresses: Vec<IpAddr>,
}

#[derive(Debug, Clone)]
pub struct RuntimeProviderEndpoint {
    pub url: String,
    pub pinned_public_addresses: Vec<IpAddr>,
}

fn ip_v4_targets_local_network(ip: std::net::Ipv4Addr) -> bool {
    crate::builtins::safe_fetch::ipv4_is_local_or_private(ip)
}

fn ip_targets_local_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip_v4_targets_local_network(ip),
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ip.to_ipv4_mapped().is_some_and(ip_v4_targets_local_network)
                || ip.to_ipv4().is_some_and(ip_v4_targets_local_network)
        }
    }
}

fn host_targets_local_network_literal(host: &str) -> bool {
    let normalized = host
        .trim()
        .trim_matches(|character| character == '[' || character == ']');
    if LOOPBACK_HOSTS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(normalized))
    {
        return true;
    }
    match normalized.parse::<IpAddr>() {
        Ok(ip) => ip_targets_local_network(ip),
        Err(_) => false,
    }
}

fn host_is_valid_v3_onion(host: &str) -> bool {
    let Some(label) = host.strip_suffix(".onion") else {
        return false;
    };
    label.len() == 56
        && label
            .chars()
            .all(|character| matches!(character, 'a'..='z' | '2'..='7'))
}

fn normalize_remote_endpoint_url(raw_url: &str, label: &str) -> Result<reqwest::Url, String> {
    let parsed =
        reqwest::Url::parse(raw_url).map_err(|error| format!("invalid {label}: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(format!("{label} must use http:// or https://"));
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("{label} must not include credentials"));
    }
    if parsed.host_str().is_none() {
        return Err(format!("{label} must include a host"));
    }
    Ok(parsed)
}

async fn resolve_remote_endpoint_addresses(host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("failed to resolve host '{host}': {error}"))?;
    let resolved: Vec<IpAddr> = addresses.map(|address| address.ip()).collect();
    if resolved.is_empty() {
        return Err(format!("failed to resolve host '{host}'"));
    }
    Ok(resolved)
}

pub async fn classify_remote_endpoint_url(
    raw_url: &str,
    label: &str,
) -> Result<ValidatedRemoteEndpoint, String> {
    let parsed = normalize_remote_endpoint_url(raw_url, label)?;
    let normalized_url = parsed.as_str().trim_end_matches('/').to_string();
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if host.ends_with(".local") || host.ends_with(".internal") {
        return Err(format!(
            "{label} host must not use .local or .internal names"
        ));
    }

    let (reachability, pinned_public_addresses) = if host.ends_with(".onion") {
        if parsed.scheme() != "http" {
            return Err(format!("{label} must use http:// for Tor onion transport"));
        }
        if !host_is_valid_v3_onion(&host) {
            return Err(format!("{label} must use a Tor v3 .onion hostname"));
        }
        (RemoteEndpointReachability::Onion, Vec::new())
    } else if host_targets_local_network_literal(&host) {
        (RemoteEndpointReachability::LocalOnly, Vec::new())
    } else {
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| format!("{label} must include a known port"))?;
        let mut addresses = resolve_remote_endpoint_addresses(&host, port).await?;
        if addresses.iter().copied().any(ip_targets_local_network) {
            (RemoteEndpointReachability::LocalOnly, Vec::new())
        } else {
            addresses.sort();
            addresses.dedup();
            (RemoteEndpointReachability::Public, addresses)
        }
    };

    if parsed.scheme() == "http" && reachability == RemoteEndpointReachability::Public {
        return Err(format!(
            "{label} must use https:// (http:// is only allowed for local-node rewrites and .onion addresses)"
        ));
    }

    Ok(ValidatedRemoteEndpoint {
        normalized_url,
        reachability,
        pinned_public_addresses,
    })
}

pub async fn validate_remote_egress_url(
    raw_url: &str,
    label: &str,
    allow_local: bool,
    allow_local_hint: &str,
) -> Result<ValidatedRemoteEndpoint, String> {
    let endpoint = classify_remote_endpoint_url(raw_url, label).await?;
    if endpoint.reachability == RemoteEndpointReachability::LocalOnly && !allow_local {
        return Err(format!(
            "{label} targets a local or private-network address; {allow_local_hint}"
        ));
    }
    Ok(endpoint)
}

pub fn configured_runtime_provider_base_url() -> Result<Option<String>, ResolutionFailure> {
    let Some(raw) = std::env::var("FROGLET_RUNTIME_PROVIDER_BASE_URL").ok() else {
        return Ok(None);
    };

    let parsed = reqwest::Url::parse(&raw).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid FROGLET_RUNTIME_PROVIDER_BASE_URL", "details": error.to_string(), "value": raw }),
        )
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "FROGLET_RUNTIME_PROVIDER_BASE_URL must use http:// or https://", "value": raw }),
        ));
    }
    if parsed.path() != "/" {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": "FROGLET_RUNTIME_PROVIDER_BASE_URL must not include a path", "value": raw }),
        ));
    }

    Ok(Some(parsed.as_str().trim_end_matches('/').to_string()))
}

pub async fn runtime_accessible_provider_endpoint(
    state: &AppState,
    raw_url: &str,
    provider_id: Option<&str>,
) -> Result<RuntimeProviderEndpoint, ResolutionFailure> {
    // The env var is an explicit override (required on split
    // runtime/provider deployments where the provider is another host).
    // Without it, a dual node falls back to its own bound provider
    // listener recorded at startup, so local self-invocation works out of
    // the box. Runtime-only nodes record no listener and stay fail-closed.
    let local_provider_base_url = match configured_runtime_provider_base_url()? {
        Some(base_url) => Some(base_url),
        None => state
            .transport_status
            .lock()
            .await
            .local_provider_bound_addr
            .map(|bound_addr| format!("http://{bound_addr}")),
    };
    let is_local_provider =
        provider_id.is_some_and(|provider_id| provider_id == state.identity.node_id());
    if is_local_provider && let Some(base_url) = local_provider_base_url.as_deref() {
        let normalized_raw_url = raw_url.trim_end_matches('/');
        if normalized_raw_url == base_url {
            return Ok(RuntimeProviderEndpoint {
                url: base_url.to_string(),
                pinned_public_addresses: Vec::new(),
            });
        }
        if state
            .config
            .public_base_url
            .as_deref()
            .is_some_and(|public_url| normalized_raw_url == public_url)
        {
            return Ok(RuntimeProviderEndpoint {
                url: base_url.to_string(),
                pinned_public_addresses: Vec::new(),
            });
        }
        let advertised_clearnet_url = state.transport_status.lock().await.clearnet_url.clone();
        if advertised_clearnet_url
            .as_deref()
            .is_some_and(|clearnet_url| normalized_raw_url == clearnet_url)
        {
            return Ok(RuntimeProviderEndpoint {
                url: base_url.to_string(),
                pinned_public_addresses: Vec::new(),
            });
        }
    }

    let validated = classify_remote_endpoint_url(raw_url, "provider URL")
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                json!({ "error": error, "value": raw_url }),
            )
        })?;
    if validated.reachability == RemoteEndpointReachability::LocalOnly {
        if is_local_provider && let Some(base_url) = local_provider_base_url {
            return Ok(RuntimeProviderEndpoint {
                url: base_url,
                pinned_public_addresses: Vec::new(),
            });
        }
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "provider URL targets a local or private-network address and is only allowed for the local node's own provider listener (dual-mode nodes trust their bound listener automatically; split runtime deployments must set FROGLET_RUNTIME_PROVIDER_BASE_URL)",
                "value": raw_url,
            }),
        ));
    }

    Ok(RuntimeProviderEndpoint {
        url: validated.normalized_url,
        pinned_public_addresses: validated.pinned_public_addresses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn public_ip_endpoint_carries_pinned_address() {
        let endpoint = classify_remote_endpoint_url("https://8.8.8.8/", "provider URL")
            .await
            .expect("classified public endpoint");

        assert_eq!(endpoint.reachability, RemoteEndpointReachability::Public);
        assert_eq!(
            endpoint.pinned_public_addresses,
            vec!["8.8.8.8".parse::<IpAddr>().expect("ip")]
        );
    }

    #[tokio::test]
    async fn endpoint_rejects_embedded_credentials() {
        let error = classify_remote_endpoint_url("https://user:pass@8.8.8.8/", "provider URL")
            .await
            .expect_err("provider URL credentials must be rejected");

        assert!(
            error.contains("must not include credentials"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn endpoint_accepts_http_v3_onion_transport() {
        let endpoint = classify_remote_endpoint_url(
            &format!("http://{}.onion/", "a".repeat(56)),
            "provider URL",
        )
        .await
        .expect("classified onion endpoint");

        assert_eq!(endpoint.reachability, RemoteEndpointReachability::Onion);
        assert!(endpoint.pinned_public_addresses.is_empty());
    }

    #[tokio::test]
    async fn endpoint_rejects_non_v3_or_https_onion_transport() {
        let short_error =
            classify_remote_endpoint_url("http://abcdefghijklmnop.onion/", "provider URL")
                .await
                .expect_err("short onion host must be rejected");
        assert!(
            short_error.contains("Tor v3"),
            "unexpected error: {short_error}"
        );

        let https_error = classify_remote_endpoint_url(
            &format!("https://{}.onion/", "a".repeat(56)),
            "provider URL",
        )
        .await
        .expect_err("https onion transport must be rejected");
        assert!(
            https_error.contains("http://"),
            "unexpected error: {https_error}"
        );
    }

    #[tokio::test]
    async fn local_endpoint_has_no_pinned_public_addresses() {
        let endpoint = classify_remote_endpoint_url("http://127.0.0.1:8080/", "provider URL")
            .await
            .expect("classified local endpoint");

        assert_eq!(endpoint.reachability, RemoteEndpointReachability::LocalOnly);
        assert!(endpoint.pinned_public_addresses.is_empty());
    }

    #[tokio::test]
    async fn remote_egress_rejects_local_endpoint_without_override() {
        let error = validate_remote_egress_url(
            "http://127.0.0.1:8080/",
            "marketplace URL",
            false,
            "set FROGLET_MARKETPLACE_ALLOW_LOCAL=1 only for explicit dev/test use",
        )
        .await
        .expect_err("local endpoint should require override");

        assert!(
            error.contains("local or private-network"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn remote_egress_allows_local_endpoint_with_override() {
        let endpoint = validate_remote_egress_url(
            "http://127.0.0.1:8080/",
            "marketplace URL",
            true,
            "set FROGLET_MARKETPLACE_ALLOW_LOCAL=1 only for explicit dev/test use",
        )
        .await
        .expect("local endpoint should be allowed with override");

        assert_eq!(endpoint.reachability, RemoteEndpointReachability::LocalOnly);
    }
}
