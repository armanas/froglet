//! Relay ingress tunnel client — the node side of `docs/RELAY.md` v1.
//!
//! The node dials OUT to a marketplace-operated relay over WSS,
//! authenticates with a Schnorr challenge–response on the provider identity
//! key, and then serves `frame.v1` request frames by forwarding them to the
//! local loopback backend listener (the same backend the Tor hidden service
//! fronts). The relay terminates TLS for
//! `https://<label>.relay.froglet.dev` — no DNS, certificates, or inbound
//! reachability are required on this node.
//!
//! One call to [`run_tunnel_once`] is one connection lifecycle; it returns
//! `Err` on any disconnect so the caller's supervision loop provides the
//! reconnect-with-backoff behavior required by the spec.

use crate::state::AppState;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

/// Domain separator for the auth signature (RELAY.md § 3).
const AUTH_DOMAIN: &[u8] = b"froglet-relay-auth/v1";
/// Only capability implemented today.
const FRAME_V1: &str = "frame.v1";
/// Per-request forward timeout; below the relay's 60s so our 504 reaches it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(55);
/// Default response-body cap (RELAY.md § 4); the relay's `ready` may lower it.
const DEFAULT_MAX_BODY_BYTES: usize = 10 * 1024 * 1024;
/// Bound on concurrently forwarded requests per tunnel.
const MAX_IN_FLIGHT: usize = 32;

/// Messages the relay sends to the node.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RelayToNode {
    Challenge {
        challenge: String,
    },
    // heartbeat_secs is also advertised in `ready`; the client ignores it —
    // liveness rides on WebSocket protocol pings from the relay (§ 2).
    Ready {
        public_url: String,
        #[serde(default)]
        max_body_bytes: Option<usize>,
    },
    Request(RequestFrame),
}

#[derive(Debug, Deserialize)]
struct RequestFrame {
    id: String,
    method: String,
    path: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body_b64: String,
}

#[derive(Debug, Serialize)]
struct ResponseFrame<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    status: u16,
    headers: HashMap<String, String>,
    body_b64: String,
}

/// Build the exact byte string the auth signature covers (RELAY.md § 3):
/// `"froglet-relay-auth/v1" || challenge_bytes || pubkey_bytes`.
fn auth_message(challenge_bytes: &[u8], pubkey_bytes: &[u8]) -> Vec<u8> {
    let mut message =
        Vec::with_capacity(AUTH_DOMAIN.len() + challenge_bytes.len() + pubkey_bytes.len());
    message.extend_from_slice(AUTH_DOMAIN);
    message.extend_from_slice(challenge_bytes);
    message.extend_from_slice(pubkey_bytes);
    message
}

/// Headers forwarded from relay frames into local requests.
fn request_header_allowed(name: &str) -> bool {
    matches!(
        name,
        "content-type" | "accept" | "authorization" | "content-length"
    ) || name.starts_with("x-froglet-")
}

/// Headers forwarded from local responses back into response frames.
fn response_header_allowed(name: &str) -> bool {
    name == "content-type" || name.starts_with("x-froglet-")
}

fn error_frame<'a>(id: &'a str, status: u16, detail: &str) -> ResponseFrame<'a> {
    ResponseFrame {
        id,
        kind: "response",
        status,
        headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
        body_b64: BASE64.encode(format!("{{\"error\":{}}}", serde_json::json!(detail))),
    }
}

/// Forward one request frame to the local backend and build the response
/// frame. Backend failures map to 502, timeouts to 504, oversized responses
/// to 502 — always producing a frame so the relay is never left hanging.
async fn forward_request(
    http: &reqwest::Client,
    backend_addr: SocketAddr,
    frame: RequestFrame,
    max_body_bytes: usize,
) -> String {
    let response_frame = forward_request_inner(http, backend_addr, &frame, max_body_bytes).await;
    let frame_out = match &response_frame {
        Ok(frame) => serde_json::to_string(frame),
        Err((status, detail)) => serde_json::to_string(&error_frame(&frame.id, *status, detail)),
    };
    // A response frame is plain data; serialization only fails on pathological
    // states, in which case a minimal literal keeps the relay's `id` matching.
    frame_out.unwrap_or_else(|_| {
        format!(
            "{{\"id\":{},\"type\":\"response\",\"status\":500,\"headers\":{{}},\"body_b64\":\"\"}}",
            serde_json::json!(frame.id)
        )
    })
}

#[allow(clippy::result_large_err)]
async fn forward_request_inner<'a>(
    http: &reqwest::Client,
    backend_addr: SocketAddr,
    frame: &'a RequestFrame,
    max_body_bytes: usize,
) -> Result<ResponseFrame<'a>, (u16, String)> {
    let method: reqwest::Method = frame
        .method
        .parse()
        .map_err(|_| (400, format!("unsupported method {}", frame.method)))?;
    if !frame.path.starts_with('/') {
        return Err((400, "path must start with '/'".to_string()));
    }
    let mut url = format!("http://{}{}", backend_addr, frame.path);
    if !frame.query.is_empty() {
        url.push('?');
        url.push_str(&frame.query);
    }
    let body = if frame.body_b64.is_empty() {
        Vec::new()
    } else {
        BASE64
            .decode(&frame.body_b64)
            .map_err(|e| (400, format!("invalid body_b64: {e}")))?
    };
    if body.len() > max_body_bytes {
        return Err((413, "request body exceeds max_body_bytes".to_string()));
    }

    let mut request = http.request(method, url).timeout(REQUEST_TIMEOUT);
    for (name, value) in &frame.headers {
        let lowered = name.to_ascii_lowercase();
        if request_header_allowed(&lowered) {
            request = request.header(lowered, value);
        }
    }
    if !body.is_empty() {
        request = request.body(body);
    }

    let response = request.send().await.map_err(|e| {
        if e.is_timeout() {
            (504, format!("backend timeout: {e}"))
        } else {
            (502, format!("backend request failed: {e}"))
        }
    })?;
    let status = response.status().as_u16();
    let mut headers = HashMap::new();
    for (name, value) in response.headers() {
        let lowered = name.as_str().to_ascii_lowercase();
        if response_header_allowed(&lowered)
            && let Ok(value) = value.to_str()
        {
            headers.insert(lowered, value.to_string());
        }
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| (502, format!("backend body read failed: {e}")))?;
    if bytes.len() > max_body_bytes {
        return Err((502, "backend response exceeds max_body_bytes".to_string()));
    }
    Ok(ResponseFrame {
        id: &frame.id,
        kind: "response",
        status,
        headers,
        body_b64: BASE64.encode(&bytes),
    })
}

async fn set_relay_status(state: &AppState, url: Option<String>, status: &str) {
    let mut transport = state.transport_status.lock().await;
    transport.relay_url = url;
    transport.relay_status = status.to_string();
}

/// One tunnel connection lifecycle: connect → authenticate → serve frames
/// until the connection drops. Always returns `Err` (a healthy tunnel never
/// finishes) so the supervising restart loop reconnects with backoff.
pub async fn run_tunnel_once(
    state: Arc<AppState>,
    relay_url: &str,
    backend_addr: SocketAddr,
) -> Result<(), String> {
    let (ws, _response) = connect_async(relay_url)
        .await
        .map_err(|e| format!("relay connect failed: {e}"))?;
    let (mut write, mut read) = ws.split();

    // ── Handshake (RELAY.md § 3) ──────────────────────────────────────────
    let provider_id = state.identity.node_id().to_string();
    let hello = serde_json::json!({
        "type": "hello",
        "provider_id": provider_id,
        "capabilities": [FRAME_V1],
    });
    write
        .send(Message::Text(hello.to_string()))
        .await
        .map_err(|e| format!("relay hello send failed: {e}"))?;

    let mut public_url: Option<String> = None;
    let mut max_body_bytes = DEFAULT_MAX_BODY_BYTES;
    while public_url.is_none() {
        let message = read
            .next()
            .await
            .ok_or("relay closed during handshake")?
            .map_err(|e| format!("relay handshake read failed: {e}"))?;
        let text = match message {
            Message::Text(text) => text,
            Message::Ping(payload) => {
                write
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|e| format!("relay pong send failed: {e}"))?;
                continue;
            }
            Message::Close(_) => return Err("relay closed during handshake".to_string()),
            _ => continue,
        };
        match serde_json::from_str::<RelayToNode>(&text)
            .map_err(|e| format!("relay sent invalid handshake message: {e}"))?
        {
            RelayToNode::Challenge { challenge } => {
                let challenge_bytes = hex::decode(&challenge)
                    .map_err(|e| format!("relay challenge is not hex: {e}"))?;
                let pubkey_bytes = hex::decode(&provider_id)
                    .map_err(|e| format!("provider id is not hex: {e}"))?;
                let signature = state
                    .identity
                    .sign_message_hex(&auth_message(&challenge_bytes, &pubkey_bytes));
                let auth = serde_json::json!({ "type": "auth", "signature": signature });
                write
                    .send(Message::Text(auth.to_string()))
                    .await
                    .map_err(|e| format!("relay auth send failed: {e}"))?;
            }
            RelayToNode::Ready {
                public_url: url,
                max_body_bytes: advertised_cap,
            } => {
                if !url.starts_with("https://") {
                    return Err(format!("relay advertised a non-https public_url: {url}"));
                }
                if let Some(cap) = advertised_cap {
                    max_body_bytes = cap.min(DEFAULT_MAX_BODY_BYTES);
                }
                public_url = Some(url);
            }
            RelayToNode::Request(frame) => {
                return Err(format!(
                    "relay sent a request frame (id {}) before ready",
                    frame.id
                ));
            }
        }
    }
    let public_url = public_url.expect("loop exits only with a url");
    info!("relay tunnel established: {public_url}");
    set_relay_status(&state, Some(public_url.clone()), "up").await;

    // ── Serve frames ──────────────────────────────────────────────────────
    // Forwarded requests run concurrently (bounded); their response frames
    // funnel through one writer task via this channel.
    let (frame_tx, mut frame_rx) = mpsc::channel::<Message>(MAX_IN_FLIGHT * 2);
    let writer = tokio::spawn(async move {
        while let Some(message) = frame_rx.recv().await {
            if let Err(error) = write.send(message).await {
                warn!("relay write failed: {error}");
                break;
            }
        }
    });

    // Local-only client: never routed through env proxies, so a Tor/egress
    // proxy configuration cannot capture loopback forwarding.
    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| format!("failed to build relay backend client: {e}"))?;
    let in_flight = Arc::new(Semaphore::new(MAX_IN_FLIGHT));

    let disconnect_reason = loop {
        let Some(message) = read.next().await else {
            break "relay stream ended".to_string();
        };
        let message = match message {
            Ok(message) => message,
            Err(error) => break format!("relay read failed: {error}"),
        };
        match message {
            Message::Text(text) => match serde_json::from_str::<RelayToNode>(&text) {
                Ok(RelayToNode::Request(frame)) => {
                    let Ok(permit) = in_flight.clone().acquire_owned().await else {
                        break "in-flight semaphore closed".to_string();
                    };
                    let http = http.clone();
                    let frame_tx = frame_tx.clone();
                    tokio::spawn(async move {
                        let response =
                            forward_request(&http, backend_addr, frame, max_body_bytes).await;
                        let _ = frame_tx.send(Message::Text(response)).await;
                        drop(permit);
                    });
                }
                Ok(_) => warn!("relay sent an unexpected control message after ready"),
                Err(error) => warn!("relay sent an unparseable frame: {error}"),
            },
            Message::Ping(payload) => {
                if frame_tx.send(Message::Pong(payload)).await.is_err() {
                    break "writer task gone".to_string();
                }
            }
            Message::Close(_) => break "relay closed the tunnel".to_string(),
            _ => {}
        }
    };

    drop(frame_tx);
    writer.abort();
    set_relay_status(&state, None, "down").await;
    Err(disconnect_reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_message_is_domain_separated_concatenation() {
        let challenge = [0xAAu8; 4];
        let pubkey = [0xBBu8; 4];
        let message = auth_message(&challenge, &pubkey);
        let mut expected = b"froglet-relay-auth/v1".to_vec();
        expected.extend_from_slice(&challenge);
        expected.extend_from_slice(&pubkey);
        assert_eq!(message, expected);
    }

    #[test]
    fn relay_messages_parse_per_spec() {
        let challenge: RelayToNode =
            serde_json::from_str(r#"{"type":"challenge","challenge":"aabb"}"#).unwrap();
        assert!(matches!(challenge, RelayToNode::Challenge { .. }));

        let ready: RelayToNode = serde_json::from_str(
            r#"{"type":"ready","public_url":"https://x.relay.froglet.dev","heartbeat_secs":30,"max_body_bytes":1024}"#,
        )
        .unwrap();
        match ready {
            RelayToNode::Ready {
                public_url,
                max_body_bytes,
            } => {
                assert_eq!(public_url, "https://x.relay.froglet.dev");
                assert_eq!(max_body_bytes, Some(1024));
            }
            other => panic!("expected ready, got {other:?}"),
        }

        let request: RelayToNode = serde_json::from_str(
            r#"{"id":"r1","type":"request","method":"GET","path":"/v1/feed","query":"limit=1","headers":{"accept":"application/json"},"body_b64":""}"#,
        )
        .unwrap();
        match request {
            RelayToNode::Request(frame) => {
                assert_eq!(frame.id, "r1");
                assert_eq!(frame.method, "GET");
                assert_eq!(frame.path, "/v1/feed");
                assert_eq!(frame.query, "limit=1");
            }
            other => panic!("expected request, got {other:?}"),
        }
    }

    #[test]
    fn header_allowlists_match_spec() {
        for allowed in [
            "content-type",
            "accept",
            "authorization",
            "content-length",
            "x-froglet-relay",
        ] {
            assert!(request_header_allowed(allowed), "{allowed} should forward");
        }
        for blocked in ["cookie", "host", "x-forwarded-for"] {
            assert!(
                !request_header_allowed(blocked),
                "{blocked} must not forward"
            );
        }
        assert!(response_header_allowed("content-type"));
        assert!(response_header_allowed("x-froglet-receipt"));
        assert!(!response_header_allowed("set-cookie"));
    }

    #[test]
    fn response_frame_serializes_with_type_tag() {
        let frame = ResponseFrame {
            id: "r1",
            kind: "response",
            status: 200,
            headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
            body_b64: BASE64.encode(b"{}"),
        };
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&frame).unwrap()).unwrap();
        assert_eq!(value["type"], "response");
        assert_eq!(value["id"], "r1");
        assert_eq!(value["status"], 200);
    }
}
