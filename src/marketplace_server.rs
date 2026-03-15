use crate::{
    crypto,
    db::DbPool,
    marketplace::{
        HeartbeatRequest, MarketplaceNodeRecord, MarketplaceSearchResponse, NodeDescriptor,
        ReclaimChallengeRequest, ReclaimChallengeResponse, ReclaimCompleteRequest, RegisterRequest,
        heartbeat_signing_payload, random_hex, reclaim_signing_payload, register_signing_payload,
    },
    settlement::current_unix_timestamp,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use std::path::Path as FsPath;

const MAX_REQUEST_AGE_SECS: i64 = 120;

fn request_is_stale(request_timestamp: i64, now: i64) -> bool {
    (now - request_timestamp).abs() > MAX_REQUEST_AGE_SECS
}

#[derive(Clone)]
pub struct MarketplaceAppState {
    pub db: DbPool,
    pub stale_after_secs: i64,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_inactive: Option<bool>,
}

fn configure_marketplace_connection(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA temp_store = MEMORY;
        BEGIN;
        CREATE TABLE IF NOT EXISTS nodes (
            node_id TEXT PRIMARY KEY,
            pubkey TEXT NOT NULL,
            descriptor_json TEXT NOT NULL,
            status TEXT NOT NULL,
            registered_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS challenges (
            challenge_id TEXT PRIMARY KEY,
            node_id TEXT NOT NULL,
            nonce TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            used_at INTEGER,
            FOREIGN KEY(node_id) REFERENCES nodes(node_id)
        );
        CREATE INDEX IF NOT EXISTS idx_nodes_last_seen ON nodes(last_seen_at);
        CREATE INDEX IF NOT EXISTS idx_challenges_node_id ON challenges(node_id);
        COMMIT;",
    )?;
    Ok(())
}

pub fn initialize_marketplace_db(path: &FsPath) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    configure_marketplace_connection(&conn)?;
    Ok(conn)
}

pub fn initialize_marketplace_db_reader(path: &FsPath) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    configure_marketplace_connection(&conn)?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    Ok(conn)
}

pub fn router(state: MarketplaceAppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/marketplace/register", post(register))
        .route("/v1/marketplace/heartbeat", post(heartbeat))
        .route("/v1/marketplace/reclaim/challenge", post(reclaim_challenge))
        .route("/v1/marketplace/reclaim/complete", post(reclaim_complete))
        .route("/v1/marketplace/nodes/:node_id", get(get_node))
        .route("/v1/marketplace/search", get(search_nodes))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "marketplace ok")
}

enum RegisterOutcome {
    Registered,
    ReclaimRequired,
}

async fn register(
    State(state): State<MarketplaceAppState>,
    Json(payload): Json<RegisterRequest>,
) -> impl IntoResponse {
    if payload.descriptor.node_id != payload.descriptor.pubkey {
        return bad_request("node_id must match pubkey");
    }

    let message = match register_signing_payload(&payload.descriptor, payload.timestamp) {
        Ok(message) => message,
        Err(error) => return bad_request(&format!("invalid descriptor: {error}")),
    };

    if !crypto::verify_message(&payload.descriptor.pubkey, &payload.signature, &message) {
        return bad_request("invalid signature");
    }

    let now = current_unix_timestamp();
    if request_is_stale(payload.timestamp, now) {
        return bad_request("request timestamp is too old or too far in the future");
    }

    let descriptor_json = match serde_json::to_string(&payload.descriptor) {
        Ok(json) => json,
        Err(error) => return bad_request(&format!("invalid descriptor: {error}")),
    };

    let node_id = payload.descriptor.node_id;
    let pubkey = payload.descriptor.pubkey;
    let stale_after_secs = state.stale_after_secs;
    match state
        .db
        .with_write_conn(move |conn| -> Result<RegisterOutcome, String> {
            if let Some(existing) =
                fetch_node_status(conn, &node_id).map_err(|error| error.to_string())?
                && requires_reclaim(
                    &existing.status,
                    existing.last_seen_at,
                    now,
                    stale_after_secs,
                )
            {
                conn.execute(
                    "UPDATE nodes SET status = 'inactive' WHERE node_id = ?1",
                    params![node_id],
                )
                .map_err(|error| error.to_string())?;
                return Ok(RegisterOutcome::ReclaimRequired);
            }

            conn.execute(
                "INSERT INTO nodes (node_id, pubkey, descriptor_json, status, registered_at, updated_at, last_seen_at)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?4, ?4)
                 ON CONFLICT(node_id) DO UPDATE SET
                     pubkey = excluded.pubkey,
                     descriptor_json = excluded.descriptor_json,
                     status = 'active',
                     updated_at = excluded.updated_at,
                     last_seen_at = excluded.last_seen_at",
                params![node_id, pubkey, descriptor_json, now],
            )
            .map_err(|error| error.to_string())?;

            Ok(RegisterOutcome::Registered)
        })
        .await
    {
        Ok(RegisterOutcome::Registered) => success_response("node registered"),
        Ok(RegisterOutcome::ReclaimRequired) => reclaim_required_response(),
        Err(error) => database_error(&error),
    }
}

enum HeartbeatOutcome {
    Recorded,
    NotFound,
    ReclaimRequired,
    InvalidSignature,
}

async fn heartbeat(
    State(state): State<MarketplaceAppState>,
    Json(payload): Json<HeartbeatRequest>,
) -> impl IntoResponse {
    let message = heartbeat_signing_payload(&payload.node_id, payload.timestamp);
    let now = current_unix_timestamp();
    if request_is_stale(payload.timestamp, now) {
        return bad_request("request timestamp is too old or too far in the future");
    }

    let node_id = payload.node_id;
    let signature = payload.signature;
    let stale_after_secs = state.stale_after_secs;
    match state
        .db
        .with_write_conn(move |conn| -> Result<HeartbeatOutcome, String> {
            let Some(existing) =
                fetch_node_status(conn, &node_id).map_err(|error| error.to_string())?
            else {
                return Ok(HeartbeatOutcome::NotFound);
            };

            if requires_reclaim(
                &existing.status,
                existing.last_seen_at,
                now,
                stale_after_secs,
            ) {
                conn.execute(
                    "UPDATE nodes SET status = 'inactive' WHERE node_id = ?1",
                    params![node_id],
                )
                .map_err(|error| error.to_string())?;
                return Ok(HeartbeatOutcome::ReclaimRequired);
            }

            if !crypto::verify_message(&existing.pubkey, &signature, &message) {
                return Ok(HeartbeatOutcome::InvalidSignature);
            }

            conn.execute(
                "UPDATE nodes SET status = 'active', updated_at = ?2, last_seen_at = ?2 WHERE node_id = ?1",
                params![node_id, now],
            )
            .map_err(|error| error.to_string())?;

            Ok(HeartbeatOutcome::Recorded)
        })
        .await
    {
        Ok(HeartbeatOutcome::Recorded) => success_response("heartbeat recorded"),
        Ok(HeartbeatOutcome::NotFound) => not_found("node not found"),
        Ok(HeartbeatOutcome::ReclaimRequired) => reclaim_required_response(),
        Ok(HeartbeatOutcome::InvalidSignature) => bad_request("invalid signature"),
        Err(error) => database_error(&error),
    }
}

async fn reclaim_challenge(
    State(state): State<MarketplaceAppState>,
    Json(payload): Json<ReclaimChallengeRequest>,
) -> impl IntoResponse {
    let node_id = payload.node_id;
    let now = current_unix_timestamp();
    match state
        .db
        .with_write_conn(move |conn| -> Result<Option<ReclaimChallengeResponse>, String> {
            let exists: Option<String> = conn
                .query_row(
                    "SELECT node_id FROM nodes WHERE node_id = ?1",
                    params![node_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?;
            if exists.is_none() {
                return Ok(None);
            }

            let challenge = ReclaimChallengeResponse {
                challenge_id: random_hex(16),
                nonce: random_hex(32),
                expires_at: now + 300,
            };

            conn.execute(
                "INSERT INTO challenges (challenge_id, node_id, nonce, expires_at, used_at) VALUES (?1, ?2, ?3, ?4, NULL)",
                params![challenge.challenge_id, node_id, challenge.nonce, challenge.expires_at],
            )
            .map_err(|error| error.to_string())?;

            Ok(Some(challenge))
        })
        .await
    {
        Ok(Some(challenge)) => (StatusCode::OK, Json(serde_json::json!(challenge))),
        Ok(None) => not_found("node not found"),
        Err(error) => database_error(&error),
    }
}

enum ReclaimCompleteOutcome {
    Completed,
    NotFound,
    BadRequest(&'static str),
}

async fn reclaim_complete(
    State(state): State<MarketplaceAppState>,
    Json(payload): Json<ReclaimCompleteRequest>,
) -> impl IntoResponse {
    let node_id = payload.node_id;
    let challenge_id = payload.challenge_id;
    let timestamp = payload.timestamp;
    let signature = payload.signature;
    let now = current_unix_timestamp();
    match state
        .db
        .with_write_conn(move |conn| -> Result<ReclaimCompleteOutcome, String> {
            let row: Option<(String, String, i64, Option<i64>)> = conn
                .query_row(
                    "SELECT n.pubkey, c.nonce, c.expires_at, c.used_at
                     FROM challenges c
                     JOIN nodes n ON n.node_id = c.node_id
                     WHERE c.challenge_id = ?1 AND c.node_id = ?2",
                    params![challenge_id, node_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|error| error.to_string())?;

            let Some((pubkey, nonce, expires_at, used_at)) = row else {
                return Ok(ReclaimCompleteOutcome::NotFound);
            };

            if used_at.is_some() || expires_at < now {
                return Ok(ReclaimCompleteOutcome::BadRequest(
                    "challenge expired or already used",
                ));
            }

            let message = reclaim_signing_payload(&node_id, &challenge_id, &nonce, timestamp);
            if !crypto::verify_message(&pubkey, &signature, &message) {
                return Ok(ReclaimCompleteOutcome::BadRequest("invalid signature"));
            }

            conn.execute(
                "UPDATE challenges SET used_at = ?2 WHERE challenge_id = ?1",
                params![challenge_id, now],
            )
            .map_err(|error| error.to_string())?;

            conn.execute(
                "UPDATE nodes SET status = 'active', updated_at = ?2, last_seen_at = ?2 WHERE node_id = ?1",
                params![node_id, now],
            )
            .map_err(|error| error.to_string())?;

            Ok(ReclaimCompleteOutcome::Completed)
        })
        .await
    {
        Ok(ReclaimCompleteOutcome::Completed) => success_response("node reclaimed"),
        Ok(ReclaimCompleteOutcome::NotFound) => not_found("challenge not found"),
        Ok(ReclaimCompleteOutcome::BadRequest(message)) => bad_request(message),
        Err(error) => database_error(&error),
    }
}

async fn get_node(
    State(state): State<MarketplaceAppState>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    let stale_after_secs = state.stale_after_secs;
    match state
        .db
        .with_read_conn(move |conn| {
            fetch_node_record(conn, &node_id, stale_after_secs).map_err(|error| error.to_string())
        })
        .await
    {
        Ok(Some(node)) => (StatusCode::OK, Json(serde_json::json!(node))),
        Ok(None) => not_found("node not found"),
        Err(error) => database_error(&error),
    }
}

async fn search_nodes(
    State(state): State<MarketplaceAppState>,
    Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).min(200) as i64;
    let include_inactive = query.include_inactive.unwrap_or(false);
    let now = current_unix_timestamp();
    let stale_after_secs = state.stale_after_secs;
    match state
        .db
        .with_read_conn(move |conn| -> Result<Vec<MarketplaceNodeRecord>, String> {
            let sql = if include_inactive {
                "SELECT node_id, descriptor_json, status, registered_at, updated_at, last_seen_at
                 FROM nodes ORDER BY last_seen_at DESC LIMIT ?1"
            } else {
                "SELECT node_id, descriptor_json, status, registered_at, updated_at, last_seen_at
                 FROM nodes WHERE status = 'active' ORDER BY last_seen_at DESC LIMIT ?1"
            };
            let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;

            let rows = stmt
                .query_map(params![limit], |row| {
                    let descriptor_json: String = row.get(1)?;
                    let descriptor: NodeDescriptor = serde_json::from_str(&descriptor_json)
                        .map_err(|err| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Text,
                                Box::new(err),
                            )
                        })?;

                    Ok(MarketplaceNodeRecord {
                        descriptor,
                        status: effective_status(
                            &row.get::<_, String>(2)?,
                            row.get(5)?,
                            now,
                            stale_after_secs,
                        ),
                        registered_at: row.get(3)?,
                        updated_at: row.get(4)?,
                        last_seen_at: row.get(5)?,
                    })
                })
                .map_err(|error| error.to_string())?;

            let mut nodes = Vec::new();
            for row in rows {
                let node = row.map_err(|error| error.to_string())?;
                if !include_inactive && node.status == "inactive" {
                    continue;
                }
                nodes.push(node);
            }

            Ok(nodes)
        })
        .await
    {
        Ok(nodes) => (
            StatusCode::OK,
            Json(serde_json::json!(MarketplaceSearchResponse { nodes })),
        ),
        Err(error) => database_error(&error),
    }
}

#[derive(Debug)]
struct NodeStatusRecord {
    pub pubkey: String,
    pub status: String,
    pub last_seen_at: i64,
}

fn fetch_node_status(
    conn: &Connection,
    node_id: &str,
) -> rusqlite::Result<Option<NodeStatusRecord>> {
    conn.query_row(
        "SELECT pubkey, status, last_seen_at FROM nodes WHERE node_id = ?1",
        params![node_id],
        |row| {
            Ok(NodeStatusRecord {
                pubkey: row.get(0)?,
                status: row.get(1)?,
                last_seen_at: row.get(2)?,
            })
        },
    )
    .optional()
}

fn fetch_node_record(
    conn: &Connection,
    node_id: &str,
    stale_after_secs: i64,
) -> rusqlite::Result<Option<MarketplaceNodeRecord>> {
    let now = current_unix_timestamp();
    conn.query_row(
        "SELECT descriptor_json, status, registered_at, updated_at, last_seen_at FROM nodes WHERE node_id = ?1",
        params![node_id],
        |row| {
            let descriptor_json: String = row.get(0)?;
            let descriptor: NodeDescriptor = serde_json::from_str(&descriptor_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            let status: String = row.get(1)?;
            let registered_at: i64 = row.get(2)?;
            let updated_at: i64 = row.get(3)?;
            let last_seen_at: i64 = row.get(4)?;

            Ok(MarketplaceNodeRecord {
                descriptor,
                status: effective_status(&status, last_seen_at, now, stale_after_secs),
                registered_at,
                updated_at,
                last_seen_at,
            })
        },
    )
    .optional()
}

fn requires_reclaim(status: &str, last_seen_at: i64, now: i64, stale_after_secs: i64) -> bool {
    if status != "active" {
        return true;
    }

    stale_after_secs > 0 && (now - last_seen_at) >= stale_after_secs
}

fn effective_status(status: &str, last_seen_at: i64, now: i64, stale_after_secs: i64) -> String {
    if requires_reclaim(status, last_seen_at, now, stale_after_secs) {
        "inactive".to_string()
    } else {
        status.to_string()
    }
}

fn success_response(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "success", "message": message })),
    )
}

fn reclaim_required_response() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": "reclaim required",
            "code": "reclaim_required"
        })),
    )
}

fn bad_request(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": message })),
    )
}

fn not_found(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": message })),
    )
}

fn database_error(error: &str) -> (StatusCode, Json<serde_json::Value>) {
    tracing::error!("Database error: {error}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": "internal database error" })),
    )
}
