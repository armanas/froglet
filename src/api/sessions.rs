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
        .route("/api/sessions", post(create_session))
        .route("/api/sessions/validate", get(validate_session))
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
