use crate::{
    discovery::{
        HeartbeatRequest, NodeDescriptor, ReclaimChallengeRequest, ReclaimCompleteRequest,
        RegisterRequest, TransportDescriptor, descriptor_digest_hex, heartbeat_signing_payload,
        reclaim_signing_payload, register_signing_payload,
    },
    jobs::FaaSDescriptor,
    settlement::current_unix_timestamp,
    state::AppState,
};
use reqwest::StatusCode;
use std::{sync::Arc, time::Duration};
use tracing::{error, info, warn};

const RECLAIM_REQUIRED_CODE: &str = "reclaim_required";

pub async fn perform_initial_sync(state: Arc<AppState>) -> Result<String, String> {
    let descriptor = build_descriptor(state.as_ref()).await?;
    let digest = descriptor_digest_hex(&descriptor).map_err(|e| e.to_string())?;
    register_node(state.as_ref(), descriptor).await?;
    Ok(digest)
}

pub async fn run_sync_loop(state: Arc<AppState>, mut last_descriptor_hash: String) {
    let Some(heartbeat_interval) = state
        .config
        .reference_discovery
        .as_ref()
        .map(|discovery| discovery.heartbeat_interval_secs)
    else {
        return;
    };

    let mut consecutive_failures: u32 = 0;

    loop {
        let delay_secs = if consecutive_failures == 0 {
            heartbeat_interval
        } else {
            let backoff = heartbeat_interval.saturating_mul(2_u64.pow(consecutive_failures.min(4)));
            backoff.min(heartbeat_interval * 16)
        };

        tokio::time::sleep(Duration::from_secs(delay_secs)).await;

        let descriptor = match build_descriptor(state.as_ref()).await {
            Ok(descriptor) => descriptor,
            Err(e) => {
                record_discovery_error(state.as_ref(), &e).await;
                warn!("Failed to build discovery descriptor: {e}");
                continue;
            }
        };

        let descriptor_hash = match descriptor_digest_hex(&descriptor) {
            Ok(hash) => hash,
            Err(e) => {
                let message = e.to_string();
                record_discovery_error(state.as_ref(), &message).await;
                continue;
            }
        };

        let result = if descriptor_hash != last_descriptor_hash {
            register_node(state.as_ref(), descriptor).await.map(|_| {
                last_descriptor_hash = descriptor_hash;
            })
        } else {
            heartbeat_node(state.as_ref()).await
        };

        if let Err(e) = result {
            warn!("Reference discovery sync failed: {e}");
            record_discovery_error(state.as_ref(), &e).await;
            consecutive_failures = consecutive_failures.saturating_add(1);
        } else {
            consecutive_failures = 0;
        }
    }
}

pub async fn build_descriptor(state: &AppState) -> Result<NodeDescriptor, String> {
    let transport_status = state.transport_status.lock().await.clone();
    let services = crate::api::current_advertised_services(state).await?;

    Ok(NodeDescriptor {
        node_id: state.identity.node_id().to_string(),
        pubkey: state.identity.public_key_hex().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        discovery_mode: state.config.discovery_mode.to_string(),
        transports: TransportDescriptor {
            clearnet_url: transport_status.clearnet_url,
            onion_url: transport_status.tor_onion_url,
            tor_status: transport_status.tor_status,
        },
        services,
        faas: FaaSDescriptor::standard(),
        updated_at: None,
    })
}

pub async fn register_node(state: &AppState, descriptor: NodeDescriptor) -> Result<(), String> {
    register_node_inner(state, descriptor, true).await
}

pub async fn heartbeat_node(state: &AppState) -> Result<(), String> {
    heartbeat_node_inner(state, true).await
}

pub async fn reclaim_node(state: &AppState) -> Result<(), String> {
    let discovery = state
        .config
        .reference_discovery
        .as_ref()
        .ok_or_else(|| "reference discovery is not configured".to_string())?;

    let challenge = state
        .http_client
        .post(format!(
            "{}/v1/discovery/providers/reclaim/challenge",
            discovery.url
        ))
        .json(&ReclaimChallengeRequest {
            node_id: state.identity.node_id().to_string(),
        })
        .send()
        .await
        .map_err(|e| format!("challenge request failed: {e}"))?;

    if !challenge.status().is_success() {
        return Err(format!(
            "challenge request failed: {} {}",
            challenge.status(),
            challenge.text().await.unwrap_or_default()
        ));
    }

    let challenge: crate::discovery::ReclaimChallengeResponse = challenge
        .json()
        .await
        .map_err(|e| format!("invalid challenge response: {e}"))?;

    let timestamp = current_unix_timestamp();
    let message = reclaim_signing_payload(
        state.identity.node_id(),
        &challenge.challenge_id,
        &challenge.nonce,
        timestamp,
    );

    let response = state
        .http_client
        .post(format!(
            "{}/v1/discovery/providers/reclaim/complete",
            discovery.url
        ))
        .json(&ReclaimCompleteRequest {
            node_id: state.identity.node_id().to_string(),
            challenge_id: challenge.challenge_id,
            timestamp,
            signature: state.identity.sign_message_hex(&message),
        })
        .send()
        .await
        .map_err(|e| format!("reclaim request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "reclaim request failed: {} {}",
            response.status(),
            response.text().await.unwrap_or_default()
        ));
    }

    info!("Reference discovery reclaim completed successfully");
    Ok(())
}

async fn register_node_inner(
    state: &AppState,
    descriptor: NodeDescriptor,
    allow_reclaim: bool,
) -> Result<(), String> {
    let discovery = state
        .config
        .reference_discovery
        .as_ref()
        .ok_or_else(|| "reference discovery is not configured".to_string())?;

    let mut allow_reclaim = allow_reclaim;
    loop {
        let timestamp = current_unix_timestamp();
        let message =
            register_signing_payload(&descriptor, timestamp).map_err(|e| e.to_string())?;
        let payload = RegisterRequest {
            signature: state.identity.sign_message_hex(&message),
            timestamp,
            descriptor: descriptor.clone(),
        };

        let response = state
            .http_client
            .post(format!("{}/v1/discovery/providers/register", discovery.url))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("register request failed: {e}"))?;

        if response.status().is_success() {
            mark_discovery_success(state, true).await;
            info!("Reference discovery registration updated successfully");
            return Ok(());
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if allow_reclaim && reclaim_required(status, &body) {
            info!(
                "Reference discovery registration requires reclaim; attempting challenge-response recovery"
            );
            reclaim_node(state).await?;
            allow_reclaim = false;
            continue;
        }

        return Err(format!("discovery register failed: {status} {body}"));
    }
}

async fn heartbeat_node_inner(state: &AppState, allow_reclaim: bool) -> Result<(), String> {
    let discovery = state
        .config
        .reference_discovery
        .as_ref()
        .ok_or_else(|| "reference discovery is not configured".to_string())?;
    let mut allow_reclaim = allow_reclaim;
    loop {
        let timestamp = current_unix_timestamp();
        let message = heartbeat_signing_payload(state.identity.node_id(), timestamp);
        let payload = HeartbeatRequest {
            node_id: state.identity.node_id().to_string(),
            timestamp,
            signature: state.identity.sign_message_hex(&message),
        };

        let response = state
            .http_client
            .post(format!(
                "{}/v1/discovery/providers/heartbeat",
                discovery.url
            ))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("heartbeat request failed: {e}"))?;

        if response.status().is_success() {
            mark_discovery_success(state, false).await;
            return Ok(());
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if status == StatusCode::NOT_FOUND {
            let descriptor = build_descriptor(state).await?;
            return register_node_inner(state, descriptor, allow_reclaim).await;
        }

        if allow_reclaim && reclaim_required(status, &body) {
            info!(
                "Reference discovery heartbeat requires reclaim; attempting challenge-response recovery"
            );
            reclaim_node(state).await?;
            allow_reclaim = false;
            continue;
        }

        return Err(format!("discovery heartbeat failed: {status} {body}"));
    }
}

fn reclaim_required(status: StatusCode, body: &str) -> bool {
    status == StatusCode::CONFLICT && body.contains(RECLAIM_REQUIRED_CODE)
}

async fn mark_discovery_success(state: &AppState, registration: bool) {
    let now = current_unix_timestamp();
    let mut status = state.reference_discovery_status.lock().await;
    status.connected = true;
    status.last_error = None;
    if registration {
        status.last_register_at = Some(now);
    } else {
        status.last_heartbeat_at = Some(now);
    }
}

async fn record_discovery_error(state: &AppState, error_message: &str) {
    let mut status = state.reference_discovery_status.lock().await;
    status.connected = false;
    status.last_error = Some(error_message.to_string());
    error!("{error_message}");
}
