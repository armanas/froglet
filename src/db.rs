use rusqlite::{Connection, OptionalExtension, Result as SqlResult, params};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task;

use crate::{api::NodeEventEnvelope, jobs, pricing::ServiceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservePaymentTokenOutcome {
    Reserved,
    InUse,
    Replay,
}

fn configure_connection(conn: &Connection) -> SqlResult<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA temp_store = MEMORY;
        BEGIN;
        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            pubkey TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            sig TEXT NOT NULL,
            tags TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_kind_created_at ON events (kind, created_at DESC);

        CREATE TABLE IF NOT EXISTS payment_redemptions (
            token_hash TEXT PRIMARY KEY,
            service_id TEXT NOT NULL,
            amount_sats INTEGER NOT NULL,
            redeemed_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS payment_tokens (
            token_hash TEXT PRIMARY KEY,
            service_id TEXT NOT NULL,
            amount_sats INTEGER NOT NULL,
            state TEXT NOT NULL,
            request_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_payment_tokens_state_updated_at ON payment_tokens (state, updated_at);
        CREATE TABLE IF NOT EXISTS jobs (
            job_id TEXT PRIMARY KEY,
            idempotency_key TEXT UNIQUE,
            request_hash TEXT NOT NULL,
            service_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            status TEXT NOT NULL,
            result_json TEXT,
            error TEXT,
            payment_token_hash TEXT,
            payment_amount_sats INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_jobs_status_updated_at ON jobs (status, updated_at DESC);
        COMMIT;",
    )?;
    Ok(())
}

pub fn initialize_db(db_path: &Path) -> SqlResult<Connection> {
    let conn = Connection::open(db_path)?;
    configure_connection(&conn)?;
    Ok(conn)
}

pub fn initialize_db_for_connection(conn: &Connection) -> SqlResult<()> {
    configure_connection(conn)
}

#[derive(Clone)]
pub struct DbPool {
    inner: Arc<Mutex<Connection>>,
}

impl DbPool {
    pub fn new(conn: Connection) -> Self {
        Self {
            inner: Arc::new(Mutex::new(conn)),
        }
    }

    pub async fn with_conn<F, R, E>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&Connection) -> std::result::Result<R, E> + Send + 'static,
        R: Send + 'static,
        E: ToString + Send + 'static,
    {
        let inner = self.inner.clone();
        task::spawn_blocking(move || {
            let conn = inner
                .lock()
                .map_err(|e| format!("database mutex poisoned: {e}"))?;
            f(&conn).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("database task join error: {e}"))?
    }
}

pub fn insert_event(conn: &Connection, event: &NodeEventEnvelope) -> SqlResult<()> {
    let tags_json = serde_json::to_string(&event.tags).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "INSERT OR IGNORE INTO events (id, pubkey, created_at, kind, content, sig, tags)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            event.id,
            event.pubkey,
            event.created_at,
            event.kind,
            event.content,
            event.sig,
            tags_json
        ],
    )?;

    Ok(())
}

pub fn query_events_by_kind(
    conn: &Connection,
    kinds: &[String],
    limit: Option<usize>,
) -> SqlResult<Vec<NodeEventEnvelope>> {
    if kinds.is_empty() {
        return Ok(vec![]);
    }

    let placeholders: Vec<String> = kinds.iter().map(|_| "?".to_string()).collect();
    let placeholders_str = placeholders.join(",");
    let limit_clamped = limit.unwrap_or(100).min(500);

    let query = format!(
        "SELECT id, pubkey, created_at, kind, content, sig, tags FROM events WHERE kind IN ({}) ORDER BY created_at DESC LIMIT ?",
        placeholders_str
    );

    let mut stmt = conn.prepare(&query)?;

    let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::new();
    for k in kinds {
        params_vec.push(k as &dyn rusqlite::ToSql);
    }
    let limit_i64 = limit_clamped as i64;
    params_vec.push(&limit_i64 as &dyn rusqlite::ToSql);

    let event_iter = stmt.query_map(&*params_vec, |row| {
        let tags_str: String = row.get(6)?;
        let tags: Vec<Vec<String>> = serde_json::from_str(&tags_str).unwrap_or_default();

        Ok(NodeEventEnvelope {
            id: row.get(0)?,
            pubkey: row.get(1)?,
            created_at: row.get(2)?,
            kind: row.get(3)?,
            content: row.get(4)?,
            sig: row.get(5)?,
            tags,
        })
    })?;

    let mut events = Vec::new();
    for event in event_iter {
        events.push(event?);
    }

    Ok(events)
}

pub fn try_record_payment_redemption(
    conn: &Connection,
    token_hash: &str,
    service_id: ServiceId,
    amount_sats: u64,
    redeemed_at: i64,
) -> SqlResult<bool> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO payment_redemptions (token_hash, service_id, amount_sats, redeemed_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            token_hash,
            service_id.as_str(),
            amount_sats as i64,
            redeemed_at
        ],
    )?;

    Ok(inserted > 0)
}

pub fn reserve_payment_token(
    conn: &Connection,
    token_hash: &str,
    service_id: ServiceId,
    amount_sats: u64,
    request_id: &str,
    now: i64,
) -> Result<ReservePaymentTokenOutcome, String> {
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO payment_tokens (
                token_hash,
                service_id,
                amount_sats,
                state,
                request_id,
                created_at,
                updated_at
             ) VALUES (?1, ?2, ?3, 'reserved', ?4, ?5, ?5)",
            params![
                token_hash,
                service_id.as_str(),
                amount_sats as i64,
                request_id,
                now
            ],
        )
        .map_err(|e| e.to_string())?;

    if inserted > 0 {
        return Ok(ReservePaymentTokenOutcome::Reserved);
    }

    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT state, request_id FROM payment_tokens WHERE token_hash = ?1",
            params![token_hash],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    match row {
        Some((state, existing_request_id))
            if state == "reserved" && existing_request_id == request_id =>
        {
            Ok(ReservePaymentTokenOutcome::Reserved)
        }
        Some((state, _)) if state == "reserved" => Ok(ReservePaymentTokenOutcome::InUse),
        Some(_) => Ok(ReservePaymentTokenOutcome::Replay),
        None => Err("payment token disappeared during reservation".to_string()),
    }
}

pub fn commit_payment_token(
    conn: &Connection,
    token_hash: &str,
    request_id: &str,
    committed_at: i64,
) -> Result<bool, String> {
    let updated = conn
        .execute(
            "UPDATE payment_tokens
             SET state = 'committed', updated_at = ?3
             WHERE token_hash = ?1 AND request_id = ?2 AND state = 'reserved'",
            params![token_hash, request_id, committed_at],
        )
        .map_err(|e| e.to_string())?;

    if updated == 0 {
        return Ok(false);
    }

    conn.execute(
        "INSERT OR REPLACE INTO payment_redemptions (token_hash, service_id, amount_sats, redeemed_at)
         SELECT token_hash, service_id, amount_sats, ?2
         FROM payment_tokens
         WHERE token_hash = ?1",
        params![token_hash, committed_at],
    )
    .map_err(|e| e.to_string())?;

    Ok(true)
}

pub fn release_payment_token(
    conn: &Connection,
    token_hash: &str,
    request_id: &str,
) -> Result<bool, String> {
    let deleted = conn
        .execute(
            "DELETE FROM payment_tokens
             WHERE token_hash = ?1 AND request_id = ?2 AND state = 'reserved'",
            params![token_hash, request_id],
        )
        .map_err(|e| e.to_string())?;

    Ok(deleted > 0)
}

pub fn recover_runtime_state(conn: &Connection, now: i64) -> Result<(), String> {
    conn.execute("DELETE FROM payment_tokens WHERE state = 'reserved'", [])
        .map_err(|e| e.to_string())?;
    jobs::fail_incomplete_jobs(conn, "node restarted before job completion", now)?;
    Ok(())
}
