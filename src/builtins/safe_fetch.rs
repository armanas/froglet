//! Shared safe-fetch helper used by demo builtins that need to read a URL.
//!
//! Builtin services run in-process inside the provider node, so they share the
//! provider's network namespace. That means a naive HTTP fetch is an SSRF
//! reflector — a buyer could pass `http://169.254.169.254/...` and the provider
//! would happily fetch cloud metadata on the buyer's behalf. To stop that,
//! every fetch goes through this module:
//!
//! 1. The URL must use `http` or `https`. Other schemes are rejected.
//! 2. If the host parses as an IP literal, that IP must not target a private
//!    network, loopback, link-local, multicast, or unique-local address.
//! 3. The host name `localhost` (and `localhost.localdomain`) is rejected.
//! 4. Resolved DNS addresses are filtered the same way before any connection
//!    is opened.
//! 5. The response body is hard-capped at `max_bytes`. A larger response is
//!    truncated and the function returns an error rather than silently
//!    returning a partial body.
//!
//! The validators in [`validate_fetch_url`] are sync and unit-testable. The
//! [`safe_fetch`] async path uses them and adds the post-DNS check + the
//! request itself.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use reqwest::{Client, Proxy, Url, redirect::Policy as RedirectPolicy};
use serde::de::DeserializeOwned;

/// Default response cap for demo fetches. Bigger than a sane web page,
/// smaller than a download. Callers can override per-request.
pub const DEFAULT_MAX_BYTES: u64 = 1_048_576; // 1 MiB

/// Hard upper bound regardless of caller request — keeps a single demo deal
/// from monopolising provider memory.
pub const MAX_ALLOWED_BYTES: u64 = 8 * 1_048_576; // 8 MiB

/// Default request timeout. Caller may shorten but never lengthen past
/// [`MAX_ALLOWED_TIMEOUT_MS`].
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;

/// Hard ceiling on per-request timeout. Stops one buyer from pinning a
/// provider thread on a slow upstream forever.
pub const MAX_ALLOWED_TIMEOUT_MS: u64 = 30_000;

/// Outcome of a successful safe fetch. The body is owned to keep the API
/// simple — these handlers operate on small responses.
#[derive(Debug)]
pub struct SafeFetchOutcome {
    pub final_url: String,
    pub status_code: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Validate that a URL string is safe to fetch *based purely on its literal
/// form*. Does not perform DNS. Intended to fail fast before any network IO.
pub fn validate_fetch_url(raw: &str) -> Result<Url, String> {
    validate_fetch_url_with_policy(raw, false)
}

/// Same as [`validate_fetch_url`] but lifts the private-network block when
/// `allow_private_networks` is true. Test-only escape hatch — production
/// callers should use [`validate_fetch_url`].
pub fn validate_fetch_url_with_policy(
    raw: &str,
    allow_private_networks: bool,
) -> Result<Url, String> {
    let parsed = Url::parse(raw).map_err(|e| format!("invalid url '{raw}': {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "unsupported url scheme '{other}'; only http and https are allowed"
            ));
        }
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "url must include a host".to_string())?
        .to_ascii_lowercase();
    if !allow_private_networks && host_is_blocked_literal(&host) {
        return Err(format!(
            "host '{host}' targets a private/loopback/link-local network"
        ));
    }
    Ok(parsed)
}

/// Returns true for IP-literal hosts that point at non-public networks, plus
/// the special `localhost` strings. Hostnames that need DNS resolution are
/// not handled here — the async path resolves them and re-checks.
fn host_is_blocked_literal(host: &str) -> bool {
    if matches!(host, "localhost" | "localhost.localdomain") {
        return true;
    }
    // url crate brackets v6 hosts; strip if present.
    let candidate = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    match candidate.parse::<IpAddr>() {
        Ok(ip) => ip_is_blocked(ip),
        Err(_) => false,
    }
}

fn ip_is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_is_blocked(v4),
        IpAddr::V6(v6) => ipv6_is_blocked(v6),
    }
}

fn ipv4_is_blocked(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        // 169.254.169.254 — cloud metadata. Already covered by is_link_local
        // for v4 but keep explicit so intent is obvious.
        || ip == Ipv4Addr::new(169, 254, 169, 254)
}

fn ipv6_is_blocked(ip: Ipv6Addr) -> bool {
    if ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
    {
        return true;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return ipv4_is_blocked(v4);
    }
    false
}

/// Per-request fetch policy. Callers fill in caps; the helper clamps them
/// to the global maxima so a buyer cannot pass `max_bytes = u64::MAX`.
#[derive(Debug, Clone)]
pub struct FetchPolicy {
    pub max_bytes: u64,
    pub timeout_ms: u64,
    /// Optional SOCKS5 proxy URL (e.g. `socks5h://127.0.0.1:9050`) used when
    /// the target hostname ends in `.onion`. `None` means "no proxy" — the
    /// fetch then attempts a direct connection, which will fail for `.onion`
    /// destinations on a host without a Tor sidecar.
    pub tor_socks_proxy: Option<String>,
    /// When true, lift the SSRF block on private/loopback/link-local
    /// addresses. Intended only for in-process integration tests where the
    /// target server bound `127.0.0.1`. Production callers MUST leave this
    /// false (the `Default::default()` value) so a buyer-supplied URL
    /// targeting `169.254.169.254` is rejected.
    pub allow_private_networks: bool,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            tor_socks_proxy: None,
            allow_private_networks: false,
        }
    }
}

impl FetchPolicy {
    /// Clamp caller-provided values against the global ceilings. Always
    /// applied before the request; never trust the input.
    fn clamped(self) -> Self {
        Self {
            max_bytes: self.max_bytes.clamp(1, MAX_ALLOWED_BYTES),
            timeout_ms: self.timeout_ms.clamp(100, MAX_ALLOWED_TIMEOUT_MS),
            tor_socks_proxy: self.tor_socks_proxy,
            allow_private_networks: self.allow_private_networks,
        }
    }
}

/// Fetch a URL with SSRF protection, size cap, and timeout cap. Redirects are
/// followed up to 5 hops; each hop is re-validated against the same SSRF
/// rules so a 302 to `http://127.0.0.1` is rejected. To close the
/// DNS-rebinding window, the resolved IPs of the initial host are pinned
/// into the reqwest client via `resolve_to_addrs`, and cross-host redirects
/// are rejected (since a different host would re-enter system DNS and
/// re-open the rebinding window).
pub async fn safe_fetch(raw_url: &str, policy: FetchPolicy) -> Result<SafeFetchOutcome, String> {
    let validated = validate_fetch_url_with_policy(raw_url, policy.allow_private_networks)?;
    let policy = policy.clamped();

    let host = validated
        .host_str()
        .ok_or_else(|| "safe_fetch: url has no host".to_string())?
        .to_ascii_lowercase();
    let is_onion = host.ends_with(".onion");

    // For .onion destinations the SOCKS proxy IS the destination as far as
    // this process is concerned — DNS resolution and IP-pinning happen
    // inside the Tor circuit, not here. For clearnet hosts we resolve and
    // validate first, then pin the resolved IPs into reqwest so the real
    // request cannot land on a different IP than the one we approved.
    let pinned_addrs: Option<Vec<SocketAddr>> = if is_onion {
        None
    } else {
        let port = validated.port_or_known_default().unwrap_or(0);
        let resolved_ips =
            check_host_resolution(&host, port, policy.allow_private_networks).await?;
        Some(
            resolved_ips
                .iter()
                .copied()
                .map(|ip| SocketAddr::new(ip, port.max(1)))
                .collect(),
        )
    };

    let initial_host = host.clone();
    let allow_private = policy.allow_private_networks;
    let mut builder = Client::builder()
        .timeout(Duration::from_millis(policy.timeout_ms))
        .redirect(RedirectPolicy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("too many redirects");
            }
            // Re-validate every redirect target: scheme + literal IP block.
            let next = match validate_fetch_url_with_policy(attempt.url().as_str(), allow_private) {
                Ok(u) => u,
                Err(e) => return attempt.error(e),
            };
            // Reject cross-host redirects: `resolve_to_addrs` only pins the
            // initial host, so a redirect to a different hostname would
            // re-enter system DNS and defeat the rebinding mitigation.
            // Same-host redirects remain safe.
            let next_host = next.host_str().unwrap_or("").to_ascii_lowercase();
            if next_host != initial_host {
                return attempt.error("safe_fetch: cross-host redirect not allowed");
            }
            attempt.follow()
        }));
    if let Some(addrs) = pinned_addrs.as_ref() {
        builder = builder.resolve_to_addrs(&host, addrs);
    }
    if is_onion {
        let proxy_url = policy.tor_socks_proxy.as_deref().ok_or_else(|| {
            "safe_fetch: .onion destination requires FetchPolicy.tor_socks_proxy".to_string()
        })?;
        let proxy = Proxy::all(proxy_url)
            .map_err(|e| format!("safe_fetch: invalid Tor SOCKS proxy {proxy_url}: {e}"))?;
        builder = builder.proxy(proxy);
    }
    let client = builder
        .build()
        .map_err(|e| format!("safe_fetch: build client: {e}"))?;

    let response = client
        .get(validated.clone())
        .header("user-agent", "froglet-demo-fetch/0.1")
        .send()
        .await
        .map_err(|e| format!("safe_fetch: request failed: {e}"))?;

    let final_url = response.url().to_string();
    let status_code = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Stream body with hard cap. We read chunks rather than `bytes()` so the
    // provider can't be tricked into allocating an oversized buffer.
    let mut body: Vec<u8> = Vec::new();
    let mut stream = response;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|e| format!("safe_fetch: read chunk: {e}"))?
    {
        if (body.len() as u64).saturating_add(chunk.len() as u64) > policy.max_bytes {
            return Err(format!(
                "safe_fetch: response exceeded max_bytes cap of {}",
                policy.max_bytes
            ));
        }
        body.extend_from_slice(&chunk);
    }

    Ok(SafeFetchOutcome {
        final_url,
        status_code,
        content_type,
        body,
    })
}

/// Typed wrapper over [`safe_fetch`] that JSON-decodes the response body into
/// `T`. Use this for consumers that fetch a Froglet-protocol artifact or any
/// other JSON document — replaces the per-call `response.json().await?`
/// idiom that previously appeared in three different code paths
/// (`froglet-adapter::fetch_json`, `marketplace-api::registration::fetch_json`,
/// and demo builtins) with one place to fix when the rules drift.
///
/// Errors:
/// - any error from [`safe_fetch`] (URL invalid, SSRF block, body cap, ...)
/// - non-2xx HTTP status — caller can extract status via [`safe_fetch`] if it
///   needs to distinguish 404 from 500
/// - JSON decode failure
pub async fn safe_fetch_json<T: DeserializeOwned>(
    raw_url: &str,
    policy: FetchPolicy,
) -> Result<T, String> {
    let outcome = safe_fetch(raw_url, policy).await?;
    if !(200..300).contains(&outcome.status_code) {
        let preview = String::from_utf8_lossy(&outcome.body)
            .chars()
            .take(200)
            .collect::<String>();
        return Err(format!(
            "safe_fetch_json: upstream returned {} for {}: {}",
            outcome.status_code, outcome.final_url, preview
        ));
    }
    serde_json::from_slice(&outcome.body).map_err(|e| {
        format!(
            "safe_fetch_json: invalid JSON from {}: {e}",
            outcome.final_url
        )
    })
}

async fn check_host_resolution(
    host: &str,
    port: u16,
    allow_private_networks: bool,
) -> Result<Vec<IpAddr>, String> {
    // If host is already an IP literal, validate_fetch_url already rejected
    // private cases (unless allow_private_networks=true) and we'd never get
    // here. For an IP literal, return the single parsed IP so the caller
    // can pin it into the reqwest client.
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    let target = format!("{host}:{}", port.max(1));
    let resolved = tokio::net::lookup_host(target)
        .await
        .map_err(|e| format!("safe_fetch: dns lookup for '{host}': {e}"))?;

    let mut ips = Vec::new();
    for sock in resolved {
        if !allow_private_networks && ip_is_blocked(sock.ip()) {
            return Err(format!(
                "safe_fetch: host '{host}' resolves to blocked address {}",
                sock.ip()
            ));
        }
        ips.push(sock.ip());
    }
    if ips.is_empty() {
        return Err(format!(
            "safe_fetch: dns lookup for '{host}' returned no addresses"
        ));
    }
    Ok(ips)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_scheme() {
        let err = validate_fetch_url("file:///etc/passwd").unwrap_err();
        assert!(err.contains("unsupported url scheme"), "got: {err}");
    }

    #[test]
    fn rejects_javascript_scheme() {
        let err = validate_fetch_url("javascript:alert(1)").unwrap_err();
        assert!(err.contains("unsupported"), "got: {err}");
    }

    #[test]
    fn rejects_ipv4_loopback_literal() {
        let err = validate_fetch_url("http://127.0.0.1/x").unwrap_err();
        assert!(err.contains("private/loopback"), "got: {err}");
    }

    #[test]
    fn rejects_ipv4_rfc1918_literal() {
        for url in &[
            "http://10.0.0.1/x",
            "http://192.168.1.1/x",
            "http://172.16.0.1/x",
        ] {
            let err = validate_fetch_url(url).unwrap_err();
            assert!(err.contains("private/loopback"), "{url}: got {err}");
        }
    }

    #[test]
    fn rejects_link_local_metadata() {
        let err = validate_fetch_url("http://169.254.169.254/latest/meta-data/").unwrap_err();
        assert!(err.contains("private/loopback"), "got: {err}");
    }

    #[test]
    fn rejects_ipv6_loopback() {
        let err = validate_fetch_url("http://[::1]/x").unwrap_err();
        assert!(err.contains("private/loopback"), "got: {err}");
    }

    #[test]
    fn rejects_ipv6_unique_local() {
        let err = validate_fetch_url("http://[fc00::1]/x").unwrap_err();
        assert!(err.contains("private/loopback"), "got: {err}");
    }

    #[test]
    fn rejects_localhost_string() {
        let err = validate_fetch_url("http://localhost/x").unwrap_err();
        assert!(err.contains("private/loopback"), "got: {err}");
    }

    #[test]
    fn accepts_public_ipv4_literal() {
        let url = validate_fetch_url("http://8.8.8.8/").expect("public dns ip should pass");
        assert_eq!(url.host_str(), Some("8.8.8.8"));
    }

    #[test]
    fn accepts_public_dns_host() {
        let url = validate_fetch_url("https://example.com/path").expect("public host should pass");
        assert_eq!(url.scheme(), "https");
    }

    #[test]
    fn rejects_unparseable() {
        // Pure scheme-only / malformed URLs hit the parser branch, not the
        // host branch — `http:///path` is parsed leniently with host="path",
        // so we exercise the truly malformed case instead.
        let err = validate_fetch_url("not a url").unwrap_err();
        assert!(err.contains("invalid url"), "got: {err}");
    }

    #[test]
    fn policy_clamps_oversized_bytes() {
        let p = FetchPolicy {
            max_bytes: u64::MAX,
            timeout_ms: 1_000,
            ..FetchPolicy::default()
        }
        .clamped();
        assert_eq!(p.max_bytes, MAX_ALLOWED_BYTES);
    }

    #[test]
    fn policy_clamps_oversized_timeout() {
        let p = FetchPolicy {
            max_bytes: 100,
            timeout_ms: u64::MAX,
            ..FetchPolicy::default()
        }
        .clamped();
        assert_eq!(p.timeout_ms, MAX_ALLOWED_TIMEOUT_MS);
    }

    #[test]
    fn policy_floor_on_tiny_inputs() {
        let p = FetchPolicy {
            max_bytes: 0,
            timeout_ms: 0,
            ..FetchPolicy::default()
        }
        .clamped();
        assert!(p.max_bytes >= 1);
        assert!(p.timeout_ms >= 100);
    }

    #[test]
    fn policy_preserves_tor_proxy_through_clamping() {
        let p = FetchPolicy {
            max_bytes: 100,
            timeout_ms: 1_000,
            tor_socks_proxy: Some("socks5h://127.0.0.1:9050".to_string()),
            ..FetchPolicy::default()
        }
        .clamped();
        assert_eq!(
            p.tor_socks_proxy.as_deref(),
            Some("socks5h://127.0.0.1:9050")
        );
    }
}
