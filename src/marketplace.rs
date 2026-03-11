use crate::{canonical_json, crypto, jobs::FaaSDescriptor, pricing::ServicePriceInfo};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportDescriptor {
    pub clearnet_url: Option<String>,
    pub onion_url: Option<String>,
    pub tor_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDescriptor {
    pub node_id: String,
    pub pubkey: String,
    pub version: String,
    pub discovery_mode: String,
    pub transports: TransportDescriptor,
    pub services: Vec<ServicePriceInfo>,
    #[serde(default)]
    pub faas: FaaSDescriptor,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub descriptor: NodeDescriptor,
    pub timestamp: i64,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub node_id: String,
    pub timestamp: i64,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReclaimChallengeRequest {
    pub node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReclaimChallengeResponse {
    pub challenge_id: String,
    pub nonce: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReclaimCompleteRequest {
    pub node_id: String,
    pub challenge_id: String,
    pub timestamp: i64,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceNodeRecord {
    pub descriptor: NodeDescriptor,
    pub status: String,
    pub registered_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceSearchResponse {
    pub nodes: Vec<MarketplaceNodeRecord>,
}

pub fn register_signing_payload(
    descriptor: &NodeDescriptor,
    timestamp: i64,
) -> Result<Vec<u8>, serde_json::Error> {
    let digest = descriptor_digest_hex(descriptor)?;
    Ok(format!(
        "froglet-register\n{}\n{}\n{}",
        descriptor.node_id, timestamp, digest
    )
    .into_bytes())
}

pub fn heartbeat_signing_payload(node_id: &str, timestamp: i64) -> Vec<u8> {
    format!("froglet-heartbeat\n{}\n{}", node_id, timestamp).into_bytes()
}

pub fn reclaim_signing_payload(
    node_id: &str,
    challenge_id: &str,
    nonce: &str,
    timestamp: i64,
) -> Vec<u8> {
    format!(
        "froglet-reclaim\n{}\n{}\n{}\n{}",
        node_id, challenge_id, nonce, timestamp
    )
    .into_bytes()
}

pub fn descriptor_digest_hex(descriptor: &NodeDescriptor) -> Result<String, serde_json::Error> {
    let json = canonical_json::to_vec(descriptor)?;
    Ok(crypto::sha256_hex(json))
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

pub fn json_hash(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex::encode(hasher.finalize())
}
