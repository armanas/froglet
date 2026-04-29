//! Hosted-trial session endpoints.
//!
//! These routes are mounted only behind the hosted-trial origin-secret gate.
//! When the session pool is disabled, the routes still exist behind that gate
//! so the worker sees a consistent 404 `session pool not enabled` shape.
//!
//! This handler only mints hosted-trial session tokens. The node validates
//! those tokens on the two hosted demo runtime endpoints
//! (`POST /v1/runtime/deals` and `GET /v1/runtime/deals/:deal_id`) via
//! `AppState.session_pool`.

use crate::state::AppState;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Serialize)]
struct SessionResponse {
    session_token: String,
    slot_id: usize,
    ttl_secs: u64,
    /// Wall-clock expiry in Unix seconds. Clients should prefer this over
    /// local-clock math based on `ttl_secs` when comparing against server
    /// time.
    expires_at_epoch_secs: u64,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: &'static str,
}

pub fn sessions_routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/api/preflight", get(preflight))
        .route("/api/sessions", post(create_session))
        .route("/api/sessions/validate", get(validate_session))
}

async fn preflight(State(state): State<Arc<AppState>>) -> Response {
    let (session_pool_enabled, session_pool_size, session_ttl_secs) =
        match state.session_pool.as_ref() {
            Some(pool) => (true, Some(pool.size()), Some(pool.ttl().as_secs())),
            None => (false, None, None),
        };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "service": "froglet-hosted-trial",
            "public_ingress": "https://try.froglet.dev",
            "session_pool": {
                "enabled": session_pool_enabled,
                "size": session_pool_size,
                "ttl_secs": session_ttl_secs,
            },
            "authorized_scope": {
                "hosts": ["try.froglet.dev"],
                "methods": ["GET", "POST"],
                "paths": [
                    "/llms.txt",
                    "/.well-known/mcp.json",
                    "/api/preflight",
                    "/api/sessions",
                    "/api/sessions/validate",
                    "/v1/provider/services",
                    "/v1/provider/services/{service_id}",
                    "/v1/runtime/deals",
                    "/v1/runtime/deals/{deal_id}",
                    "/v1/feed"
                ],
                "rules": [
                    "Only call the public hosted trial host and paths listed here.",
                    "Do not scan, fuzz, enumerate unrelated paths, attack third-party hosts, or use arbitrary user-supplied URLs unless the user owns or controls them.",
                    "Only demo.* services are part of the public hosted proof."
                ]
            },
            "required_client_capabilities": {
                "https_get": true,
                "https_post_json": true,
                "custom_headers": ["Authorization: Bearer <session-token>", "content-type: application/json"],
                "polling": true
            },
            "public_hosted_proof_services": [
                "demo.add",
                "demo.echo",
                "demo.fetch-witness",
                "demo.hash-verify",
                "demo.notarize"
            ],
            "non_demo_services": "Other service IDs may appear on the reference node, but they are outside the public hosted proof contract.",
            "chat_only_fallback": "If this client cannot fetch URLs, POST JSON, send Bearer auth, or poll, report a tool limitation and ask the user to use an HTTP-capable agent or the documented curl flow."
        })),
    )
        .into_response()
}

async fn create_session(State(state): State<Arc<AppState>>) -> Response {
    let Some(pool) = state.session_pool.as_ref() else {
        return session_pool_not_enabled_response();
    };

    let Some(info) = pool.assign() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                serde_json::to_value(ErrorResponse {
                    error: "session pool exhausted — try again shortly",
                })
                .expect("ErrorResponse always serializes"),
            ),
        )
            .into_response();
    };

    let ttl_secs = info.ttl.as_secs();
    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = SessionResponse {
        session_token: info.token,
        slot_id: info.slot_id,
        ttl_secs,
        expires_at_epoch_secs: now_epoch.saturating_add(ttl_secs),
    };

    tracing::info!(
        slot_id = info.slot_id,
        ttl_secs,
        "minted session token from pool"
    );

    (
        StatusCode::OK,
        Json(serde_json::to_value(body).expect("SessionResponse always serializes")),
    )
        .into_response()
}

async fn validate_session(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(pool) = state.session_pool.as_ref() else {
        return session_pool_not_enabled_response();
    };

    let Some(token) = super::extract_bearer_token(&headers) else {
        return invalid_session_response();
    };
    if pool.validate(&token).is_none() {
        return invalid_session_response();
    }

    (
        StatusCode::OK,
        Json(
            serde_json::to_value(SessionValidationResponse { valid: true })
                .expect("SessionValidationResponse always serializes"),
        ),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct SessionValidationResponse {
    valid: bool,
}

fn invalid_session_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(
            serde_json::to_value(ErrorResponse {
                error: "invalid or expired session token",
            })
            .expect("ErrorResponse always serializes"),
        ),
    )
        .into_response()
}

fn session_pool_not_enabled_response() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(
            serde_json::to_value(ErrorResponse {
                error: "session pool not enabled on this node",
            })
            .expect("ErrorResponse always serializes"),
        ),
    )
        .into_response()
}
