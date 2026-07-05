//! Minimal OCI / Docker Registry HTTP V2 pull client.
//!
//! Replaces the `oci-registry-client` crate, whose reqwest dependency
//! hard-wired the native-tls backend (and therefore libssl) into the
//! binary. Froglet only pulls: anonymous token auth, manifest fetch, and
//! blob download. Runs on the workspace reqwest (rustls-only), so removing
//! the crate also removes the binary's only OpenSSL linkage.
//!
//! Registry hosts are allowlisted by the caller (`src/api/mod.rs`); this
//! module still validates image names, references, and digests before they
//! are interpolated into request paths so a hostile manifest or submission
//! cannot redirect requests to other endpoints on the allowlisted host.

use std::time::Duration;

use serde::Deserialize;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Socket-level read stall bound; applies to every read, including the
/// blob body stream (whose total size is capped by the caller).
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Total time bound for the small token/manifest requests.
const SMALL_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The OCI distribution spec recommends manifests stay well under 4 MiB.
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

const ACCEPT_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json";

/// Layer descriptor from an OCI image manifest / Docker manifest V2 schema 2.
/// The declared `size` is intentionally not modeled: it is attacker-supplied,
/// so the download path enforces its own streaming byte cap instead.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestLayer {
    #[serde(default)]
    pub media_type: String,
    pub digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageManifest {
    pub layers: Vec<ManifestLayer>,
}

/// Token endpoint response. Registries disagree on the field name:
/// Docker Hub returns both `token` and `access_token`, ghcr.io returns
/// only `token`.
#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

pub struct RegistryClient {
    service: String,
    api_url: String,
    auth_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl RegistryClient {
    /// * `service` - token-scope service name (example: `registry.docker.io`)
    /// * `api_url` - registry base URL (example: `https://registry-1.docker.io`)
    /// * `auth_url` - token endpoint URL (example: `https://auth.docker.io/token`)
    pub fn new(service: &str, api_url: &str, auth_url: &str) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("froglet/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|error| format!("failed to build OCI registry client: {error}"))?;
        Ok(Self {
            service: service.to_string(),
            api_url: api_url.trim_end_matches('/').to_string(),
            auth_url: auth_url.to_string(),
            token: None,
            http,
        })
    }

    /// Fetch an anonymous pull token for `image` from the token endpoint and
    /// keep it for subsequent requests.
    pub async fn auth_pull(&mut self, image: &str) -> Result<(), String> {
        validate_image_name(image)?;
        let response = self
            .http
            .get(&self.auth_url)
            .query(&[
                ("service", self.service.as_str()),
                ("scope", &format!("repository:{image}:pull")),
            ])
            .timeout(SMALL_REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| format!("token request failed: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("token endpoint returned {status}"));
        }
        let body = read_body_capped(response, MAX_TOKEN_RESPONSE_BYTES).await?;
        let parsed: TokenResponse = serde_json::from_slice(&body)
            .map_err(|error| format!("invalid token response: {error}"))?;
        self.token = parsed
            .token
            .or(parsed.access_token)
            .filter(|token| !token.is_empty());
        if self.token.is_none() {
            return Err("token endpoint returned no token".to_string());
        }
        Ok(())
    }

    /// Fetch the image manifest for `image` at `reference` (tag or digest).
    pub async fn manifest(&self, image: &str, reference: &str) -> Result<ImageManifest, String> {
        validate_image_name(image)?;
        validate_reference(reference)?;
        let url = format!("{}/v2/{}/manifests/{}", self.api_url, image, reference);
        let mut request = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, ACCEPT_MANIFEST)
            .timeout(SMALL_REQUEST_TIMEOUT);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("manifest request failed: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("registry returned {status} for manifest"));
        }
        let body = read_body_capped(response, MAX_MANIFEST_BYTES).await?;
        serde_json::from_slice(&body).map_err(|error| format!("invalid manifest: {error}"))
    }

    /// Start a blob download. Returns the streaming response; callers pull
    /// chunks via [`reqwest::Response::chunk`] and enforce their own size cap.
    pub async fn blob(&self, image: &str, digest: &str) -> Result<reqwest::Response, String> {
        validate_image_name(image)?;
        if !is_valid_digest(digest) {
            return Err(format!("invalid OCI blob digest {digest:?}"));
        }
        let url = format!("{}/v2/{}/blobs/{}", self.api_url, image, digest);
        let mut request = self.http.get(&url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("blob request failed: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("registry returned {status} for blob"));
        }
        Ok(response)
    }
}

async fn read_body_capped(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > cap {
                    return Err(format!("registry response exceeds {cap} bytes"));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(body),
            Err(error) => return Err(format!("failed reading registry response: {error}")),
        }
    }
}

/// Accept repository names matching the OCI distribution grammar closely
/// enough to block path traversal: lowercase alphanumerics with `._-`
/// separators, slash-separated, no empty/dot segments.
fn validate_image_name(image: &str) -> Result<(), String> {
    let valid = !image.is_empty()
        && image.len() <= 255
        && image.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
        });
    if valid {
        Ok(())
    } else {
        Err(format!("invalid OCI image name {image:?}"))
    }
}

/// A reference is either a tag (`[A-Za-z0-9_][A-Za-z0-9._-]{0,127}`) or a
/// digest (`algorithm:encoded`).
fn validate_reference(reference: &str) -> Result<(), String> {
    if is_valid_digest(reference) {
        return Ok(());
    }
    let mut chars = reference.chars();
    let first_valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let valid = first_valid
        && reference.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if valid {
        Ok(())
    } else {
        Err(format!("invalid OCI reference {reference:?}"))
    }
}

fn is_valid_digest(digest: &str) -> bool {
    let Some((algorithm, encoded)) = digest.split_once(':') else {
        return false;
    };
    let algorithm_valid = !algorithm.is_empty()
        && algorithm
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+._-".contains(c));
    let encoded_valid = encoded.len() >= 32
        && encoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "=_-".contains(c));
    algorithm_valid && encoded_valid
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, Query, State};
    use axum::routing::get;
    use axum::{Json, Router};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::TcpListener;

    const SHA256_A: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[derive(Clone)]
    struct StubState {
        /// Token JSON body served by /token.
        token_body: serde_json::Value,
        /// Bearer token /v2/ endpoints require; requests without it get 401.
        expected_token: &'static str,
        saw_expected_scope: Arc<AtomicBool>,
    }

    async fn stub_token(
        State(state): State<StubState>,
        Query(params): Query<HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        if params.get("service").map(String::as_str) == Some("stub.example")
            && params.get("scope").map(String::as_str) == Some("repository:org/module:pull")
        {
            state.saw_expected_scope.store(true, Ordering::SeqCst);
        }
        Json(state.token_body.clone())
    }

    fn authorized(state: &StubState, headers: &axum::http::HeaderMap) -> bool {
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some(&format!("Bearer {}", state.expected_token))
    }

    // Axum path params split on '/', so the two-segment image name arrives
    // as separate "org" and "module" params; rejoin before comparing.
    async fn stub_manifest(
        State(state): State<StubState>,
        Path((org, image, reference)): Path<(String, String, String)>,
        headers: axum::http::HeaderMap,
    ) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
        if !authorized(&state, &headers) {
            return Err(axum::http::StatusCode::UNAUTHORIZED);
        }
        if format!("{org}/{image}") != "org/module" || reference != "v1" {
            return Err(axum::http::StatusCode::NOT_FOUND);
        }
        Ok(Json(serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": { "mediaType": "application/vnd.oci.image.config.v1+json",
                        "digest": SHA256_A, "size": 2 },
            "layers": [
                { "mediaType": "application/wasm", "digest": SHA256_A, "size": 4 },
            ],
        })))
    }

    async fn stub_blob(
        State(state): State<StubState>,
        Path((org, image, digest)): Path<(String, String, String)>,
        headers: axum::http::HeaderMap,
    ) -> Result<Vec<u8>, axum::http::StatusCode> {
        if !authorized(&state, &headers) {
            return Err(axum::http::StatusCode::UNAUTHORIZED);
        }
        if format!("{org}/{image}") != "org/module" || digest != SHA256_A {
            return Err(axum::http::StatusCode::NOT_FOUND);
        }
        Ok(b"wasm".to_vec())
    }

    async fn spawn_stub_registry(state: StubState) -> String {
        let app = Router::new()
            .route("/token", get(stub_token))
            .route("/v2/:org/:image/manifests/:reference", get(stub_manifest))
            .route("/v2/:org/:image/blobs/:digest", get(stub_blob))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub registry");
        let addr = listener.local_addr().expect("stub registry addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{}", addr)
    }

    async fn pull_via_client(base: &str) -> Result<Vec<u8>, String> {
        let mut client =
            RegistryClient::new("stub.example", base, &format!("{base}/token")).unwrap();
        client.auth_pull("org/module").await?;
        let manifest = client.manifest("org/module", "v1").await?;
        let layer = manifest
            .layers
            .iter()
            .find(|layer| layer.media_type.contains("wasm"))
            .ok_or_else(|| "no wasm layer".to_string())?;
        let mut response = client.blob("org/module", &layer.digest).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("chunk failed: {error}"))?
        {
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    #[tokio::test]
    async fn pulls_wasm_layer_with_ghcr_style_token() {
        let saw_expected_scope = Arc::new(AtomicBool::new(false));
        let base = spawn_stub_registry(StubState {
            token_body: serde_json::json!({ "token": "tok-ghcr" }),
            expected_token: "tok-ghcr",
            saw_expected_scope: saw_expected_scope.clone(),
        })
        .await;
        let bytes = pull_via_client(&base).await.expect("pull succeeds");
        assert_eq!(bytes, b"wasm");
        assert!(saw_expected_scope.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn pulls_wasm_layer_with_docker_hub_style_token() {
        let base = spawn_stub_registry(StubState {
            token_body: serde_json::json!({
                "access_token": "tok-hub",
                "expires_in": 300,
                "issued_at": "2026-01-01T00:00:00Z",
            }),
            expected_token: "tok-hub",
            saw_expected_scope: Arc::new(AtomicBool::new(false)),
        })
        .await;
        let bytes = pull_via_client(&base).await.expect("pull succeeds");
        assert_eq!(bytes, b"wasm");
    }

    #[tokio::test]
    async fn empty_token_response_is_an_error() {
        let base = spawn_stub_registry(StubState {
            token_body: serde_json::json!({}),
            expected_token: "unused",
            saw_expected_scope: Arc::new(AtomicBool::new(false)),
        })
        .await;
        let mut client =
            RegistryClient::new("stub.example", &base, &format!("{base}/token")).unwrap();
        let error = client.auth_pull("org/module").await.unwrap_err();
        assert!(error.contains("no token"), "unexpected error: {error}");
    }

    #[tokio::test]
    async fn unauthorized_manifest_reports_status() {
        let base = spawn_stub_registry(StubState {
            token_body: serde_json::json!({ "token": "tok" }),
            expected_token: "tok",
            saw_expected_scope: Arc::new(AtomicBool::new(false)),
        })
        .await;
        // Skip auth_pull: manifest request goes out without a bearer token.
        let client = RegistryClient::new("stub.example", &base, &format!("{base}/token")).unwrap();
        let error = client.manifest("org/module", "v1").await.unwrap_err();
        assert!(error.contains("401"), "unexpected error: {error}");
    }

    #[test]
    fn manifest_json_parses_layer_fields() {
        let manifest: ImageManifest = serde_json::from_value(serde_json::json!({
            "schemaVersion": 2,
            "layers": [ { "digest": SHA256_A } ],
        }))
        .expect("manifest parses");
        assert_eq!(manifest.layers.len(), 1);
        assert_eq!(manifest.layers[0].digest, SHA256_A);
        assert_eq!(manifest.layers[0].media_type, "");
    }

    #[test]
    fn image_name_validation_blocks_traversal() {
        assert!(validate_image_name("org/module").is_ok());
        assert!(validate_image_name("library/ubuntu").is_ok());
        assert!(validate_image_name("a/b/c-d_e.f").is_ok());
        assert!(validate_image_name("").is_err());
        assert!(validate_image_name("org//module").is_err());
        assert!(validate_image_name("org/../module").is_err());
        assert!(validate_image_name("Org/Module").is_err());
        assert!(validate_image_name("org/mod?ule").is_err());
    }

    #[test]
    fn reference_validation_accepts_tags_and_digests() {
        assert!(validate_reference("latest").is_ok());
        assert!(validate_reference("v1.2.3").is_ok());
        assert!(validate_reference(SHA256_A).is_ok());
        assert!(validate_reference("").is_err());
        assert!(validate_reference("-leading-dash").is_err());
        assert!(validate_reference("has/slash").is_err());
        assert!(validate_reference("sha256:short").is_err());
    }

    #[test]
    fn digest_validation_blocks_url_metacharacters() {
        assert!(is_valid_digest(SHA256_A));
        assert!(!is_valid_digest("sha256"));
        assert!(!is_valid_digest("sha256:../../../v2/other/blobs/x"));
        assert!(!is_valid_digest(
            "sha256:aaaa?aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
    }
}
