//! Marketplace registration + exact-offer projection polling.
//!
//! After the local daemon has signed + persisted the offer in its
//! `/v1/feed`, the engine POSTs `/v1/registrations` on the marketplace
//! with the provider's public URL. The marketplace fetches the feed,
//! verifies signatures, and writes a `feed_sources` row.
//!
//! Indexer projection (`/v1/offers/<artifact_hash>` returning the exact signed
//! offer) is eventually-consistent; the engine polls for up to 90 seconds.

use crate::error::{MarketplaceStateUnknownContext, PublishError};
use froglet_protocol::{
    canonical_json, crypto,
    publication::{
        PUBLICATION_CANARY_REQUEST_SCHEMA_V1, PublicationCanaryRequest,
        SignedPublicationCanaryResult, SignedPublicationRevision,
    },
};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use url::Url;

const POLL_TIMEOUT: Duration = Duration::from_secs(90);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const POLL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
// Candidate registration may include one exact external canary. The provider
// revision can permit up to five minutes of local execution, so the client
// must not time out before the marketplace's bounded validation path.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(315);
const REQUESTER_CANARY_MAX_RESPONSE_BYTES: usize = 128 * 1024;
const REQUESTER_CANARY_MAX_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequesterCanaryEvidence {
    pub challenge: String,
    pub input_hash: String,
    pub result_hash: String,
    pub signed_payload_hash: String,
}

fn publicly_routable_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ip.octets()[0] == 0
                || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
                || (ip.octets()[0] == 192 && ip.octets()[1] == 0 && ip.octets()[2] == 0)
                || (ip.octets()[0] == 198 && (18..=19).contains(&ip.octets()[1])))
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
                || (octets[0..4] == [0x20, 0x01, 0x0d, 0xb8]))
        }
    }
}

async fn requester_canary_client(
    endpoint: &Url,
    timeout: Duration,
) -> Result<reqwest::Client, PublishError> {
    let host = endpoint
        .host_str()
        .ok_or_else(|| PublishError::Verification {
            tries: 1,
            url: endpoint.to_string(),
            reason: "requester canary URL has no host".to_string(),
        })?;
    let port = endpoint
        .port_or_known_default()
        .ok_or_else(|| PublishError::Verification {
            tries: 1,
            url: endpoint.to_string(),
            reason: "requester canary URL has no effective port".to_string(),
        })?;
    let mut addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| PublishError::Verification {
            tries: 1,
            url: endpoint.to_string(),
            reason: format!("requester canary DNS resolution failed: {error}"),
        })?
        .collect::<Vec<SocketAddr>>();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|addr| !publicly_routable_ip(addr.ip()))
    {
        return Err(PublishError::Verification {
            tries: 1,
            url: endpoint.to_string(),
            reason: "requester canary DNS must resolve only to public addresses".to_string(),
        });
    }
    crate::http_client_builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|error| PublishError::Http(error.to_string()))
}

#[derive(Debug, Clone, Copy)]
struct PollPolicy {
    timeout: Duration,
    interval: Duration,
    request_timeout: Duration,
}

impl Default for PollPolicy {
    fn default() -> Self {
        Self {
            timeout: POLL_TIMEOUT,
            interval: POLL_INTERVAL,
            request_timeout: POLL_REQUEST_TIMEOUT,
        }
    }
}

#[derive(Debug, Serialize)]
struct RegistrationRequest<'a> {
    provider_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<&'a str>,
    /// Exact signed offer this registration candidate must expose. Legacy
    /// clients may omit it and remain on the marketplace's manual-review path.
    #[serde(skip_serializing_if = "Option::is_none")]
    offer_hash: Option<&'a str>,
    /// Signed higher-layer binding for the exact executable/currency-aware
    /// publication revision. Legacy registrations omit both exact fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    publication_revision: Option<&'a SignedPublicationRevision>,
    /// Private, one-shot marketplace canary input. The marketplace must use
    /// it only to invoke the exact candidate and must not expose it in public
    /// offer records.
    #[serde(skip_serializing_if = "Option::is_none")]
    canary_input: Option<&'a serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct RegistrationResponse {
    pub status: String,
    pub provider_id: String,
    pub provider_url: String,
    pub transport: String,
    pub descriptor_hash: String,
    #[serde(default)]
    pub offers_seen: u64,
    #[serde(default)]
    pub already_registered: bool,
    #[serde(default)]
    pub validation_mode: Option<String>,
    #[serde(default)]
    pub validated_offer_hash: Option<String>,
    #[serde(default)]
    pub validated_revision_hash: Option<String>,
    #[serde(default)]
    pub validation_candidate_id: Option<i64>,
    #[serde(default)]
    pub deprecation_warning: Option<String>,
}

/// POST `/v1/registrations` and return the marketplace's verification.
pub async fn register_with_marketplace(
    marketplace_url: &Url,
    provider_url: &str,
    transport_hint: Option<&str>,
    offer_hash: Option<&str>,
    publication_revision: Option<&SignedPublicationRevision>,
    canary_input: Option<&serde_json::Value>,
) -> Result<RegistrationResponse, PublishError> {
    let mut endpoint = marketplace_url.clone();
    endpoint.set_path("/v1/registrations");

    let client = crate::http_client_builder()
        .timeout(REGISTRATION_TIMEOUT)
        .build()?;

    let request = RegistrationRequest {
        provider_url,
        transport: transport_hint,
        offer_hash,
        publication_revision,
        canary_input,
    };
    let mut ambiguous_reasons = Vec::new();
    for attempt in 0..2 {
        let response = match client.post(endpoint.clone()).json(&request).send().await {
            Ok(response) => response,
            Err(error) => {
                ambiguous_reasons.push(format!("attempt {} lost response: {error}", attempt + 1));
                continue;
            }
        };
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<empty>".to_string());
            if ambiguous_reasons.is_empty() {
                return Err(PublishError::Registration {
                    url: endpoint.to_string(),
                    status: status.as_u16(),
                    body,
                });
            }
            ambiguous_reasons.push(format!(
                "attempt {} returned HTTP {} after an ambiguous prior attempt: {body}",
                attempt + 1,
                status.as_u16()
            ));
            break;
        }
        match response.json::<RegistrationResponse>().await {
            Ok(parsed) => return Ok(parsed),
            Err(error) => ambiguous_reasons.push(format!(
                "attempt {} returned success with invalid JSON: {error}",
                attempt + 1
            )),
        }
    }

    match (offer_hash, publication_revision) {
        (Some(offer_hash), Some(revision)) => Err(PublishError::MarketplaceStateUnknown(Box::new(
            MarketplaceStateUnknownContext {
                service_id: revision.payload.service_id.clone(),
                offer_hash: offer_hash.to_string(),
                revision_hash: revision.revision_hash.clone(),
                provider_url: provider_url.to_string(),
                visibility_bound_secs: 60,
                reason: ambiguous_reasons.join("; "),
                local_pause_result: "pending_exact_compensation".to_string(),
                relay_withdrawal_result: "pending_exact_compensation".to_string(),
            },
        ))),
        _ => Err(PublishError::Registration {
            url: endpoint.to_string(),
            status: 200,
            body: format!(
                "registration response remained ambiguous after exact replay: {}",
                ambiguous_reasons.join("; ")
            ),
        }),
    }
}

/// Run a requester-owned invocation after exact transport activation and
/// before marketplace submission. This separate network round prevents a
/// requester-side failure from leaving an avoidable active listing lease.
pub async fn run_requester_canary(
    provider_url: &str,
    revision: &SignedPublicationRevision,
    input: &serde_json::Value,
) -> Result<RequesterCanaryEvidence, PublishError> {
    revision
        .verify()
        .map_err(|error| PublishError::Verification {
            tries: 1,
            url: provider_url.to_string(),
            reason: format!("cannot canary an invalid publication revision: {error}"),
        })?;
    let input_hash = crypto::sha256_hex(canonical_json::to_vec(input).map_err(|error| {
        PublishError::Verification {
            tries: 1,
            url: provider_url.to_string(),
            reason: format!("canary input is not canonical JSON: {error}"),
        }
    })?);
    if input_hash != revision.payload.local_verification.input_hash {
        return Err(PublishError::Verification {
            tries: 1,
            url: provider_url.to_string(),
            reason: "requester canary input does not match the signed local fixture hash"
                .to_string(),
        });
    }

    let mut endpoint = Url::parse(provider_url).map_err(|error| PublishError::Verification {
        tries: 1,
        url: provider_url.to_string(),
        reason: format!("provider URL is invalid: {error}"),
    })?;
    if endpoint.scheme() != "https" || endpoint.host_str().is_none() {
        return Err(PublishError::Verification {
            tries: 1,
            url: provider_url.to_string(),
            reason: "requester canary requires an HTTPS public provider URL".to_string(),
        });
    }
    endpoint.set_path(&format!(
        "/v1/publications/{}/canary",
        revision.revision_hash
    ));
    endpoint.set_query(None);
    endpoint.set_fragment(None);

    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    let challenge = hex::encode(nonce);
    let request = PublicationCanaryRequest {
        schema_version: PUBLICATION_CANARY_REQUEST_SCHEMA_V1.to_string(),
        revision_hash: revision.revision_hash.clone(),
        offer_hash: revision.payload.offer_hash.clone(),
        challenge: challenge.clone(),
        input: input.clone(),
    };
    request
        .validate()
        .map_err(|error| PublishError::Verification {
            tries: 1,
            url: endpoint.to_string(),
            reason: format!("requester canary request is invalid: {error}"),
        })?;
    let timeout_ms = revision
        .payload
        .limits
        .max_runtime_ms
        .saturating_add(5_000)
        .clamp(1_000, REQUESTER_CANARY_MAX_TIMEOUT.as_millis() as u64);
    let client = requester_canary_client(&endpoint, Duration::from_millis(timeout_ms)).await?;
    let mut response = client.post(endpoint.clone()).json(&request).send().await?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > REQUESTER_CANARY_MAX_RESPONSE_BYTES {
            return Err(PublishError::Verification {
                tries: 1,
                url: endpoint.to_string(),
                reason: format!(
                    "requester canary response exceeded {REQUESTER_CANARY_MAX_RESPONSE_BYTES} bytes"
                ),
            });
        }
        body.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(PublishError::Registration {
            url: endpoint.to_string(),
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    }
    let result: SignedPublicationCanaryResult =
        serde_json::from_slice(&body).map_err(|error| PublishError::Verification {
            tries: 1,
            url: endpoint.to_string(),
            reason: format!("requester canary response was invalid JSON: {error}"),
        })?;
    verify_requester_canary_result(&request, revision, &result).map_err(|reason| {
        PublishError::Verification {
            tries: 1,
            url: endpoint.to_string(),
            reason,
        }
    })
}

fn verify_requester_canary_result(
    request: &PublicationCanaryRequest,
    revision: &SignedPublicationRevision,
    result: &SignedPublicationCanaryResult,
) -> Result<RequesterCanaryEvidence, String> {
    result
        .verify()
        .map_err(|error| format!("requester canary signature verification failed: {error}"))?;
    let expected = [
        (
            "provider_id",
            result.payload.provider_id.as_str(),
            revision.payload.provider_id.as_str(),
        ),
        (
            "revision_hash",
            result.payload.revision_hash.as_str(),
            request.revision_hash.as_str(),
        ),
        (
            "offer_hash",
            result.payload.offer_hash.as_str(),
            request.offer_hash.as_str(),
        ),
        (
            "challenge",
            result.payload.challenge.as_str(),
            request.challenge.as_str(),
        ),
        (
            "input_hash",
            result.payload.input_hash.as_str(),
            revision.payload.local_verification.input_hash.as_str(),
        ),
        (
            "result_hash",
            result.payload.result_hash.as_str(),
            revision.payload.local_verification.result_hash.as_str(),
        ),
    ];
    for (field, actual, expected) in expected {
        if actual != expected {
            return Err(format!(
                "requester canary {field} {actual:?} does not match {expected:?}"
            ));
        }
    }
    Ok(RequesterCanaryEvidence {
        challenge: result.payload.challenge.clone(),
        input_hash: result.payload.input_hash.clone(),
        result_hash: result.payload.result_hash.clone(),
        signed_payload_hash: result.payload_hash.clone(),
    })
}

#[derive(Debug, Deserialize)]
struct IndexedOffer {
    artifact_hash: String,
}

/// Poll `/v1/offers/<artifact_hash>` until the indexer returns that exact
/// signed offer or the timeout fires. Returns the number of seconds waited.
///
/// Provider-level readiness is deliberately insufficient: a provider may
/// already be indexed while a newly published offer is missing or rejected.
pub async fn wait_for_offer(marketplace_url: &Url, offer_hash: &str) -> Result<u32, PublishError> {
    wait_for_offer_with_policy(marketplace_url, offer_hash, PollPolicy::default()).await
}

async fn wait_for_offer_with_policy(
    marketplace_url: &Url,
    offer_hash: &str,
    policy: PollPolicy,
) -> Result<u32, PublishError> {
    let mut endpoint = marketplace_url.clone();
    endpoint.set_path(&format!("/v1/offers/{offer_hash}"));

    let client = crate::http_client_builder()
        .timeout(policy.request_timeout)
        .build()?;
    let deadline = std::time::Instant::now() + policy.timeout;
    let start = std::time::Instant::now();
    let mut tries = 0u32;
    let mut last_observation = "request not attempted".to_string();

    while std::time::Instant::now() < deadline {
        tries = tries.saturating_add(1);
        match client.get(endpoint.clone()).send().await {
            Ok(resp) if resp.status().is_success() => {
                let indexed = resp.json::<IndexedOffer>().await.map_err(|error| {
                    PublishError::Verification {
                        tries,
                        url: endpoint.to_string(),
                        reason: format!("offer response was not valid marketplace JSON: {error}"),
                    }
                })?;
                if indexed.artifact_hash != offer_hash {
                    return Err(PublishError::Verification {
                        tries,
                        url: endpoint.to_string(),
                        reason: format!(
                            "marketplace returned artifact_hash {:?}, expected {:?}",
                            indexed.artifact_hash, offer_hash
                        ),
                    });
                }
                return Ok(start.elapsed().as_secs() as u32);
            }
            Ok(resp) => last_observation = format!("HTTP {}", resp.status().as_u16()),
            Err(error) => last_observation = format!("request failed: {error}"),
        }
        tokio::time::sleep(policy.interval).await;
    }

    Err(PublishError::Verification {
        tries,
        url: endpoint.to_string(),
        reason: format!(
            "last observation: {last_observation}; indexer did not project offer {offer_hash} within {:?}",
            policy.timeout
        ),
    })
}

/// Build the canonical marketplace URL for an offer.
pub fn marketplace_offer_url(marketplace_url: &Url, offer_hash: &str) -> String {
    let mut url = marketplace_url.clone();
    url.set_path(&format!("/v1/offers/{offer_hash}"));
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_offer_server(
        responses: Vec<(u16, String)>,
    ) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut paths = Vec::with_capacity(responses.len());
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = vec![0u8; 4096];
                let read = stream.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..read]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_string();
                paths.push(path);

                let reason = if status == 200 { "OK" } else { "Not Found" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            paths
        });
        (
            Url::parse(&format!("http://{address}"))
                .expect("loopback listener is a valid base URL"),
            handle,
        )
    }

    fn signed_revision_and_request() -> (
        froglet_protocol::crypto::NodeSigningKey,
        SignedPublicationRevision,
        PublicationCanaryRequest,
    ) {
        use froglet_protocol::publication::{
            LocalVerificationEvidence, PUBLICATION_REVISION_SCHEMA_V1, PublicationCurrency,
            PublicationRevisionPayload, PublicationRevisionPrice, PublicationRevisionService,
            PublicationSettlement, ResolvedPublicationLimits, sign_publication_revision,
        };

        let key = froglet_protocol::crypto::generate_signing_key();
        let provider_id = froglet_protocol::crypto::public_key_hex(&key);
        let input = serde_json::json!({"op": "describe"});
        let input_hash = crypto::sha256_hex(canonical_json::to_vec(&input).unwrap());
        let revision = sign_publication_revision(
            PublicationRevisionPayload {
                schema_version: PUBLICATION_REVISION_SCHEMA_V1.to_string(),
                provider_id,
                service_id: "catalog".to_string(),
                offer_id: "catalog".to_string(),
                offer_hash: "11".repeat(32),
                binding_hash: "22".repeat(32),
                package_digest: "22".repeat(32),
                runtime: "builtin".to_string(),
                package_kind: "builtin".to_string(),
                build_evidence: None,
                service: PublicationRevisionService {
                    project_id: None,
                    summary: Some("Catalog".to_string()),
                    starter: None,
                    source_kind: "data_query.json".to_string(),
                    entrypoint_kind: "builtin".to_string(),
                    entrypoint: "catalog".to_string(),
                    contract_version: "froglet.builtin.data_query.json.v1".to_string(),
                    mode: "sync".to_string(),
                    mounts: Vec::new(),
                    capabilities: Vec::new(),
                    input_schema: None,
                    output_schema: None,
                },
                limits: ResolvedPublicationLimits {
                    max_input_bytes: 4096,
                    max_runtime_ms: 2500,
                    max_memory_bytes: 0,
                    max_output_bytes: 4096,
                    fuel_limit: 0,
                },
                price: PublicationRevisionPrice {
                    settlement_method: PublicationSettlement::None,
                    currency: PublicationCurrency::Sat,
                    base_amount_minor: 0,
                    success_amount_minor: 0,
                    offer_settlement_method: "none".to_string(),
                },
                local_verification: LocalVerificationEvidence {
                    input_hash,
                    result_hash: "33".repeat(32),
                    expected_output_matched: Some(true),
                },
            },
            |message| froglet_protocol::crypto::sign_message_hex(&key, message),
        )
        .unwrap();
        let request = PublicationCanaryRequest {
            schema_version: PUBLICATION_CANARY_REQUEST_SCHEMA_V1.to_string(),
            revision_hash: revision.revision_hash.clone(),
            offer_hash: revision.payload.offer_hash.clone(),
            challenge: "44".repeat(32),
            input,
        };
        (key, revision, request)
    }

    #[test]
    fn offer_url_builds_correctly() {
        let base = Url::parse("https://marketplace.froglet.dev").unwrap();
        let url = marketplace_offer_url(&base, "abc123");
        assert_eq!(url, "https://marketplace.froglet.dev/v1/offers/abc123");
    }

    #[test]
    fn candidate_registration_binds_exact_offer_hash() {
        let value = serde_json::to_value(RegistrationRequest {
            provider_url: "https://provider.example",
            transport: Some("clearnet"),
            offer_hash: Some("offer-abc"),
            publication_revision: None,
            canary_input: Some(&serde_json::json!({"op": "describe"})),
        })
        .unwrap();

        assert_eq!(value["offer_hash"], "offer-abc");
        assert_eq!(value["canary_input"]["op"], "describe");
    }

    #[test]
    fn legacy_registration_omits_offer_hash() {
        let value = serde_json::to_value(RegistrationRequest {
            provider_url: "https://provider.example",
            transport: Some("clearnet"),
            offer_hash: None,
            publication_revision: None,
            canary_input: None,
        })
        .unwrap();

        assert!(value.get("offer_hash").is_none());
    }

    #[test]
    fn registration_response_preserves_validated_revision_hash() {
        let response: RegistrationResponse = serde_json::from_value(serde_json::json!({
            "status": "active",
            "provider_id": "provider",
            "provider_url": "https://provider.example",
            "transport": "relay",
            "descriptor_hash": "descriptor",
            "validated_offer_hash": "offer",
            "validated_revision_hash": "revision"
        }))
        .expect("registration response");

        assert_eq!(
            response.validated_revision_hash.as_deref(),
            Some("revision")
        );
    }

    #[tokio::test]
    async fn exact_registration_replays_invalid_success_once() {
        let (_key, revision, _canary) = signed_revision_and_request();
        let valid = serde_json::json!({
            "status": "active",
            "provider_id": revision.payload.provider_id.clone(),
            "provider_url": "https://provider.example",
            "transport": "relay",
            "descriptor_hash": "descriptor",
            "validated_offer_hash": revision.payload.offer_hash.clone(),
            "validated_revision_hash": revision.revision_hash.clone(),
        })
        .to_string();
        let (base, server) =
            spawn_offer_server(vec![(200, "not-json".to_string()), (200, valid)]).await;

        let response = register_with_marketplace(
            &base,
            "https://provider.example",
            Some("relay"),
            Some(&revision.payload.offer_hash),
            Some(&revision),
            None,
        )
        .await
        .expect("exact replay converges");
        assert_eq!(response.status, "active");
        assert_eq!(
            server.await.unwrap(),
            vec!["/v1/registrations".to_string(); 2]
        );
    }

    #[tokio::test]
    async fn exact_registration_reports_bounded_unknown_state_after_two_ambiguous_successes() {
        let (_key, revision, _canary) = signed_revision_and_request();
        let (base, server) = spawn_offer_server(vec![
            (200, "not-json".to_string()),
            (200, "still-not-json".to_string()),
        ])
        .await;

        let error = register_with_marketplace(
            &base,
            "https://provider.example",
            Some("relay"),
            Some(&revision.payload.offer_hash),
            Some(&revision),
            None,
        )
        .await
        .expect_err("two ambiguous successes cannot claim registration state");
        match error {
            PublishError::MarketplaceStateUnknown(context) => {
                assert_eq!(context.offer_hash, revision.payload.offer_hash);
                assert_eq!(context.revision_hash, revision.revision_hash);
                assert_eq!(context.provider_url, "https://provider.example");
                assert_eq!(context.visibility_bound_secs, 60);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(
            server.await.unwrap(),
            vec!["/v1/registrations".to_string(); 2]
        );
    }

    #[test]
    fn requester_canary_rejects_a_validly_signed_wrong_challenge() {
        use froglet_protocol::publication::{
            PUBLICATION_CANARY_RESULT_SCHEMA_V1, PublicationCanaryResultPayload,
            sign_publication_canary_result,
        };

        let (key, revision, request) = signed_revision_and_request();
        let signed = sign_publication_canary_result(
            PublicationCanaryResultPayload {
                schema_version: PUBLICATION_CANARY_RESULT_SCHEMA_V1.to_string(),
                provider_id: revision.payload.provider_id.clone(),
                revision_hash: revision.revision_hash.clone(),
                offer_hash: revision.payload.offer_hash.clone(),
                challenge: "55".repeat(32),
                input_hash: revision.payload.local_verification.input_hash.clone(),
                result_hash: revision.payload.local_verification.result_hash.clone(),
                status: "succeeded".to_string(),
            },
            |message| froglet_protocol::crypto::sign_message_hex(&key, message),
        )
        .unwrap();

        let error = verify_requester_canary_result(&request, &revision, &signed)
            .expect_err("a fresh requester challenge cannot accept a prior signed response");
        assert!(error.contains("challenge"));
    }

    #[test]
    fn requester_canary_dns_policy_rejects_non_public_ranges() {
        assert!(publicly_routable_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(publicly_routable_ip(IpAddr::V6(
            "2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap()
        )));
        for ip in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6("fc00::1".parse().unwrap()),
            IpAddr::V6("fe80::1".parse().unwrap()),
            IpAddr::V6("2001:db8::1".parse().unwrap()),
            IpAddr::V6("::ffff:127.0.0.1".parse().unwrap()),
        ] {
            assert!(!publicly_routable_ip(ip), "unexpectedly allowed {ip}");
        }
    }

    #[tokio::test]
    async fn exact_offer_poll_ignores_provider_level_readiness() {
        let offer_hash = "offer-abc";
        let (base, server) = spawn_offer_server(vec![
            (404, r#"{"error":"not indexed"}"#.to_string()),
            (200, format!(r#"{{"artifact_hash":"{offer_hash}"}}"#)),
        ])
        .await;

        let waited = wait_for_offer_with_policy(
            &base,
            offer_hash,
            PollPolicy {
                timeout: Duration::from_secs(1),
                interval: Duration::from_millis(1),
                request_timeout: Duration::from_secs(1),
            },
        )
        .await
        .expect("second response projects the exact offer");

        assert_eq!(waited, 0);
        assert_eq!(
            server.await.unwrap(),
            vec![
                "/v1/offers/offer-abc".to_string(),
                "/v1/offers/offer-abc".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn exact_offer_poll_rejects_mismatched_artifact() {
        let (base, server) = spawn_offer_server(vec![(
            200,
            r#"{"artifact_hash":"different-offer"}"#.to_string(),
        )])
        .await;

        let error = wait_for_offer_with_policy(
            &base,
            "expected-offer",
            PollPolicy {
                timeout: Duration::from_secs(1),
                interval: Duration::from_millis(1),
                request_timeout: Duration::from_secs(1),
            },
        )
        .await
        .expect_err("a different offer must not satisfy publication verification");

        assert!(error.to_string().contains("different-offer"));
        assert_eq!(
            server.await.unwrap(),
            vec!["/v1/offers/expected-offer".to_string()]
        );
    }
}
