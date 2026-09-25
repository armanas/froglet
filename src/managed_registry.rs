//! Minimal OCI Distribution activation adapter for Managed Publications.
//!
//! This speaks the provider-neutral registry HTTP API directly. It does not
//! invoke Docker, a cloud CLI, or a provider-specific SDK, and it never places
//! registry credentials in a plan, operation record, command line, or error.

use crate::managed_publication::{ManagedRegistryAuthV1, ManagedTargetProfileV1};
use froglet_publish_engine::managed_oci::{
    OCI_IMAGE_MANIFEST_MEDIA_TYPE, OciDescriptorV1, PlannedManagedOciLayoutV1,
};
use reqwest::{Method, Response, StatusCode, Url, header};
use serde::Deserialize;
use std::net::IpAddr;

enum RegistryCredentials {
    Anonymous,
    Basic { username: String, password: String },
    Bearer(String),
}

struct RegistryReference {
    registry: String,
    repository: String,
    base_url: Url,
    origin: String,
}

impl RegistryReference {
    fn parse(repository: &str, insecure_http: bool) -> Result<Self, String> {
        let (registry, repository) = repository
            .split_once('/')
            .ok_or_else(|| "OCI repository must include an explicit registry host".to_string())?;
        if registry.is_empty()
            || repository.is_empty()
            || repository.split('/').any(|segment| {
                segment.is_empty()
                    || segment == "."
                    || segment == ".."
                    || !segment.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })
            })
        {
            return Err("OCI repository contains an invalid registry or path segment".to_string());
        }
        let scheme = if insecure_http { "http" } else { "https" };
        let base_url = Url::parse(&format!("{scheme}://{registry}/"))
            .map_err(|error| format!("OCI registry host is invalid: {error}"))?;
        if base_url.scheme() != scheme
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.path() != "/"
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err("OCI registry must be a credential-free host with an optional port".into());
        }
        if insecure_http {
            let host = base_url.host_str().unwrap_or_default();
            let loopback = host == "localhost"
                || host
                    .parse::<IpAddr>()
                    .ok()
                    .is_some_and(|address| address.is_loopback());
            if !loopback {
                return Err(
                    "insecure OCI registry HTTP is allowed only for a loopback registry"
                        .to_string(),
                );
            }
        }
        let origin = base_url.origin().ascii_serialization();
        Ok(Self {
            registry: registry.to_string(),
            repository: repository.to_string(),
            base_url,
            origin,
        })
    }

    fn endpoint(&self, suffix: &str) -> Result<Url, String> {
        self.base_url
            .join(&format!("v2/{}/{suffix}", self.repository))
            .map_err(|error| format!("OCI registry endpoint could not be built: {error}"))
    }
}

struct RegistryClient {
    http: reqwest::Client,
    credentials: RegistryCredentials,
    challenge_token: Option<String>,
    approved_origin: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

impl RegistryClient {
    fn new(profile: &ManagedTargetProfileV1, approved_origin: String) -> Result<Self, String> {
        let credentials = match &profile.registry_auth {
            ManagedRegistryAuthV1::Anonymous => RegistryCredentials::Anonymous,
            ManagedRegistryAuthV1::Basic {
                username_environment,
                password_environment,
            } => RegistryCredentials::Basic {
                username: required_secret_environment(username_environment)?,
                password: required_secret_environment(password_environment)?,
            },
            ManagedRegistryAuthV1::Bearer { token_environment } => {
                RegistryCredentials::Bearer(required_secret_environment(token_environment)?)
            }
        };
        let http = crate::tls::reqwest_client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|error| format!("OCI registry client could not be built: {error}"))?;
        Ok(Self {
            http,
            credentials,
            challenge_token: None,
            approved_origin,
        })
    }

    async fn send(
        &mut self,
        method: Method,
        url: Url,
        content_type: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> Result<Response, String> {
        if url.origin().ascii_serialization() != self.approved_origin {
            return Err(
                "OCI registry request attempted to leave the approved registry host".to_string(),
            );
        }
        let response = self
            .send_once(
                method.clone(),
                url.clone(),
                content_type,
                body.clone(),
                None,
            )
            .await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let challenge = response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                "OCI registry rejected authorization without a Bearer challenge".to_string()
            })?
            .to_string();
        let token = self.exchange_bearer_challenge(&challenge).await?;
        self.challenge_token = Some(token.clone());
        self.send_once(method, url, content_type, body, Some(&token))
            .await
    }

    async fn send_once(
        &self,
        method: Method,
        url: Url,
        content_type: Option<&str>,
        body: Option<Vec<u8>>,
        bearer_override: Option<&str>,
    ) -> Result<Response, String> {
        let mut request = self.http.request(method, url);
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        if let Some(token) = bearer_override.or(self.challenge_token.as_deref()) {
            request = request.bearer_auth(token);
        } else {
            match &self.credentials {
                RegistryCredentials::Anonymous => {}
                RegistryCredentials::Basic { username, password } => {
                    request = request.basic_auth(username, Some(password));
                }
                RegistryCredentials::Bearer(token) => {
                    request = request.bearer_auth(token);
                }
            }
        }
        request
            .send()
            .await
            .map_err(|error| format!("OCI registry request failed: {error}"))
    }

    async fn exchange_bearer_challenge(&self, challenge: &str) -> Result<String, String> {
        let fields = parse_bearer_challenge(challenge)?;
        let realm = fields
            .iter()
            .find_map(|(name, value)| (name == "realm").then_some(value.as_str()))
            .ok_or_else(|| "OCI registry Bearer challenge omitted realm".to_string())?;
        let mut url = Url::parse(realm)
            .map_err(|_| "OCI registry Bearer challenge realm is invalid".to_string())?;
        if url.scheme() != "https"
            && !(url.scheme() == "http"
                && url.origin().ascii_serialization() == self.approved_origin)
        {
            return Err("OCI registry Bearer challenge realm is not trusted".to_string());
        }
        {
            let mut query = url.query_pairs_mut();
            for (name, value) in &fields {
                if matches!(name.as_str(), "service" | "scope") {
                    query.append_pair(name, value);
                }
            }
        }
        let mut request = self.http.get(url);
        if let RegistryCredentials::Basic { username, password } = &self.credentials {
            request = request.basic_auth(username, Some(password));
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("OCI registry token exchange failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "OCI registry token exchange returned HTTP {}",
                response.status()
            ));
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|_| "OCI registry token response is invalid JSON".to_string())?;
        token
            .token
            .or(token.access_token)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| "OCI registry token response omitted a token".to_string())
    }
}

fn required_secret_environment(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("required OCI registry credential environment {name} is missing"))
}

fn parse_bearer_challenge(challenge: &str) -> Result<Vec<(String, String)>, String> {
    let value = challenge
        .strip_prefix("Bearer ")
        .or_else(|| challenge.strip_prefix("bearer "))
        .ok_or_else(|| "OCI registry authorization challenge is not Bearer".to_string())?;
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (index, character) in value.char_indices() {
        match character {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return Err("OCI registry Bearer challenge has an unterminated quote".to_string());
    }
    parts.push(&value[start..]);
    let mut fields = Vec::new();
    for part in parts {
        let (name, value) = part
            .trim()
            .split_once('=')
            .ok_or_else(|| "OCI registry Bearer challenge is malformed".to_string())?;
        let value = value
            .trim()
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .ok_or_else(|| "OCI registry Bearer challenge values must be quoted".to_string())?;
        fields.push((name.trim().to_ascii_lowercase(), value.to_string()));
    }
    Ok(fields)
}

fn verify_blob(descriptor: &OciDescriptorV1, bytes: &[u8], label: &str) -> Result<(), String> {
    let expected_digest = format!("sha256:{}", crate::crypto::sha256_hex(bytes));
    if descriptor.digest != expected_digest || descriptor.size != bytes.len() as u64 {
        return Err(format!(
            "{label} bytes do not match their approved OCI descriptor"
        ));
    }
    Ok(())
}

async fn ensure_blob(
    client: &mut RegistryClient,
    output: &RegistryReference,
    descriptor: &OciDescriptorV1,
    bytes: &[u8],
) -> Result<(), String> {
    verify_blob(descriptor, bytes, "managed OCI blob")?;
    let head = client
        .send(
            Method::HEAD,
            output.endpoint(&format!("blobs/{}", descriptor.digest))?,
            None,
            None,
        )
        .await?;
    if head.status().is_success() {
        return Ok(());
    }
    if head.status() != StatusCode::NOT_FOUND {
        return Err(format!(
            "OCI registry blob probe returned HTTP {}",
            head.status()
        ));
    }
    let started = client
        .send(Method::POST, output.endpoint("blobs/uploads/")?, None, None)
        .await?;
    if started.status() != StatusCode::ACCEPTED {
        return Err(format!(
            "OCI registry blob upload start returned HTTP {}",
            started.status()
        ));
    }
    let location = started
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "OCI registry upload response omitted Location".to_string())?;
    let mut upload_url = output
        .base_url
        .join(location)
        .map_err(|_| "OCI registry upload Location is invalid".to_string())?;
    if upload_url.origin().ascii_serialization() != output.origin {
        return Err("OCI registry upload Location changed host".to_string());
    }
    upload_url
        .query_pairs_mut()
        .append_pair("digest", &descriptor.digest);
    let completed = client
        .send(
            Method::PUT,
            upload_url,
            Some("application/octet-stream"),
            Some(bytes.to_vec()),
        )
        .await?;
    if completed.status() != StatusCode::CREATED {
        return Err(format!(
            "OCI registry blob upload returned HTTP {}",
            completed.status()
        ));
    }
    Ok(())
}

async fn ensure_base_layer(
    client: &mut RegistryClient,
    output: &RegistryReference,
    base: &RegistryReference,
    descriptor: &OciDescriptorV1,
) -> Result<(), String> {
    let head = client
        .send(
            Method::HEAD,
            output.endpoint(&format!("blobs/{}", descriptor.digest))?,
            None,
            None,
        )
        .await?;
    if head.status().is_success() {
        return Ok(());
    }
    if head.status() != StatusCode::NOT_FOUND {
        return Err(format!(
            "OCI registry base-layer probe returned HTTP {}",
            head.status()
        ));
    }
    let mut mount_url = output.endpoint("blobs/uploads/")?;
    mount_url
        .query_pairs_mut()
        .append_pair("mount", &descriptor.digest)
        .append_pair("from", &base.repository);
    let mounted = client.send(Method::POST, mount_url, None, None).await?;
    if mounted.status() == StatusCode::CREATED {
        return Ok(());
    }
    Err(format!(
        "OCI registry could not mount base layer {}; pre-seed the approved base runner in the output repository",
        descriptor.digest
    ))
}

pub async fn push_managed_layout(
    profile: &ManagedTargetProfileV1,
    layout: &PlannedManagedOciLayoutV1,
) -> Result<(), String> {
    profile.validate_package(&layout.package)?;
    verify_blob(
        &layout.manifest_descriptor,
        &layout.manifest_bytes,
        "managed OCI manifest",
    )?;
    verify_blob(
        &layout.config_descriptor,
        &layout.config_bytes,
        "managed OCI config",
    )?;
    verify_blob(
        &layout.package_layer_descriptor,
        &layout.package_layer_bytes,
        "managed OCI package layer",
    )?;
    if layout.package.image.digest != layout.manifest_descriptor.digest {
        return Err("managed package image digest does not match OCI manifest".to_string());
    }
    let output = RegistryReference::parse(
        &layout.package.image.repository,
        profile.registry_insecure_http,
    )?;
    let base = RegistryReference::parse(
        &layout.package.base_runner_image.repository,
        profile.registry_insecure_http,
    )?;
    if output.registry != base.registry {
        return Err(
            "managed target schema v1 requires base and output repositories on one registry for blob mounting"
                .to_string(),
        );
    }
    let mut client = RegistryClient::new(profile, output.origin.clone())?;
    for layer in &layout.base_layers {
        ensure_base_layer(&mut client, &output, &base, layer).await?;
    }
    ensure_blob(
        &mut client,
        &output,
        &layout.config_descriptor,
        &layout.config_bytes,
    )
    .await?;
    ensure_blob(
        &mut client,
        &output,
        &layout.package_layer_descriptor,
        &layout.package_layer_bytes,
    )
    .await?;
    let manifest_url =
        output.endpoint(&format!("manifests/{}", layout.manifest_descriptor.digest))?;
    let published = client
        .send(
            Method::PUT,
            manifest_url.clone(),
            Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE),
            Some(layout.manifest_bytes.clone()),
        )
        .await?;
    if published.status() != StatusCode::CREATED {
        return Err(format!(
            "OCI registry manifest upload returned HTTP {}",
            published.status()
        ));
    }
    let manifest_digest_header = header::HeaderName::from_static("docker-content-digest");
    if published
        .headers()
        .get(&manifest_digest_header)
        .and_then(|value| value.to_str().ok())
        != Some(layout.manifest_descriptor.digest.as_str())
    {
        return Err("OCI registry manifest upload did not confirm the exact digest".to_string());
    }
    let verified = client.send(Method::HEAD, manifest_url, None, None).await?;
    if !verified.status().is_success() {
        return Err(format!(
            "OCI registry did not expose the exact uploaded manifest: HTTP {}",
            verified.status()
        ));
    }
    if verified
        .headers()
        .get(manifest_digest_header)
        .and_then(|value| value.to_str().ok())
        != Some(layout.manifest_descriptor.digest.as_str())
    {
        return Err("OCI registry manifest probe did not confirm the exact digest".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_publication::{
        MANAGED_BUNDLE_CONTAINER_PATH, MANAGED_BUNDLE_PATH_ENV, MANAGED_CAPSULE_ENV,
        MANAGED_IDENTITY_SEED_ENV, MANAGED_PUBLIC_BASE_URL_ENV, ManagedDeploymentTemplateV1,
    };
    use froglet_protocol::managed_deployment::{
        HttpHealthCheckV1, LifecycleIntentV1, LogicalSecretReferenceV1, ObservabilityIntentV1,
        PortIntentV1, PortProtocolV1, ResourceIntentV1, WorkloadArchitectureV1,
    };
    use froglet_protocol::managed_publication::{
        MANAGED_PUBLICATION_PACKAGE_SCHEMA_V1, MANAGED_PUBLICATION_RUNNER_CONTRACT_V1,
        ManagedPublicationContentVisibilityV1, ManagedPublicationPackageV1,
    };
    use std::collections::BTreeMap;

    #[test]
    fn bearer_challenge_parser_is_strict_and_preserves_scope() {
        let parsed = parse_bearer_challenge(
            r#"Bearer realm="https://auth.example/token",service="registry.example",scope="repository:a/b:pull,push""#,
        )
        .unwrap();
        assert_eq!(
            parsed[0],
            (
                "realm".to_string(),
                "https://auth.example/token".to_string()
            )
        );
        assert_eq!(parsed[2].0, "scope");
        assert!(parse_bearer_challenge("Basic realm=x").is_err());
        assert!(parse_bearer_challenge("Bearer realm=https://bad").is_err());
    }

    #[test]
    fn insecure_registry_is_loopback_only() {
        assert!(RegistryReference::parse("127.0.0.1:5000/team/app", true).is_ok());
        assert!(RegistryReference::parse("localhost:5000/team/app", true).is_ok());
        assert!(RegistryReference::parse("registry.example/team/app", true).is_err());
        assert!(RegistryReference::parse("registry.example/team/app", false).is_ok());
    }

    #[tokio::test]
    async fn pushes_exact_blobs_and_digest_manifest_through_distribution_api() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut observed = Vec::new();
            for _ in 0..10 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut scratch = [0_u8; 4096];
                let header_end = loop {
                    let read = stream.read(&mut scratch).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&scratch[..read]);
                    if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        break position + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let request_line = headers.lines().next().unwrap().to_string();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while bytes.len() - header_end < content_length {
                    let read = stream.read(&mut scratch).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&scratch[..read]);
                }
                observed.push(request_line.clone());
                let response = if request_line.starts_with("HEAD ") {
                    if request_line.contains("manifests/") {
                        let digest = request_line
                            .split("manifests/")
                            .nth(1)
                            .and_then(|tail| tail.split_whitespace().next())
                            .unwrap();
                        format!(
                            "HTTP/1.1 200 OK\r\ndocker-content-digest: {digest}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        )
                    } else {
                        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                            .to_string()
                    }
                } else if request_line.starts_with("POST ") && request_line.contains("mount=") {
                    "HTTP/1.1 201 Created\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        .to_string()
                } else if request_line.starts_with("POST ") {
                    format!(
                        "HTTP/1.1 202 Accepted\r\nlocation: http://{address}/upload/session\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                } else if request_line.starts_with("PUT ") {
                    if request_line.contains("manifests/") {
                        let digest = request_line
                            .split("manifests/")
                            .nth(1)
                            .and_then(|tail| tail.split_whitespace().next())
                            .unwrap();
                        format!(
                            "HTTP/1.1 201 Created\r\ndocker-content-digest: {digest}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        )
                    } else {
                        "HTTP/1.1 201 Created\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                            .to_string()
                    }
                } else {
                    panic!("unexpected registry request {request_line}");
                };
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            observed
        });

        let directory = tempfile::tempdir().unwrap();
        let operator = directory.path().join("operator");
        let adapter_config = directory.path().join("adapter.json");
        let base_manifest = directory.path().join("manifest.json");
        let base_config = directory.path().join("config.json");
        for path in [&operator, &adapter_config, &base_manifest, &base_config] {
            std::fs::write(path, b"{}").unwrap();
        }
        let manifest_bytes = br#"{"schemaVersion":2}"#.to_vec();
        let config_bytes = br#"{"architecture":"amd64"}"#.to_vec();
        let layer_bytes = b"managed-package-layer".to_vec();
        let descriptor = |media_type: &str, bytes: &[u8]| OciDescriptorV1 {
            media_type: media_type.to_string(),
            digest: format!("sha256:{}", crate::crypto::sha256_hex(bytes)),
            size: bytes.len() as u64,
        };
        let manifest_descriptor = descriptor(OCI_IMAGE_MANIFEST_MEDIA_TYPE, &manifest_bytes);
        let config_descriptor = descriptor(
            froglet_publish_engine::managed_oci::OCI_IMAGE_CONFIG_MEDIA_TYPE,
            &config_bytes,
        );
        let layer_descriptor = descriptor(
            froglet_publish_engine::managed_oci::OCI_LAYER_MEDIA_TYPE,
            &layer_bytes,
        );
        let registry = address.to_string();
        let package = ManagedPublicationPackageV1 {
            schema_version: MANAGED_PUBLICATION_PACKAGE_SCHEMA_V1.to_string(),
            source_package_digest: "1".repeat(64),
            build_evidence_digest: "2".repeat(64),
            bundle_manifest_digest: format!("sha256:{}", "3".repeat(64)),
            release_bundle_digest: format!("sha256:{}", "4".repeat(64)),
            base_runner_image: froglet_protocol::managed_deployment::OciImageV1 {
                repository: format!("{registry}/base/runner"),
                digest: format!("sha256:{}", "5".repeat(64)),
            },
            image: froglet_protocol::managed_deployment::OciImageV1 {
                repository: format!("{registry}/private/service"),
                digest: manifest_descriptor.digest.clone(),
            },
            content_visibility: ManagedPublicationContentVisibilityV1::PrivateRegistryRequired,
            runtime: "wasm".to_string(),
            package_kind: "inline_module".to_string(),
            builder_id: "test-builder".to_string(),
            builder_fingerprint: "6".repeat(64),
            runner_contract: MANAGED_PUBLICATION_RUNNER_CONTRACT_V1.to_string(),
        };
        let profile = ManagedTargetProfileV1 {
            operator_binary: operator,
            adapter: "ssh-oci".to_string(),
            adapter_config_path: adapter_config,
            output_repository: package.image.repository.clone(),
            base_runner_image: package.base_runner_image.clone(),
            release_bundle_digest: package.release_bundle_digest.clone(),
            base_manifest_path: base_manifest,
            base_config_path: base_config,
            public_url: "https://managed.example".to_string(),
            provision: false,
            deployment: ManagedDeploymentTemplateV1 {
                deployment_id: "managed-registry-test".to_string(),
                environment: BTreeMap::from([
                    (
                        MANAGED_BUNDLE_PATH_ENV.to_string(),
                        MANAGED_BUNDLE_CONTAINER_PATH.to_string(),
                    ),
                    (
                        MANAGED_PUBLIC_BASE_URL_ENV.to_string(),
                        "https://managed.example".to_string(),
                    ),
                ]),
                secrets: vec![
                    LogicalSecretReferenceV1 {
                        environment_name: MANAGED_CAPSULE_ENV.to_string(),
                        reference: "secret://test/capsule".to_string(),
                    },
                    LogicalSecretReferenceV1 {
                        environment_name: MANAGED_IDENTITY_SEED_ENV.to_string(),
                        reference: "secret://test/identity".to_string(),
                    },
                    LogicalSecretReferenceV1 {
                        environment_name: crate::identity::NOSTR_PUBLICATION_IDENTITY_SEED_ENV
                            .to_string(),
                        reference: "secret://test/nostr-publication-identity".to_string(),
                    },
                ],
                resources: ResourceIntentV1 {
                    cpu_millis: 100,
                    memory_bytes: 128 * 1024 * 1024,
                    architecture: WorkloadArchitectureV1::Amd64,
                },
                ports: vec![PortIntentV1 {
                    name: "froglet".to_string(),
                    container_port: 8080,
                    protocol: PortProtocolV1::Tcp,
                }],
                persistent_volumes: Vec::new(),
                health_check: HttpHealthCheckV1 {
                    port_name: "froglet".to_string(),
                    path: "/healthz".to_string(),
                    interval_seconds: 10,
                    timeout_seconds: 2,
                },
                ingress: None,
                observability: ObservabilityIntentV1 {
                    structured_logs: false,
                },
                lifecycle: LifecycleIntentV1 {
                    rollback_required: false,
                },
            },
            capsule_source_environment: "FROGLET_TEST_CAPSULE".to_string(),
            registry_auth: ManagedRegistryAuthV1::Anonymous,
            registry_insecure_http: true,
        };
        let layout = PlannedManagedOciLayoutV1 {
            package,
            manifest_bytes,
            manifest_descriptor,
            config_bytes,
            config_descriptor,
            package_layer_bytes: layer_bytes,
            package_layer_descriptor: layer_descriptor,
            base_layers: vec![OciDescriptorV1 {
                media_type: froglet_publish_engine::managed_oci::OCI_LAYER_MEDIA_TYPE.to_string(),
                digest: format!("sha256:{}", "a".repeat(64)),
                size: 123,
            }],
        };
        push_managed_layout(&profile, &layout).await.unwrap();
        let observed = server.await.unwrap();
        assert_eq!(observed.len(), 10);
        assert!(observed.iter().any(|line| line.contains("mount=")));
        assert!(
            observed
                .iter()
                .any(|line| line.contains("manifests/sha256:"))
        );
    }
}
