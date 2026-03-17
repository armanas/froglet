use std::{
    collections::BTreeMap,
    future::Future,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use reqwest::{
    Client, Method, Url,
    header::{HeaderMap, HeaderName, HeaderValue},
    redirect::Policy as RedirectPolicy,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    runtime::{Builder as RuntimeBuilder, Handle, RuntimeFlavor},
    task::block_in_place,
};

use crate::{
    config::WasmHttpPolicy,
    wasm::{WASM_CAPABILITY_HTTP_FETCH, WASM_CAPABILITY_HTTP_FETCH_AUTH_PREFIX},
};

#[derive(Debug, Deserialize)]
pub struct HttpFetchRequest {
    #[serde(default = "default_http_method")]
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body_text: Option<String>,
    #[serde(default)]
    pub body_base64: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub auth_profile: Option<String>,
}

pub fn fetch(
    policy: &WasmHttpPolicy,
    client: &Client,
    granted_capabilities: &[String],
    http_calls_used: &mut u32,
    request: HttpFetchRequest,
    deadline: Option<Instant>,
) -> Result<Value, String> {
    require_capability(granted_capabilities, WASM_CAPABILITY_HTTP_FETCH)?;

    if *http_calls_used >= policy.max_calls_per_execution {
        return Err("Wasm HTTP call limit exceeded".to_string());
    }
    *http_calls_used = http_calls_used.saturating_add(1);

    let parsed_url =
        Url::parse(&request.url).map_err(|error| format!("invalid http url: {error}"))?;
    match parsed_url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(format!(
                "unsupported http url scheme '{scheme}'; allowed schemes are http and https"
            ));
        }
    }

    let host = parsed_url
        .host_str()
        .ok_or_else(|| "http url must include a host".to_string())?
        .to_ascii_lowercase();
    if !policy
        .allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&host))
    {
        return Err(format!(
            "http host '{host}' is not allowed by provider policy"
        ));
    }
    let host_port = parsed_url
        .port_or_known_default()
        .ok_or_else(|| "http url must include a known port".to_string())?;

    let timeout_ms = request
        .timeout_ms
        .unwrap_or(policy.max_timeout_ms)
        .min(policy.max_timeout_ms);
    if timeout_ms == 0 {
        return Err("http timeout_ms must be greater than zero".to_string());
    }

    let method = Method::from_bytes(request.method.as_bytes())
        .map_err(|error| format!("invalid http method: {error}"))?;
    let mut header_map = build_header_map(&request.headers)?;

    if let Some(profile_name) = request.auth_profile.as_ref() {
        let capability = format!("{WASM_CAPABILITY_HTTP_FETCH_AUTH_PREFIX}{profile_name}");
        require_capability(granted_capabilities, &capability)?;
        let profile = policy.auth_profiles.get(profile_name).ok_or_else(|| {
            format!("http auth profile '{profile_name}' is not configured on this provider")
        })?;
        let profile_header_name = HeaderName::from_str(&profile.header_name).map_err(|error| {
            format!(
                "invalid http auth profile header name '{}': {error}",
                profile.header_name
            )
        })?;
        let profile_header_value =
            HeaderValue::from_str(&profile.header_value).map_err(|error| {
                format!(
                    "invalid http auth profile header value for '{}': {error}",
                    profile.header_name
                )
            })?;
        header_map.insert(profile_header_name, profile_header_value);
    }

    let body = request_body_bytes(&request)?;
    if body.len() > policy.max_request_body_bytes {
        return Err("http request body exceeds provider policy limit".to_string());
    }

    let request_timeout = Duration::from_millis(timeout_ms);
    let allow_private_networks = policy.allow_private_networks;
    let host_for_resolution = host.clone();
    let max_response_body_bytes = policy.max_response_body_bytes;
    let default_client = client.clone();
    run_abortable_http_task(
        async move {
            let request_client = if allow_private_networks {
                default_client
            } else {
                let vetted_addresses =
                    resolve_public_http_host_addresses(&host_for_resolution, host_port, deadline)
                        .await?;
                build_resolved_http_client(
                    &host_for_resolution,
                    host_port,
                    &vetted_addresses,
                )?
            };
            let mut builder = request_client
                .request(method, parsed_url)
                .headers(header_map)
                .timeout(request_timeout);
            if !body.is_empty() {
                builder = builder.body(body);
            }
            fetch_streamed_response(builder, request_timeout, max_response_body_bytes, deadline)
                .await
        },
        deadline,
    )
}

fn default_http_method() -> String {
    "GET".to_string()
}

fn build_header_map(headers: &BTreeMap<String, String>) -> Result<HeaderMap, String> {
    let mut header_map = HeaderMap::new();
    for (name, value) in headers {
        let header_name = HeaderName::from_str(name)
            .map_err(|error| format!("invalid http header name '{name}': {error}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|error| format!("invalid http header value for '{name}': {error}"))?;
        header_map.insert(header_name, header_value);
    }
    Ok(header_map)
}

fn request_body_bytes(request: &HttpFetchRequest) -> Result<Vec<u8>, String> {
    match (&request.body_text, &request.body_base64) {
        (Some(_), Some(_)) => {
            Err("http request must not set both body_text and body_base64".into())
        }
        (Some(body_text), None) => Ok(body_text.as_bytes().to_vec()),
        (None, Some(body_base64)) => BASE64_STANDARD
            .decode(body_base64)
            .map_err(|error| format!("invalid http body_base64: {error}")),
        (None, None) => Ok(Vec::new()),
    }
}

fn require_capability(granted_capabilities: &[String], capability: &str) -> Result<(), String> {
    if granted_capabilities
        .iter()
        .any(|granted| granted == capability)
    {
        Ok(())
    } else {
        Err(format!("missing granted capability '{capability}'"))
    }
}

async fn fetch_streamed_response(
    builder: reqwest::RequestBuilder,
    request_timeout: Duration,
    max_response_body_bytes: usize,
    deadline: Option<Instant>,
) -> Result<Value, String> {
    let response = await_reqwest_with_deadline(
        deadline,
        builder.timeout(request_timeout).send(),
        "http request failed",
        "http request deadline exceeded",
    )
    .await?;
    let status = response.status();
    let mut response_headers = BTreeMap::new();
    for (name, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            response_headers.insert(name.to_string(), value.to_string());
        }
    }

    let mut response = response;
    let mut response_bytes = Vec::new();
    while let Some(chunk) = await_reqwest_with_deadline(
        deadline,
        response.chunk(),
        "failed to read http response body",
        "http request deadline exceeded",
    )
    .await?
    {
        let next_size = response_bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| "http response body exceeds provider policy limit".to_string())?;
        if next_size > max_response_body_bytes {
            return Err("http response body exceeds provider policy limit".to_string());
        }
        response_bytes.extend_from_slice(&chunk);
    }

    let (body_text, body_base64) = match String::from_utf8(response_bytes) {
        Ok(body_text) => (Some(body_text), None),
        Err(error) => (None, Some(BASE64_STANDARD.encode(error.into_bytes()))),
    };

    Ok(json!({
        "status": status.as_u16(),
        "headers": response_headers,
        "body_text": body_text,
        "body_base64": body_base64,
    }))
}

fn build_resolved_http_client(
    host: &str,
    port: u16,
    vetted_addresses: &[IpAddr],
) -> Result<Client, String> {
    let resolved_socket_addrs: Vec<SocketAddr> = vetted_addresses
        .iter()
        .copied()
        .map(|ip| SocketAddr::new(ip, port))
        .collect();
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .redirect(RedirectPolicy::none())
        .resolve_to_addrs(host, &resolved_socket_addrs)
        .build()
        .map_err(|error| format!("failed to build async http client: {error}"))
}

fn run_abortable_http_task<F>(future: F, deadline: Option<Instant>) -> Result<Value, String>
where
    F: Future<Output = Result<Value, String>> + Send + 'static,
{
    if deadline_expired(deadline) {
        return Err("http request deadline exceeded".to_string());
    }

    if let Ok(handle) = Handle::try_current()
        && handle.runtime_flavor() == RuntimeFlavor::MultiThread
    {
        return block_in_place(|| handle.block_on(run_spawned_http_task(future, deadline)));
    }

    let runtime = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to initialize runtime for host http call: {error}"))?;
    runtime.block_on(run_spawned_http_task(future, deadline))
}

async fn run_spawned_http_task<F>(future: F, deadline: Option<Instant>) -> Result<Value, String>
where
    F: Future<Output = Result<Value, String>> + Send + 'static,
{
    if deadline_expired(deadline) {
        return Err("http request deadline exceeded".to_string());
    }

    let mut task = tokio::spawn(future);
    let joined = if let Some(remaining) = deadline_remaining(deadline) {
        match tokio::time::timeout(remaining, &mut task).await {
            Ok(joined) => joined,
            Err(_) => {
                task.abort();
                return Err("http request deadline exceeded".to_string());
            }
        }
    } else if deadline.is_some() {
        task.abort();
        return Err("http request deadline exceeded".to_string());
    } else {
        task.await
    };

    joined.map_err(|error| format!("host http task failed: {error}"))?
}

async fn await_reqwest_with_deadline<F, T>(
    deadline: Option<Instant>,
    future: F,
    request_error_prefix: &str,
    timeout_error: &str,
) -> Result<T, String>
where
    F: Future<Output = Result<T, reqwest::Error>>,
{
    let mapped = async move {
        future
            .await
            .map_err(|error| format!("{request_error_prefix}: {error}"))
    };

    if let Some(remaining) = deadline_remaining(deadline) {
        tokio::time::timeout(remaining, mapped)
            .await
            .map_err(|_| timeout_error.to_string())?
    } else if deadline.is_some() {
        Err(timeout_error.to_string())
    } else {
        mapped.await
    }
}

fn deadline_remaining(deadline: Option<Instant>) -> Option<Duration> {
    deadline.and_then(|limit| limit.checked_duration_since(Instant::now()))
}

fn deadline_expired(deadline: Option<Instant>) -> bool {
    deadline.is_some() && deadline_remaining(deadline).is_none()
}

fn ip_v4_targets_private_network(ip: std::net::Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback() || ip.is_link_local()
}

fn ip_targets_private_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip_v4_targets_private_network(ip),
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || ip.to_ipv4_mapped()
                    .map_or(false, ip_v4_targets_private_network)
                || ip.to_ipv4()
                    .map_or(false, ip_v4_targets_private_network)
        }
    }
}

fn host_targets_private_network_literal(host: &str) -> bool {
    if matches!(host, "localhost" | "localhost.localdomain") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => ip_targets_private_network(ip),
        Err(_) => false,
    }
}

async fn resolve_http_host_addresses(
    host: &str,
    port: u16,
    deadline: Option<Instant>,
) -> Result<Vec<IpAddr>, String> {
    let lookup = tokio::net::lookup_host((host, port));
    let addresses = if let Some(remaining) = deadline_remaining(deadline) {
        tokio::time::timeout(remaining, lookup)
            .await
            .map_err(|_| "http request deadline exceeded".to_string())?
            .map_err(|error| format!("failed to resolve http host '{host}': {error}"))?
    } else if deadline.is_some() {
        return Err("http request deadline exceeded".to_string());
    } else {
        lookup
            .await
            .map_err(|error| format!("failed to resolve http host '{host}': {error}"))?
    };

    let resolved: Vec<IpAddr> = addresses.map(|address| address.ip()).collect();
    if resolved.is_empty() {
        return Err(format!("failed to resolve http host '{host}'"));
    }
    Ok(resolved)
}

async fn resolve_public_http_host_addresses(
    host: &str,
    port: u16,
    deadline: Option<Instant>,
) -> Result<Vec<IpAddr>, String> {
    if host_targets_private_network_literal(host) {
        return Err(format!(
            "http host '{host}' is not allowed by provider private-network policy"
        ));
    }

    let resolved_addresses = resolve_http_host_addresses(host, port, deadline).await?;
    if resolved_addresses
        .iter()
        .copied()
        .any(ip_targets_private_network)
    {
        return Err(format!(
            "http host '{host}' resolved to a private-network address, which is not allowed by provider policy"
        ));
    }
    Ok(resolved_addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WasmHttpAuthProfile, WasmHttpPolicy};
    use std::time::Instant;

    fn test_policy() -> WasmHttpPolicy {
        WasmHttpPolicy {
            allowed_hosts: vec![
                "127.0.0.1".to_string(),
                "localhost.".to_string(),
                "example.com".to_string(),
            ],
            allow_private_networks: false,
            max_calls_per_execution: 2,
            max_timeout_ms: 5_000,
            max_request_body_bytes: 1_024,
            max_response_body_bytes: 1_024,
            max_redirects: 2,
            auth_profiles: BTreeMap::from([(
                "github".to_string(),
                WasmHttpAuthProfile {
                    header_name: "authorization".to_string(),
                    header_value: "Bearer token".to_string(),
                },
            )]),
        }
    }

    #[test]
    fn private_network_hosts_are_blocked_by_default() {
        let mut http_calls_used = 0;
        let error = fetch(
            &test_policy(),
            &Client::builder().build().unwrap(),
            &[WASM_CAPABILITY_HTTP_FETCH.to_string()],
            &mut http_calls_used,
            HttpFetchRequest {
                method: "GET".to_string(),
                url: "http://127.0.0.1:8080".to_string(),
                headers: BTreeMap::new(),
                body_text: None,
                body_base64: None,
                timeout_ms: None,
                auth_profile: None,
            },
            None,
        )
        .expect_err("expected private network policy failure");

        assert!(
            error.contains("private-network"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn dns_resolved_private_network_hosts_are_blocked_by_default() {
        let mut http_calls_used = 0;
        let error = fetch(
            &test_policy(),
            &Client::builder().build().unwrap(),
            &[WASM_CAPABILITY_HTTP_FETCH.to_string()],
            &mut http_calls_used,
            HttpFetchRequest {
                method: "GET".to_string(),
                url: "http://localhost.:8080".to_string(),
                headers: BTreeMap::new(),
                body_text: None,
                body_base64: None,
                timeout_ms: None,
                auth_profile: None,
            },
            None,
        )
        .expect_err("expected dns-based private network policy failure");

        assert!(
            error.contains("private-network") || error.contains("resolved to a private-network"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn auth_profile_requires_matching_capability() {
        let mut http_calls_used = 0;
        let error = fetch(
            &test_policy(),
            &Client::builder().build().unwrap(),
            &[WASM_CAPABILITY_HTTP_FETCH.to_string()],
            &mut http_calls_used,
            HttpFetchRequest {
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                headers: BTreeMap::new(),
                body_text: None,
                body_base64: None,
                timeout_ms: None,
                auth_profile: Some("github".to_string()),
            },
            None,
        )
        .expect_err("expected missing auth capability");

        assert!(
            error.contains("missing granted capability"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn expired_deadline_fails_before_network_dispatch() {
        let mut http_calls_used = 0;
        let error = fetch(
            &test_policy(),
            &Client::builder().build().unwrap(),
            &[WASM_CAPABILITY_HTTP_FETCH.to_string()],
            &mut http_calls_used,
            HttpFetchRequest {
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                headers: BTreeMap::new(),
                body_text: None,
                body_base64: None,
                timeout_ms: None,
                auth_profile: None,
            },
            Some(Instant::now()),
        )
        .expect_err("expected deadline failure");

        assert!(error.contains("deadline"), "unexpected error: {error}");
    }

    #[test]
    fn empty_allowed_hosts_denies_all_requests() {
        let mut policy = test_policy();
        policy.allowed_hosts = Vec::new();
        let mut http_calls_used = 0;
        let error = fetch(
            &policy,
            &Client::builder().build().unwrap(),
            &[WASM_CAPABILITY_HTTP_FETCH.to_string()],
            &mut http_calls_used,
            HttpFetchRequest {
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                headers: BTreeMap::new(),
                body_text: None,
                body_base64: None,
                timeout_ms: None,
                auth_profile: None,
            },
            None,
        )
        .expect_err("expected empty allowed_hosts denial");

        assert!(
            error.contains("not allowed by provider policy"),
            "unexpected error: {error}"
        );
    }
}
