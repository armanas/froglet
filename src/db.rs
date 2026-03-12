use rusqlite::{Connection, OptionalExtension, Result as SqlResult, params};
use serde::Serialize;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task;

use crate::{
    api::NodeEventEnvelope,
    canonical_json, crypto, deals, jobs,
    pricing::ServiceId,
    protocol::{InvoiceBundleLegState, InvoiceBundlePayload, SignedArtifact},
};

const PAYMENT_TOKEN_STATE_RESERVED: &str = "reserved";
const PAYMENT_TOKEN_STATE_COMMITTED: &str = "committed";
const PAYMENT_TOKEN_STATE_RELEASED: &str = "released";
const PAYMENT_TOKEN_STATE_EXPIRED: &str = "expired";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservePaymentTokenOutcome {
    Reserved,
    InUse,
    Replay,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LedgerArtifact {
    pub cursor: i64,
    pub hash: String,
    pub payload_hash: String,
    pub kind: String,
    pub actor_id: String,
    pub created_at: i64,
    pub document: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArtifactDocumentRecord {
    pub artifact_hash: String,
    pub payload_hash: String,
    pub artifact_kind: String,
    pub actor_id: String,
    pub created_at: i64,
    pub document: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArtifactFeedEntryRecord {
    pub sequence: i64,
    pub artifact_hash: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionEvidenceRecord {
    pub evidence_id: i64,
    pub subject_kind: String,
    pub subject_id: String,
    pub evidence_kind: String,
    pub content_hash: String,
    pub created_at: i64,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LightningInvoiceBundleRecord {
    pub session_id: String,
    pub bundle: SignedArtifact<InvoiceBundlePayload>,
    pub base_state: InvoiceBundleLegState,
    pub success_state: InvoiceBundleLegState,
    pub created_at: i64,
    pub updated_at: i64,
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
        CREATE TABLE IF NOT EXISTS artifacts (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            artifact_hash TEXT NOT NULL UNIQUE,
            payload_hash TEXT NOT NULL,
            artifact_kind TEXT NOT NULL,
            actor_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            document_json TEXT NOT NULL,
            UNIQUE (actor_id, artifact_kind, payload_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_artifacts_sequence ON artifacts (sequence ASC);
        CREATE INDEX IF NOT EXISTS idx_artifacts_actor_kind_created_at ON artifacts (actor_id, artifact_kind, created_at DESC);
        CREATE TABLE IF NOT EXISTS artifact_documents (
            artifact_hash TEXT PRIMARY KEY,
            payload_hash TEXT NOT NULL,
            artifact_kind TEXT NOT NULL,
            actor_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            document_json TEXT NOT NULL,
            UNIQUE (actor_id, artifact_kind, payload_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_artifact_documents_actor_kind_created_at ON artifact_documents (actor_id, artifact_kind, created_at DESC);
        CREATE TABLE IF NOT EXISTS artifact_feed (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            artifact_hash TEXT NOT NULL UNIQUE,
            observed_at INTEGER NOT NULL,
            FOREIGN KEY (artifact_hash) REFERENCES artifact_documents(artifact_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_artifact_feed_sequence ON artifact_feed (sequence ASC);
        CREATE TABLE IF NOT EXISTS execution_evidence (
            evidence_id INTEGER PRIMARY KEY AUTOINCREMENT,
            subject_kind TEXT NOT NULL,
            subject_id TEXT NOT NULL,
            evidence_kind TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            content_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            UNIQUE (subject_kind, subject_id, evidence_kind, content_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_execution_evidence_subject ON execution_evidence (subject_kind, subject_id, evidence_id ASC);
        CREATE TABLE IF NOT EXISTS lightning_invoice_bundles (
            session_id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            requester_id TEXT NOT NULL,
            quote_hash TEXT NOT NULL,
            deal_hash TEXT NOT NULL,
            destination_identity TEXT NOT NULL,
            base_invoice_hash TEXT NOT NULL,
            base_payment_hash TEXT NOT NULL,
            base_fee_msat INTEGER NOT NULL,
            base_state TEXT NOT NULL,
            success_invoice_hash TEXT NOT NULL,
            success_payment_hash TEXT NOT NULL,
            success_fee_msat INTEGER NOT NULL,
            success_state TEXT NOT NULL,
            bundle_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_lightning_invoice_bundles_quote_hash ON lightning_invoice_bundles (quote_hash);
        CREATE INDEX IF NOT EXISTS idx_lightning_invoice_bundles_deal_hash ON lightning_invoice_bundles (deal_hash);
        CREATE TABLE IF NOT EXISTS quotes (
            quote_id TEXT PRIMARY KEY,
            artifact_hash TEXT NOT NULL UNIQUE,
            offer_id TEXT NOT NULL,
            service_id TEXT NOT NULL,
            workload_hash TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            price_sats INTEGER NOT NULL,
            quote_json TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_quotes_offer_expires_at ON quotes (offer_id, expires_at DESC);
        CREATE TABLE IF NOT EXISTS deals (
            deal_id TEXT PRIMARY KEY,
            idempotency_key TEXT UNIQUE,
            quote_id TEXT NOT NULL,
            quote_hash TEXT NOT NULL,
            offer_id TEXT NOT NULL,
            service_id TEXT NOT NULL,
            workload_hash TEXT NOT NULL,
            spec_json TEXT NOT NULL,
            quote_json TEXT NOT NULL,
            deal_artifact_json TEXT NOT NULL,
            status TEXT NOT NULL,
            result_json TEXT,
            result_hash TEXT,
            error TEXT,
            payment_method TEXT,
            payment_token_hash TEXT,
            payment_amount_sats INTEGER,
            receipt_artifact_json TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_deals_status_updated_at ON deals (status, updated_at DESC);
        COMMIT;",
    )?;
    ensure_column(&conn, "jobs", "workload_evidence_hash", "TEXT")?;
    ensure_column(&conn, "jobs", "result_evidence_hash", "TEXT")?;
    ensure_column(&conn, "jobs", "failure_evidence_hash", "TEXT")?;
    ensure_column(&conn, "deals", "workload_evidence_hash", "TEXT")?;
    ensure_column(&conn, "deals", "deal_artifact_hash", "TEXT")?;
    ensure_column(&conn, "deals", "result_evidence_hash", "TEXT")?;
    ensure_column(&conn, "deals", "failure_evidence_hash", "TEXT")?;
    ensure_column(&conn, "deals", "receipt_artifact_hash", "TEXT")?;
    ensure_column(&conn, "deals", "payment_method", "TEXT")?;
    migrate_legacy_artifacts(&conn)?;
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

    let reclaimed = conn
        .execute(
            "UPDATE payment_tokens
             SET service_id = ?2,
                 amount_sats = ?3,
                 state = ?4,
                 request_id = ?5,
                 updated_at = ?6
             WHERE token_hash = ?1 AND state IN (?7, ?8)",
            params![
                token_hash,
                service_id.as_str(),
                amount_sats as i64,
                PAYMENT_TOKEN_STATE_RESERVED,
                request_id,
                now,
                PAYMENT_TOKEN_STATE_RELEASED,
                PAYMENT_TOKEN_STATE_EXPIRED,
            ],
        )
        .map_err(|e| e.to_string())?;

    if reclaimed > 0 {
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
            if state == PAYMENT_TOKEN_STATE_RESERVED && existing_request_id == request_id =>
        {
            Ok(ReservePaymentTokenOutcome::Reserved)
        }
        Some((state, _)) if state == PAYMENT_TOKEN_STATE_RESERVED => {
            Ok(ReservePaymentTokenOutcome::InUse)
        }
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
             SET state = ?3, updated_at = ?4
             WHERE token_hash = ?1 AND request_id = ?2 AND state = ?5",
            params![
                token_hash,
                request_id,
                PAYMENT_TOKEN_STATE_COMMITTED,
                committed_at,
                PAYMENT_TOKEN_STATE_RESERVED
            ],
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
    released_at: i64,
) -> Result<bool, String> {
    let updated = conn
        .execute(
            "UPDATE payment_tokens
             SET state = ?3, updated_at = ?4
             WHERE token_hash = ?1 AND request_id = ?2 AND state = ?5",
            params![
                token_hash,
                request_id,
                PAYMENT_TOKEN_STATE_RELEASED,
                released_at,
                PAYMENT_TOKEN_STATE_RESERVED
            ],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

pub fn expire_reserved_payment_tokens(conn: &Connection, expired_at: i64) -> Result<usize, String> {
    conn.execute(
        "UPDATE payment_tokens
         SET state = ?1, updated_at = ?2
         WHERE state = ?3",
        params![
            PAYMENT_TOKEN_STATE_EXPIRED,
            expired_at,
            PAYMENT_TOKEN_STATE_RESERVED
        ],
    )
    .map_err(|e| e.to_string())
}

pub fn recover_runtime_state(conn: &Connection, now: i64) -> Result<(), String> {
    let _ = expire_reserved_payment_tokens(conn, now)?;
    jobs::fail_incomplete_jobs(conn, "node restarted before job completion", now)?;
    deals::fail_incomplete_deals(conn, "node restarted before deal completion", now)?;
    Ok(())
}

fn ensure_column(
    conn: &Connection,
    table_name: &str,
    column_name: &str,
    column_definition: &str,
) -> SqlResult<()> {
    let pragma = format!("PRAGMA table_info({table_name})");
    let mut stmt = conn.prepare(&pragma)?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for column in columns {
        if column? == column_name {
            return Ok(());
        }
    }

    let alter = format!("ALTER TABLE {table_name} ADD COLUMN {column_name} {column_definition}");
    conn.execute(&alter, [])?;
    Ok(())
}

fn migrate_legacy_artifacts(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "INSERT OR IGNORE INTO artifact_documents (
            artifact_hash,
            payload_hash,
            artifact_kind,
            actor_id,
            created_at,
            document_json
         )
         SELECT
            artifact_hash,
            payload_hash,
            artifact_kind,
            actor_id,
            created_at,
            document_json
         FROM artifacts;

         INSERT OR IGNORE INTO artifact_feed (
            sequence,
            artifact_hash,
            observed_at
         )
         SELECT
            sequence,
            artifact_hash,
            created_at
         FROM artifacts
         ORDER BY sequence ASC;",
    )?;
    Ok(())
}

pub fn insert_artifact_document(
    conn: &Connection,
    artifact_hash: &str,
    payload_hash: &str,
    kind: &str,
    actor_id: &str,
    created_at: i64,
    document_json: &str,
) -> Result<(), String> {
    let mut stored_artifact_hash = artifact_hash.to_string();
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO artifact_documents (
            artifact_hash,
            payload_hash,
            artifact_kind,
            actor_id,
            created_at,
            document_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                artifact_hash,
                payload_hash,
                kind,
                actor_id,
                created_at,
                document_json
            ],
        )
        .map_err(|e| e.to_string())?;

    if inserted == 0 {
        let existing_hash: Option<String> = conn
            .query_row(
                "SELECT artifact_hash
                 FROM artifact_documents
                 WHERE actor_id = ?1 AND artifact_kind = ?2 AND payload_hash = ?3",
                params![actor_id, kind, payload_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        match existing_hash {
            Some(existing_hash) if existing_hash == artifact_hash => {}
            Some(existing_hash) => {
                stored_artifact_hash = existing_hash;
            }
            None => {
                let by_hash: Option<String> = conn
                    .query_row(
                        "SELECT artifact_hash
                         FROM artifact_documents
                         WHERE artifact_hash = ?1",
                        params![artifact_hash],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                if by_hash.is_none() {
                    return Err("artifact document insert was ignored unexpectedly".to_string());
                }
            }
        }
    }

    conn.execute(
        "INSERT OR IGNORE INTO artifact_feed (
            artifact_hash,
            observed_at
         ) VALUES (?1, ?2)",
        params![stored_artifact_hash, created_at],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn get_artifact_by_actor_kind_payload(
    conn: &Connection,
    actor_id: &str,
    kind: &str,
    payload_hash: &str,
) -> Result<Option<LedgerArtifact>, String> {
    conn.query_row(
        "SELECT
            f.sequence,
            d.artifact_hash,
            d.payload_hash,
            d.artifact_kind,
            d.actor_id,
            d.created_at,
            d.document_json
         FROM artifact_documents d
         JOIN artifact_feed f ON f.artifact_hash = d.artifact_hash
         WHERE d.actor_id = ?1 AND d.artifact_kind = ?2 AND d.payload_hash = ?3",
        params![actor_id, kind, payload_hash],
        decode_artifact_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn get_artifact_by_hash(
    conn: &Connection,
    artifact_hash: &str,
) -> Result<Option<LedgerArtifact>, String> {
    conn.query_row(
        "SELECT
            f.sequence,
            d.artifact_hash,
            d.payload_hash,
            d.artifact_kind,
            d.actor_id,
            d.created_at,
            d.document_json
         FROM artifact_documents d
         JOIN artifact_feed f ON f.artifact_hash = d.artifact_hash
         WHERE d.artifact_hash = ?1",
        params![artifact_hash],
        decode_artifact_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn get_artifact_document_by_hash(
    conn: &Connection,
    artifact_hash: &str,
) -> Result<Option<ArtifactDocumentRecord>, String> {
    conn.query_row(
        "SELECT
            artifact_hash,
            payload_hash,
            artifact_kind,
            actor_id,
            created_at,
            document_json
         FROM artifact_documents
         WHERE artifact_hash = ?1",
        params![artifact_hash],
        decode_artifact_document_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn get_artifact_feed_entry_by_hash(
    conn: &Connection,
    artifact_hash: &str,
) -> Result<Option<ArtifactFeedEntryRecord>, String> {
    conn.query_row(
        "SELECT sequence, artifact_hash, observed_at
         FROM artifact_feed
         WHERE artifact_hash = ?1",
        params![artifact_hash],
        |row| {
            Ok(ArtifactFeedEntryRecord {
                sequence: row.get(0)?,
                artifact_hash: row.get(1)?,
                observed_at: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn list_artifacts(
    conn: &Connection,
    cursor: Option<i64>,
    limit: usize,
) -> Result<(Vec<LedgerArtifact>, bool), String> {
    let limit = limit.min(100).max(1) as i64;
    let cursor = cursor.unwrap_or(0);
    let mut stmt = conn
        .prepare(
            "SELECT
                f.sequence,
                d.artifact_hash,
                d.payload_hash,
                d.artifact_kind,
                d.actor_id,
                d.created_at,
                d.document_json
             FROM artifact_feed f
             JOIN artifact_documents d ON d.artifact_hash = f.artifact_hash
             WHERE f.sequence > ?1
             ORDER BY f.sequence ASC
             LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![cursor, limit + 1], decode_artifact_row)
        .map_err(|e| e.to_string())?;

    let mut artifacts = Vec::new();
    for row in rows {
        artifacts.push(row.map_err(|e| e.to_string())?);
    }

    let has_more = artifacts.len() as i64 > limit;
    if has_more {
        artifacts.truncate(limit as usize);
    }

    Ok((artifacts, has_more))
}

fn decode_artifact_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LedgerArtifact> {
    let document_json: String = row.get(6)?;
    let document = serde_json::from_str(&document_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(err))
    })?;

    Ok(LedgerArtifact {
        cursor: row.get(0)?,
        hash: row.get(1)?,
        payload_hash: row.get(2)?,
        kind: row.get(3)?,
        actor_id: row.get(4)?,
        created_at: row.get(5)?,
        document,
    })
}

fn decode_artifact_document_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ArtifactDocumentRecord> {
    let document_json: String = row.get(5)?;
    let document = serde_json::from_str(&document_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(err))
    })?;

    Ok(ArtifactDocumentRecord {
        artifact_hash: row.get(0)?,
        payload_hash: row.get(1)?,
        artifact_kind: row.get(2)?,
        actor_id: row.get(3)?,
        created_at: row.get(4)?,
        document,
    })
}

pub fn insert_lightning_invoice_bundle(
    conn: &Connection,
    session_id: &str,
    bundle: &SignedArtifact<InvoiceBundlePayload>,
    base_state: InvoiceBundleLegState,
    success_state: InvoiceBundleLegState,
    created_at: i64,
) -> Result<(), String> {
    let bundle_json = serde_json::to_string(bundle).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO lightning_invoice_bundles (
            session_id,
            provider_id,
            requester_id,
            quote_hash,
            deal_hash,
            destination_identity,
            base_invoice_hash,
            base_payment_hash,
            base_fee_msat,
            base_state,
            success_invoice_hash,
            success_payment_hash,
            success_fee_msat,
            success_state,
            bundle_json,
            created_at,
            updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
        params![
            session_id,
            &bundle.payload.provider_id,
            &bundle.payload.requester_id,
            &bundle.payload.quote_hash,
            &bundle.payload.deal_hash,
            &bundle.payload.destination_identity,
            &bundle.payload.base_invoice.invoice_hash,
            &bundle.payload.base_invoice.payment_hash,
            bundle.payload.base_invoice.amount_msat as i64,
            invoice_leg_state_str(base_state),
            &bundle.payload.success_hold_invoice.invoice_hash,
            &bundle.payload.success_hold_invoice.payment_hash,
            bundle.payload.success_hold_invoice.amount_msat as i64,
            invoice_leg_state_str(success_state),
            &bundle_json,
            created_at,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn get_lightning_invoice_bundle(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<LightningInvoiceBundleRecord>, String> {
    conn.query_row(
        "SELECT session_id, bundle_json, base_state, success_state, created_at, updated_at
         FROM lightning_invoice_bundles
         WHERE session_id = ?1",
        params![session_id],
        decode_lightning_invoice_bundle_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn get_lightning_invoice_bundle_by_deal_hash(
    conn: &Connection,
    deal_hash: &str,
) -> Result<Option<LightningInvoiceBundleRecord>, String> {
    conn.query_row(
        "SELECT session_id, bundle_json, base_state, success_state, created_at, updated_at
         FROM lightning_invoice_bundles
         WHERE deal_hash = ?1",
        params![deal_hash],
        decode_lightning_invoice_bundle_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn update_lightning_invoice_bundle_states(
    conn: &Connection,
    session_id: &str,
    base_state: InvoiceBundleLegState,
    success_state: InvoiceBundleLegState,
    updated_at: i64,
) -> Result<bool, String> {
    let updated = conn
        .execute(
            "UPDATE lightning_invoice_bundles
             SET base_state = ?2,
                 success_state = ?3,
                 updated_at = ?4
             WHERE session_id = ?1",
            params![
                session_id,
                invoice_leg_state_str(base_state),
                invoice_leg_state_str(success_state),
                updated_at
            ],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

fn invoice_leg_state_str(state: InvoiceBundleLegState) -> &'static str {
    match state {
        InvoiceBundleLegState::Open => "open",
        InvoiceBundleLegState::Accepted => "accepted",
        InvoiceBundleLegState::Settled => "settled",
        InvoiceBundleLegState::Canceled => "canceled",
        InvoiceBundleLegState::Expired => "expired",
    }
}

fn parse_invoice_leg_state(value: &str, column: usize) -> rusqlite::Result<InvoiceBundleLegState> {
    match value {
        "open" => Ok(InvoiceBundleLegState::Open),
        "accepted" => Ok(InvoiceBundleLegState::Accepted),
        "settled" => Ok(InvoiceBundleLegState::Settled),
        "canceled" => Ok(InvoiceBundleLegState::Canceled),
        "expired" => Ok(InvoiceBundleLegState::Expired),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            format!("invalid invoice leg state: {value}").into(),
        )),
    }
}

fn decode_lightning_invoice_bundle_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LightningInvoiceBundleRecord> {
    let bundle_json: String = row.get(1)?;
    let bundle: SignedArtifact<InvoiceBundlePayload> =
        serde_json::from_str(&bundle_json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
        })?;
    let base_state: String = row.get(2)?;
    let success_state: String = row.get(3)?;
    let parsed_base_state = parse_invoice_leg_state(&base_state, 2)?;
    let parsed_success_state = parse_invoice_leg_state(&success_state, 3)?;

    Ok(LightningInvoiceBundleRecord {
        session_id: row.get(0)?,
        bundle,
        base_state: parsed_base_state,
        success_state: parsed_success_state,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

pub fn insert_execution_evidence<T: Serialize>(
    conn: &Connection,
    subject_kind: &str,
    subject_id: &str,
    evidence_kind: &str,
    content: &T,
    created_at: i64,
) -> Result<String, String> {
    let content_json = canonical_json::to_string(content).map_err(|e| e.to_string())?;
    let content_hash = crypto::sha256_hex(content_json.as_bytes());

    conn.execute(
        "INSERT OR IGNORE INTO execution_evidence (
            subject_kind,
            subject_id,
            evidence_kind,
            content_hash,
            content_json,
            created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            subject_kind,
            subject_id,
            evidence_kind,
            &content_hash,
            &content_json,
            created_at
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(content_hash)
}

pub fn list_execution_evidence_for_subject(
    conn: &Connection,
    subject_kind: &str,
    subject_id: &str,
) -> Result<Vec<ExecutionEvidenceRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT
                evidence_id,
                subject_kind,
                subject_id,
                evidence_kind,
                content_hash,
                created_at,
                content_json
             FROM execution_evidence
             WHERE subject_kind = ?1 AND subject_id = ?2
             ORDER BY evidence_id ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![subject_kind, subject_id], |row| {
            let content_json: String = row.get(6)?;
            let content = serde_json::from_str(&content_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;

            Ok(ExecutionEvidenceRecord {
                evidence_id: row.get(0)?,
                subject_kind: row.get(1)?,
                subject_id: row.get(2)?,
                evidence_kind: row.get(3)?,
                content_hash: row.get(4)?,
                created_at: row.get(5)?,
                content,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut evidence = Vec::new();
    for row in rows {
        evidence.push(row.map_err(|e| e.to_string())?);
    }

    Ok(evidence)
}
