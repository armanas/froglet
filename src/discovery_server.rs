use crate::{
    api, crypto,
    db::DbPool,
    discovery::{
        DiscoveryNodeRecord, DiscoverySearchResponse, HeartbeatRequest, NodeDescriptor,
        ReclaimChallengeRequest, ReclaimChallengeResponse, ReclaimCompleteRequest, RegisterRequest,
        heartbeat_signing_payload, random_hex, reclaim_signing_payload, register_signing_payload,
    },
    settlement::current_unix_timestamp,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Deserialize;
use std::{collections::HashSet, path::Path as FsPath};

const MAX_REQUEST_AGE_SECS: i64 = 120;

fn request_is_stale(request_timestamp: i64, now: i64) -> bool {
    (now - request_timestamp).abs() > MAX_REQUEST_AGE_SECS
}

#[derive(Clone)]
pub struct DiscoveryAppState {
    pub db: DbPool,
    pub stale_after_secs: i64,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_inactive: Option<bool>,
}

fn configure_discovery_connection(conn: &Connection) -> rusqlite::Result<()> {
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
            clearnet_url TEXT,
            onion_url TEXT,
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
        CREATE INDEX IF NOT EXISTS idx_nodes_status_last_seen ON nodes(status, last_seen_at DESC);
        CREATE INDEX IF NOT EXISTS idx_challenges_node_id ON challenges(node_id);
        CREATE INDEX IF NOT EXISTS idx_challenges_expires_at ON challenges(expires_at);
        COMMIT;",
    )?;
    ensure_node_transport_columns(conn)?;
    backfill_node_transport_claims(conn)?;
    prune_duplicate_endpoint_claims(conn)?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_nodes_unique_clearnet_url
         ON nodes(clearnet_url) WHERE clearnet_url IS NOT NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_nodes_unique_onion_url
         ON nodes(onion_url) WHERE onion_url IS NOT NULL;",
    )?;
    Ok(())
}

pub fn initialize_discovery_db(path: &FsPath) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    configure_discovery_connection(&conn)?;
    Ok(conn)
}

pub fn initialize_discovery_db_reader(path: &FsPath) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    configure_discovery_connection(&conn)?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    Ok(conn)
}

pub fn router(state: DiscoveryAppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/discovery/providers/register", post(register))
        .route("/v1/discovery/providers/heartbeat", post(heartbeat))
        .route(
            "/v1/discovery/providers/reclaim/challenge",
            post(reclaim_challenge),
        )
        .route(
            "/v1/discovery/providers/reclaim/complete",
            post(reclaim_complete),
        )
        .route("/v1/discovery/providers/:node_id", get(get_node))
        .route("/v1/discovery/search", post(search_nodes))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "reference discovery ok")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EndpointClaims {
    clearnet_url: Option<String>,
    onion_url: Option<String>,
}

impl EndpointClaims {
    #[cfg(test)]
    fn from_descriptor(descriptor: &NodeDescriptor) -> Result<Self, String> {
        Ok(Self {
            clearnet_url: normalize_endpoint(descriptor.transports.clearnet_url.clone())?,
            onion_url: normalize_endpoint(descriptor.transports.onion_url.clone())?,
        })
    }

    fn from_descriptor_for_startup(node_id: &str, descriptor: &NodeDescriptor) -> Self {
        Self {
            clearnet_url: normalize_legacy_endpoint_claim(
                node_id,
                "clearnet_url",
                descriptor.transports.clearnet_url.clone(),
            ),
            onion_url: normalize_legacy_endpoint_claim(
                node_id,
                "onion_url",
                descriptor.transports.onion_url.clone(),
            ),
        }
    }

    fn from_descriptor_json_for_startup(
        node_id: &str,
        descriptor_json: &str,
    ) -> rusqlite::Result<Self> {
        let descriptor: NodeDescriptor =
            serde_json::from_str(descriptor_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
        Ok(Self::from_descriptor_for_startup(node_id, &descriptor))
    }

    fn keys(&self) -> impl Iterator<Item = String> + '_ {
        self.clearnet_url
            .iter()
            .map(|value| format!("clearnet:{value}"))
            .chain(self.onion_url.iter().map(|value| format!("onion:{value}")))
    }

    fn is_empty(&self) -> bool {
        self.clearnet_url.is_none() && self.onion_url.is_none()
    }
}

#[derive(Debug)]
struct EndpointConflictRecord {
    node_id: String,
    status: String,
    last_seen_at: i64,
    endpoint: String,
}

fn normalize_endpoint(endpoint: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = endpoint else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed =
        reqwest::Url::parse(trimmed).map_err(|error| format!("invalid endpoint URL: {error}"))?;
    match parsed.scheme() {
        "https" => {}
        "http" => {
            let host = parsed.host_str().unwrap_or("");
            const LOOPBACK_HOSTS: &[&str] = &["127.0.0.1", "localhost", "::1", "[::1]"];
            if !LOOPBACK_HOSTS.contains(&host) && !host.ends_with(".onion") {
                return Err(format!(
                    "endpoint must use https:// (http:// is only allowed for loopback and .onion addresses), got {trimmed}"
                ));
            }
        }
        other => return Err(format!("endpoint must use http or https, got {other}")),
    }
    Ok(Some(trimmed.to_string()))
}

async fn validated_endpoint_claims_from_descriptor(
    descriptor: &NodeDescriptor,
) -> Result<EndpointClaims, String> {
    let clearnet_url = match descriptor.transports.clearnet_url.as_deref() {
        Some(url) => Some(api::validate_discovery_endpoint_url(url).await?),
        None => None,
    };
    let onion_url = match descriptor.transports.onion_url.as_deref() {
        Some(url) => Some(api::validate_discovery_endpoint_url(url).await?),
        None => None,
    };
    Ok(EndpointClaims {
        clearnet_url,
        onion_url,
    })
}

fn normalize_legacy_endpoint_claim(
    node_id: &str,
    field: &str,
    endpoint: Option<String>,
) -> Option<String> {
    match normalize_endpoint(endpoint) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                node_id = %node_id,
                field,
                error = %error,
                "ignoring invalid legacy discovery transport endpoint during startup"
            );
            None
        }
    }
}

fn ensure_node_transport_columns(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(nodes)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<HashSet<_>, _>>()?;
    if !columns.contains("clearnet_url") {
        conn.execute("ALTER TABLE nodes ADD COLUMN clearnet_url TEXT", [])?;
    }
    if !columns.contains("onion_url") {
        conn.execute("ALTER TABLE nodes ADD COLUMN onion_url TEXT", [])?;
    }
    Ok(())
}

fn backfill_node_transport_claims(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("SELECT node_id, descriptor_json FROM nodes")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (node_id, descriptor_json) = row?;
        let claims = EndpointClaims::from_descriptor_json_for_startup(&node_id, &descriptor_json)?;
        conn.execute(
            "UPDATE nodes SET clearnet_url = ?2, onion_url = ?3 WHERE node_id = ?1",
            params![node_id, claims.clearnet_url, claims.onion_url],
        )?;
    }
    Ok(())
}

fn delete_node_record(conn: &Connection, node_id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM challenges WHERE node_id = ?1",
        params![node_id],
    )
    .map_err(|error| error.to_string())?;
    conn.execute("DELETE FROM nodes WHERE node_id = ?1", params![node_id])
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn prune_duplicate_endpoint_claims(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT node_id, descriptor_json
         FROM nodes
         ORDER BY updated_at DESC, last_seen_at DESC, registered_at DESC, node_id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut seen = HashSet::new();
    let mut duplicate_node_ids = Vec::new();
    for row in rows {
        let (node_id, descriptor_json) = row?;
        let claims = EndpointClaims::from_descriptor_json_for_startup(&node_id, &descriptor_json)?;
        if claims.is_empty() {
            continue;
        }
        let keys: Vec<String> = claims.keys().collect();
        if keys.iter().any(|key| seen.contains(key)) {
            duplicate_node_ids.push(node_id);
            continue;
        }
        for key in keys {
            seen.insert(key);
        }
    }
    for node_id in duplicate_node_ids {
        delete_node_record(conn, &node_id).map_err(|error| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(error)))
        })?;
    }
    Ok(())
}

fn map_node_record_row(
    row: &Row<'_>,
    now: i64,
    stale_after_secs: i64,
) -> rusqlite::Result<DiscoveryNodeRecord> {
    let descriptor_json: String = row.get(1)?;
    let descriptor: NodeDescriptor = serde_json::from_str(&descriptor_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let status: String = row.get(2)?;
    let registered_at: i64 = row.get(3)?;
    let updated_at: i64 = row.get(4)?;
    let last_seen_at: i64 = row.get(5)?;
    Ok(DiscoveryNodeRecord {
        descriptor,
        status: effective_status(&status, last_seen_at, now, stale_after_secs),
        registered_at,
        updated_at,
        last_seen_at,
    })
}

fn load_search_records(
    conn: &Connection,
    limit: i64,
    include_inactive: bool,
    now: i64,
    stale_after_secs: i64,
) -> Result<Vec<DiscoveryNodeRecord>, String> {
    let minimum_last_seen = active_minimum_last_seen(now, stale_after_secs);
    let sql = if include_inactive {
        "SELECT node_id, descriptor_json, status, registered_at, updated_at, last_seen_at
         FROM nodes ORDER BY last_seen_at DESC LIMIT ?1"
    } else if minimum_last_seen.is_some() {
        "SELECT node_id, descriptor_json, status, registered_at, updated_at, last_seen_at
         FROM nodes
         WHERE status = 'active' AND last_seen_at > ?2
         ORDER BY last_seen_at DESC
         LIMIT ?1"
    } else {
        "SELECT node_id, descriptor_json, status, registered_at, updated_at, last_seen_at
         FROM nodes
         WHERE status = 'active'
         ORDER BY last_seen_at DESC
         LIMIT ?1"
    };
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let mut nodes = Vec::new();
    let push_row = |nodes: &mut Vec<DiscoveryNodeRecord>,
                    row_result: rusqlite::Result<DiscoveryNodeRecord>| {
        let record = row_result.map_err(|error| error.to_string())?;
        if include_inactive || record.status == "active" {
            nodes.push(record);
        }
        Ok::<(), String>(())
    };
    if include_inactive {
        let rows = stmt
            .query_map(params![limit], |row| {
                map_node_record_row(row, now, stale_after_secs)
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            push_row(&mut nodes, row)?;
        }
    } else if let Some(minimum_last_seen) = minimum_last_seen {
        let rows = stmt
            .query_map(params![limit, minimum_last_seen], |row| {
                map_node_record_row(row, now, stale_after_secs)
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            push_row(&mut nodes, row)?;
        }
    } else {
        let rows = stmt
            .query_map(params![limit], |row| {
                map_node_record_row(row, now, stale_after_secs)
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            push_row(&mut nodes, row)?;
        }
    }
    Ok(nodes)
}

fn fetch_conflicting_endpoint_claims(
    conn: &Connection,
    node_id: &str,
    claims: &EndpointClaims,
) -> Result<Vec<EndpointConflictRecord>, String> {
    let mut conflicts = Vec::new();
    if let Some(clearnet_url) = claims.clearnet_url.as_deref() {
        let mut stmt = conn
            .prepare(
                "SELECT node_id, status, last_seen_at
                 FROM nodes
                 WHERE node_id != ?1 AND clearnet_url = ?2",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map(params![node_id, clearnet_url], |row| {
                Ok(EndpointConflictRecord {
                    node_id: row.get(0)?,
                    status: row.get(1)?,
                    last_seen_at: row.get(2)?,
                    endpoint: clearnet_url.to_string(),
                })
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            conflicts.push(row.map_err(|error| error.to_string())?);
        }
    }
    if let Some(onion_url) = claims.onion_url.as_deref() {
        let mut stmt = conn
            .prepare(
                "SELECT node_id, status, last_seen_at
                 FROM nodes
                 WHERE node_id != ?1 AND onion_url = ?2",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map(params![node_id, onion_url], |row| {
                Ok(EndpointConflictRecord {
                    node_id: row.get(0)?,
                    status: row.get(1)?,
                    last_seen_at: row.get(2)?,
                    endpoint: onion_url.to_string(),
                })
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            conflicts.push(row.map_err(|error| error.to_string())?);
        }
    }
    Ok(conflicts)
}

#[cfg(test)]
fn register_node_record(
    conn: &Connection,
    descriptor: &NodeDescriptor,
    descriptor_json: &str,
    now: i64,
    stale_after_secs: i64,
) -> Result<RegisterOutcome, String> {
    let claims = EndpointClaims::from_descriptor(descriptor)?;
    register_node_record_with_claims(
        conn,
        descriptor,
        descriptor_json,
        &claims,
        now,
        stale_after_secs,
    )
}

fn register_node_record_with_claims(
    conn: &Connection,
    descriptor: &NodeDescriptor,
    descriptor_json: &str,
    claims: &EndpointClaims,
    now: i64,
    stale_after_secs: i64,
) -> Result<RegisterOutcome, String> {
    let node_id = descriptor.node_id.clone();
    let pubkey = descriptor.pubkey.clone();
    conn.execute_batch("SAVEPOINT discovery_register")
        .map_err(|error| error.to_string())?;
    let outcome = (|| -> Result<RegisterOutcome, String> {
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

        for conflict in fetch_conflicting_endpoint_claims(conn, &node_id, claims)? {
            if requires_reclaim(
                &conflict.status,
                conflict.last_seen_at,
                now,
                stale_after_secs,
            ) {
                delete_node_record(conn, &conflict.node_id)?;
                continue;
            }
            return Ok(RegisterOutcome::EndpointInUse {
                node_id: conflict.node_id,
                endpoint: conflict.endpoint,
            });
        }

        conn.execute(
            "INSERT INTO nodes (
                 node_id, pubkey, descriptor_json, clearnet_url, onion_url, status,
                 registered_at, updated_at, last_seen_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6, ?6)
             ON CONFLICT(node_id) DO UPDATE SET
                 pubkey = excluded.pubkey,
                 descriptor_json = excluded.descriptor_json,
                 clearnet_url = excluded.clearnet_url,
                 onion_url = excluded.onion_url,
                 status = 'active',
                 updated_at = excluded.updated_at,
                 last_seen_at = excluded.last_seen_at",
            params![
                node_id,
                pubkey,
                descriptor_json,
                claims.clearnet_url,
                claims.onion_url,
                now
            ],
        )
        .map_err(|error| error.to_string())?;

        Ok(RegisterOutcome::Registered)
    })();
    match outcome {
        Ok(result) => {
            conn.execute_batch("RELEASE discovery_register")
                .map_err(|error| error.to_string())?;
            Ok(result)
        }
        Err(error) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO discovery_register;
                 RELEASE discovery_register;",
            );
            Err(error)
        }
    }
}

enum RegisterOutcome {
    Registered,
    ReclaimRequired,
    EndpointInUse { node_id: String, endpoint: String },
}

async fn register(
    State(state): State<DiscoveryAppState>,
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

    let descriptor = payload.descriptor;
    let claims = match validated_endpoint_claims_from_descriptor(&descriptor).await {
        Ok(claims) => claims,
        Err(error) => return bad_request(&error),
    };
    let descriptor_json = match serde_json::to_string(&descriptor) {
        Ok(json) => json,
        Err(error) => return bad_request(&format!("invalid descriptor: {error}")),
    };
    let stale_after_secs = state.stale_after_secs;
    match state
        .db
        .with_write_conn(move |conn| -> Result<RegisterOutcome, String> {
            register_node_record_with_claims(
                conn,
                &descriptor,
                &descriptor_json,
                &claims,
                now,
                stale_after_secs,
            )
        })
        .await
    {
        Ok(RegisterOutcome::Registered) => success_response("node registered"),
        Ok(RegisterOutcome::ReclaimRequired) => reclaim_required_response(),
        Ok(RegisterOutcome::EndpointInUse { node_id, endpoint }) => {
            endpoint_in_use_response(&node_id, &endpoint)
        }
        Err(error) => database_error(&error),
    }
}

enum HeartbeatOutcome {
    Recorded,
    NotFound,
    ReclaimRequired,
    InvalidSignature,
}

fn purge_stale_challenges(conn: &Connection, now: i64) -> Result<(), String> {
    conn.execute(
        "DELETE FROM challenges WHERE expires_at < ?1 OR used_at IS NOT NULL",
        params![now],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn heartbeat(
    State(state): State<DiscoveryAppState>,
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
    State(state): State<DiscoveryAppState>,
    Json(payload): Json<ReclaimChallengeRequest>,
) -> impl IntoResponse {
    let node_id = payload.node_id;
    let now = current_unix_timestamp();
    match state
        .db
        .with_write_conn(move |conn| -> Result<Option<ReclaimChallengeResponse>, String> {
            purge_stale_challenges(conn, now)?;
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
    State(state): State<DiscoveryAppState>,
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
            purge_stale_challenges(conn, now)?;

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
    State(state): State<DiscoveryAppState>,
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
    State(state): State<DiscoveryAppState>,
    Json(query): Json<SearchQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).min(200) as i64;
    let include_inactive = query.include_inactive.unwrap_or(false);
    let now = current_unix_timestamp();
    let stale_after_secs = state.stale_after_secs;
    match state
        .db
        .with_read_conn(move |conn| {
            load_search_records(conn, limit, include_inactive, now, stale_after_secs)
        })
        .await
    {
        Ok(nodes) => (
            StatusCode::OK,
            Json(serde_json::json!(DiscoverySearchResponse { nodes })),
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
) -> rusqlite::Result<Option<DiscoveryNodeRecord>> {
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

            Ok(DiscoveryNodeRecord {
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

fn active_minimum_last_seen(now: i64, stale_after_secs: i64) -> Option<i64> {
    (stale_after_secs > 0).then(|| now.saturating_sub(stale_after_secs))
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

fn endpoint_in_use_response(
    node_id: &str,
    endpoint: &str,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": "transport endpoint already claimed by another provider",
            "code": "endpoint_in_use",
            "conflicting_node_id": node_id,
            "endpoint": endpoint
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

#[cfg(test)]
mod tests {
    use super::{
        EndpointClaims, RegisterOutcome, active_minimum_last_seen, configure_discovery_connection,
        load_search_records, register_node_record, validated_endpoint_claims_from_descriptor,
    };
    use crate::{
        discovery::{NodeDescriptor, TransportDescriptor},
        jobs::FaaSDescriptor,
        pricing::ServicePriceInfo,
    };
    use rusqlite::{Connection, params};

    fn test_descriptor(node_id: &str, clearnet_url: &str) -> NodeDescriptor {
        NodeDescriptor {
            node_id: node_id.to_string(),
            pubkey: node_id.to_string(),
            version: "test".to_string(),
            discovery_mode: "reference".to_string(),
            transports: TransportDescriptor {
                clearnet_url: Some(clearnet_url.to_string()),
                onion_url: None,
                tor_status: "disabled".to_string(),
            },
            services: vec![ServicePriceInfo {
                service_id: "execute.compute".to_string(),
                price_sats: 0,
                payment_required: false,
            }],
            faas: FaaSDescriptor::standard(),
            updated_at: None,
        }
    }

    fn insert_node(
        conn: &Connection,
        descriptor: &NodeDescriptor,
        status: &str,
        registered_at: i64,
        updated_at: i64,
        last_seen_at: i64,
    ) {
        let descriptor_json = serde_json::to_string(descriptor).unwrap();
        let claims = EndpointClaims::from_descriptor(descriptor).unwrap();
        conn.execute(
            "INSERT INTO nodes (
                node_id, pubkey, descriptor_json, clearnet_url, onion_url, status,
                registered_at, updated_at, last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                descriptor.node_id,
                descriptor.pubkey,
                descriptor_json,
                claims.clearnet_url,
                claims.onion_url,
                status,
                registered_at,
                updated_at,
                last_seen_at
            ],
        )
        .unwrap();
    }

    #[test]
    fn non_positive_staleness_disables_active_cutoff() {
        assert_eq!(active_minimum_last_seen(100, 0), None);
        assert_eq!(active_minimum_last_seen(100, -10), None);
    }

    #[test]
    fn positive_staleness_computes_active_cutoff() {
        assert_eq!(active_minimum_last_seen(100, 30), Some(70));
    }

    #[test]
    fn default_search_excludes_boundary_stale_records() {
        let conn = Connection::open_in_memory().unwrap();
        configure_discovery_connection(&conn).unwrap();
        let descriptor = test_descriptor("node-boundary", "https://provider-boundary.example.test");
        insert_node(&conn, &descriptor, "active", 70, 70, 70);

        let active_only = load_search_records(&conn, 10, false, 100, 30).unwrap();
        assert!(active_only.is_empty());

        let including_inactive = load_search_records(&conn, 10, true, 100, 30).unwrap();
        assert_eq!(including_inactive.len(), 1);
        assert_eq!(including_inactive[0].descriptor.node_id, "node-boundary");
        assert_eq!(including_inactive[0].status, "inactive");
    }

    #[test]
    fn configure_connection_prunes_legacy_duplicate_endpoint_claims() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE nodes (
                 node_id TEXT PRIMARY KEY,
                 pubkey TEXT NOT NULL,
                 descriptor_json TEXT NOT NULL,
                 status TEXT NOT NULL,
                 registered_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 last_seen_at INTEGER NOT NULL
             );
             CREATE TABLE challenges (
                 challenge_id TEXT PRIMARY KEY,
                 node_id TEXT NOT NULL,
                 nonce TEXT NOT NULL,
                 expires_at INTEGER NOT NULL,
                 used_at INTEGER,
                 FOREIGN KEY(node_id) REFERENCES nodes(node_id)
             );",
        )
        .unwrap();
        let older = test_descriptor("node-older", "https://provider-dup.example.test");
        let newer = test_descriptor("node-newer", "https://provider-dup.example.test");
        conn.execute(
            "INSERT INTO nodes (node_id, pubkey, descriptor_json, status, registered_at, updated_at, last_seen_at)
             VALUES (?1, ?2, ?3, 'active', 1, 1, 1)",
            params![
                older.node_id,
                older.pubkey,
                serde_json::to_string(&older).unwrap()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nodes (node_id, pubkey, descriptor_json, status, registered_at, updated_at, last_seen_at)
             VALUES (?1, ?2, ?3, 'active', 2, 2, 2)",
            params![
                newer.node_id,
                newer.pubkey,
                serde_json::to_string(&newer).unwrap()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO challenges (challenge_id, node_id, nonce, expires_at, used_at)
             VALUES ('challenge-1', 'node-older', 'nonce', 10, NULL)",
            [],
        )
        .unwrap();

        configure_discovery_connection(&conn).unwrap();

        let remaining: Vec<String> = conn
            .prepare("SELECT node_id FROM nodes ORDER BY updated_at DESC")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(remaining, vec!["node-newer".to_string()]);
        let challenge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM challenges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(challenge_count, 0);
    }

    #[test]
    fn configure_connection_ignores_legacy_invalid_endpoint_claims() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE nodes (
                 node_id TEXT PRIMARY KEY,
                 pubkey TEXT NOT NULL,
                 descriptor_json TEXT NOT NULL,
                 status TEXT NOT NULL,
                 registered_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 last_seen_at INTEGER NOT NULL
             );
             CREATE TABLE challenges (
                 challenge_id TEXT PRIMARY KEY,
                 node_id TEXT NOT NULL,
                 nonce TEXT NOT NULL,
                 expires_at INTEGER NOT NULL,
                 used_at INTEGER,
                 FOREIGN KEY(node_id) REFERENCES nodes(node_id)
             );",
        )
        .unwrap();
        let descriptor = test_descriptor("node-legacy", "http://provider:8080");
        conn.execute(
            "INSERT INTO nodes (node_id, pubkey, descriptor_json, status, registered_at, updated_at, last_seen_at)
             VALUES (?1, ?2, ?3, 'active', 1, 1, 1)",
            params![
                descriptor.node_id,
                descriptor.pubkey,
                serde_json::to_string(&descriptor).unwrap()
            ],
        )
        .unwrap();

        configure_discovery_connection(&conn).unwrap();

        let claims: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT clearnet_url, onion_url FROM nodes WHERE node_id = 'node-legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(claims, (None, None));
    }

    #[test]
    fn register_replaces_stale_endpoint_claim_with_new_owner() {
        let conn = Connection::open_in_memory().unwrap();
        configure_discovery_connection(&conn).unwrap();
        let stale_descriptor = test_descriptor("node-stale", "https://provider-live.example.test");
        insert_node(&conn, &stale_descriptor, "active", 10, 10, 10);

        let fresh_descriptor = test_descriptor("node-fresh", "https://provider-live.example.test");
        let fresh_json = serde_json::to_string(&fresh_descriptor).unwrap();
        let outcome = register_node_record(&conn, &fresh_descriptor, &fresh_json, 100, 30).unwrap();
        assert!(matches!(outcome, RegisterOutcome::Registered));

        let nodes = load_search_records(&conn, 10, true, 100, 30).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].descriptor.node_id, "node-fresh");
        assert_eq!(
            nodes[0].descriptor.transports.clearnet_url.as_deref(),
            Some("https://provider-live.example.test")
        );
    }

    #[test]
    fn register_rejects_active_endpoint_conflict() {
        let conn = Connection::open_in_memory().unwrap();
        configure_discovery_connection(&conn).unwrap();
        let active_descriptor =
            test_descriptor("node-active", "https://provider-conflict.example.test");
        insert_node(&conn, &active_descriptor, "active", 95, 95, 95);

        let new_descriptor =
            test_descriptor("node-other", "https://provider-conflict.example.test");
        let new_json = serde_json::to_string(&new_descriptor).unwrap();
        let outcome = register_node_record(&conn, &new_descriptor, &new_json, 100, 30).unwrap();
        assert!(matches!(
            outcome,
            RegisterOutcome::EndpointInUse { node_id, endpoint }
                if node_id == "node-active" && endpoint == "https://provider-conflict.example.test"
        ));

        let nodes = load_search_records(&conn, 10, true, 100, 30).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].descriptor.node_id, "node-active");
    }

    #[tokio::test]
    async fn register_validation_rejects_https_loopback_endpoints() {
        let descriptor = test_descriptor("node-loopback", "https://127.0.0.1:8080");

        let error = validated_endpoint_claims_from_descriptor(&descriptor)
            .await
            .expect_err("https loopback should be rejected");

        assert!(
            error.contains("private or local-network"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn register_validation_allows_plain_http_loopback_for_local_dev() {
        let descriptor = test_descriptor("node-loopback-http", "http://127.0.0.1:8080");

        let claims = validated_endpoint_claims_from_descriptor(&descriptor)
            .await
            .expect("loopback http should remain allowed for local dev");

        assert_eq!(
            claims.clearnet_url.as_deref(),
            Some("http://127.0.0.1:8080")
        );
    }
}
