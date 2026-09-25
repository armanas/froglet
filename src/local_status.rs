//! An authenticated, read-only loopback surface, never part of public_router.
use crate::cli::{CliError, doctor, pop_flag, prepare};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, net::SocketAddr, path::Path, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;

#[derive(Serialize, Deserialize)]
struct ListenerInfo {
    url: String,
    launch_key: String,
}
struct StatusState {
    authority: String,
    launch_key: String,
    sessions: RwLock<BTreeMap<String, i64>>,
    snapshot: RwLock<Value>,
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
fn allowed_host(headers: &HeaderMap, state: &StatusState) -> bool {
    headers.get(header::HOST).and_then(|v| v.to_str().ok()) == Some(state.authority.as_str())
        && headers
            .get(header::ORIGIN)
            .is_none_or(|v| v.to_str().ok() == Some(format!("http://{}", state.authority).as_str()))
}

async fn page(State(state): State<Arc<StatusState>>, headers: HeaderMap) -> impl IntoResponse {
    if !allowed_host(&headers, &state) {
        return (
            StatusCode::FORBIDDEN,
            HeaderMap::new(),
            "Local status is available only on its loopback address.",
        );
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "text/html; charset=utf-8".parse().unwrap(),
    );
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    headers.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers.insert(header::CONTENT_SECURITY_POLICY, "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'".parse().unwrap());
    (StatusCode::OK, headers, include_str!("local_status.html"))
}

async fn session(State(state): State<Arc<StatusState>>, headers: HeaderMap) -> impl IntoResponse {
    let valid = bearer(&headers)
        .is_some_and(|token| bool::from(token.as_bytes().ct_eq(state.launch_key.as_bytes())));
    if !allowed_host(&headers, &state) || !valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"local_auth_required"})),
        );
    }
    let now = crate::settlement::current_unix_timestamp();
    let mut sessions = state.sessions.write().await;
    sessions.retain(|_, expires| *expires > now);
    if sessions.len() >= 16 {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(
                json!({"error":"too_many_status_sessions", "next_action":"Reuse an open status tab or wait for a session to expire."}),
            ),
        );
    }
    let token = hex::encode(rand::random::<[u8; 32]>());
    sessions.insert(token.clone(), now + 900);
    (
        StatusCode::OK,
        Json(json!({"token":token, "expires_at":now+900})),
    )
}

async fn snapshot(State(state): State<Arc<StatusState>>, headers: HeaderMap) -> impl IntoResponse {
    let valid = if let Some(token) = bearer(&headers) {
        state
            .sessions
            .read()
            .await
            .get(token)
            .is_some_and(|expires| *expires > crate::settlement::current_unix_timestamp())
    } else {
        false
    };
    if !allowed_host(&headers, &state) || !valid {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::CACHE_CONTROL, "no-store")],
            Json(
                json!({"error":"session_expired", "next_action":"Ask your agent to open Froglet status again."}),
            ),
        );
    }
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.snapshot.read().await.clone()),
    )
}

fn router(state: Arc<StatusState>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/session", post(session))
        .route("/snapshot", get(snapshot))
        .with_state(state)
}

pub async fn start(
    node: Arc<crate::state::AppState>,
    runtime_address: Option<SocketAddr>,
) -> Result<(), CliError> {
    let root = node.config.storage.data_dir.clone();
    let local_origin = |address: Option<SocketAddr>| {
        address
            .map(|mut address| {
                if address.ip().is_unspecified() {
                    address.set_ip(if address.is_ipv4() {
                        std::net::Ipv4Addr::LOCALHOST.into()
                    } else {
                        std::net::Ipv6Addr::LOCALHOST.into()
                    });
                }
                format!("http://{address}")
            })
            .unwrap_or_else(|| "http://127.0.0.1:0".into())
    };
    let provider_url = local_origin(node.transport_status.lock().await.local_provider_bound_addr);
    let runtime_url = local_origin(runtime_address);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let info = ListenerInfo {
        url: format!("http://{address}"),
        launch_key: hex::encode(rand::random::<[u8; 32]>()),
    };
    let state = Arc::new(StatusState {
        authority: address.to_string(),
        launch_key: info.launch_key.clone(),
        sessions: RwLock::new(BTreeMap::new()),
        snapshot: RwLock::new(json!({"status":"checking", "issues":[], "checked_at":null})),
    });
    prepare::private_dir(&root)?;
    prepare::atomic_private_write(
        &root.join("local-status.json"),
        &serde_json::to_vec(&info).map_err(|e| CliError::Other(e.to_string()))?,
    )?;
    let app = router(state.clone());
    tokio::spawn(async move {
        let polling = async {
            loop {
                let report = doctor::snapshot_at(&root, &provider_url, &runtime_url).await;
                *state.snapshot.write().await = report;
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        };
        tokio::select! {
            result = axum::serve(listener, app) => { if let Err(error) = result { tracing::warn!("local status listener stopped: {error}"); } },
            _ = polling => {},
        }
    });
    Ok(())
}

pub async fn open(root: &Path) -> Result<Value, CliError> {
    let bytes = tokio::fs::read(root.join("local-status.json"))
        .await
        .map_err(|_| {
            CliError::Other(
                "local_status_unavailable: start the installed node, or run froglet-node doctor"
                    .into(),
            )
        })?;
    if bytes.len() > 4096 {
        return Err(CliError::Other(
            "invalid local status listener record".into(),
        ));
    }
    let info: ListenerInfo =
        serde_json::from_slice(&bytes).map_err(|e| CliError::Other(e.to_string()))?;
    let address: SocketAddr = info
        .url
        .strip_prefix("http://")
        .and_then(|v| v.parse().ok())
        .filter(|address: &SocketAddr| address.ip().is_loopback())
        .ok_or_else(|| {
            CliError::Other("local status must use a literal loopback address".into())
        })?;
    let response = crate::tls::reqwest_client_builder()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| CliError::Other(e.to_string()))?
        .post(format!("http://{address}/session"))
        .bearer_auth(info.launch_key)
        .send()
        .await
        .map_err(|e| CliError::Daemon(format!("local_status_unavailable: {e}")))?;
    if !response.status().is_success() {
        return Err(CliError::Daemon(format!(
            "local status session returned {}",
            response.status()
        )));
    }
    let session: Value =
        crate::http_body::read_json_response_limited(response, 4096, "status session")
            .await
            .map_err(CliError::Daemon)?;
    let token = session["token"]
        .as_str()
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| CliError::Daemon("invalid status session token".into()))?;
    Ok(
        json!({"status":"ok", "url":format!("http://{address}/#token={token}"), "expires_at":session["expires_at"], "read_only":true, "next_action":"Open this local URL to see status. It expires after 15 minutes; request a new link when needed."}),
    )
}

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let open_browser = pop_flag(&mut args, "--open");
    let json_mode = pop_flag(&mut args, "--json");
    if !args.is_empty() {
        return Err(CliError::BadArgs(
            "usage: froglet-node status [--open] [--json]".into(),
        ));
    }
    if open_browser {
        let report = open(&prepare::data_root()).await?;
        if json_mode {
            println!("{report}");
        } else {
            println!("{}", report["url"].as_str().unwrap_or(""));
        }
    } else {
        doctor::run(if json_mode {
            vec!["--json".into()]
        } else {
            vec![]
        })
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;
    #[tokio::test]
    async fn status_requires_exact_host_and_scoped_session() {
        let state = Arc::new(StatusState {
            authority: "127.0.0.1:8800".into(),
            launch_key: "launch-secret".into(),
            sessions: RwLock::new(BTreeMap::from([(
                "read-only".into(),
                crate::settlement::current_unix_timestamp() + 60,
            )])),
            snapshot: RwLock::new(json!({"status":"ok"})),
        });
        for (host, token, expected) in [
            ("127.0.0.1:8800", "read-only", StatusCode::OK),
            ("attacker.example", "read-only", StatusCode::UNAUTHORIZED),
            ("127.0.0.1:8800", "launch-secret", StatusCode::UNAUTHORIZED),
            ("127.0.0.1:8800", "", StatusCode::UNAUTHORIZED),
        ] {
            let response = router(state.clone())
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/snapshot")
                        .header("host", host)
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/provider/publications")
                    .header("host", "127.0.0.1:8800")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
