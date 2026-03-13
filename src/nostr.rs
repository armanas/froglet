use crate::{canonical_json, crypto, protocol};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const KIND_FROGLET_DESCRIPTOR_SUMMARY: u32 = 30390;
pub const KIND_FROGLET_OFFER_SUMMARY: u32 = 30391;
pub const KIND_FROGLET_RECEIPT_SUMMARY: u32 = 1390;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NostrEvent {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DescriptorSummaryContent {
    pub schema_version: u32,
    pub provider_id: String,
    pub artifact_kind: String,
    pub artifact_hash: String,
    pub payment_methods: Vec<String>,
    pub runtimes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoint_hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfferSummaryContent {
    pub schema_version: u32,
    pub provider_id: String,
    pub service_id: String,
    pub artifact_kind: String,
    pub artifact_hash: String,
    pub descriptor_hash: String,
    pub resource_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    pub payment_required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payment_methods: Vec<String>,
    pub price_sats: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiptSummaryContent {
    pub schema_version: u32,
    pub provider_id: String,
    pub artifact_kind: String,
    pub receipt_hash: String,
    pub quote_hash: String,
    pub deal_state: String,
}

pub fn build_event(
    pubkey: &str,
    created_at: i64,
    kind: u32,
    tags: Vec<Vec<String>>,
    content: String,
    sign_message_hex: impl Fn(&[u8]) -> String,
) -> NostrEvent {
    let id_bytes = event_id_preimage(pubkey, created_at, kind, &tags, &content);
    let id = crypto::sha256_hex(&id_bytes);
    let sig = sign_message_hex(&hex::decode(&id).expect("event id is valid hex"));
    NostrEvent {
        id,
        pubkey: pubkey.to_string(),
        created_at,
        kind,
        tags,
        content,
        sig,
    }
}

pub fn verify_event(event: &NostrEvent) -> bool {
    let expected_id = crypto::sha256_hex(event_id_preimage(
        &event.pubkey,
        event.created_at,
        event.kind,
        &event.tags,
        &event.content,
    ));
    if expected_id != event.id {
        return false;
    }

    let Ok(id_bytes) = hex::decode(&event.id) else {
        return false;
    };
    crypto::verify_message(&event.pubkey, &event.sig, &id_bytes)
}

pub fn descriptor_coordinate(pubkey: &str, provider_id: &str) -> String {
    format!("{KIND_FROGLET_DESCRIPTOR_SUMMARY}:{pubkey}:{provider_id}")
}

pub fn build_descriptor_summary_event(
    descriptor: &protocol::SignedArtifact<protocol::DescriptorPayload>,
    publication_pubkey: &str,
    sign_message_hex: impl Fn(&[u8]) -> String,
) -> Result<NostrEvent, String> {
    ensure_descriptor_publication_key(descriptor, publication_pubkey)?;
    let endpoint_hints = descriptor
        .payload
        .transport_endpoints
        .iter()
        .map(|endpoint| endpoint.uri.clone())
        .collect::<Vec<_>>();
    let content = canonical_content(&DescriptorSummaryContent {
        schema_version: 1,
        provider_id: descriptor.payload.provider_id.clone(),
        artifact_kind: protocol::ARTIFACT_KIND_DESCRIPTOR.to_string(),
        artifact_hash: descriptor.hash.clone(),
        payment_methods: Vec::new(),
        runtimes: descriptor.payload.capabilities.execution_runtimes.clone(),
        endpoint_hints,
    })?;

    let mut tags = vec![
        vec!["d".to_string(), descriptor.payload.provider_id.clone()],
        vec!["x".to_string(), descriptor.hash.clone()],
        vec![
            "alt".to_string(),
            "Froglet provider descriptor summary".to_string(),
        ],
        vec!["t".to_string(), "froglet".to_string()],
        vec!["t".to_string(), "descriptor".to_string()],
    ];

    for runtime in &descriptor.payload.capabilities.execution_runtimes {
        tags.push(vec!["t".to_string(), runtime.clone()]);
    }

    Ok(build_event(
        publication_pubkey,
        descriptor.created_at,
        KIND_FROGLET_DESCRIPTOR_SUMMARY,
        tags,
        content,
        sign_message_hex,
    ))
}

pub fn build_offer_summary_event(
    descriptor: &protocol::SignedArtifact<protocol::DescriptorPayload>,
    offer: &protocol::SignedArtifact<protocol::OfferPayload>,
    publication_pubkey: &str,
    sign_message_hex: impl Fn(&[u8]) -> String,
) -> Result<NostrEvent, String> {
    ensure_descriptor_publication_key(descriptor, publication_pubkey)?;
    let price_sats = (offer.payload.price_schedule.base_fee_msat
        + offer.payload.price_schedule.success_fee_msat)
        / 1_000;
    let resource_kind = if offer.payload.offer_kind.starts_with("compute.") {
        "compute".to_string()
    } else {
        "data".to_string()
    };
    let content = canonical_content(&OfferSummaryContent {
        schema_version: 1,
        provider_id: descriptor.payload.provider_id.clone(),
        service_id: offer.payload.offer_id.clone(),
        artifact_kind: protocol::ARTIFACT_KIND_OFFER.to_string(),
        artifact_hash: offer.hash.clone(),
        descriptor_hash: descriptor.hash.clone(),
        resource_kind: resource_kind.clone(),
        runtime: Some(offer.payload.execution_profile.runtime.clone()),
        payment_required: price_sats > 0,
        payment_methods: vec![offer.payload.settlement_method.clone()],
        price_sats,
    })?;

    let mut tags = vec![
        vec!["d".to_string(), offer.payload.offer_id.clone()],
        vec![
            "a".to_string(),
            descriptor_coordinate(publication_pubkey, &descriptor.payload.provider_id),
        ],
        vec!["x".to_string(), offer.hash.clone()],
        vec!["alt".to_string(), "Froglet offer summary".to_string()],
        vec!["t".to_string(), "froglet".to_string()],
        vec!["t".to_string(), resource_kind],
        vec!["t".to_string(), offer.payload.offer_kind.clone()],
    ];
    tags.push(vec![
        "t".to_string(),
        offer.payload.execution_profile.runtime.clone(),
    ]);
    tags.push(vec![
        "t".to_string(),
        offer.payload.execution_profile.abi_version.clone(),
    ]);
    for method in [offer.payload.settlement_method.clone()] {
        tags.push(vec!["t".to_string(), method.clone()]);
    }
    if let Some(expires_at) = offer.payload.expires_at {
        tags.push(vec!["expiration".to_string(), expires_at.to_string()]);
    }

    Ok(build_event(
        publication_pubkey,
        offer.created_at,
        KIND_FROGLET_OFFER_SUMMARY,
        tags,
        content,
        sign_message_hex,
    ))
}

pub fn build_receipt_summary_event(
    receipt: &protocol::SignedArtifact<protocol::ReceiptPayload>,
    publication_pubkey: &str,
    sign_message_hex: impl Fn(&[u8]) -> String,
) -> Result<NostrEvent, String> {
    let content = canonical_content(&ReceiptSummaryContent {
        schema_version: 1,
        provider_id: receipt.signer.clone(),
        artifact_kind: protocol::ARTIFACT_KIND_RECEIPT.to_string(),
        receipt_hash: receipt.hash.clone(),
        quote_hash: receipt.payload.quote_hash.clone(),
        deal_state: receipt.payload.deal_state.clone(),
    })?;

    let mut tags = vec![
        vec!["x".to_string(), receipt.hash.clone()],
        vec!["alt".to_string(), "Froglet receipt summary".to_string()],
        vec!["t".to_string(), "froglet".to_string()],
        vec!["t".to_string(), receipt.payload.deal_state.clone()],
        vec!["t".to_string(), receipt.payload.execution_state.clone()],
    ];
    if let Some(failure_code) = &receipt.payload.failure_code {
        tags.push(vec!["t".to_string(), failure_code.clone()]);
    }

    Ok(build_event(
        publication_pubkey,
        receipt.created_at,
        KIND_FROGLET_RECEIPT_SUMMARY,
        tags,
        content,
        sign_message_hex,
    ))
}

fn canonical_content<T: Serialize>(value: &T) -> Result<String, String> {
    String::from_utf8(canonical_json::to_vec(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn ensure_descriptor_publication_key(
    descriptor: &protocol::SignedArtifact<protocol::DescriptorPayload>,
    publication_pubkey: &str,
) -> Result<(), String> {
    let linked = descriptor
        .payload
        .linked_identities
        .iter()
        .find(|identity| {
            identity.identity_kind == protocol::LINKED_IDENTITY_KIND_NOSTR
                && identity.identity == publication_pubkey
                && protocol::linked_identity_has_scope(
                    identity,
                    protocol::LINKED_IDENTITY_SCOPE_PUBLICATION_NOSTR,
                )
        });

    if linked.is_some() {
        Ok(())
    } else {
        Err("descriptor does not link the requested Nostr publication key".to_string())
    }
}

fn event_id_preimage(
    pubkey: &str,
    created_at: i64,
    kind: u32,
    tags: &[Vec<String>],
    content: &str,
) -> Vec<u8> {
    serde_json::to_vec(&json!([0, pubkey, created_at, kind, tags, content]))
        .expect("nostr event preimage should serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto;

    #[test]
    fn signed_event_roundtrip_verifies() {
        let signing_key = crypto::generate_signing_key();
        let pubkey = crypto::public_key_hex(&signing_key);
        let event = build_event(
            &pubkey,
            123,
            KIND_FROGLET_DESCRIPTOR_SUMMARY,
            vec![vec!["d".to_string(), "froglet".to_string()]],
            "{\"hello\":\"world\"}".to_string(),
            |message| crypto::sign_message_hex(&signing_key, message),
        );

        assert!(verify_event(&event));
    }

    #[test]
    fn tampered_event_fails_verification() {
        let signing_key = crypto::generate_signing_key();
        let pubkey = crypto::public_key_hex(&signing_key);
        let mut event = build_event(
            &pubkey,
            123,
            KIND_FROGLET_DESCRIPTOR_SUMMARY,
            vec![vec!["d".to_string(), "froglet".to_string()]],
            "{\"hello\":\"world\"}".to_string(),
            |message| crypto::sign_message_hex(&signing_key, message),
        );
        event.content = "{\"hello\":\"froglet\"}".to_string();

        assert!(!verify_event(&event));
    }
}
