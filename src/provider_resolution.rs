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
}

fn ip_v4_targets_local_network(ip: std::net::Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.octets() == [169, 254, 169, 254]
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

fn normalize_remote_endpoint_url(raw_url: &str, label: &str) -> Result<reqwest::Url, String> {
    let parsed =
        reqwest::Url::parse(raw_url).map_err(|error| format!("invalid {label}: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(format!("{label} must use http:// or https://"));
        }
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

    let reachability = if host.ends_with(".onion") {
        RemoteEndpointReachability::Onion
    } else if host_targets_local_network_literal(&host) {
        RemoteEndpointReachability::LocalOnly
    } else {
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| format!("{label} must include a known port"))?;
        let addresses = resolve_remote_endpoint_addresses(&host, port).await?;
        if addresses.into_iter().any(ip_targets_local_network) {
            RemoteEndpointReachability::LocalOnly
        } else {
            RemoteEndpointReachability::Public
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
    })
}

pub async fn validate_discovery_endpoint_url(raw_url: &str) -> Result<String, String> {
    let validated = classify_remote_endpoint_url(raw_url, "endpoint URL").await?;
    if validated.reachability != RemoteEndpointReachability::LocalOnly {
        return Ok(validated.normalized_url);
    }

    let parsed = reqwest::Url::parse(&validated.normalized_url)
        .map_err(|error| format!("invalid endpoint URL: {error}"))?;
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if parsed.scheme() == "http" && LOOPBACK_HOSTS.contains(&host.as_str()) {
        return Ok(validated.normalized_url);
    }

    Err(format!(
        "endpoint URL targets a private or local-network address and cannot be advertised through discovery: {raw_url}"
    ))
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

pub async fn runtime_accessible_provider_url(
    state: &AppState,
    raw_url: &str,
    provider_id: Option<&str>,
) -> Result<String, ResolutionFailure> {
    let local_provider_base_url = configured_runtime_provider_base_url()?;
    let is_local_provider =
        provider_id.is_none_or(|provider_id| provider_id == state.identity.node_id());
    if is_local_provider
        && let Some(base_url) = local_provider_base_url.as_deref()
        && raw_url.trim_end_matches('/') == base_url
    {
        return Ok(base_url.to_string());
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
            return Ok(base_url);
        }
        if provider_id.is_some() && is_local_provider {
            return Ok(validated.normalized_url);
        }
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": "provider URL targets a local or private-network address and is only allowed for the local node via FROGLET_RUNTIME_PROVIDER_BASE_URL",
                "value": raw_url,
            }),
        ));
    }

    Ok(validated.normalized_url)
}
