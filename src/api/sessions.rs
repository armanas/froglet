//! `POST /api/sessions` — mint a new short-lived session token.
//!
//! Mounted on the public router only when `FROGLET_SESSION_POOL_ENABLED=1`.
//! When disabled, the route still exists (to give LLM callers a consistent
//! error shape) and returns 404 `session pool not enabled`.
//!
//! The actual authentication of session tokens on other endpoints is NOT
//! done here — that lands in Week 4 (split per user decision). For now,
//! this handler just hands out tokens; they are stored in
//! `AppState.session_pool` and validated by that module.

use crate::state::AppState;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse, routing::post};
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
    axum::Router::new().route("/api/sessions", post(create_session))
}

async fn create_session(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(pool) = state.session_pool.as_ref() else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::to_value(ErrorResponse {
                error: "session pool not enabled on this node",
            })
            .expect("ErrorResponse always serializes")),
        );
    };

    let Some(info) = pool.assign() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::to_value(ErrorResponse {
                error: "session pool exhausted — try again shortly",
            })
            .expect("ErrorResponse always serializes")),
        );
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
}
