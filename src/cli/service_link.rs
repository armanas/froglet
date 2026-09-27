//! First-party share links are references, never authority to run or pay.
//! Resolve identity locally and use the existing bounded HTTPS fetcher and
//! artifact verifier. No local credentials are sent to a shared endpoint.
use super::CliError;
use serde_json::{Value, json};

#[derive(Debug, PartialEq, Eq)]
pub struct ServiceLink {
    pub url: String,
    pub service_id: String,
    pub provider_id: String,
    pub provider_url: String,
}

impl ServiceLink {
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        let invalid = || {
            CliError::BadArgs("service_url must be a Froglet HTTPS share link (/s/<provider>/<service> or /service/?provider=…&service=…), without extra parameters".into())
        };
        let url = url::Url::parse(raw).map_err(|_| invalid())?;
        if url.scheme() != "https"
            || !matches!(
                url.host_str(),
                Some("froglet.dev" | "candidate.froglet.dev")
            )
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid());
        }
        let (provider_id, service_id) = if matches!(url.path(), "/service" | "/service/") {
            let pairs = url.query_pairs().collect::<Vec<_>>();
            if pairs.len() != 2
                || pairs.iter().filter(|(k, _)| k == "provider").count() != 1
                || pairs.iter().filter(|(k, _)| k == "service").count() != 1
            {
                return Err(invalid());
            }
            (
                pairs
                    .iter()
                    .find(|(k, _)| k == "provider")
                    .unwrap()
                    .1
                    .to_string(),
                pairs
                    .iter()
                    .find(|(k, _)| k == "service")
                    .unwrap()
                    .1
                    .to_string(),
            )
        } else {
            if url.query().is_some() {
                return Err(invalid());
            }
            let parts = url
                .path()
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            if parts.len() != 4 || parts[1] != "s" {
                return Err(invalid());
            }
            (
                parts[2].to_string(),
                urlencoding::decode(parts[3])
                    .map_err(|_| invalid())?
                    .into_owned(),
            )
        };
        if provider_id.len() != 64
            || !provider_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || service_id.is_empty()
            || service_id.len() > 128
            || !service_id.as_bytes()[0].is_ascii_alphanumeric()
            || !service_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(invalid());
        }
        let label = data_encoding::BASE32_NOPAD
            .encode(&hex::decode(&provider_id).map_err(|_| invalid())?)
            .to_lowercase();
        let relay = if url.host_str() == Some("candidate.froglet.dev") {
            "relay-candidate.froglet.dev"
        } else {
            "relay.froglet.dev"
        };
        Ok(Self {
            url: format!(
                "https://{}/s/{provider_id}/{service_id}",
                url.host_str().unwrap()
            ),
            provider_url: format!("https://{label}.{relay}"),
            provider_id,
            service_id,
        })
    }
}

async fn read(url: &str) -> Result<Value, CliError> {
    crate::safe_fetch::safe_fetch_json(
        url,
        crate::safe_fetch::FetchPolicy {
            max_bytes: 1024 * 1024,
            timeout_ms: 15_000,
            ..Default::default()
        },
    )
    .await
    .map_err(|error| CliError::Daemon(format!("provider_unavailable: {error}")))
}

/// Inspect without a local daemon, authentication, or an execution deal.
pub async fn inspect(link: &ServiceLink) -> Result<Value, CliError> {
    let response = read(&format!(
        "{}/v1/provider/services/{}",
        link.provider_url, link.service_id
    ))
    .await?;
    let revision = &response["publication_revision"];
    let payload = &revision["payload"];
    let service = &response["service"];
    if payload["provider_id"] != link.provider_id
        || payload["service_id"] != link.service_id
        || service["provider_id"] != link.provider_id
        || service["service_id"] != link.service_id
        || payload["offer_id"] != service["offer_id"]
        || payload["binding_hash"] != service["binding_hash"]
    {
        return Err(CliError::Other(
            "service_identity_mismatch: publication does not match the shared link".into(),
        ));
    }
    let offer_hash = artifact_hash(&payload["offer_hash"])?;
    let offer = read(&format!("{}/v1/artifacts/{offer_hash}", link.provider_url)).await?;
    let offer = &offer["document"];
    let descriptor_hash = artifact_hash(&offer["payload"]["descriptor_hash"])?;
    let descriptor = read(&format!(
        "{}/v1/artifacts/{descriptor_hash}",
        link.provider_url
    ))
    .await?;
    let descriptor = &descriptor["document"];
    verified_inspection(link, revision, offer, descriptor)
}

fn artifact_hash(value: &Value) -> Result<&str, CliError> {
    value
        .as_str()
        .filter(|v| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .ok_or_else(|| CliError::Other("invalid_service_evidence: invalid artifact hash".into()))
}

fn verified_inspection(
    link: &ServiceLink,
    revision: &Value,
    offer: &Value,
    descriptor: &Value,
) -> Result<Value, CliError> {
    let report = froglet_verify::verify_service_link_evidence(revision, offer, descriptor);
    if !report.valid {
        return Err(CliError::Other(format!(
            "invalid_service_evidence: {}",
            report.reason.unwrap_or_default()
        )));
    }
    let payload = &revision["payload"];
    if payload["provider_id"] != link.provider_id || payload["service_id"] != link.service_id {
        return Err(CliError::Other(
            "service_identity_mismatch: signed identity differs from link".into(),
        ));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for artifact in [offer, descriptor] {
        if artifact["payload"]["expires_at"]
            .as_u64()
            .is_some_and(|expires| expires <= now)
        {
            return Err(CliError::Other(
                "service_unavailable: signed evidence has expired".into(),
            ));
        }
    }
    let price = &payload["price"];
    let free = price["base_amount_minor"] == 0
        && price["success_amount_minor"] == 0
        && price["settlement_method"] == "none"
        && price["offer_settlement_method"] == "none";
    let service = &payload["service"];
    let example = service["starter"]
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    Ok(json!({
        "status":"ok", "service_url":link.url, "provider_id":link.provider_id,
        "provider_url":link.provider_url, "service_id":link.service_id,
        "summary":service["summary"], "input_schema":service["input_schema"],
        "output_schema":service["output_schema"], "example_input":example,
        "limits":payload["limits"], "price":price, "free_call_supported":free,
        "availability":{"state":"provider_responded", "checked_at_unix":now, "requester_execution":"not_run"},
        "verification":{"status":"verified", "revision_hash":revision["revision_hash"], "offer_hash":offer["hash"], "descriptor_hash":descriptor["hash"]},
        "scope":"Only the advertised collections and fields are exposed. Summary counts do not imply access to individual records. Field meanings and scientific provenance must come from the publisher.",
        "next_action": if free { "Use invoke_service with this service_url and schema-valid input when the user requests execution." } else { "Paid Lightning calls require explicit max_price_sats plus a configured buyer wallet and cumulative spend budget. Inspection does not authorize payment. Stripe uses the runtime payment-token API." },
        "evidence_url":format!("{}/manifest.json", link.url)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_both_share_formats_to_one_identity() {
        let provider = "ea6da1dd13403af7b6abaa4bf20875f9e38f9bcc998ce3008370a49c2874766e";
        let canonical = ServiceLink::parse(&format!(
            "https://froglet.dev/s/{provider}/hla-peptidome-catalog"
        ))
        .unwrap();
        let legacy = ServiceLink::parse(&format!(
            "https://froglet.dev/service/?provider={provider}&service=hla-peptidome-catalog"
        ))
        .unwrap();
        assert_eq!(canonical, legacy);
        assert_eq!(
            canonical.provider_url,
            "https://5jw2dxitia5ppnvlvjf7ecdv7hry7g6mtggogaedocsjykduozxa.relay.froglet.dev"
        );
    }
    #[test]
    fn rejects_ambiguous_or_untrusted_links() {
        let p = "11".repeat(32);
        for raw in [
            format!("http://froglet.dev/s/{p}/catalog"),
            format!("https://evil.test/s/{p}/catalog"),
            format!("https://froglet.dev@evil.test/s/{p}/catalog"),
            format!("https://froglet.dev/s/{p}/catalog?revision=abc"),
            format!("https://froglet.dev/s/{p}/catalog#fragment"),
            format!("https://froglet.dev/s/{p}/catalog%2Fevil"),
            format!("https://froglet.dev/service/?provider={p}&service=a&service=b"),
            format!("https://froglet.dev/s/{p}/.."),
            format!("https://froglet.dev:8443/s/{p}/a"),
        ] {
            assert!(ServiceLink::parse(&raw).is_err(), "accepted {raw}");
        }
    }
    #[test]
    fn rejects_unsigned_inspection_evidence() {
        let link = ServiceLink::parse(&format!(
            "https://froglet.dev/s/{}/catalog",
            "11".repeat(32)
        ))
        .unwrap();
        assert!(verified_inspection(&link, &json!({}), &json!({}), &json!({})).is_err());
    }
}
