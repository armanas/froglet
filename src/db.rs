use rusqlite::{params, Connection, Result};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task;

use crate::{api::NodeEventEnvelope, pricing::ServiceId};

fn configure_connection(conn: &Connection) -> Result<()> {
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
        COMMIT;",
    )?;
    Ok(())
}

pub fn initialize_db(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    configure_connection(&conn)?;
    Ok(conn)
}

pub fn initialize_db_for_connection(conn: &Connection) -> Result<()> {
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

    pub async fn with_conn<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
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

pub fn insert_event(conn: &Connection, event: &NodeEventEnvelope) -> Result<()> {
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
) -> Result<Vec<NodeEventEnvelope>> {
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
) -> Result<bool> {
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
