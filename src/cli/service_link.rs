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
    let mut inspected = verified_inspection(link, revision, offer, descriptor)?;
    let access = response["execution_access"].as_str().unwrap_or("unknown");
    inspected["availability"]["execution_access"] = json!(match access {
        "open" | "invite" | "private" | "trial" | "paid" => access,
        _ => "unknown",
    });
    if access == "invite" {
        inspected["next_action"] = json!(
            "Ask the provider for an invitation credential file separately. Use invoke_service with access_token_file and schema-valid input. Never paste a token into the conversation or a share link."
        );
    } else if access == "private" {
        inspected["free_call_supported"] = json!(false);
        inspected["next_action"] =
            json!("Private execution: only the provider can call this service.");
    }
    if inspected["contract_version"] == froglet_protocol::file_download::CONTRACT {
        inspected["next_action"] = json!(
            "Use download_file with service_url and a new absolute destination path only when the user requests a download. For invitations use access_token_file, never a token in chat. A download verifies bytes but is not an execution receipt."
        );
        inspected["scope"] = json!(
            "Download-only immutable file; metadata is public, file bytes require download admission."
        );
        inspected["free_call_supported"] = json!(false);
    }
    Ok(inspected)
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
        "contract_version":service["contract_version"], "summary":service["summary"], "input_schema":service["input_schema"],
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

    #[test]
    fn verified_inspection_preserves_profile_and_refuses_unsigned_profile_changes() {
        use froglet_protocol::{
            crypto,
            protocol::{
                ARTIFACT_TYPE_DESCRIPTOR, ARTIFACT_TYPE_OFFER, DescriptorPayload, OfferPayload,
                sign_artifact,
            },
            publication::{PublicationRevisionPayload, sign_publication_revision},
        };
        let fixture: Value =
            serde_json::from_str(include_str!("../../conformance/kernel_v1.json")).unwrap();
        let key = crypto::signing_key_from_seed_bytes(&[0x11; 32]).unwrap();
        let provider = crypto::public_key_hex(&key);
        let now = crate::settlement::current_unix_timestamp();
        let mut descriptor_payload: DescriptorPayload = serde_json::from_value(
            fixture["artifacts"]["descriptor"]["artifact"]["payload"].clone(),
        )
        .unwrap();
        descriptor_payload.expires_at = Some(now + 3600);
        let descriptor = serde_json::to_value(
            sign_artifact(
                &provider,
                |message| crypto::sign_message_hex(&key, message),
                ARTIFACT_TYPE_DESCRIPTOR,
                now,
                descriptor_payload,
            )
            .unwrap(),
        )
        .unwrap();
        let mut offer_payload: OfferPayload = serde_json::from_value(
            fixture["artifacts"]["free_offer"]["artifact"]["payload"].clone(),
        )
        .unwrap();
        offer_payload.descriptor_hash = descriptor["hash"].as_str().unwrap().into();
        offer_payload.expires_at = Some(now + 3600);
        offer_payload.execution_profile.package_kind = "inline_module".into();
        offer_payload.execution_profile.contract_version = "froglet.wasm.run_json.v1".into();
        let limits = serde_json::to_value(&offer_payload.execution_profile).unwrap();
        let offer = serde_json::to_value(
            sign_artifact(
                &provider,
                |message| crypto::sign_message_hex(&key, message),
                ARTIFACT_TYPE_OFFER,
                now,
                offer_payload,
            )
            .unwrap(),
        )
        .unwrap();
        let profile = json!({
            "schema_version":"froglet.research-profile/v1",
            "collections":{"rows":{"fields":{"id":{"type":"string","nullable":false,"unit":"none","identifier":{"namespace":"urn:demo","version":"1","prefix":"DEMO:"}}}}},
            "provenance":{"source":"urn:synthetic","version":"1","citation":"Synthetic fixture","license":"Apache-2.0"}
        });
        let schema = json!({"type":"object",crate::research_profile::ANNOTATION:profile});
        let payload: PublicationRevisionPayload = serde_json::from_value(json!({
            "schema_version":"froglet.publication-revision.v1", "provider_id":provider,
            "service_id":"catalog", "offer_id":offer["payload"]["offer_id"], "offer_hash":offer["hash"],
            "binding_hash":"22".repeat(32),"package_digest":"22".repeat(32),"runtime":"wasm","package_kind":"inline_module",
            "service":{"source_kind":"artifact","entrypoint_kind":"module","entrypoint":"run","contract_version":"froglet.wasm.run_json.v1","mode":"sync","output_schema":schema},
            "limits":{"max_input_bytes":limits["max_input_bytes"],"max_runtime_ms":limits["max_runtime_ms"],"max_memory_bytes":limits["max_memory_bytes"],"max_output_bytes":limits["max_output_bytes"],"fuel_limit":limits["fuel_limit"]},
            "price":{"settlement_method":"none","currency":"sat","base_amount_minor":0,"success_amount_minor":0,"offer_settlement_method":"none"},
            "local_verification":{"input_hash":"33".repeat(32),"result_hash":"44".repeat(32)}
        })).unwrap();
        let revision =
            sign_publication_revision(payload, |message| crypto::sign_message_hex(&key, message))
                .unwrap();
        let mut signed = serde_json::to_value(revision).unwrap();
        let link =
            ServiceLink::parse(&format!("https://froglet.dev/s/{provider}/catalog")).unwrap();
        let inspected = verified_inspection(&link, &signed, &offer, &descriptor).unwrap();
        assert_eq!(inspected["verification"]["status"], "verified");
        assert_eq!(inspected["output_schema"], schema);
        signed["payload"]["service"]["output_schema"][crate::research_profile::ANNOTATION]["provenance"]
            ["version"] = json!("2");
        assert!(verified_inspection(&link, &signed, &offer, &descriptor).is_err());
    }
}
