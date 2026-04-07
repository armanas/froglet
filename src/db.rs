use rusqlite::{Connection, OptionalExtension, Result as SqlResult, params};
use serde::Serialize;
use std::{
    fs,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::task;

use crate::{
    api::NodeEventEnvelope,
    canonical_json, crypto,
    protocol::{InvoiceBundleLegState, InvoiceBundlePayload, SignedArtifact},
};

const LEGACY_ARTIFACTS_MIGRATION: &str = "20260313_legacy_artifacts_backfill";
const DEFAULT_DB_READ_CONNECTIONS: usize = 4;
pub const MAX_EVENT_QUERY_KINDS: usize = 100;

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DealSettlementMaterializationRecord {
    pub deal_id: String,
    pub materialization_kind: String,
    pub request_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderManagedOfferRecord {
    pub offer_id: String,
    pub definition: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct WalCheckpointMetrics {
    pub wal_size_bytes: u64,
    pub busy: i64,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
    pub duration_ms: u128,
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
        CREATE TABLE IF NOT EXISTS schema_migrations (
            name TEXT PRIMARY KEY,
            applied_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );
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
         CREATE TABLE IF NOT EXISTS requester_deals (
            deal_id TEXT PRIMARY KEY,
            idempotency_key TEXT UNIQUE,
            provider_id TEXT NOT NULL,
            provider_url TEXT NOT NULL,
            spec_json TEXT NOT NULL,
            quote_json TEXT NOT NULL,
            deal_artifact_json TEXT NOT NULL,
            status TEXT NOT NULL,
            result_json TEXT,
            result_hash TEXT,
            error TEXT,
            receipt_artifact_json TEXT,
            success_preimage TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_requester_deals_status_updated_at
            ON requester_deals (status, updated_at DESC);
        CREATE TABLE IF NOT EXISTS provider_managed_offers (
            offer_id TEXT PRIMARY KEY,
            definition_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_provider_managed_offers_updated_at
            ON provider_managed_offers (updated_at DESC);
        COMMIT;",
    )?;
    ensure_column(conn, "jobs", "workload_evidence_hash", "TEXT")?;
    ensure_column(conn, "jobs", "result_evidence_hash", "TEXT")?;
    ensure_column(conn, "jobs", "failure_evidence_hash", "TEXT")?;
    ensure_column(conn, "deals", "workload_evidence_hash", "TEXT")?;
    ensure_column(conn, "deals", "deal_artifact_hash", "TEXT")?;
    ensure_column(conn, "deals", "result_evidence_hash", "TEXT")?;
    ensure_column(conn, "deals", "failure_evidence_hash", "TEXT")?;
    ensure_column(conn, "deals", "receipt_artifact_hash", "TEXT")?;
    ensure_column(conn, "deals", "payment_method", "TEXT")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_execution_evidence_content_hash
            ON execution_evidence (content_hash);
         CREATE INDEX IF NOT EXISTS idx_deals_payment_method_status_created_at
            ON deals (payment_method, status, created_at);
         CREATE INDEX IF NOT EXISTS idx_deals_deal_artifact_hash
            ON deals (deal_artifact_hash);
         CREATE TABLE IF NOT EXISTS deal_settlement_materializations (
            deal_id TEXT PRIMARY KEY,
            materialization_kind TEXT NOT NULL,
            request_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (deal_id) REFERENCES deals(deal_id)
         );
         CREATE INDEX IF NOT EXISTS idx_deal_settlement_materializations_updated_at
            ON deal_settlement_materializations (updated_at ASC);",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS deal_quarantine (
            quarantine_id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_rowid INTEGER NOT NULL,
            deal_id TEXT,
            status TEXT,
            reason TEXT NOT NULL,
            snapshot_json TEXT NOT NULL,
            quarantined_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_deal_quarantine_quarantined_at
            ON deal_quarantine (quarantined_at DESC);",
    )?;
    apply_migration_once(conn, LEGACY_ARTIFACTS_MIGRATION, migrate_legacy_artifacts)?;
    Ok(())
}

pub fn initialize_db(db_path: &Path) -> SqlResult<Connection> {
    let conn = Connection::open(db_path)?;
    configure_connection(&conn)?;
    Ok(conn)
}

pub fn initialize_db_reader(db_path: &Path) -> SqlResult<Connection> {
    let conn = Connection::open(db_path)?;
    configure_connection(&conn)?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    Ok(conn)
}

pub fn initialize_db_for_connection(conn: &Connection) -> SqlResult<()> {
    configure_connection(conn)
}

#[derive(Clone)]
pub struct DbPool {
    write: Arc<Mutex<Connection>>,
    readers: Arc<Vec<Arc<Mutex<Connection>>>>,
    next_reader: Arc<AtomicUsize>,
}

impl DbPool {
    pub fn new(conn: Connection) -> Self {
        Self {
            write: Arc::new(Mutex::new(conn)),
            readers: Arc::new(Vec::new()),
            next_reader: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn open(db_path: &Path) -> SqlResult<Self> {
        Self::open_with(db_path, initialize_db, initialize_db_reader)
    }

    pub fn open_with(
        db_path: &Path,
        init_write: fn(&Path) -> SqlResult<Connection>,
        init_read: fn(&Path) -> SqlResult<Connection>,
    ) -> SqlResult<Self> {
        let write = init_write(db_path)?;
        let reader_count = db_read_connection_count();
        let mut readers = Vec::with_capacity(reader_count);
        for _ in 0..reader_count {
            readers.push(Arc::new(Mutex::new(init_read(db_path)?)));
        }
        Ok(Self {
            write: Arc::new(Mutex::new(write)),
            readers: Arc::new(readers),
            next_reader: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn read_connection_count(&self) -> usize {
        self.readers.len().max(1)
    }

    pub async fn with_read_conn<F, R, E>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&Connection) -> std::result::Result<R, E> + Send + 'static,
        R: Send + 'static,
        E: ToString + Send + 'static,
    {
        let connection = self.next_read_connection();
        Self::run_locked_connection(connection, f).await
    }

    pub async fn with_write_conn<F, R, E>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&Connection) -> std::result::Result<R, E> + Send + 'static,
        R: Send + 'static,
        E: ToString + Send + 'static,
    {
        Self::run_locked_connection(self.write.clone(), f).await
    }

    pub async fn with_conn<F, R, E>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&Connection) -> std::result::Result<R, E> + Send + 'static,
        R: Send + 'static,
        E: ToString + Send + 'static,
    {
        self.with_write_conn(f).await
    }

    fn next_read_connection(&self) -> Arc<Mutex<Connection>> {
        if self.readers.is_empty() {
            return self.write.clone();
        }

        let index = self.next_reader.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        self.readers[index].clone()
    }

    async fn run_locked_connection<F, R, E>(
        inner: Arc<Mutex<Connection>>,
        f: F,
    ) -> Result<R, String>
    where
        F: FnOnce(&Connection) -> std::result::Result<R, E> + Send + 'static,
        R: Send + 'static,
        E: ToString + Send + 'static,
    {
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

fn db_read_connection_count() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get().clamp(2, 8))
        .unwrap_or(DEFAULT_DB_READ_CONNECTIONS)
}

pub fn collect_wal_checkpoint_metrics(
    conn: &Connection,
    db_path: &Path,
) -> Result<WalCheckpointMetrics, String> {
    let wal_path = Path::new(&format!("{}-wal", db_path.display())).to_path_buf();
    let wal_size_bytes = fs::metadata(&wal_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let started_at = Instant::now();
    let (busy, log_frames, checkpointed_frames) = conn
        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|error| error.to_string())?;

    Ok(WalCheckpointMetrics {
        wal_size_bytes,
        busy,
        log_frames,
        checkpointed_frames,
        duration_ms: started_at.elapsed().as_millis(),
    })
}

pub fn insert_event(conn: &Connection, event: &NodeEventEnvelope) -> SqlResult<bool> {
    let tags_json = serde_json::to_string(&event.tags).unwrap_or_else(|_| "[]".to_string());

    let inserted = conn.execute(
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

    Ok(inserted > 0)
}

/// Insert multiple events in a single transaction, returning per-event results.
/// Each entry is `true` if the event was newly inserted, `false` if it already existed.
pub fn batch_insert_events(
    conn: &Connection,
    events: &[NodeEventEnvelope],
) -> SqlResult<Vec<bool>> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    if events.len() == 1 {
        return insert_event(conn, &events[0]).map(|ok| vec![ok]);
    }

    conn.execute_batch("BEGIN IMMEDIATE")?;
    let mut results = Vec::with_capacity(events.len());
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO events (id, pubkey, created_at, kind, content, sig, tags)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;

    for event in events {
        let tags_json = serde_json::to_string(&event.tags).unwrap_or_else(|_| "[]".to_string());
        match stmt.execute(params![
            event.id,
            event.pubkey,
            event.created_at,
            event.kind,
            event.content,
            event.sig,
            tags_json
        ]) {
            Ok(n) => results.push(n > 0),
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
    }

    conn.execute_batch("COMMIT")?;
    Ok(results)
}

pub fn query_events_by_kind(
    conn: &Connection,
    kinds: &[String],
    limit: Option<usize>,
) -> SqlResult<Vec<NodeEventEnvelope>> {
    if kinds.is_empty() {
        return Ok(vec![]);
    }
    if kinds.len() > MAX_EVENT_QUERY_KINDS {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "events query exceeds maximum of {MAX_EVENT_QUERY_KINDS} kinds"
        )));
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

fn ensure_column(
    conn: &Connection,
    table_name: &str,
    column_name: &str,
    column_definition: &str,
) -> SqlResult<()> {
    validate_sql_identifier("table name", table_name)?;
    validate_sql_identifier("column name", column_name)?;
    validate_column_definition(column_definition)?;

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

fn apply_migration_once<F>(conn: &Connection, name: &str, apply: F) -> SqlResult<()>
where
    F: FnOnce(&Connection) -> SqlResult<()>,
{
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let operation = (|| -> SqlResult<()> {
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (name) VALUES (?1)",
            params![name],
        )?;
        if inserted == 0 {
            return Ok(());
        }
        apply(conn)?;
        Ok(())
    })();

    if let Err(error) = operation {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(error);
    }

    conn.execute_batch("COMMIT")?;
    Ok(())
}

fn validate_sql_identifier(kind: &str, value: &str) -> SqlResult<()> {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "invalid {kind}: {value}"
            )));
        }
    }

    if chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidParameterName(format!(
            "invalid {kind}: {value}"
        )))
    }
}

fn validate_column_definition(column_definition: &str) -> SqlResult<()> {
    match column_definition {
        "TEXT" | "INTEGER" | "REAL" | "BLOB" => Ok(()),
        _ => Err(rusqlite::Error::InvalidParameterName(format!(
            "invalid column definition: {column_definition}"
        ))),
    }
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

pub fn get_latest_artifact_by_actor_kind(
    conn: &Connection,
    actor_id: &str,
    kind: &str,
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
         WHERE d.actor_id = ?1 AND d.artifact_kind = ?2
         ORDER BY d.created_at DESC, f.sequence DESC
         LIMIT 1",
        params![actor_id, kind],
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
    let limit = limit.clamp(1, 100) as i64;
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
            &bundle.payload.base_fee.invoice_hash,
            &bundle.payload.base_fee.payment_hash,
            bundle.payload.base_fee.amount_msat as i64,
            invoice_leg_state_str(base_state),
            &bundle.payload.success_fee.invoice_hash,
            &bundle.payload.success_fee.payment_hash,
            bundle.payload.success_fee.amount_msat as i64,
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

pub fn insert_deal_settlement_materialization(
    conn: &Connection,
    deal_id: &str,
    materialization_kind: &str,
    request_json: &str,
    created_at: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO deal_settlement_materializations (
            deal_id,
            materialization_kind,
            request_json,
            created_at,
            updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![deal_id, materialization_kind, request_json, created_at],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn get_deal_settlement_materialization(
    conn: &Connection,
    deal_id: &str,
) -> Result<Option<DealSettlementMaterializationRecord>, String> {
    conn.query_row(
        "SELECT deal_id, materialization_kind, request_json, created_at, updated_at
         FROM deal_settlement_materializations
         WHERE deal_id = ?1",
        params![deal_id],
        |row| {
            Ok(DealSettlementMaterializationRecord {
                deal_id: row.get(0)?,
                materialization_kind: row.get(1)?,
                request_json: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub fn list_deal_settlement_materializations(
    conn: &Connection,
) -> Result<Vec<DealSettlementMaterializationRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT deal_id, materialization_kind, request_json, created_at, updated_at
             FROM deal_settlement_materializations
             ORDER BY updated_at ASC, deal_id ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DealSettlementMaterializationRecord {
                deal_id: row.get(0)?,
                materialization_kind: row.get(1)?,
                request_json: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })
        .map_err(|error| error.to_string())?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|error| error.to_string())?);
    }
    Ok(records)
}

pub fn delete_deal_settlement_materialization(
    conn: &Connection,
    deal_id: &str,
) -> Result<bool, String> {
    let deleted = conn
        .execute(
            "DELETE FROM deal_settlement_materializations WHERE deal_id = ?1",
            params![deal_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(deleted > 0)
}

pub fn upsert_provider_managed_offer(
    conn: &Connection,
    offer_id: &str,
    definition_json: &str,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO provider_managed_offers (
            offer_id,
            definition_json,
            created_at,
            updated_at
         ) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(offer_id) DO UPDATE SET
            definition_json = excluded.definition_json,
            updated_at = excluded.updated_at",
        params![offer_id, definition_json, now],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn get_provider_managed_offer(
    conn: &Connection,
    offer_id: &str,
) -> Result<Option<ProviderManagedOfferRecord>, String> {
    conn.query_row(
        "SELECT offer_id, definition_json, created_at, updated_at
         FROM provider_managed_offers
         WHERE offer_id = ?1",
        params![offer_id],
        |row| {
            let definition_json: String = row.get(1)?;
            let definition = serde_json::from_str(&definition_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            Ok(ProviderManagedOfferRecord {
                offer_id: row.get(0)?,
                definition,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub fn list_provider_managed_offers(
    conn: &Connection,
) -> Result<Vec<ProviderManagedOfferRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT offer_id, definition_json, created_at, updated_at
             FROM provider_managed_offers
             ORDER BY updated_at DESC, offer_id ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let definition_json: String = row.get(1)?;
            let definition = serde_json::from_str(&definition_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            Ok(ProviderManagedOfferRecord {
                offer_id: row.get(0)?,
                definition,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })
        .map_err(|error| error.to_string())?;

    let mut offers = Vec::new();
    for row in rows {
        offers.push(row.map_err(|error| error.to_string())?);
    }
    Ok(offers)
}

pub fn list_duplicate_deal_artifact_hashes(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<(String, u64)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT deal_artifact_hash, COUNT(*) AS duplicate_count
             FROM deals
             WHERE deal_artifact_hash IS NOT NULL AND deal_artifact_hash != ''
             GROUP BY deal_artifact_hash
             HAVING COUNT(*) > 1
             ORDER BY duplicate_count DESC, deal_artifact_hash ASC
             LIMIT ?1",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![limit as i64], |row| {
            Ok((row.get(0)?, row.get::<_, i64>(1)? as u64))
        })
        .map_err(|error| error.to_string())?;

    let mut duplicates = Vec::new();
    for row in rows {
        duplicates.push(row.map_err(|error| error.to_string())?);
    }
    Ok(duplicates)
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

/// Write-coalescing event batch writer.
///
/// Collects pending event inserts via a channel and flushes them in batched
/// transactions, reducing per-event write-mutex contention and WAL sync overhead.
pub struct EventBatchWriter {
    tx: tokio::sync::mpsc::Sender<(
        NodeEventEnvelope,
        tokio::sync::oneshot::Sender<Result<bool, String>>,
    )>,
}

impl Clone for EventBatchWriter {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

/// Maximum events to coalesce into a single transaction.
const EVENT_BATCH_MAX_SIZE: usize = 64;
/// Maximum time to wait for a batch to fill before flushing.
const EVENT_BATCH_LINGER: Duration = Duration::from_millis(5);

impl EventBatchWriter {
    /// Spawn the background flush loop. The returned writer can be cloned and
    /// shared across request handlers.
    pub fn spawn(db: DbPool) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel::<(
            NodeEventEnvelope,
            tokio::sync::oneshot::Sender<Result<bool, String>>,
        )>(512);

        tokio::spawn(Self::flush_loop(db, rx));

        Self { tx }
    }

    /// Submit a single event for batched insertion. Returns `Ok(true)` if the
    /// event was newly inserted, `Ok(false)` if it already existed.
    pub async fn insert(&self, event: NodeEventEnvelope) -> Result<bool, String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send((event, reply_tx))
            .await
            .map_err(|_| "event batch writer closed".to_string())?;
        reply_rx
            .await
            .map_err(|_| "event batch writer dropped reply".to_string())?
    }

    async fn flush_loop(
        db: DbPool,
        mut rx: tokio::sync::mpsc::Receiver<(
            NodeEventEnvelope,
            tokio::sync::oneshot::Sender<Result<bool, String>>,
        )>,
    ) {
        let mut batch: Vec<(
            NodeEventEnvelope,
            tokio::sync::oneshot::Sender<Result<bool, String>>,
        )> = Vec::with_capacity(EVENT_BATCH_MAX_SIZE);

        loop {
            // Wait for the first item (blocks until work arrives).
            let first = rx.recv().await;
            let Some(first) = first else {
                break; // Channel closed — shut down.
            };
            batch.push(first);

            // Drain up to EVENT_BATCH_MAX_SIZE more items with a short linger.
            let deadline = tokio::time::Instant::now() + EVENT_BATCH_LINGER;
            loop {
                if batch.len() >= EVENT_BATCH_MAX_SIZE {
                    break;
                }
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Some(item)) => batch.push(item),
                    _ => break, // Timeout or channel closed.
                }
            }

            // Flush the batch.
            let events: Vec<NodeEventEnvelope> = batch.iter().map(|(e, _)| e.clone()).collect();
            let result = db
                .with_write_conn(move |conn| batch_insert_events(conn, &events))
                .await;

            match result {
                Ok(results) => {
                    for ((_event, reply), inserted) in batch.drain(..).zip(results.into_iter()) {
                        let _ = reply.send(Ok(inserted));
                    }
                }
                Err(error) => {
                    for (_event, reply) in batch.drain(..) {
                        let _ = reply.send(Err(error.clone()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::runtime::Runtime;

    fn temp_db_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "froglet-{label}-{}-{unique}.db",
            std::process::id()
        ))
    }

    #[test]
    fn configure_connection_applies_legacy_artifact_backfill_once() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE artifacts (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                artifact_hash TEXT NOT NULL UNIQUE,
                payload_hash TEXT NOT NULL,
                artifact_kind TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                document_json TEXT NOT NULL,
                UNIQUE (actor_id, artifact_kind, payload_hash)
            );
            INSERT INTO artifacts (
                artifact_hash,
                payload_hash,
                artifact_kind,
                actor_id,
                created_at,
                document_json
            ) VALUES (
                'artifact-hash',
                'payload-hash',
                'quote',
                'actor-id',
                123,
                '{\"hash\":\"artifact-hash\"}'
            );",
        )
        .expect("seed legacy artifacts");

        configure_connection(&conn).expect("initial configure");
        configure_connection(&conn).expect("reconfigure");

        let artifact_documents: i64 = conn
            .query_row("SELECT COUNT(*) FROM artifact_documents", [], |row| {
                row.get(0)
            })
            .expect("artifact document count");
        let artifact_feed: i64 = conn
            .query_row("SELECT COUNT(*) FROM artifact_feed", [], |row| row.get(0))
            .expect("artifact feed count");
        let applied_migrations: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE name = ?1",
                params![LEGACY_ARTIFACTS_MIGRATION],
                |row| row.get(0),
            )
            .expect("migration count");

        assert_eq!(artifact_documents, 1);
        assert_eq!(artifact_feed, 1);
        assert_eq!(applied_migrations, 1);
    }

    #[test]
    fn ensure_column_rejects_invalid_identifiers() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute("CREATE TABLE safe_table (id INTEGER)", [])
            .expect("create table");

        let error = ensure_column(
            &conn,
            "safe_table; DROP TABLE safe_table",
            "column_name",
            "TEXT",
        )
        .expect_err("expected invalid identifier error");
        assert!(error.to_string().contains("invalid table name"));
    }

    #[test]
    fn db_pool_read_connections_are_query_only() {
        let db_path = temp_db_path("reader-pool");
        let pool = DbPool::open(&db_path).expect("open pool");
        let runtime = Runtime::new().expect("tokio runtime");

        let error = runtime
            .block_on(pool.with_read_conn(|conn| {
                conn.execute("CREATE TABLE forbidden_write (id INTEGER)", [])
                    .map(|_| ())
            }))
            .expect_err("read pool should reject writes");

        assert!(
            error.contains("readonly") || error.contains("query-only"),
            "unexpected read-connection write error: {error}"
        );

        drop(pool);
        drop(runtime);
        let _ = fs::remove_file(db_path);
    }
}
