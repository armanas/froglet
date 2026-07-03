//! End-to-end exercise of the relay tunnel client (docs/RELAY.md v1)
//! against an in-process stub relay and stub HTTP backend: WSS-less ws://
//! loopback transport, real Schnorr challenge–response verification, one
//! forwarded `frame.v1` request, response-header allowlisting, and
//! transport-status teardown on relay close.

use axum::Router;
use axum::http::{HeaderName, StatusCode, header};
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use froglet::confidential::ConfidentialConfig;
use froglet::config::{
    IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig, PaymentBackend,
    PricingConfig, RelayConfig, StorageConfig, TorSidecarConfig, WasmConfig,
};
use froglet::state::{AppState, build_app_state};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

fn unique_temp_dir() -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("valid time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("froglet-relay-tunnel-{unique}"));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn relay_test_state() -> Arc<AppState> {
    let temp_dir = unique_temp_dir();
    let config = NodeConfig {
        network_mode: NetworkMode::Clearnet,
        listen_addr: "127.0.0.1:0".to_string(),
        public_base_url: None,
        runtime_listen_addr: "127.0.0.1:0".to_string(),
        runtime_allow_non_loopback: false,
        http_ca_cert_path: None,
        tor: TorSidecarConfig {
            binary_path: "tor".to_string(),
            backend_listen_addr: "127.0.0.1:0".to_string(),
            startup_timeout_secs: 90,
        },
        relay: RelayConfig {
            // The URL used by the test goes straight to run_tunnel_once; the
            // config only drives TransportStatus initialization here.
            url: Some("ws://127.0.0.1:0".to_string()),
            enabled: true,
        },
        identity: IdentityConfig {
            auto_generate: true,
        },
        pricing: PricingConfig {
            events_query: 0,
            execute_wasm: 0,
        },
        payment_backends: vec![PaymentBackend::None],
        execution_timeout_secs: 10,
        process_limits: Default::default(),
        public_quota: Default::default(),
        lightning: LightningConfig {
            mode: LightningMode::Mock,
            destination_identity: None,
            base_invoice_expiry_secs: 300,
            success_hold_expiry_secs: 300,
            min_final_cltv_expiry: 18,
            sync_interval_ms: 1_000,
            lnd_rest: None,
            phoenixd: None,
        },
        x402: None,
        stripe: None,
        buyer_stripe: None,
        buyer_phoenixd: None,
        requester_spend: Default::default(),
        storage: StorageConfig {
            data_dir: temp_dir.clone(),
            db_path: temp_dir.join("node.db"),
            identity_dir: temp_dir.join("identity"),
            identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
            nostr_publication_seed_path: temp_dir.join("identity/nostr-publication.secp256k1.seed"),
            runtime_dir: temp_dir.join("runtime"),
            runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
            consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
            provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
            tor_dir: temp_dir.join("tor"),
            host_readable_control_token: false,
        },
        wasm: WasmConfig {
            policy_path: None,
            policy: None,
        },
        gpu: Default::default(),
        confidential: ConfidentialConfig {
            policy_path: None,
            policy: None,
            session_ttl_secs: 300,
        },
        marketplace_url: None,
        marketplace_allow_local: false,
        provider_artifact_root: None,
        postgres_mounts: std::collections::BTreeMap::new(),
        session_pool: Default::default(),
        hosted_trial_origin_secret: None,
    };
    build_app_state(config).expect("app state")
}

async fn read_text_json(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> serde_json::Value {
    loop {
        match ws.next().await.expect("ws open").expect("ws read") {
            Message::Text(text) => return serde_json::from_str(&text).expect("valid json"),
            Message::Ping(payload) => ws.send(Message::Pong(payload)).await.expect("pong"),
            other => panic!("unexpected ws message: {other:?}"),
        }
    }
}

#[tokio::test]
async fn tunnel_authenticates_forwards_and_tears_down() {
    let state = relay_test_state();
    let provider_id = state.identity.node_id().to_string();

    // ── Stub backend the tunnel forwards into ────────────────────────────
    let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind backend");
    let backend_addr = backend_listener.local_addr().expect("backend addr");
    let app = Router::new().route(
        "/v1/ping",
        get(|| async {
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/json"),
                    // Must survive the response allowlist:
                    (HeaderName::from_static("x-froglet-test"), "yes"),
                    // Must be stripped by the response allowlist:
                    (header::SET_COOKIE, "secret=1"),
                ],
                r#"{"ok":true}"#,
            )
        }),
    );
    tokio::spawn(async move {
        axum::serve(backend_listener, app).await.expect("backend");
    });

    // ── Stub relay implementing the RELAY.md § 3 handshake ───────────────
    let relay_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind relay");
    let relay_url = format!("ws://{}", relay_listener.local_addr().expect("relay addr"));
    let (served_tx, served_rx) = tokio::sync::oneshot::channel::<serde_json::Value>();
    let (close_tx, close_rx) = tokio::sync::oneshot::channel::<()>();
    let relay_provider_id = provider_id.clone();
    tokio::spawn(async move {
        let (stream, _) = relay_listener.accept().await.expect("relay accept");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("ws accept");

        let hello = read_text_json(&mut ws).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["provider_id"], relay_provider_id.as_str());
        assert!(
            hello["capabilities"]
                .as_array()
                .expect("capabilities array")
                .iter()
                .any(|c| c == "frame.v1"),
            "hello must advertise frame.v1"
        );

        let challenge_bytes = [7u8; 32];
        ws.send(Message::Text(
            serde_json::json!({"type": "challenge", "challenge": hex::encode(challenge_bytes)})
                .to_string(),
        ))
        .await
        .expect("send challenge");

        let auth = read_text_json(&mut ws).await;
        assert_eq!(auth["type"], "auth");
        let signature = auth["signature"].as_str().expect("signature");
        let mut signed = b"froglet-relay-auth/v1".to_vec();
        signed.extend_from_slice(&challenge_bytes);
        signed.extend_from_slice(&hex::decode(&relay_provider_id).expect("pubkey hex"));
        assert!(
            froglet_protocol::crypto::verify_message(&relay_provider_id, signature, &signed),
            "auth signature must verify against the provider identity key"
        );

        ws.send(Message::Text(
            serde_json::json!({
                "type": "ready",
                "public_url": "https://testlabel.relay.froglet.dev",
                "heartbeat_secs": 30,
                "max_body_bytes": 1_048_576,
            })
            .to_string(),
        ))
        .await
        .expect("send ready");

        ws.send(Message::Text(
            serde_json::json!({
                "id": "req-1",
                "type": "request",
                "method": "GET",
                "path": "/v1/ping",
                "query": "a=1",
                "headers": {"accept": "application/json", "cookie": "must-not-forward"},
                "body_b64": "",
            })
            .to_string(),
        ))
        .await
        .expect("send request frame");

        let response = read_text_json(&mut ws).await;
        served_tx.send(response).expect("hand response to test");

        // Hold the tunnel open until the test has asserted the "up" state.
        close_rx.await.expect("close signal");
        ws.send(Message::Close(None)).await.ok();
    });

    // ── Run one tunnel lifecycle ──────────────────────────────────────────
    let tunnel_state = state.clone();
    let tunnel = tokio::spawn(async move {
        froglet::relay_tunnel::run_tunnel_once(tunnel_state, &relay_url, backend_addr).await
    });

    let response = tokio::time::timeout(Duration::from_secs(10), served_rx)
        .await
        .expect("tunnel served within deadline")
        .expect("relay task alive");
    assert_eq!(response["type"], "response");
    assert_eq!(response["id"], "req-1");
    assert_eq!(response["status"], 200);
    let headers = response["headers"].as_object().expect("headers object");
    assert!(headers.contains_key("content-type"));
    assert_eq!(
        headers.get("x-froglet-test").and_then(|v| v.as_str()),
        Some("yes")
    );
    assert!(
        !headers.contains_key("set-cookie"),
        "set-cookie must be stripped by the response allowlist"
    );
    let body = BASE64
        .decode(response["body_b64"].as_str().expect("body_b64"))
        .expect("valid base64");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).expect("json body"),
        serde_json::json!({"ok": true})
    );

    // While serving, the transport status advertises the relay URL.
    {
        let transport = state.transport_status.lock().await;
        assert_eq!(transport.relay_status, "up");
        assert_eq!(
            transport.relay_url.as_deref(),
            Some("https://testlabel.relay.froglet.dev")
        );
    }

    // Relay closes → the lifecycle returns Err (supervisor would reconnect)
    // and the transport status is cleared.
    close_tx.send(()).expect("signal close");
    let result = tokio::time::timeout(Duration::from_secs(10), tunnel)
        .await
        .expect("tunnel exits after close")
        .expect("tunnel task not panicked");
    let error = result.expect_err("a closed tunnel must return Err for the supervisor");
    assert!(
        error.contains("closed") || error.contains("ended"),
        "unexpected disconnect reason: {error}"
    );
    let transport = state.transport_status.lock().await;
    assert_eq!(transport.relay_status, "down");
    assert_eq!(transport.relay_url, None);
}
