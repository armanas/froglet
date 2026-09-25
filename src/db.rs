use rand::RngCore;
use rusqlite::{
    Connection, OptionalExtension, Result as SqlResult, Row, Transaction, TransactionBehavior,
    params,
};
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
const ENFORCE_DEAL_ARTIFACT_HASH_UNIQUE: &str = "20261108_deal_artifact_hash_unique";
const DOCUMENT_IDEMPOTENCY_KEY_UNIQUE: &str = "20261108_idempotency_key_partial_unique";
const BACKFILL_QUOTE_USAGES: &str = "20261108_quote_usages_backfill";
const PUBLICATION_ACTIVATION_TOKEN_MIGRATION: &str =
    "20260711_publication_activation_token_backfill";
const DEFAULT_DB_READ_CONNECTIONS: usize = 4;
pub const MAX_EVENT_QUERY_KINDS: usize = 100;

pub(crate) fn u64_to_sqlite_integer(value: u64, field: &str) -> Result<i64, String> {
    i64::try_from(value)
        .map_err(|_| format!("{field} exceeds SQLite INTEGER maximum ({})", i64::MAX))
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DealSettlementMaterializationRecord {
    pub deal_id: String,
    pub materialization_kind: String,
    pub request_json: String,
    pub phase: String,
    pub resource_json: Option<String>,
    pub attempt_count: i64,
    pub next_attempt_at: i64,
    pub last_error_code: Option<String>,
    pub claim_token: Option<String>,
    pub claim_expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StripeSettlementOutboxRecord {
    pub deal_id: String,
    pub action: String,
    pub details_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DealPrepaidInvoiceRecord {
    pub deal_id: String,
    pub bolt11: String,
    pub payment_hash: String,
    pub amount_sat: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StripeWebhookEventRecord {
    pub event_id: String,
    pub event_type: String,
    pub object_id: Option<String>,
    pub payment_intent_id: Option<String>,
    pub payload_hash: String,
    pub received_at: i64,
    pub processed_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderManagedOfferRecord {
    pub offer_id: String,
    pub definition: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PublicationRevisionRecord {
    pub revision_hash: String,
    pub service_id: String,
    pub offer_id: String,
    pub offer_hash: String,
    pub binding_hash: String,
    pub validation_status: String,
    pub signed_revision: serde_json::Value,
    pub definition: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PublicationLifecycleRecord {
    pub service_id: String,
    pub status: String,
    pub active_revision_hash: Option<String>,
    pub selected_revision_hash: String,
    pub activation_token: String,
    pub revision_count: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PublicationOperationRecord {
    pub operation_id: i64,
    pub service_id: String,
    pub revision_hash: Option<String>,
    pub operation: String,
    pub status: String,
    pub evidence: serde_json::Value,
    pub created_at: i64,
}

/// Durable, non-Kernel authorization for one exact publication activation to
/// keep a public transport online. Multiple services may grant the same
/// transport; withdrawing one grant must not disconnect the others.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublicationTransportGrantRecord {
    pub transport: String,
    pub service_id: String,
    pub revision_hash: String,
    pub offer_id: String,
    pub offer_hash: String,
    pub activation_token: String,
    pub public_url: String,
    pub relay_control_url: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Exact coordinates required to create or idempotently replace one durable
/// transport authorization.
pub struct NewPublicationTransportGrant<'a> {
    pub transport: &'a str,
    pub service_id: &'a str,
    pub revision_hash: &'a str,
    pub activation_token: &'a str,
    pub public_url: &'a str,
    pub relay_control_url: &'a str,
    pub now: i64,
}

#[derive(Debug, Clone)]
pub struct NewPublicationRevision {
    pub revision_hash: String,
    pub service_id: String,
    pub offer_id: String,
    pub offer_hash: String,
    pub binding_hash: String,
    pub signed_revision_json: String,
    pub definition_json: String,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct WalCheckpointMetrics {
    pub wal_size_bytes: u64,
    pub busy: i64,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
    pub duration_ms: u128,
}

fn begin_transaction(
    conn: &Connection,
    behavior: TransactionBehavior,
) -> SqlResult<Transaction<'_>> {
    // The pool exposes shared `Connection` references while serializing access
    // externally. `new_unchecked` relaxes only rusqlite's mutable-borrow check;
    // SQLite still rejects a genuinely nested transaction.
    Transaction::new_unchecked(conn, behavior)
}

fn with_sql_transaction<T>(
    conn: &Connection,
    behavior: TransactionBehavior,
    operation: impl FnOnce(&Connection) -> SqlResult<T>,
) -> SqlResult<T> {
    let transaction = begin_transaction(conn, behavior)?;
    let result = operation(&transaction)?;
    transaction.commit()?;
    Ok(result)
}

fn with_immediate_sql_transaction<T>(
    conn: &Connection,
    operation: impl FnOnce(&Connection) -> SqlResult<T>,
) -> SqlResult<T> {
    with_sql_transaction(conn, TransactionBehavior::Immediate, operation)
}

/// Run a write operation under an eager SQLite writer transaction.
///
/// `Transaction` rolls back on drop, including during unwinding and when its
/// explicit `COMMIT` fails. That keeps the pooled writer connection in
/// autocommit mode and reusable after every error path.
pub fn with_immediate_transaction<T>(
    conn: &Connection,
    operation: impl FnOnce(&Connection) -> Result<T, String>,
) -> Result<T, String> {
    let transaction = begin_transaction(conn, TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let result = operation(&transaction)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(result)
}

fn configure_connection(conn: &Connection) -> SqlResult<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA temp_store = MEMORY;",
    )?;
    // Preserve the original plain `BEGIN` (deferred) behavior for schema
    // initialization while moving rollback/commit ownership into RAII.
    with_sql_transaction(conn, TransactionBehavior::Deferred, |transaction| {
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
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
            base_fee_msat INTEGER NOT NULL CHECK (base_fee_msat >= 0),
            base_state TEXT NOT NULL,
            success_invoice_hash TEXT NOT NULL,
            success_payment_hash TEXT NOT NULL,
            success_fee_msat INTEGER NOT NULL CHECK (success_fee_msat >= 0),
            success_state TEXT NOT NULL,
            bundle_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_lightning_invoice_bundles_quote_hash ON lightning_invoice_bundles (quote_hash);
        CREATE INDEX IF NOT EXISTS idx_lightning_invoice_bundles_deal_hash ON lightning_invoice_bundles (deal_hash);
        CREATE TRIGGER IF NOT EXISTS lightning_invoice_bundles_validate_insert
        BEFORE INSERT ON lightning_invoice_bundles
        WHEN NEW.base_fee_msat < 0 OR NEW.success_fee_msat < 0
        BEGIN
            SELECT RAISE(ABORT, 'invalid lightning invoice bundle amount');
        END;
        CREATE TRIGGER IF NOT EXISTS lightning_invoice_bundles_validate_update
        BEFORE UPDATE OF base_fee_msat, success_fee_msat ON lightning_invoice_bundles
        WHEN NEW.base_fee_msat < 0 OR NEW.success_fee_msat < 0
        BEGIN
            SELECT RAISE(ABORT, 'invalid lightning invoice bundle amount');
        END;
        CREATE TABLE IF NOT EXISTS quotes (
            quote_id TEXT PRIMARY KEY,
            artifact_hash TEXT NOT NULL UNIQUE,
            offer_id TEXT NOT NULL,
            service_id TEXT NOT NULL,
            workload_hash TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            price_sats INTEGER NOT NULL CHECK (price_sats >= 0),
            quote_json TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_quotes_offer_expires_at ON quotes (offer_id, expires_at DESC);
        CREATE TRIGGER IF NOT EXISTS quotes_validate_price_insert
        BEFORE INSERT ON quotes
        WHEN NEW.price_sats < 0
        BEGIN
            SELECT RAISE(ABORT, 'invalid quote price');
        END;
        CREATE TRIGGER IF NOT EXISTS quotes_validate_price_update
        BEFORE UPDATE OF price_sats ON quotes
        WHEN NEW.price_sats < 0
        BEGIN
            SELECT RAISE(ABORT, 'invalid quote price');
        END;
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
            payment_amount_sats INTEGER CHECK (
                payment_amount_sats IS NULL OR payment_amount_sats >= 0
            ),
            receipt_artifact_json TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_deals_status_updated_at ON deals (status, updated_at DESC);
        CREATE TRIGGER IF NOT EXISTS deals_validate_payment_amount_insert
        BEFORE INSERT ON deals
        WHEN NEW.payment_amount_sats < 0
        BEGIN
            SELECT RAISE(ABORT, 'invalid deal payment amount');
        END;
        CREATE TRIGGER IF NOT EXISTS deals_validate_payment_amount_update
        BEFORE UPDATE OF payment_amount_sats ON deals
        WHEN NEW.payment_amount_sats < 0
        BEGIN
            SELECT RAISE(ABORT, 'invalid deal payment amount');
        END;
         CREATE TABLE IF NOT EXISTS requester_deals (
            deal_id TEXT PRIMARY KEY,
            idempotency_key TEXT UNIQUE,
            provider_id TEXT NOT NULL,
            provider_url TEXT NOT NULL,
            provider_sync_url TEXT,
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
        CREATE TABLE IF NOT EXISTS publication_revisions (
            revision_hash TEXT PRIMARY KEY,
            service_id TEXT NOT NULL,
            offer_id TEXT NOT NULL,
            offer_hash TEXT NOT NULL,
            binding_hash TEXT NOT NULL,
            validation_status TEXT NOT NULL CHECK (validation_status = 'validated'),
            signed_revision_json TEXT NOT NULL,
            definition_json TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_publication_revisions_service_created_at
            ON publication_revisions (service_id, created_at DESC, revision_hash ASC);
        CREATE TABLE IF NOT EXISTS publication_service_lifecycle (
            service_id TEXT PRIMARY KEY,
            status TEXT NOT NULL CHECK (status IN ('active', 'paused', 'unpublished')),
            active_revision_hash TEXT,
            selected_revision_hash TEXT NOT NULL,
            activation_token TEXT NOT NULL CHECK (
                length(activation_token) = 64
                AND activation_token NOT GLOB '*[^0-9a-f]*'
            ),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (active_revision_hash) REFERENCES publication_revisions(revision_hash),
            FOREIGN KEY (selected_revision_hash) REFERENCES publication_revisions(revision_hash),
            CHECK (
                (status = 'active' AND active_revision_hash IS NOT NULL)
                OR (status != 'active' AND active_revision_hash IS NULL)
            )
        );
        CREATE INDEX IF NOT EXISTS idx_publication_service_lifecycle_status_updated_at
            ON publication_service_lifecycle (status, updated_at DESC, service_id ASC);
        CREATE TABLE IF NOT EXISTS publication_operation_log (
            operation_id INTEGER PRIMARY KEY AUTOINCREMENT,
            service_id TEXT NOT NULL,
            revision_hash TEXT,
            operation TEXT NOT NULL CHECK (
                operation IN ('publish', 'pause', 'resume', 'unpublish', 'rollback')
            ),
            status TEXT NOT NULL CHECK (status = 'succeeded'),
            evidence_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (revision_hash) REFERENCES publication_revisions(revision_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_publication_operation_log_service_id
            ON publication_operation_log (service_id, operation_id DESC);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_publication_operation_publish_once
            ON publication_operation_log (service_id, revision_hash, operation)
            WHERE operation = 'publish';
        CREATE TABLE IF NOT EXISTS publication_transport_grants (
            transport TEXT NOT NULL,
            service_id TEXT NOT NULL,
            revision_hash TEXT NOT NULL,
            activation_token TEXT NOT NULL CHECK (
                length(activation_token) = 64
                AND activation_token NOT GLOB '*[^0-9a-f]*'
            ),
            public_url TEXT NOT NULL CHECK (length(public_url) > 0),
            relay_control_url TEXT NOT NULL CHECK (length(relay_control_url) > 0),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (transport, service_id),
            FOREIGN KEY (service_id) REFERENCES publication_service_lifecycle(service_id),
            FOREIGN KEY (revision_hash) REFERENCES publication_revisions(revision_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_publication_transport_grants_transport
            ON publication_transport_grants (transport, updated_at ASC, service_id ASC);
        CREATE TABLE IF NOT EXISTS managed_publication_operations (
            operation_id TEXT PRIMARY KEY CHECK (
                length(operation_id) = 64
                AND operation_id NOT GLOB '*[^0-9a-f]*'
            ),
            service_id TEXT NOT NULL,
            plan_hash TEXT NOT NULL CHECK (
                length(plan_hash) = 64
                AND plan_hash NOT GLOB '*[^0-9a-f]*'
            ),
            consent_hash TEXT NOT NULL CHECK (
                length(consent_hash) = 64
                AND consent_hash NOT GLOB '*[^0-9a-f]*'
            ),
            phase TEXT NOT NULL CHECK (phase IN (
                'planned', 'local_revision_ready', 'provisioning', 'deploying',
                'reconciling', 'deployed', 'canary_verified', 'registering',
                'active', 'compensating', 'compensated',
                'reconciliation_required', 'failed'
            )),
            revision_hash TEXT,
            plan_json TEXT NOT NULL,
            operation_json TEXT NOT NULL,
            capsule_json TEXT,
            state_version INTEGER NOT NULL CHECK (state_version >= 1),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_managed_publication_operations_recovery
            ON managed_publication_operations (phase, updated_at ASC, operation_id ASC);
        CREATE INDEX IF NOT EXISTS idx_managed_publication_operations_service
            ON managed_publication_operations (service_id, updated_at DESC, operation_id ASC);",
        )
    })?;
    ensure_column(conn, "jobs", "workload_evidence_hash", "TEXT")?;
    ensure_column(conn, "jobs", "result_evidence_hash", "TEXT")?;
    ensure_column(conn, "jobs", "failure_evidence_hash", "TEXT")?;
    ensure_column(conn, "deals", "workload_evidence_hash", "TEXT")?;
    ensure_column(conn, "deals", "deal_artifact_hash", "TEXT")?;
    ensure_column(conn, "deals", "result_evidence_hash", "TEXT")?;
    ensure_column(conn, "deals", "failure_evidence_hash", "TEXT")?;
    ensure_column(conn, "deals", "receipt_artifact_hash", "TEXT")?;
    ensure_column(conn, "deals", "payment_method", "TEXT")?;
    ensure_column(conn, "requester_deals", "provider_sync_url", "TEXT")?;
    ensure_column(
        conn,
        "publication_service_lifecycle",
        "activation_token",
        "TEXT",
    )?;
    ensure_column(
        conn,
        "publication_transport_grants",
        "relay_control_url",
        "TEXT",
    )?;
    // A pre-binding relay grant cannot prove which control endpoint the user
    // approved. Remove it instead of silently inheriting the current config.
    conn.execute(
        "DELETE FROM publication_transport_grants
         WHERE transport = 'relay'
           AND (relay_control_url IS NULL OR relay_control_url = '')",
        [],
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_execution_evidence_content_hash
            ON execution_evidence (content_hash);
         CREATE INDEX IF NOT EXISTS idx_deals_quote_hash_created_at
            ON deals (quote_hash, created_at);
         CREATE INDEX IF NOT EXISTS idx_deals_payment_method_status_created_at
            ON deals (payment_method, status, created_at);
         CREATE INDEX IF NOT EXISTS idx_deals_deal_artifact_hash
            ON deals (deal_artifact_hash);
         CREATE TABLE IF NOT EXISTS deal_settlement_materializations (
            deal_id TEXT PRIMARY KEY,
            materialization_kind TEXT NOT NULL,
            request_json TEXT NOT NULL,
            phase TEXT NOT NULL DEFAULT 'pending',
            resource_json TEXT,
            attempt_count INTEGER NOT NULL DEFAULT 0,
            next_attempt_at INTEGER NOT NULL DEFAULT 0,
            last_error_code TEXT,
            claim_token TEXT,
            claim_expires_at INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (deal_id) REFERENCES deals(deal_id)
         );
         CREATE INDEX IF NOT EXISTS idx_deal_settlement_materializations_updated_at
            ON deal_settlement_materializations (updated_at ASC);
         CREATE TABLE IF NOT EXISTS stripe_settlement_outbox (
            deal_id TEXT PRIMARY KEY,
            action TEXT NOT NULL CHECK (action IN ('capture', 'cancel')),
            details_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (deal_id) REFERENCES deals(deal_id)
         );
         CREATE INDEX IF NOT EXISTS idx_stripe_settlement_outbox_updated_at
            ON stripe_settlement_outbox (updated_at ASC, deal_id ASC);
         CREATE TABLE IF NOT EXISTS deal_prepaid_invoices (
            deal_id TEXT PRIMARY KEY,
            bolt11 TEXT NOT NULL,
            payment_hash TEXT NOT NULL UNIQUE,
            amount_sat INTEGER NOT NULL CHECK (amount_sat >= 0),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (deal_id) REFERENCES deals(deal_id)
         );
         CREATE INDEX IF NOT EXISTS idx_deal_prepaid_invoices_payment_hash
            ON deal_prepaid_invoices (payment_hash);
         CREATE TRIGGER IF NOT EXISTS deal_prepaid_invoices_validate_insert
         BEFORE INSERT ON deal_prepaid_invoices
         WHEN NEW.amount_sat < 0
         BEGIN
            SELECT RAISE(ABORT, 'invalid prepaid invoice amount');
         END;
         CREATE TRIGGER IF NOT EXISTS deal_prepaid_invoices_validate_update
         BEFORE UPDATE OF amount_sat ON deal_prepaid_invoices
         WHEN NEW.amount_sat < 0
         BEGIN
            SELECT RAISE(ABORT, 'invalid prepaid invoice amount');
         END;
         CREATE TABLE IF NOT EXISTS stripe_webhook_events (
            event_id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            object_id TEXT,
            payment_intent_id TEXT,
            payload_hash TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            received_at INTEGER NOT NULL,
            processed_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_stripe_webhook_events_type_received_at
            ON stripe_webhook_events (event_type, received_at DESC);
         CREATE INDEX IF NOT EXISTS idx_stripe_webhook_events_payment_intent
            ON stripe_webhook_events (payment_intent_id, received_at DESC);",
    )?;
    ensure_column(
        conn,
        "deal_settlement_materializations",
        "claim_token",
        "TEXT",
    )?;
    ensure_column(conn, "deal_settlement_materializations", "phase", "TEXT")?;
    ensure_column(
        conn,
        "deal_settlement_materializations",
        "resource_json",
        "TEXT",
    )?;
    ensure_column(
        conn,
        "deal_settlement_materializations",
        "attempt_count",
        "INTEGER",
    )?;
    ensure_column(
        conn,
        "deal_settlement_materializations",
        "next_attempt_at",
        "INTEGER",
    )?;
    ensure_column(
        conn,
        "deal_settlement_materializations",
        "last_error_code",
        "TEXT",
    )?;
    conn.execute(
        "UPDATE deal_settlement_materializations
         SET phase = COALESCE(phase, 'pending'),
             attempt_count = COALESCE(attempt_count, 0),
             next_attempt_at = COALESCE(next_attempt_at, 0)",
        [],
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_deal_settlement_materializations_due
            ON deal_settlement_materializations (next_attempt_at ASC, updated_at ASC);",
    )?;
    apply_migration_once(
        conn,
        PUBLICATION_ACTIVATION_TOKEN_MIGRATION,
        migrate_publication_activation_tokens,
    )?;
    ensure_column(
        conn,
        "deal_settlement_materializations",
        "claim_expires_at",
        "INTEGER",
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
            ON deal_quarantine (quarantined_at DESC);
         CREATE TABLE IF NOT EXISTS requester_spend_ledger (
            deal_hash TEXT PRIMARY KEY,
            deal_id TEXT,
            provider_id TEXT NOT NULL,
            amount_msat INTEGER NOT NULL CHECK (amount_msat >= 0),
            settlement_method TEXT NOT NULL,
            state TEXT NOT NULL CHECK (
                state IN ('reserved', 'external_pending', 'committed', 'archived')
            ),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_requester_spend_ledger_state
            ON requester_spend_ledger (state);
         CREATE TRIGGER IF NOT EXISTS requester_spend_ledger_validate_insert
         BEFORE INSERT ON requester_spend_ledger
         WHEN NEW.amount_msat < 0
           OR NEW.state NOT IN ('reserved', 'external_pending', 'committed', 'archived')
         BEGIN
            SELECT RAISE(ABORT, 'invalid requester spend ledger row');
         END;
         CREATE TRIGGER IF NOT EXISTS requester_spend_ledger_validate_update
         BEFORE UPDATE OF amount_msat, state ON requester_spend_ledger
         WHEN NEW.amount_msat < 0
           OR NEW.state NOT IN ('reserved', 'external_pending', 'committed', 'archived')
         BEGIN
            SELECT RAISE(ABORT, 'invalid requester spend ledger row');
         END;
         CREATE TABLE IF NOT EXISTS quote_usages (
            quote_hash TEXT PRIMARY KEY,
            deal_id TEXT NOT NULL,
            deal_artifact_hash TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_quote_usages_deal_artifact_hash
            ON quote_usages (deal_artifact_hash);",
    )?;
    apply_migration_once(conn, LEGACY_ARTIFACTS_MIGRATION, migrate_legacy_artifacts)?;
    apply_migration_once(
        conn,
        ENFORCE_DEAL_ARTIFACT_HASH_UNIQUE,
        migrate_enforce_deal_artifact_hash_unique,
    )?;
    apply_migration_once(
        conn,
        DOCUMENT_IDEMPOTENCY_KEY_UNIQUE,
        migrate_idempotency_key_partial_unique,
    )?;
    apply_migration_once(conn, BACKFILL_QUOTE_USAGES, migrate_backfill_quote_usages)?;
    Ok(())
}

pub fn initialize_db(db_path: &Path) -> SqlResult<Connection> {
    secure_sqlite_storage(db_path)?;
    let conn = Connection::open(db_path)?;
    configure_connection(&conn)?;
    secure_sqlite_storage(db_path)?;
    Ok(conn)
}

pub fn initialize_db_reader(db_path: &Path) -> SqlResult<Connection> {
    secure_sqlite_storage(db_path)?;
    let conn = Connection::open(db_path)?;
    configure_connection(&conn)?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    secure_sqlite_storage(db_path)?;
    Ok(conn)
}

pub fn initialize_db_for_connection(conn: &Connection) -> SqlResult<()> {
    configure_connection(conn)
}

#[cfg(unix)]
fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut path = db_path.as_os_str().to_os_string();
    path.push(suffix);
    std::path::PathBuf::from(path)
}

#[cfg(unix)]
fn sqlite_storage_error(action: &str, path: &Path, error: std::io::Error) -> rusqlite::Error {
    let result_code = if error.kind() == std::io::ErrorKind::PermissionDenied {
        rusqlite::ffi::SQLITE_PERM
    } else {
        rusqlite::ffi::SQLITE_CANTOPEN
    };
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(result_code),
        Some(format!("{action} {}: {error}", path.display())),
    )
}

/// Create the main SQLite file privately before SQLite can create WAL
/// sidecars. SQLite derives new `-wal`/`-shm` modes from the database file, and
/// the second pass also repairs sidecars left by older versions or an unclean
/// shutdown. This belongs in the DB initializer so embedded/library callers
/// get the same protection as the daemon entrypoint.
fn secure_sqlite_storage(db_path: &Path) -> SqlResult<()> {
    if db_path == Path::new(":memory:") {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        // Never open and close an extra descriptor for an existing SQLite
        // database. POSIX close() releases this process's advisory locks for
        // that inode, including locks held by live SQLite connections. Another
        // process could then remove the WAL while our pool still uses it.
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(db_path)
        {
            Ok(file) => drop(file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(sqlite_storage_error("failed to create", db_path, error)),
        }
        fs::set_permissions(db_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| sqlite_storage_error("failed to secure", db_path, error))?;

        for suffix in ["-wal", "-shm"] {
            let path = sqlite_sidecar_path(db_path, suffix);
            match fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(sqlite_storage_error("failed to secure", &path, error));
                }
            }
        }
    }

    Ok(())
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
        // `open_with` is public for embedders that supply connection hooks.
        // Keep the filesystem guarantee at the pool boundary as well as in
        // the default initializers so a custom hook cannot create the file
        // under the process umask before it is made private.
        secure_sqlite_storage(db_path)?;
        let write = init_write(db_path)?;
        secure_sqlite_storage(db_path)?;
        let reader_count = db_read_connection_count();
        let mut readers = Vec::with_capacity(reader_count);
        for _ in 0..reader_count {
            readers.push(Arc::new(Mutex::new(init_read(db_path)?)));
            secure_sqlite_storage(db_path)?;
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

    /// Reconcile persisted publication truth before the daemon begins serving.
    /// This synchronous startup-only path holds the sole writer connection;
    /// request-time callers continue to use the async methods below.
    pub fn pause_publications_for_provider_identity(
        &self,
        current_provider_id: &str,
        now: i64,
    ) -> Result<Vec<String>, String> {
        let conn = self
            .write
            .lock()
            .map_err(|error| format!("database mutex poisoned: {error}"))?;
        pause_active_publications_for_provider_identity(&conn, current_provider_id, now)
    }

    /// Remove grants that no longer match an exact active lifecycle and return
    /// the remaining startup-safe set. This runs before any public transport
    /// task is spawned.
    pub fn reconcile_publication_transport_grants(
        &self,
        expected_relay: Option<(&str, &str)>,
    ) -> Result<Vec<PublicationTransportGrantRecord>, String> {
        let conn = self
            .write
            .lock()
            .map_err(|error| format!("database mutex poisoned: {error}"))?;
        reconcile_publication_transport_grants(&conn)?;
        with_immediate_transaction(&conn, |conn| {
            match expected_relay {
                Some((expected_public_url, expected_control_url)) => conn
                    .execute(
                        "DELETE FROM publication_transport_grants
                         WHERE transport = 'relay'
                           AND (
                               public_url != ?1
                               OR relay_control_url IS NULL
                               OR relay_control_url != ?2
                           )",
                        params![expected_public_url, expected_control_url],
                    )
                    .map_err(|error| error.to_string())?,
                None => conn
                    .execute(
                        "DELETE FROM publication_transport_grants WHERE transport = 'relay'",
                        [],
                    )
                    .map_err(|error| error.to_string())?,
            };
            list_publication_transport_grants_in_transaction(conn, "relay")
        })
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
            let conn = match inner.lock() {
                Ok(conn) => conn,
                Err(poisoned) => {
                    let conn = poisoned.into_inner();
                    if !conn.is_autocommit() {
                        conn.execute_batch("ROLLBACK").map_err(|error| {
                            format!(
                                "database mutex was poisoned and its transaction could not be rolled back: {error}"
                            )
                        })?;
                    }
                    inner.clear_poison();
                    conn
                }
            };
            if !conn.is_autocommit() {
                conn.execute_batch("ROLLBACK").map_err(|error| {
                    format!("pooled database connection leaked a transaction: {error}")
                })?;
            }
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

    with_immediate_sql_transaction(conn, |transaction| {
        let mut results = Vec::with_capacity(events.len());
        let mut stmt = transaction.prepare_cached(
            "INSERT OR IGNORE INTO events (id, pubkey, created_at, kind, content, sig, tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;

        for event in events {
            let tags_json = serde_json::to_string(&event.tags).unwrap_or_else(|_| "[]".to_string());
            let inserted = stmt.execute(params![
                event.id,
                event.pubkey,
                event.created_at,
                event.kind,
                event.content,
                event.sig,
                tags_json
            ])?;
            results.push(inserted > 0);
        }

        Ok(results)
    })
}

pub fn insert_stripe_webhook_event(
    conn: &Connection,
    record: &StripeWebhookEventRecord,
    payload_json: &str,
) -> SqlResult<bool> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO stripe_webhook_events (
            event_id,
            event_type,
            object_id,
            payment_intent_id,
            payload_hash,
            payload_json,
            received_at,
            processed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            record.event_id,
            record.event_type,
            record.object_id,
            record.payment_intent_id,
            record.payload_hash,
            payload_json,
            record.received_at,
            record.processed_at
        ],
    )?;

    Ok(inserted > 0)
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
    with_immediate_sql_transaction(conn, |transaction| {
        let inserted = transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations (name) VALUES (?1)",
            params![name],
        )?;
        if inserted == 0 {
            return Ok(());
        }
        apply(transaction)?;
        Ok(())
    })
}

pub fn is_publication_activation_token(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn new_publication_activation_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn migrate_publication_activation_tokens(conn: &Connection) -> SqlResult<()> {
    let lifecycles = {
        let mut stmt = conn.prepare(
            "SELECT service_id, activation_token
             FROM publication_service_lifecycle
             ORDER BY service_id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        rows.collect::<SqlResult<Vec<_>>>()?
    };

    for (service_id, activation_token) in lifecycles {
        if activation_token
            .as_deref()
            .is_some_and(is_publication_activation_token)
        {
            continue;
        }
        conn.execute(
            "UPDATE publication_service_lifecycle
             SET activation_token = ?2
             WHERE service_id = ?1",
            params![service_id, new_publication_activation_token()],
        )?;
    }
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

/// Adds a partial UNIQUE index over `deals.deal_artifact_hash` for non-NULL
/// values. Application code in `crate::deals` already enforces uniqueness via
/// SELECT-then-INSERT under an immediate writer transaction; this is defense-in-depth at
/// the DB layer. Refuses to apply if existing duplicates are present so
/// operators can resolve them via the existing
/// `audit_duplicate_deal_hashes` startup audit before retrying.
fn migrate_enforce_deal_artifact_hash_unique(conn: &Connection) -> SqlResult<()> {
    let dup_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
             SELECT deal_artifact_hash
               FROM deals
              WHERE deal_artifact_hash IS NOT NULL
           GROUP BY deal_artifact_hash HAVING COUNT(*) > 1
         )",
        [],
        |row| row.get(0),
    )?;
    if dup_count > 0 {
        return Err(rusqlite::Error::ToSqlConversionFailure(
            format!(
                "cannot enforce UNIQUE on deals.deal_artifact_hash: {dup_count} \
                 duplicate hash(es) present; inspect via audit_duplicate_deal_hashes \
                 and resolve before retrying"
            )
            .into(),
        ));
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS uniq_deals_deal_artifact_hash
            ON deals(deal_artifact_hash)
            WHERE deal_artifact_hash IS NOT NULL;",
    )?;
    Ok(())
}

/// Documents the existing column-level UNIQUE behaviour on `idempotency_key`
/// across `jobs`, `deals`, and `requester_deals` via explicit partial unique
/// indexes. SQLite already treats NULLs as not-equal in `UNIQUE` columns, so
/// this is functionally equivalent but makes the intent visible at the
/// schema level — future maintainers won't try to "fix" the column-level
/// UNIQUE under the assumption that it disallows multiple NULLs.
fn migrate_idempotency_key_partial_unique(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS uniq_jobs_idempotency_key
            ON jobs(idempotency_key) WHERE idempotency_key IS NOT NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS uniq_deals_idempotency_key
            ON deals(idempotency_key) WHERE idempotency_key IS NOT NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS uniq_requester_deals_idempotency_key
            ON requester_deals(idempotency_key) WHERE idempotency_key IS NOT NULL;",
    )?;
    Ok(())
}

fn migrate_backfill_quote_usages(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "INSERT OR IGNORE INTO quote_usages (
            quote_hash,
            deal_id,
            deal_artifact_hash,
            created_at
         )
         SELECT
            d.quote_hash,
            d.deal_id,
            d.deal_artifact_hash,
            d.created_at
           FROM deals d
          WHERE d.quote_hash IS NOT NULL
            AND d.deal_artifact_hash IS NOT NULL
            AND d.rowid = (
                SELECT d2.rowid
                  FROM deals d2
                 WHERE d2.quote_hash = d.quote_hash
                   AND d2.deal_artifact_hash IS NOT NULL
              ORDER BY d2.created_at ASC, d2.rowid ASC
                 LIMIT 1
            );",
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
         ORDER BY f.sequence DESC
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
    let base_fee_msat = u64_to_sqlite_integer(
        bundle.payload.base_fee.amount_msat,
        "lightning base_fee.amount_msat",
    )?;
    let success_fee_msat = u64_to_sqlite_integer(
        bundle.payload.success_fee.amount_msat,
        "lightning success_fee.amount_msat",
    )?;
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
            base_fee_msat,
            invoice_leg_state_str(base_state),
            &bundle.payload.success_fee.invoice_hash,
            &bundle.payload.success_fee.payment_hash,
            success_fee_msat,
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
             SET base_state = CASE
                     WHEN base_state IN ('settled', 'canceled', 'expired') THEN base_state
                     WHEN base_state = 'accepted' AND ?2 = 'open' THEN base_state
                     ELSE ?2
                 END,
                 success_state = CASE
                     WHEN success_state IN ('settled', 'canceled', 'expired') THEN success_state
                     WHEN success_state = 'accepted' AND ?3 = 'open' THEN success_state
                     ELSE ?3
                 END,
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
        "INSERT INTO deal_settlement_materializations (
            deal_id,
            materialization_kind,
            request_json,
            phase,
            attempt_count,
            next_attempt_at,
            created_at,
            updated_at
         ) VALUES (?1, ?2, ?3, 'pending', 0, ?4, ?4, ?4)
         ON CONFLICT(deal_id) DO NOTHING",
        params![deal_id, materialization_kind, request_json, created_at],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn decode_deal_settlement_materialization_row(
    row: &Row<'_>,
) -> rusqlite::Result<DealSettlementMaterializationRecord> {
    Ok(DealSettlementMaterializationRecord {
        deal_id: row.get(0)?,
        materialization_kind: row.get(1)?,
        request_json: row.get(2)?,
        phase: row.get(3)?,
        resource_json: row.get(4)?,
        attempt_count: row.get(5)?,
        next_attempt_at: row.get(6)?,
        last_error_code: row.get(7)?,
        claim_token: row.get(8)?,
        claim_expires_at: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

pub fn get_deal_settlement_materialization(
    conn: &Connection,
    deal_id: &str,
) -> Result<Option<DealSettlementMaterializationRecord>, String> {
    conn.query_row(
        "SELECT deal_id, materialization_kind, request_json,
                COALESCE(phase, 'pending'), resource_json,
                COALESCE(attempt_count, 0), COALESCE(next_attempt_at, 0),
                last_error_code, claim_token, claim_expires_at,
                created_at, updated_at
         FROM deal_settlement_materializations
         WHERE deal_id = ?1",
        params![deal_id],
        decode_deal_settlement_materialization_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

/// Atomically claim and return a settlement materialization.
///
/// Standalone calls own an immediate RAII transaction. Calls made from a
/// larger deal transaction join it so the deal read and claim can share one
/// commit. `UPDATE ... RETURNING` ensures a losing claimant can never observe
/// or return the winning claimant's row.
pub fn claim_deal_settlement_materialization(
    conn: &Connection,
    deal_id: &str,
    claim_token: &str,
    claim_expires_at: i64,
    now: i64,
) -> Result<Option<DealSettlementMaterializationRecord>, String> {
    if conn.is_autocommit() {
        return with_immediate_transaction(conn, |transaction| {
            claim_deal_settlement_materialization_in_transaction(
                transaction,
                deal_id,
                claim_token,
                claim_expires_at,
                now,
            )
        });
    }

    claim_deal_settlement_materialization_in_transaction(
        conn,
        deal_id,
        claim_token,
        claim_expires_at,
        now,
    )
}

fn claim_deal_settlement_materialization_in_transaction(
    conn: &Connection,
    deal_id: &str,
    claim_token: &str,
    claim_expires_at: i64,
    now: i64,
) -> Result<Option<DealSettlementMaterializationRecord>, String> {
    conn.query_row(
        "UPDATE deal_settlement_materializations
                SET claim_token = ?2,
                    claim_expires_at = ?3,
                    attempt_count = COALESCE(attempt_count, 0) + 1,
                    updated_at = ?4
              WHERE deal_id = ?1
                AND COALESCE(next_attempt_at, 0) <= ?4
                AND (
                    claim_token IS NULL
                    OR claim_expires_at IS NULL
                    OR claim_expires_at <= ?4
                )
              RETURNING deal_id, materialization_kind, request_json,
                        COALESCE(phase, 'pending'), resource_json,
                        COALESCE(attempt_count, 0), COALESCE(next_attempt_at, 0),
                        last_error_code, claim_token, claim_expires_at,
                        created_at, updated_at",
        params![deal_id, claim_token, claim_expires_at, now],
        decode_deal_settlement_materialization_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub fn list_deal_settlement_materializations(
    conn: &Connection,
) -> Result<Vec<DealSettlementMaterializationRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT deal_id, materialization_kind, request_json,
                    COALESCE(phase, 'pending'), resource_json,
                    COALESCE(attempt_count, 0), COALESCE(next_attempt_at, 0),
                    last_error_code, claim_token, claim_expires_at,
                    created_at, updated_at
             FROM deal_settlement_materializations
             ORDER BY updated_at ASC, deal_id ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([], decode_deal_settlement_materialization_row)
        .map_err(|error| error.to_string())?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|error| error.to_string())?);
    }
    Ok(records)
}

pub fn list_due_deal_settlement_materializations(
    conn: &Connection,
    now: i64,
    limit: usize,
) -> Result<Vec<DealSettlementMaterializationRecord>, String> {
    let limit = limit.clamp(1, 128) as i64;
    let mut stmt = conn
        .prepare(
            "SELECT deal_id, materialization_kind, request_json,
                    COALESCE(phase, 'pending'), resource_json,
                    COALESCE(attempt_count, 0), COALESCE(next_attempt_at, 0),
                    last_error_code, claim_token, claim_expires_at,
                    created_at, updated_at
             FROM deal_settlement_materializations
             WHERE COALESCE(next_attempt_at, 0) <= ?1
               AND (
                    claim_token IS NULL
                    OR claim_expires_at IS NULL
                    OR claim_expires_at <= ?1
               )
             ORDER BY next_attempt_at ASC, updated_at ASC, deal_id ASC
             LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(
            params![now, limit],
            decode_deal_settlement_materialization_row,
        )
        .map_err(|error| error.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())
}

/// Release every in-flight claim left by the previous process. This is only
/// safe during pre-listener startup while this process exclusively owns the
/// node database.
pub fn reset_deal_settlement_materialization_claims(
    conn: &Connection,
    now: i64,
) -> Result<usize, String> {
    conn.execute(
        "UPDATE deal_settlement_materializations
         SET claim_token = NULL,
             claim_expires_at = NULL,
             updated_at = ?1
         WHERE claim_token IS NOT NULL OR claim_expires_at IS NOT NULL",
        params![now],
    )
    .map_err(|error| error.to_string())
}

pub fn renew_deal_settlement_materialization_claim(
    conn: &Connection,
    deal_id: &str,
    claim_token: &str,
    claim_expires_at: i64,
    now: i64,
) -> Result<bool, String> {
    if claim_token.trim().is_empty() {
        return Err("settlement materialization claim token must not be empty".to_string());
    }
    let updated = conn
        .execute(
            "UPDATE deal_settlement_materializations
             SET claim_expires_at = ?3, updated_at = ?4
             WHERE deal_id = ?1 AND claim_token = ?2",
            params![deal_id, claim_token, claim_expires_at, now],
        )
        .map_err(|error| error.to_string())?;
    Ok(updated > 0)
}

pub fn reschedule_deal_settlement_materialization_if_claim_token(
    conn: &Connection,
    deal_id: &str,
    claim_token: &str,
    next_attempt_at: i64,
    last_error_code: &str,
    now: i64,
) -> Result<bool, String> {
    if claim_token.trim().is_empty() || last_error_code.trim().is_empty() {
        return Err(
            "settlement materialization claim token and error code must not be empty".to_string(),
        );
    }
    let updated = conn
        .execute(
            "UPDATE deal_settlement_materializations
             SET claim_token = NULL,
                 claim_expires_at = NULL,
                 next_attempt_at = ?3,
                 last_error_code = ?4,
                 updated_at = ?5
             WHERE deal_id = ?1 AND claim_token = ?2",
            params![deal_id, claim_token, next_attempt_at, last_error_code, now],
        )
        .map_err(|error| error.to_string())?;
    Ok(updated > 0)
}

pub fn set_deal_settlement_materialization_resource_if_claim_token(
    conn: &Connection,
    deal_id: &str,
    claim_token: &str,
    phase: &str,
    resource_json: &str,
    now: i64,
) -> Result<bool, String> {
    if claim_token.trim().is_empty() {
        return Err("settlement materialization claim token must not be empty".to_string());
    }
    if !matches!(phase, "resource_ready" | "cleanup_pending") {
        return Err(format!(
            "unsupported settlement materialization phase: {phase}"
        ));
    }
    if resource_json.trim().is_empty() {
        return Err("settlement materialization resource JSON must not be empty".to_string());
    }
    let updated = conn
        .execute(
            "UPDATE deal_settlement_materializations
             SET phase = ?3,
                 resource_json = ?4,
                 last_error_code = NULL,
                 updated_at = ?5
             WHERE deal_id = ?1 AND claim_token = ?2",
            params![deal_id, claim_token, phase, resource_json, now],
        )
        .map_err(|error| error.to_string())?;
    Ok(updated > 0)
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

/// Delete a settlement materialization only while the caller still owns its
/// exact claim. This primitive intentionally does not open a transaction so it
/// can participate in the caller's larger atomic state transition.
pub fn delete_deal_settlement_materialization_if_claim_token(
    conn: &Connection,
    deal_id: &str,
    claim_token: &str,
) -> Result<bool, String> {
    if claim_token.trim().is_empty() {
        return Err("settlement materialization claim token must not be empty".to_string());
    }

    let deleted = conn
        .execute(
            "DELETE FROM deal_settlement_materializations
             WHERE deal_id = ?1 AND claim_token = ?2",
            params![deal_id, claim_token],
        )
        .map_err(|error| error.to_string())?;
    Ok(deleted > 0)
}

pub fn insert_stripe_settlement_outbox(
    conn: &Connection,
    deal_id: &str,
    action: &str,
    details_json: &str,
    created_at: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO stripe_settlement_outbox (
            deal_id, action, details_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![deal_id, action, details_json, created_at],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn get_stripe_settlement_outbox(
    conn: &Connection,
    deal_id: &str,
) -> Result<Option<StripeSettlementOutboxRecord>, String> {
    conn.query_row(
        "SELECT deal_id, action, details_json, created_at, updated_at
         FROM stripe_settlement_outbox
         WHERE deal_id = ?1",
        params![deal_id],
        decode_stripe_settlement_outbox_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub fn list_stripe_settlement_outbox(
    conn: &Connection,
) -> Result<Vec<StripeSettlementOutboxRecord>, String> {
    let mut statement = conn
        .prepare(
            "SELECT deal_id, action, details_json, created_at, updated_at
             FROM stripe_settlement_outbox
             ORDER BY updated_at ASC, deal_id ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], decode_stripe_settlement_outbox_row)
        .map_err(|error| error.to_string())?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|error| error.to_string())?);
    }
    Ok(records)
}

pub fn list_stripe_settlement_outbox_batch(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<StripeSettlementOutboxRecord>, String> {
    let mut statement = conn
        .prepare(
            "SELECT deal_id, action, details_json, created_at, updated_at
             FROM stripe_settlement_outbox
             ORDER BY updated_at ASC, deal_id ASC
             LIMIT ?1",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![limit.clamp(1, 1_000) as i64],
            decode_stripe_settlement_outbox_row,
        )
        .map_err(|error| error.to_string())?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|error| error.to_string())?);
    }
    Ok(records)
}

fn decode_stripe_settlement_outbox_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StripeSettlementOutboxRecord> {
    Ok(StripeSettlementOutboxRecord {
        deal_id: row.get(0)?,
        action: row.get(1)?,
        details_json: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

pub fn delete_stripe_settlement_outbox(conn: &Connection, deal_id: &str) -> Result<bool, String> {
    let deleted = conn
        .execute(
            "DELETE FROM stripe_settlement_outbox WHERE deal_id = ?1",
            params![deal_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(deleted > 0)
}

pub fn insert_deal_prepaid_invoice(
    conn: &Connection,
    deal_id: &str,
    bolt11: &str,
    payment_hash: &str,
    amount_sat: u64,
    created_at: i64,
) -> Result<(), String> {
    let amount_sat = u64_to_sqlite_integer(amount_sat, "prepaid invoice amount_sat")?;
    conn.execute(
        "INSERT INTO deal_prepaid_invoices (
            deal_id,
            bolt11,
            payment_hash,
            amount_sat,
            created_at,
            updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![deal_id, bolt11, payment_hash, amount_sat, created_at],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn get_deal_prepaid_invoice(
    conn: &Connection,
    deal_id: &str,
) -> Result<Option<DealPrepaidInvoiceRecord>, String> {
    conn.query_row(
        "SELECT deal_id, bolt11, payment_hash, amount_sat, created_at, updated_at
         FROM deal_prepaid_invoices
         WHERE deal_id = ?1",
        params![deal_id],
        |row| {
            let amount_sat: i64 = row.get(3)?;
            let amount_sat = u64::try_from(amount_sat)
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, amount_sat))?;
            Ok(DealPrepaidInvoiceRecord {
                deal_id: row.get(0)?,
                bolt11: row.get(1)?,
                payment_hash: row.get(2)?,
                amount_sat,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())
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

fn decode_publication_revision_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PublicationRevisionRecord> {
    let signed_revision_json: String = row.get(6)?;
    let definition_json: String = row.get(7)?;
    let signed_revision = serde_json::from_str(&signed_revision_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let definition = serde_json::from_str(&definition_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(PublicationRevisionRecord {
        revision_hash: row.get(0)?,
        service_id: row.get(1)?,
        offer_id: row.get(2)?,
        offer_hash: row.get(3)?,
        binding_hash: row.get(4)?,
        validation_status: row.get(5)?,
        signed_revision,
        definition,
        created_at: row.get(8)?,
    })
}

fn decode_publication_lifecycle_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PublicationLifecycleRecord> {
    let activation_token: String = row.get(4)?;
    if !is_publication_activation_token(&activation_token) {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stored publication activation token is not 64 lowercase hexadecimal characters",
            )),
        ));
    }
    let revision_count: i64 = row.get(5)?;
    Ok(PublicationLifecycleRecord {
        service_id: row.get(0)?,
        status: row.get(1)?,
        active_revision_hash: row.get(2)?,
        selected_revision_hash: row.get(3)?,
        activation_token,
        revision_count: revision_count.max(0) as u64,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

pub fn get_publication_revision(
    conn: &Connection,
    service_id: &str,
    revision_hash: &str,
) -> Result<Option<PublicationRevisionRecord>, String> {
    conn.query_row(
        "SELECT revision_hash, service_id, offer_id, offer_hash, binding_hash,
                validation_status, signed_revision_json, definition_json, created_at
         FROM publication_revisions
         WHERE service_id = ?1 AND revision_hash = ?2",
        params![service_id, revision_hash],
        decode_publication_revision_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub fn list_publication_revisions(
    conn: &Connection,
    service_id: &str,
    limit: usize,
) -> Result<Vec<PublicationRevisionRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT revision_hash, service_id, offer_id, offer_hash, binding_hash,
                    validation_status, signed_revision_json, definition_json, created_at
             FROM publication_revisions
             WHERE service_id = ?1
             ORDER BY created_at DESC, revision_hash ASC
             LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![service_id, limit.clamp(1, 100) as i64], |row| {
            decode_publication_revision_row(row)
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub fn get_publication_lifecycle(
    conn: &Connection,
    service_id: &str,
) -> Result<Option<PublicationLifecycleRecord>, String> {
    conn.query_row(
        "SELECT lifecycle.service_id, lifecycle.status,
                lifecycle.active_revision_hash, lifecycle.selected_revision_hash,
                lifecycle.activation_token,
                (SELECT COUNT(*) FROM publication_revisions revisions
                 WHERE revisions.service_id = lifecycle.service_id),
                lifecycle.created_at, lifecycle.updated_at
         FROM publication_service_lifecycle lifecycle
         WHERE lifecycle.service_id = ?1",
        params![service_id],
        decode_publication_lifecycle_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub fn list_publication_lifecycles(
    conn: &Connection,
) -> Result<Vec<PublicationLifecycleRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT lifecycle.service_id, lifecycle.status,
                    lifecycle.active_revision_hash, lifecycle.selected_revision_hash,
                    lifecycle.activation_token,
                    (SELECT COUNT(*) FROM publication_revisions revisions
                     WHERE revisions.service_id = lifecycle.service_id),
                    lifecycle.created_at, lifecycle.updated_at
             FROM publication_service_lifecycle lifecycle
             ORDER BY lifecycle.updated_at DESC, lifecycle.service_id ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([], decode_publication_lifecycle_row)
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn decode_publication_transport_grant_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PublicationTransportGrantRecord> {
    let activation_token: String = row.get(5)?;
    if !is_publication_activation_token(&activation_token) {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stored transport grant activation token is invalid",
            )),
        ));
    }
    Ok(PublicationTransportGrantRecord {
        transport: row.get(0)?,
        service_id: row.get(1)?,
        revision_hash: row.get(2)?,
        offer_id: row.get(3)?,
        offer_hash: row.get(4)?,
        activation_token,
        public_url: row.get(6)?,
        relay_control_url: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn list_publication_transport_grants_in_transaction(
    conn: &Connection,
    transport: &str,
) -> Result<Vec<PublicationTransportGrantRecord>, String> {
    let mut statement = conn
        .prepare(
            "SELECT grant_row.transport, grant_row.service_id,
                    grant_row.revision_hash, revision.offer_id, revision.offer_hash,
                    grant_row.activation_token, grant_row.public_url,
                    grant_row.relay_control_url,
                    grant_row.created_at, grant_row.updated_at
             FROM publication_transport_grants grant_row
             JOIN publication_service_lifecycle lifecycle
               ON lifecycle.service_id = grant_row.service_id
              AND lifecycle.status = 'active'
              AND lifecycle.active_revision_hash = grant_row.revision_hash
              AND lifecycle.activation_token = grant_row.activation_token
             JOIN publication_revisions revision
               ON revision.revision_hash = grant_row.revision_hash
              AND revision.service_id = grant_row.service_id
             WHERE grant_row.transport = ?1
             ORDER BY grant_row.updated_at ASC, grant_row.service_id ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![transport], decode_publication_transport_grant_row)
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub fn list_publication_transport_grants(
    conn: &Connection,
    transport: &str,
) -> Result<Vec<PublicationTransportGrantRecord>, String> {
    list_publication_transport_grants_in_transaction(conn, transport)
}

/// Persist a grant only when all three lifecycle coordinates identify the
/// exact currently active publication. Repeating the same grant is
/// idempotent; a newer exact activation for the same service replaces it.
pub fn persist_publication_transport_grant(
    conn: &Connection,
    grant: &NewPublicationTransportGrant<'_>,
) -> Result<PublicationTransportGrantRecord, String> {
    if grant.transport.is_empty() || grant.transport.len() > 32 || !grant.transport.is_ascii() {
        return Err("transport grant name must contain 1-32 ASCII bytes".to_string());
    }
    if !is_publication_activation_token(grant.activation_token) {
        return Err("transport grant activation token is invalid".to_string());
    }
    if grant.public_url.is_empty()
        || grant.public_url.len() > 2048
        || grant.public_url.chars().any(char::is_control)
    {
        return Err("transport grant public URL is invalid".to_string());
    }
    if grant.relay_control_url.is_empty()
        || grant.relay_control_url.len() > 2048
        || grant.relay_control_url.chars().any(char::is_control)
    {
        return Err("transport grant relay control URL is invalid".to_string());
    }
    with_immediate_transaction(conn, |conn| {
        let lifecycle = get_publication_lifecycle(conn, grant.service_id)?
            .ok_or_else(|| format!("publication service {} was not found", grant.service_id))?;
        if lifecycle.status != "active"
            || lifecycle.active_revision_hash.as_deref() != Some(grant.revision_hash)
            || lifecycle.activation_token != grant.activation_token
        {
            return Err(format!(
                "transport grant precondition mismatch for publication service {}",
                grant.service_id
            ));
        }
        conn.execute(
            "INSERT INTO publication_transport_grants (
                transport, service_id, revision_hash, activation_token,
                public_url, relay_control_url, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(transport, service_id) DO UPDATE SET
                revision_hash = excluded.revision_hash,
                activation_token = excluded.activation_token,
                public_url = excluded.public_url,
                relay_control_url = excluded.relay_control_url,
                updated_at = excluded.updated_at",
            params![
                grant.transport,
                grant.service_id,
                grant.revision_hash,
                grant.activation_token,
                grant.public_url,
                grant.relay_control_url,
                grant.now,
            ],
        )
        .map_err(|error| error.to_string())?;
        conn.query_row(
            "SELECT grant_row.transport, grant_row.service_id,
                    grant_row.revision_hash, revision.offer_id, revision.offer_hash,
                    grant_row.activation_token, grant_row.public_url,
                    grant_row.relay_control_url,
                    grant_row.created_at, grant_row.updated_at
             FROM publication_transport_grants grant_row
             JOIN publication_revisions revision
               ON revision.revision_hash = grant_row.revision_hash
              AND revision.service_id = grant_row.service_id
             WHERE grant_row.transport = ?1 AND grant_row.service_id = ?2",
            params![grant.transport, grant.service_id],
            decode_publication_transport_grant_row,
        )
        .map_err(|error| error.to_string())
    })
}

/// Withdraw one exact grant. Lifecycle and endpoint coordinates are all part
/// of the compare-and-delete, so delayed compensation cannot remove a newer
/// activation or a re-approved grant for changed relay configuration.
pub fn remove_publication_transport_grant_exact(
    conn: &Connection,
    transport: &str,
    service_id: &str,
    revision_hash: &str,
    activation_token: &str,
    public_url: &str,
    relay_control_url: &str,
) -> Result<bool, String> {
    with_immediate_transaction(conn, |conn| {
        conn.execute(
            "DELETE FROM publication_transport_grants
             WHERE transport = ?1
               AND service_id = ?2
               AND revision_hash = ?3
               AND activation_token = ?4
               AND public_url = ?5
               AND relay_control_url = ?6",
            params![
                transport,
                service_id,
                revision_hash,
                activation_token,
                public_url,
                relay_control_url,
            ],
        )
        .map(|removed| removed > 0)
        .map_err(|error| error.to_string())
    })
}

fn delete_publication_transport_grants_for_service_in_transaction(
    conn: &Connection,
    service_id: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM publication_transport_grants WHERE service_id = ?1",
        params![service_id],
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Fail closed after crashes or upgrades: only exact active lifecycle grants
/// survive and may cause startup reconnection.
pub fn reconcile_publication_transport_grants(
    conn: &Connection,
) -> Result<Vec<PublicationTransportGrantRecord>, String> {
    with_immediate_transaction(conn, |conn| {
        conn.execute(
            "DELETE FROM publication_transport_grants
             WHERE NOT EXISTS (
                SELECT 1
                FROM publication_service_lifecycle lifecycle
                WHERE lifecycle.service_id = publication_transport_grants.service_id
                  AND lifecycle.status = 'active'
                  AND lifecycle.active_revision_hash = publication_transport_grants.revision_hash
                  AND lifecycle.activation_token = publication_transport_grants.activation_token
             )",
            [],
        )
        .map_err(|error| error.to_string())?;
        list_publication_transport_grants_in_transaction(conn, "relay")
    })
}

pub fn list_publication_operations(
    conn: &Connection,
    service_id: &str,
    limit: usize,
) -> Result<Vec<PublicationOperationRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT operation_id, service_id, revision_hash, operation, status,
                    evidence_json, created_at
             FROM publication_operation_log
             WHERE service_id = ?1
             ORDER BY operation_id DESC
             LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![service_id, limit.clamp(1, 100) as i64], |row| {
            let evidence_json: String = row.get(5)?;
            let evidence = serde_json::from_str(&evidence_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(PublicationOperationRecord {
                operation_id: row.get(0)?,
                service_id: row.get(1)?,
                revision_hash: row.get(2)?,
                operation: row.get(3)?,
                status: row.get(4)?,
                evidence,
                created_at: row.get(6)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn insert_publication_revision(
    conn: &Connection,
    revision: &NewPublicationRevision,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO publication_revisions (
            revision_hash, service_id, offer_id, offer_hash, binding_hash,
            validation_status, signed_revision_json, definition_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'validated', ?6, ?7, ?8)",
        params![
            revision.revision_hash.as_str(),
            revision.service_id.as_str(),
            revision.offer_id.as_str(),
            revision.offer_hash.as_str(),
            revision.binding_hash.as_str(),
            revision.signed_revision_json.as_str(),
            revision.definition_json.as_str(),
            now,
        ],
    )
    .map_err(|error| error.to_string())?;

    let stored = conn
        .query_row(
            "SELECT service_id, offer_id, offer_hash, binding_hash,
                    signed_revision_json, definition_json
             FROM publication_revisions
             WHERE revision_hash = ?1",
            params![revision.revision_hash.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .map_err(|error| error.to_string())?;
    let expected = (
        revision.service_id.to_string(),
        revision.offer_id.to_string(),
        revision.offer_hash.to_string(),
        revision.binding_hash.to_string(),
        revision.signed_revision_json.to_string(),
        revision.definition_json.to_string(),
    );
    if stored != expected {
        return Err(format!(
            "publication revision {} conflicts with an existing immutable record",
            revision.revision_hash
        ));
    }
    Ok(())
}

fn insert_publication_operation(
    conn: &Connection,
    service_id: &str,
    revision_hash: Option<&str>,
    operation: &str,
    evidence_json: &str,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO publication_operation_log (
            service_id, revision_hash, operation, status, evidence_json, created_at
         ) VALUES (?1, ?2, ?3, 'succeeded', ?4, ?5)",
        params![service_id, revision_hash, operation, evidence_json, now],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn ensure_active_shared_offer_hash(
    conn: &Connection,
    service_id: &str,
    offer_id: &str,
    offer_hash: &str,
) -> Result<(), String> {
    let conflict = conn
        .query_row(
            "SELECT lifecycle.service_id, revision.offer_hash
             FROM publication_service_lifecycle lifecycle
             JOIN publication_revisions revision
               ON revision.revision_hash = lifecycle.active_revision_hash
             WHERE lifecycle.status = 'active'
               AND lifecycle.service_id != ?1
               AND revision.offer_id = ?2
               AND revision.offer_hash != ?3
             ORDER BY lifecycle.service_id ASC
             LIMIT 1",
            params![service_id, offer_id, offer_hash],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some((conflicting_service_id, conflicting_offer_hash)) = conflict {
        return Err(format!(
            "shared offer invariant violation: service {service_id} cannot activate offer_id {offer_id} with offer_hash {offer_hash}; active sibling {conflicting_service_id} uses {conflicting_offer_hash}"
        ));
    }
    Ok(())
}

/// Persist a publication revision while the caller owns an immediate writer
/// transaction. This is exposed separately so the exact Descriptor and Offer
/// documents/feed entries can share the revision/lifecycle commit.
pub fn persist_and_activate_publication_revision_in_transaction(
    conn: &Connection,
    revision: &NewPublicationRevision,
    initial_status: &str,
    evidence_json: &str,
    now: i64,
) -> Result<PublicationLifecycleRecord, String> {
    if !matches!(initial_status, "active" | "paused") {
        return Err(format!(
            "initial publication status must be active or paused, got {initial_status}"
        ));
    }
    insert_publication_revision(conn, revision, now)?;
    if initial_status == "active" {
        ensure_active_shared_offer_hash(
            conn,
            &revision.service_id,
            &revision.offer_id,
            &revision.offer_hash,
        )?;
    }
    upsert_provider_managed_offer(conn, &revision.offer_id, &revision.definition_json, now)?;
    let active_revision_hash =
        (initial_status == "active").then_some(revision.revision_hash.as_str());
    let current_lifecycle = get_publication_lifecycle(conn, &revision.service_id)?;
    let preserves_exact_activation = current_lifecycle.as_ref().is_some_and(|current| {
        current.status == initial_status && current.selected_revision_hash == revision.revision_hash
    });
    let activation_token = current_lifecycle
        .filter(|_| preserves_exact_activation)
        .map_or_else(new_publication_activation_token, |current| {
            current.activation_token
        });
    conn.execute(
        "INSERT INTO publication_service_lifecycle (
            service_id, status, active_revision_hash, selected_revision_hash,
            activation_token, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(service_id) DO UPDATE SET
            status = excluded.status,
            active_revision_hash = excluded.active_revision_hash,
            selected_revision_hash = excluded.selected_revision_hash,
            activation_token = excluded.activation_token,
            updated_at = excluded.updated_at",
        params![
            revision.service_id.as_str(),
            initial_status,
            active_revision_hash,
            revision.revision_hash.as_str(),
            activation_token,
            now,
        ],
    )
    .map_err(|error| error.to_string())?;
    if !preserves_exact_activation {
        delete_publication_transport_grants_for_service_in_transaction(conn, &revision.service_id)?;
    }
    insert_publication_operation(
        conn,
        &revision.service_id,
        Some(&revision.revision_hash),
        "publish",
        evidence_json,
        now,
    )?;
    get_publication_lifecycle(conn, &revision.service_id)?
        .ok_or_else(|| "publication lifecycle missing after activation".to_string())
}

pub fn persist_and_activate_publication_revision(
    conn: &Connection,
    revision: &NewPublicationRevision,
    initial_status: &str,
    evidence_json: &str,
    now: i64,
) -> Result<PublicationLifecycleRecord, String> {
    with_immediate_transaction(conn, |conn| {
        persist_and_activate_publication_revision_in_transaction(
            conn,
            revision,
            initial_status,
            evidence_json,
            now,
        )
    })
}

#[derive(Debug, Clone)]
pub enum PublicationLifecycleMutation {
    Pause,
    PauseExact {
        revision_hash: String,
        activation_token: String,
    },
    Resume,
    Unpublish,
    Rollback {
        revision_hash: String,
    },
}

impl PublicationLifecycleMutation {
    fn operation(&self) -> &'static str {
        match self {
            Self::Pause | Self::PauseExact { .. } => "pause",
            Self::Resume => "resume",
            Self::Unpublish => "unpublish",
            Self::Rollback { .. } => "rollback",
        }
    }
}

pub fn mutate_publication_lifecycle(
    conn: &Connection,
    service_id: &str,
    mutation: PublicationLifecycleMutation,
    evidence_json: &str,
    now: i64,
) -> Result<PublicationLifecycleRecord, String> {
    with_immediate_transaction(conn, |conn| {
        let current = get_publication_lifecycle(conn, service_id)?
            .ok_or_else(|| format!("publication service {service_id} was not found"))?;
        let operation_name = mutation.operation();
        let (target_status, target_revision_hash, changed, rotate_activation_token) = match mutation
        {
            PublicationLifecycleMutation::Pause => match current.status.as_str() {
                "active" => (
                    "paused",
                    current.selected_revision_hash.clone(),
                    true,
                    false,
                ),
                "paused" => (
                    "paused",
                    current.selected_revision_hash.clone(),
                    false,
                    false,
                ),
                "unpublished" => {
                    return Err("an unpublished service cannot be resumed or paused; publish a new revision or roll back explicitly".to_string());
                }
                status => return Err(format!("invalid stored publication status {status}")),
            },
            PublicationLifecycleMutation::PauseExact {
                revision_hash,
                activation_token,
            } => {
                if current.selected_revision_hash != revision_hash {
                    return Err(format!(
                        "publication revision precondition mismatch: service {service_id} currently selects {}, not {revision_hash}",
                        current.selected_revision_hash
                    ));
                }
                if current.activation_token != activation_token {
                    return Err(format!(
                        "publication activation token precondition mismatch: service {service_id} revision {revision_hash} no longer identifies the requested activation"
                    ));
                }
                match current.status.as_str() {
                    "active" => ("paused", revision_hash, true, false),
                    "paused" => ("paused", revision_hash, false, false),
                    "unpublished" => {
                        return Err("an unpublished service cannot be resumed or paused; publish a new revision or roll back explicitly".to_string());
                    }
                    status => return Err(format!("invalid stored publication status {status}")),
                }
            }
            PublicationLifecycleMutation::Resume => match current.status.as_str() {
                "paused" => ("active", current.selected_revision_hash.clone(), true, true),
                "active" => (
                    "active",
                    current.selected_revision_hash.clone(),
                    false,
                    true,
                ),
                "unpublished" => {
                    return Err("an unpublished service cannot be resumed; publish a new revision or roll back explicitly".to_string());
                }
                status => return Err(format!("invalid stored publication status {status}")),
            },
            PublicationLifecycleMutation::Unpublish => (
                "unpublished",
                current.selected_revision_hash.clone(),
                current.status != "unpublished",
                false,
            ),
            PublicationLifecycleMutation::Rollback { revision_hash } => {
                let revision = get_publication_revision(conn, service_id, &revision_hash)?
                    .ok_or_else(|| {
                        format!(
                            "validated local publication revision {revision_hash} was not found for service {service_id}"
                        )
                    })?;
                if revision.validation_status != "validated" {
                    return Err(format!(
                        "publication revision {revision_hash} is not validated"
                    ));
                }
                upsert_provider_managed_offer(
                    conn,
                    &revision.offer_id,
                    &serde_json::to_string(&revision.definition)
                        .map_err(|error| error.to_string())?,
                    now,
                )?;
                (
                    "active",
                    revision_hash.to_string(),
                    current.status != "active" || current.selected_revision_hash != revision_hash,
                    true,
                )
            }
        };

        if target_status == "active" {
            let target_revision = get_publication_revision(
                conn,
                service_id,
                &target_revision_hash,
            )?
            .ok_or_else(|| {
                format!(
                    "selected publication revision {target_revision_hash} is missing for service {service_id}"
                )
            })?;
            ensure_active_shared_offer_hash(
                conn,
                service_id,
                &target_revision.offer_id,
                &target_revision.offer_hash,
            )?;
        }

        if !changed {
            return Ok(current);
        }
        let activation_token = if rotate_activation_token {
            new_publication_activation_token()
        } else {
            current.activation_token
        };
        let active_revision_hash =
            (target_status == "active").then_some(target_revision_hash.as_str());
        conn.execute(
            "UPDATE publication_service_lifecycle
             SET status = ?2,
                 active_revision_hash = ?3,
                 selected_revision_hash = ?4,
                 activation_token = ?5,
                 updated_at = ?6
             WHERE service_id = ?1",
            params![
                service_id,
                target_status,
                active_revision_hash,
                target_revision_hash,
                activation_token,
                now,
            ],
        )
        .map_err(|error| error.to_string())?;
        // A lifecycle mutation that changes the exact revision/token always
        // withdraws its prior transport authorization. Resume/rollback must
        // be explicitly activated under their newly rotated token.
        delete_publication_transport_grants_for_service_in_transaction(conn, service_id)?;
        insert_publication_operation(
            conn,
            service_id,
            Some(&target_revision_hash),
            operation_name,
            evidence_json,
            now,
        )?;
        get_publication_lifecycle(conn, service_id)?
            .ok_or_else(|| "publication lifecycle missing after mutation".to_string())
    })
}

/// Atomically pause active services whose immutable signed revision belongs to
/// a different provider identity. Revisions and the selected revision remain
/// untouched for audit and explicit republishing; only active advertisement is
/// withdrawn.
pub fn pause_active_publications_for_provider_identity(
    conn: &Connection,
    current_provider_id: &str,
    now: i64,
) -> Result<Vec<String>, String> {
    if current_provider_id.len() != 64
        || !current_provider_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("current provider identity must be 64 lowercase hex characters".to_string());
    }
    with_immediate_transaction(conn, |conn| {
        let active = {
            let mut statement = conn
                .prepare(
                    "SELECT lifecycle.service_id, revision.revision_hash,
                            revision.signed_revision_json
                     FROM publication_service_lifecycle lifecycle
                     JOIN publication_revisions revision
                       ON revision.revision_hash = lifecycle.active_revision_hash
                     WHERE lifecycle.status = 'active'
                     ORDER BY lifecycle.service_id ASC",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| error.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?
        };

        let mut mismatches = Vec::new();
        for (service_id, revision_hash, signed_revision_json) in active {
            let signed: froglet_protocol::publication::SignedPublicationRevision =
                serde_json::from_str(&signed_revision_json).map_err(|error| {
                    format!(
                        "active publication {service_id} revision {revision_hash} is invalid: {error}"
                    )
                })?;
            signed.verify().map_err(|error| {
                format!(
                    "active publication {service_id} revision {revision_hash} failed signature verification: {error}"
                )
            })?;
            if signed.revision_hash != revision_hash {
                return Err(format!(
                    "active publication {service_id} selects revision {revision_hash}, but its signed document names {}",
                    signed.revision_hash
                ));
            }
            if signed.payload.provider_id == current_provider_id {
                continue;
            }
            mismatches.push((service_id, revision_hash, signed.payload.provider_id));
        }

        let mut paused = Vec::with_capacity(mismatches.len());
        for (service_id, revision_hash, revision_provider_id) in mismatches {
            let changed = conn
                .execute(
                    "UPDATE publication_service_lifecycle
                     SET status = 'paused', active_revision_hash = NULL, updated_at = ?3
                     WHERE service_id = ?1
                       AND status = 'active'
                       AND active_revision_hash = ?2",
                    params![service_id, revision_hash, now],
                )
                .map_err(|error| error.to_string())?;
            if changed != 1 {
                return Err(format!(
                    "active publication {service_id} changed during identity reconciliation"
                ));
            }
            delete_publication_transport_grants_for_service_in_transaction(conn, &service_id)?;
            let evidence_json = serde_json::to_string(&serde_json::json!({
                "schema_version": "froglet.publication-identity-reconciliation.v1",
                "reason": "provider_identity_changed",
                "revision_provider_id": revision_provider_id,
                "current_provider_id": current_provider_id,
                "required_action": "republish",
            }))
            .map_err(|error| error.to_string())?;
            insert_publication_operation(
                conn,
                &service_id,
                Some(&revision_hash),
                "pause",
                &evidence_json,
                now,
            )?;
            paused.push(service_id);
        }
        Ok(paused)
    })
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

pub fn list_duplicate_quote_hashes(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<(String, u64)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT quote_hash, COUNT(*) AS duplicate_count
             FROM deals
             WHERE quote_hash IS NOT NULL AND quote_hash != ''
             GROUP BY quote_hash
             HAVING COUNT(*) > 1
             ORDER BY duplicate_count DESC, quote_hash ASC
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

    #[cfg(unix)]
    fn assert_private_file(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(path)
            .unwrap_or_else(|error| panic!("metadata for {}: {error}", path.display()))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode,
            0o600,
            "{} must be owner-readable and owner-writable only (mode {mode:o})",
            path.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn initialize_db_creates_private_database_and_wal_sidecars() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary database directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
            .expect("host-readable data directory");
        let db_path = directory.path().join("node.db");

        let conn = initialize_db(&db_path).expect("initialize private database");
        conn.execute(
            "INSERT INTO events (id, pubkey, created_at, kind, content, sig, tags)
             VALUES ('permission-event', 'pk', 1, 'test', '', 'sig', '[]')",
            [],
        )
        .expect("force WAL write");

        assert_private_file(&db_path);
        assert_private_file(&sqlite_sidecar_path(&db_path, "-wal"));
        assert_private_file(&sqlite_sidecar_path(&db_path, "-shm"));
    }

    #[cfg(unix)]
    #[test]
    fn initialize_db_repairs_existing_database_and_sidecar_modes() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary database directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
            .expect("host-readable data directory");
        let db_path = directory.path().join("node.db");
        let existing = Connection::open(&db_path).expect("open legacy database");
        existing
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE legacy_permissions (value TEXT);
                 INSERT INTO legacy_permissions VALUES ('sensitive');",
            )
            .expect("create legacy WAL files");
        let wal_path = sqlite_sidecar_path(&db_path, "-wal");
        let shm_path = sqlite_sidecar_path(&db_path, "-shm");
        for path in [&db_path, &wal_path, &shm_path] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).expect("seed legacy mode");
        }

        let repaired = initialize_db(&db_path).expect("reopen and repair database modes");
        assert_private_file(&db_path);
        assert_private_file(&wal_path);
        assert_private_file(&shm_path);
        drop(repaired);
        drop(existing);
    }

    #[cfg(unix)]
    #[test]
    fn db_pool_custom_initializers_still_create_private_sqlite_files() {
        use std::os::unix::fs::PermissionsExt;

        fn custom_write(path: &Path) -> SqlResult<Connection> {
            let conn = Connection::open(path)?;
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE custom_initializer (value TEXT);
                 INSERT INTO custom_initializer VALUES ('sensitive');",
            )?;
            Ok(conn)
        }

        fn custom_reader(path: &Path) -> SqlResult<Connection> {
            Connection::open(path)
        }

        let directory = tempfile::tempdir().expect("temporary database directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
            .expect("host-readable data directory");
        let db_path = directory.path().join("node.db");

        let pool = DbPool::open_with(&db_path, custom_write, custom_reader)
            .expect("open pool through custom library hooks");
        assert_private_file(&db_path);
        assert_private_file(&sqlite_sidecar_path(&db_path, "-wal"));
        assert_private_file(&sqlite_sidecar_path(&db_path, "-shm"));
        drop(pool);
    }

    #[test]
    fn unsigned_values_are_checked_before_sqlite_integer_storage() {
        assert_eq!(
            u64_to_sqlite_integer(i64::MAX as u64, "test amount").unwrap(),
            i64::MAX
        );
        let error = u64_to_sqlite_integer(i64::MAX as u64 + 1, "test amount")
            .expect_err("value above SQLite INTEGER range must be rejected");
        assert!(error.contains("test amount"), "error: {error}");

        let conn = Connection::open_in_memory().expect("open in-memory db");
        configure_connection(&conn).expect("configure");
        let error = insert_deal_prepaid_invoice(
            &conn,
            "deal-overflow",
            "bolt11",
            "payment-hash-overflow",
            i64::MAX as u64 + 1,
            1,
        )
        .expect_err("prepaid amount above SQLite INTEGER range must be rejected");
        assert!(
            error.contains("prepaid invoice amount_sat"),
            "error: {error}"
        );
    }

    #[test]
    fn numeric_schema_constraints_reject_negative_amounts() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        configure_connection(&conn).expect("configure");

        let quote = conn.execute(
            "INSERT INTO quotes (
                quote_id, artifact_hash, offer_id, service_id, workload_hash,
                expires_at, price_sats, quote_json, created_at
             ) VALUES ('quote', 'quote-artifact', 'offer', 'service', 'workload',
                       2, -1, '{}', 1)",
            [],
        );
        let quote = quote.expect_err("negative quote price must be rejected");
        assert!(
            quote.to_string().contains("invalid quote price")
                || quote.to_string().contains("price_sats >= 0"),
            "unexpected error: {quote}"
        );

        let deal = conn.execute(
            "INSERT INTO deals (
                deal_id, quote_id, quote_hash, offer_id, service_id,
                workload_hash, spec_json, quote_json, deal_artifact_json,
                status, payment_amount_sats, created_at, updated_at
             ) VALUES ('deal', 'quote', 'quote-hash', 'offer', 'service',
                       'workload', '{}', '{}', '{}', 'accepted', -1, 1, 1)",
            [],
        );
        let deal = deal.expect_err("negative deal payment must be rejected");
        assert!(
            deal.to_string().contains("invalid deal payment amount")
                || deal.to_string().contains("payment_amount_sats"),
            "unexpected error: {deal}"
        );

        let lightning = conn.execute(
            "INSERT INTO lightning_invoice_bundles (
                session_id, provider_id, requester_id, quote_hash, deal_hash,
                destination_identity, base_invoice_hash, base_payment_hash,
                base_fee_msat, base_state, success_invoice_hash,
                success_payment_hash, success_fee_msat, success_state,
                bundle_json, created_at, updated_at
             ) VALUES (
                'session', 'provider', 'requester', 'quote', 'deal',
                'destination', 'base-invoice', 'base-payment', -1, 'pending',
                'success-invoice', 'success-payment', 0, 'pending', '{}', 1, 1
             )",
            [],
        );
        let lightning = lightning.expect_err("negative lightning fee must be rejected");
        assert!(
            lightning
                .to_string()
                .contains("lightning invoice bundle amount")
                || lightning.to_string().contains("base_fee_msat >= 0"),
            "unexpected error: {lightning}"
        );

        let prepaid = conn.execute(
            "INSERT INTO deal_prepaid_invoices (
                deal_id, bolt11, payment_hash, amount_sat, created_at, updated_at
             ) VALUES ('deal', 'bolt11', 'payment-hash', -1, 1, 1)",
            [],
        );
        let prepaid = prepaid.expect_err("negative prepaid amount must be rejected");
        assert!(
            prepaid.to_string().contains("prepaid invoice amount")
                || prepaid.to_string().contains("amount_sat >= 0"),
            "unexpected error: {prepaid}"
        );

        let ledger = conn.execute(
            "INSERT INTO requester_spend_ledger (
                deal_hash, provider_id, amount_msat, settlement_method,
                state, created_at, updated_at
             ) VALUES ('deal-hash', 'provider', -1, 'lightning', 'reserved', 1, 1)",
            [],
        );
        let ledger = ledger.expect_err("negative requester spend must be rejected");
        assert!(
            ledger.to_string().contains("requester spend ledger row")
                || ledger.to_string().contains("amount_msat >= 0"),
            "unexpected error: {ledger}"
        );
    }

    #[test]
    fn prepaid_invoice_reader_rejects_legacy_negative_amount() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE deal_prepaid_invoices (
                deal_id TEXT PRIMARY KEY,
                bolt11 TEXT NOT NULL,
                payment_hash TEXT NOT NULL,
                amount_sat INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
             );
             INSERT INTO deal_prepaid_invoices
                (deal_id, bolt11, payment_hash, amount_sat, created_at, updated_at)
             VALUES ('deal', 'bolt11', 'payment-hash', -1, 1, 1);",
        )
        .expect("seed legacy negative amount");

        let error = get_deal_prepaid_invoice(&conn, "deal")
            .expect_err("legacy negative amount must not be clamped to zero");
        assert!(error.contains("out of range"), "error: {error}");
    }

    fn test_event(id: &str) -> NodeEventEnvelope {
        NodeEventEnvelope {
            id: id.to_string(),
            pubkey: "test-pubkey".to_string(),
            created_at: 1,
            kind: "test-kind".to_string(),
            tags: Vec::new(),
            content: "test-content".to_string(),
            sig: "test-signature".to_string(),
        }
    }

    fn test_publication_revision(
        revision_hash: &str,
        service_id: &str,
        offer_id: &str,
    ) -> NewPublicationRevision {
        NewPublicationRevision {
            revision_hash: revision_hash.to_string(),
            service_id: service_id.to_string(),
            offer_id: offer_id.to_string(),
            offer_hash: format!("offer-{revision_hash}"),
            binding_hash: format!("binding-{revision_hash}"),
            signed_revision_json: format!(r#"{{"revision_hash":"{revision_hash}"}}"#),
            definition_json: format!(r#"{{"offer_id":"{offer_id}","service_id":"{service_id}"}}"#),
        }
    }

    #[test]
    fn failed_immediate_transaction_commit_rolls_back_and_releases_connection() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute_batch(
            "CREATE TABLE deferred_parent (id INTEGER PRIMARY KEY);
             CREATE TABLE deferred_child (
                 parent_id INTEGER NOT NULL,
                 FOREIGN KEY (parent_id) REFERENCES deferred_parent(id)
                     DEFERRABLE INITIALLY DEFERRED
             );",
        )
        .expect("deferred foreign-key fixture");

        let error = with_immediate_transaction(&conn, |transaction| {
            transaction
                .execute("INSERT INTO deferred_child (parent_id) VALUES (7)", [])
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect_err("deferred constraint must reject COMMIT");
        assert!(error.contains("FOREIGN KEY constraint failed"));
        assert!(
            conn.is_autocommit(),
            "a failed RAII COMMIT must roll back before returning the pooled connection"
        );
        let child_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM deferred_child", [], |row| row.get(0))
            .expect("rolled-back child count");
        assert_eq!(child_count, 0);
        conn.execute("INSERT INTO deferred_parent (id) VALUES (7)", [])
            .expect("connection remains writable after failed commit");
    }

    #[test]
    fn unwinding_immediate_transaction_rolls_back_and_releases_connection() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute("CREATE TABLE unwind_guard (value INTEGER NOT NULL)", [])
            .expect("unwind fixture");

        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Result<(), String> = with_immediate_transaction(&conn, |transaction| {
                transaction
                    .execute("INSERT INTO unwind_guard (value) VALUES (1)", [])
                    .map_err(|error| error.to_string())?;
                panic!("injected transaction unwind");
            });
        }));
        assert!(unwind.is_err(), "injected panic must unwind");
        assert!(
            conn.is_autocommit(),
            "unwinding transaction must return connection to autocommit"
        );
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM unwind_guard", [], |row| row.get(0))
            .expect("rolled-back row count");
        assert_eq!(row_count, 0);
        conn.execute("INSERT INTO unwind_guard (value) VALUES (2)", [])
            .expect("connection remains writable after unwind");
    }

    #[test]
    fn pooled_connection_recovers_after_transaction_unwind() {
        let db_path = temp_db_path("pooled-unwind");
        let pool = DbPool::open(&db_path).expect("open pool");
        let runtime = Runtime::new().expect("tokio runtime");
        runtime
            .block_on(pool.with_write_conn(|conn| {
                conn.execute("CREATE TABLE pooled_unwind (value INTEGER NOT NULL)", [])
                    .map(|_| ())
            }))
            .expect("create unwind fixture");

        let error = runtime
            .block_on(pool.with_write_conn(|conn| {
                with_immediate_transaction(conn, |transaction| -> Result<(), String> {
                    transaction
                        .execute("INSERT INTO pooled_unwind (value) VALUES (1)", [])
                        .map_err(|error| error.to_string())?;
                    panic!("injected pooled transaction unwind");
                })
            }))
            .expect_err("the panicking database task must fail");
        assert!(error.contains("database task join error"));

        let values: i64 = runtime
            .block_on(pool.with_write_conn(|conn| {
                assert!(conn.is_autocommit());
                conn.execute("INSERT INTO pooled_unwind (value) VALUES (2)", [])
                    .map_err(|error| error.to_string())?;
                conn.query_row("SELECT SUM(value) FROM pooled_unwind", [], |row| row.get(0))
                    .map_err(|error| error.to_string())
            }))
            .expect("poisoned pool recovers for the next writer");
        assert_eq!(values, 2_i64, "the unwound insert must have rolled back");

        drop(pool);
        drop(runtime);
        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn schema_batch_failure_rolls_back_and_allows_configuration_retry() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute("CREATE VIEW jobs AS SELECT 1 AS job_id", [])
            .expect("conflicting schema fixture");

        configure_connection(&conn).expect_err("conflicting view must reject schema batch");
        assert!(
            conn.is_autocommit(),
            "failed schema batch must release transaction"
        );
        let events_object_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'events'",
                [],
                |row| row.get(0),
            )
            .expect("events object count");
        assert_eq!(
            events_object_count, 0,
            "objects created before the schema error must roll back"
        );

        conn.execute("DROP VIEW jobs", [])
            .expect("connection remains writable");
        configure_connection(&conn).expect("configuration retry");
    }

    #[test]
    fn batch_event_commit_failure_rolls_back_and_connection_is_reusable() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute_batch(
            "CREATE TABLE event_commit_parent (id INTEGER PRIMARY KEY);
             CREATE TABLE event_commit_child (
                 parent_id INTEGER NOT NULL,
                 FOREIGN KEY (parent_id) REFERENCES event_commit_parent(id)
                     DEFERRABLE INITIALLY DEFERRED
             );
             CREATE TRIGGER event_commit_guard
             AFTER INSERT ON events
             BEGIN
                 INSERT INTO event_commit_child (parent_id) VALUES (7);
             END;",
        )
        .expect("deferred event fixture");
        let events = [test_event("event-1"), test_event("event-2")];

        let error = batch_insert_events(&conn, &events)
            .expect_err("deferred constraint must reject batch COMMIT");
        assert!(error.to_string().contains("FOREIGN KEY constraint failed"));
        assert!(
            conn.is_autocommit(),
            "failed batch must release transaction"
        );
        let event_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .expect("rolled-back event count");
        let guard_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM event_commit_child", [], |row| {
                row.get(0)
            })
            .expect("rolled-back guard count");
        assert_eq!((event_count, guard_count), (0, 0));

        conn.execute("INSERT INTO event_commit_parent (id) VALUES (7)", [])
            .expect("connection remains writable");
        assert_eq!(
            batch_insert_events(&conn, &events).expect("retry batch"),
            vec![true, true]
        );
    }

    #[test]
    fn migration_commit_failure_rolls_back_marker_and_allows_retry() {
        const TEST_MIGRATION: &str = "test_deferred_commit_migration";

        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute_batch(
            "CREATE TABLE migration_commit_parent (id INTEGER PRIMARY KEY);
             CREATE TABLE migration_commit_child (
                 parent_id INTEGER NOT NULL,
                 FOREIGN KEY (parent_id) REFERENCES migration_commit_parent(id)
                     DEFERRABLE INITIALLY DEFERRED
             );",
        )
        .expect("deferred migration fixture");

        let apply = |transaction: &Connection| {
            transaction
                .execute(
                    "INSERT INTO migration_commit_child (parent_id) VALUES (9)",
                    [],
                )
                .map(|_| ())
        };
        let error = apply_migration_once(&conn, TEST_MIGRATION, apply)
            .expect_err("deferred constraint must reject migration COMMIT");
        assert!(error.to_string().contains("FOREIGN KEY constraint failed"));
        assert!(
            conn.is_autocommit(),
            "failed migration must release transaction"
        );
        let marker_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE name = ?1",
                params![TEST_MIGRATION],
                |row| row.get(0),
            )
            .expect("rolled-back migration marker count");
        assert_eq!(marker_count, 0);

        conn.execute("INSERT INTO migration_commit_parent (id) VALUES (9)", [])
            .expect("connection remains writable");
        apply_migration_once(&conn, TEST_MIGRATION, apply).expect("migration retry");
        let marker_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE name = ?1",
                params![TEST_MIGRATION],
                |row| row.get(0),
            )
            .expect("committed migration marker count");
        assert_eq!(marker_count, 1);
    }

    #[test]
    fn publication_revision_storage_is_immutable_and_activation_is_separate() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        let first = test_publication_revision("revision-1", "service-1", "offer-1");
        let lifecycle = persist_and_activate_publication_revision(
            &conn,
            &first,
            "active",
            r#"{"operation":"publish"}"#,
            10,
        )
        .expect("first activation");
        assert_eq!(lifecycle.status, "active");
        assert_eq!(
            lifecycle.active_revision_hash.as_deref(),
            Some("revision-1")
        );
        assert!(is_publication_activation_token(&lifecycle.activation_token));
        let first_activation_token = lifecycle.activation_token.clone();

        let idempotent = persist_and_activate_publication_revision(
            &conn,
            &first,
            "active",
            r#"{"operation":"publish"}"#,
            11,
        )
        .expect("idempotent activation");
        assert_eq!(idempotent.activation_token, first_activation_token);
        assert_eq!(
            list_publication_revisions(&conn, "service-1", 100)
                .expect("revision history")
                .len(),
            1
        );
        assert_eq!(
            list_publication_operations(&conn, "service-1", 100)
                .expect("operation history")
                .len(),
            1
        );

        let mut collision = first.clone();
        collision.definition_json =
            r#"{"offer_id":"offer-1","service_id":"service-1","changed":true}"#.to_string();
        let error = persist_and_activate_publication_revision(
            &conn,
            &collision,
            "active",
            r#"{"operation":"publish"}"#,
            12,
        )
        .expect_err("same revision hash cannot replace immutable snapshot");
        assert!(error.contains("conflicts with an existing immutable record"));
        let stored = get_publication_revision(&conn, "service-1", "revision-1")
            .expect("revision lookup")
            .expect("stored revision");
        assert!(stored.definition.get("changed").is_none());

        let paused = mutate_publication_lifecycle(
            &conn,
            "service-1",
            PublicationLifecycleMutation::Pause,
            r#"{"operation":"pause"}"#,
            13,
        )
        .expect("pause");
        assert_eq!(paused.status, "paused");
        assert!(paused.active_revision_hash.is_none());
        assert_eq!(paused.activation_token, first_activation_token);
        let unpublished = mutate_publication_lifecycle(
            &conn,
            "service-1",
            PublicationLifecycleMutation::Unpublish,
            r#"{"operation":"unpublish"}"#,
            14,
        )
        .expect("unpublish");
        assert_eq!(unpublished.status, "unpublished");
        assert_eq!(unpublished.activation_token, first_activation_token);
        let error = mutate_publication_lifecycle(
            &conn,
            "service-1",
            PublicationLifecycleMutation::Resume,
            r#"{"operation":"resume"}"#,
            15,
        )
        .expect_err("resume must not reactivate unpublished service");
        assert!(error.contains("cannot be resumed"));
        let rolled_back = mutate_publication_lifecycle(
            &conn,
            "service-1",
            PublicationLifecycleMutation::Rollback {
                revision_hash: "revision-1".to_string(),
            },
            r#"{"operation":"rollback"}"#,
            16,
        )
        .expect("explicit rollback reactivates");
        assert_eq!(rolled_back.status, "active");
        assert_eq!(
            rolled_back.active_revision_hash.as_deref(),
            Some("revision-1")
        );
        assert_ne!(rolled_back.activation_token, first_activation_token);
    }

    #[test]
    fn exact_pause_is_idempotent_and_never_pauses_a_newer_revision() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        let first = test_publication_revision("revision-1", "service-1", "offer-1");
        let second = test_publication_revision("revision-2", "service-1", "offer-1");
        let first_lifecycle = persist_and_activate_publication_revision(
            &conn,
            &first,
            "active",
            r#"{"operation":"publish"}"#,
            10,
        )
        .expect("first activation");
        let second_lifecycle = persist_and_activate_publication_revision(
            &conn,
            &second,
            "active",
            r#"{"operation":"publish"}"#,
            11,
        )
        .expect("second activation");

        let error = mutate_publication_lifecycle(
            &conn,
            "service-1",
            PublicationLifecycleMutation::PauseExact {
                revision_hash: "revision-1".to_string(),
                activation_token: first_lifecycle.activation_token,
            },
            r#"{"operation":"pause"}"#,
            12,
        )
        .expect_err("stale compensation must not pause a newer revision");
        assert!(error.contains("revision precondition mismatch"));
        let active = get_publication_lifecycle(&conn, "service-1")
            .expect("lifecycle lookup")
            .expect("lifecycle");
        assert_eq!(active.status, "active");
        assert_eq!(active.active_revision_hash.as_deref(), Some("revision-2"));

        let paused = mutate_publication_lifecycle(
            &conn,
            "service-1",
            PublicationLifecycleMutation::PauseExact {
                revision_hash: "revision-2".to_string(),
                activation_token: second_lifecycle.activation_token.clone(),
            },
            r#"{"operation":"pause"}"#,
            13,
        )
        .expect("exact compensation pause");
        assert_eq!(paused.status, "paused");
        let repeated = mutate_publication_lifecycle(
            &conn,
            "service-1",
            PublicationLifecycleMutation::PauseExact {
                revision_hash: "revision-2".to_string(),
                activation_token: second_lifecycle.activation_token.clone(),
            },
            r#"{"operation":"pause"}"#,
            14,
        )
        .expect("exact compensation is idempotent");
        assert_eq!(repeated.status, "paused");
        assert_eq!(repeated.activation_token, second_lifecycle.activation_token);
        assert_eq!(
            list_publication_operations(&conn, "service-1", 100)
                .expect("operation history")
                .iter()
                .filter(|operation| operation.operation == "pause")
                .count(),
            1
        );

        let resumed = mutate_publication_lifecycle(
            &conn,
            "service-1",
            PublicationLifecycleMutation::Resume,
            r#"{"operation":"resume"}"#,
            15,
        )
        .expect("resume exact revision");
        assert_eq!(resumed.selected_revision_hash, "revision-2");
        assert_ne!(resumed.activation_token, repeated.activation_token);

        let stale_error = mutate_publication_lifecycle(
            &conn,
            "service-1",
            PublicationLifecycleMutation::PauseExact {
                revision_hash: "revision-2".to_string(),
                activation_token: repeated.activation_token,
            },
            r#"{"operation":"pause"}"#,
            16,
        )
        .expect_err("delayed compensation cannot pause a resumed activation");
        assert!(stale_error.contains("activation token precondition mismatch"));
        let still_active = get_publication_lifecycle(&conn, "service-1")
            .expect("lifecycle lookup")
            .expect("lifecycle");
        assert_eq!(still_active.status, "active");
        assert_eq!(still_active.activation_token, resumed.activation_token);
    }

    #[test]
    fn transport_grants_are_exact_durable_and_independent_per_service() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        let first = test_publication_revision("revision-a", "service-a", "offer-a");
        let second = test_publication_revision("revision-b", "service-b", "offer-b");
        let first_lifecycle = persist_and_activate_publication_revision(
            &conn,
            &first,
            "active",
            r#"{"operation":"publish"}"#,
            10,
        )
        .expect("first publication");
        let second_lifecycle = persist_and_activate_publication_revision(
            &conn,
            &second,
            "active",
            r#"{"operation":"publish"}"#,
            11,
        )
        .expect("second publication");
        let public_url = "https://identity.relay.example";
        let relay_control_url = "wss://control.relay.example/v1/tunnel";
        persist_publication_transport_grant(
            &conn,
            &NewPublicationTransportGrant {
                transport: "relay",
                service_id: "service-a",
                revision_hash: "revision-a",
                activation_token: &first_lifecycle.activation_token,
                public_url,
                relay_control_url,
                now: 12,
            },
        )
        .expect("first grant");
        persist_publication_transport_grant(
            &conn,
            &NewPublicationTransportGrant {
                transport: "relay",
                service_id: "service-b",
                revision_hash: "revision-b",
                activation_token: &second_lifecycle.activation_token,
                public_url,
                relay_control_url,
                now: 13,
            },
        )
        .expect("second grant");
        assert_eq!(
            list_publication_transport_grants(&conn, "relay")
                .expect("grants")
                .len(),
            2
        );
        assert!(
            !remove_publication_transport_grant_exact(
                &conn,
                "relay",
                "service-a",
                "revision-a",
                &first_lifecycle.activation_token,
                public_url,
                "wss://different-control.relay.example/v1/tunnel",
            )
            .expect("stale endpoint withdrawal"),
            "endpoint-stale compensation must not remove the current grant"
        );

        // Repeating the exact publication preserves its authorization.
        let repeated = persist_and_activate_publication_revision(
            &conn,
            &first,
            "active",
            r#"{"operation":"publish"}"#,
            14,
        )
        .expect("idempotent publication");
        assert_eq!(repeated.activation_token, first_lifecycle.activation_token);
        assert_eq!(
            list_publication_transport_grants(&conn, "relay")
                .expect("grants")
                .len(),
            2
        );

        mutate_publication_lifecycle(
            &conn,
            "service-a",
            PublicationLifecycleMutation::PauseExact {
                revision_hash: "revision-a".to_string(),
                activation_token: first_lifecycle.activation_token,
            },
            r#"{"operation":"pause"}"#,
            15,
        )
        .expect("pause first");
        let remaining = list_publication_transport_grants(&conn, "relay").expect("remaining");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].service_id, "service-b");

        let resumed = mutate_publication_lifecycle(
            &conn,
            "service-a",
            PublicationLifecycleMutation::Resume,
            r#"{"operation":"resume"}"#,
            16,
        )
        .expect("resume first");
        assert_ne!(resumed.activation_token, repeated.activation_token);
        assert_eq!(
            list_publication_transport_grants(&conn, "relay")
                .expect("second grant survives")
                .len(),
            1,
            "resume must require a new exact grant without dropping another service"
        );
    }

    #[test]
    fn startup_transport_grant_reconciliation_removes_stale_rows() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        let revision = test_publication_revision("revision-a", "service-a", "offer-a");
        let lifecycle = persist_and_activate_publication_revision(
            &conn,
            &revision,
            "active",
            r#"{"operation":"publish"}"#,
            10,
        )
        .expect("publication");
        persist_publication_transport_grant(
            &conn,
            &NewPublicationTransportGrant {
                transport: "relay",
                service_id: "service-a",
                revision_hash: "revision-a",
                activation_token: &lifecycle.activation_token,
                public_url: "https://identity.relay.example",
                relay_control_url: "wss://control.relay.example/v1/tunnel",
                now: 11,
            },
        )
        .expect("grant");

        conn.execute(
            "UPDATE publication_service_lifecycle
             SET status = 'paused', active_revision_hash = NULL
             WHERE service_id = 'service-a'",
            [],
        )
        .expect("simulate legacy stale grant");
        assert!(
            reconcile_publication_transport_grants(&conn)
                .expect("reconcile")
                .is_empty()
        );
        assert!(
            list_publication_transport_grants(&conn, "relay")
                .expect("grants")
                .is_empty()
        );
    }

    #[test]
    fn configure_connection_backfills_legacy_publication_activation_token_once() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE publication_revisions (
                revision_hash TEXT PRIMARY KEY,
                service_id TEXT NOT NULL,
                offer_id TEXT NOT NULL,
                offer_hash TEXT NOT NULL,
                binding_hash TEXT NOT NULL,
                validation_status TEXT NOT NULL CHECK (validation_status = 'validated'),
                signed_revision_json TEXT NOT NULL,
                definition_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE publication_service_lifecycle (
                service_id TEXT PRIMARY KEY,
                status TEXT NOT NULL CHECK (status IN ('active', 'paused', 'unpublished')),
                active_revision_hash TEXT,
                selected_revision_hash TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (active_revision_hash) REFERENCES publication_revisions(revision_hash),
                FOREIGN KEY (selected_revision_hash) REFERENCES publication_revisions(revision_hash)
             );
             INSERT INTO publication_revisions (
                revision_hash, service_id, offer_id, offer_hash, binding_hash,
                validation_status, signed_revision_json, definition_json, created_at
             ) VALUES (
                'legacy-revision', 'legacy-service', 'legacy-offer', 'legacy-offer-hash',
                'legacy-binding-hash', 'validated', '{}', '{}', 10
             );
             INSERT INTO publication_service_lifecycle (
                service_id, status, active_revision_hash, selected_revision_hash,
                created_at, updated_at
             ) VALUES (
                'legacy-service', 'active', 'legacy-revision', 'legacy-revision', 10, 10
             );",
        )
        .expect("seed pre-token publication schema");

        configure_connection(&conn).expect("migrate legacy lifecycle");
        let migrated = get_publication_lifecycle(&conn, "legacy-service")
            .expect("migrated lifecycle query")
            .expect("migrated lifecycle");
        assert!(is_publication_activation_token(&migrated.activation_token));

        configure_connection(&conn).expect("repeat configuration");
        let repeated = get_publication_lifecycle(&conn, "legacy-service")
            .expect("reconfigured lifecycle query")
            .expect("reconfigured lifecycle");
        assert_eq!(repeated.activation_token, migrated.activation_token);
    }

    #[test]
    fn shared_offer_hash_invariant_is_atomic_across_publication_and_activation() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        let first = test_publication_revision("shared-a-1", "shared-a", "shared-offer");
        persist_and_activate_publication_revision(
            &conn,
            &first,
            "active",
            r#"{"operation":"publish"}"#,
            10,
        )
        .expect("first shared offer activation");

        let incompatible =
            test_publication_revision("shared-b-incompatible", "shared-b", "shared-offer");
        let error = persist_and_activate_publication_revision(
            &conn,
            &incompatible,
            "active",
            r#"{"operation":"publish"}"#,
            11,
        )
        .expect_err("a different exact offer hash must not activate");
        assert!(error.contains("shared offer invariant violation"));
        assert!(
            get_publication_revision(&conn, "shared-b", "shared-b-incompatible")
                .expect("incompatible revision lookup")
                .is_none(),
            "the rejected activation transaction must roll back its revision insert"
        );
        assert!(
            get_publication_lifecycle(&conn, "shared-b")
                .expect("incompatible lifecycle lookup")
                .is_none()
        );

        let mut compatible =
            test_publication_revision("shared-b-compatible", "shared-b", "shared-offer");
        compatible.offer_hash.clone_from(&first.offer_hash);
        persist_and_activate_publication_revision(
            &conn,
            &compatible,
            "active",
            r#"{"operation":"publish"}"#,
            12,
        )
        .expect("the same exact shared offer may activate");

        let alternate = test_publication_revision("shared-a-alternate", "shared-a", "shared-offer");
        persist_and_activate_publication_revision(
            &conn,
            &alternate,
            "paused",
            r#"{"operation":"publish"}"#,
            13,
        )
        .expect("an inactive alternative revision may be retained");
        let error = mutate_publication_lifecycle(
            &conn,
            "shared-a",
            PublicationLifecycleMutation::Resume,
            r#"{"operation":"resume"}"#,
            14,
        )
        .expect_err("resume must recheck the exact shared offer");
        assert!(error.contains("shared offer invariant violation"));
        let paused = get_publication_lifecycle(&conn, "shared-a")
            .expect("paused lifecycle lookup")
            .expect("paused lifecycle");
        assert_eq!(paused.status, "paused");
        assert!(paused.active_revision_hash.is_none());

        let error = mutate_publication_lifecycle(
            &conn,
            "shared-a",
            PublicationLifecycleMutation::Rollback {
                revision_hash: alternate.revision_hash.clone(),
            },
            r#"{"operation":"rollback"}"#,
            15,
        )
        .expect_err("rollback must recheck the exact shared offer");
        assert!(error.contains("shared offer invariant violation"));
        let restored = mutate_publication_lifecycle(
            &conn,
            "shared-a",
            PublicationLifecycleMutation::Rollback {
                revision_hash: first.revision_hash.clone(),
            },
            r#"{"operation":"rollback"}"#,
            16,
        )
        .expect("rollback to the exact active shared offer");
        assert_eq!(
            restored.active_revision_hash.as_deref(),
            Some(first.revision_hash.as_str())
        );
    }

    #[test]
    fn concurrent_shared_offer_activations_cannot_commit_different_hashes() {
        use std::sync::Barrier;

        let db_path = temp_db_path("shared-offer-concurrency");
        let setup = Connection::open(&db_path).expect("open shared-offer db");
        configure_connection(&setup).expect("configure shared-offer db");
        drop(setup);

        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for revision in [
            test_publication_revision("race-a", "race-service-a", "race-offer"),
            test_publication_revision("race-b", "race-service-b", "race-offer"),
        ] {
            let db_path = db_path.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                let conn = Connection::open(db_path).expect("open worker db");
                configure_connection(&conn).expect("configure worker db");
                barrier.wait();
                persist_and_activate_publication_revision(
                    &conn,
                    &revision,
                    "active",
                    r#"{"operation":"publish"}"#,
                    20,
                )
            }));
        }
        barrier.wait();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().expect("shared-offer worker"))
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .any(|error| error.contains("shared offer invariant violation"))
        );

        let conn = Connection::open(&db_path).expect("reopen shared-offer db");
        configure_connection(&conn).expect("reconfigure shared-offer db");
        let active = list_publication_lifecycles(&conn)
            .expect("list race lifecycles")
            .into_iter()
            .filter(|lifecycle| lifecycle.status == "active")
            .collect::<Vec<_>>();
        assert_eq!(active.len(), 1);
        let active_revision = get_publication_revision(
            &conn,
            &active[0].service_id,
            active[0]
                .active_revision_hash
                .as_deref()
                .expect("active revision hash"),
        )
        .expect("active race revision lookup")
        .expect("active race revision");
        assert_eq!(active_revision.offer_id, "race-offer");
        drop(conn);

        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = fs::remove_file(format!("{}-shm", db_path.display()));
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
    fn settlement_materialization_claim_is_exclusive_until_expiry() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute(
            "INSERT INTO deals (
                deal_id, idempotency_key, quote_id, quote_hash, offer_id,
                service_id, workload_hash, spec_json, quote_json,
                deal_artifact_json, status, created_at, updated_at
             ) VALUES (
                'deal-1', NULL, 'quote-1', 'quote-hash', 'offer-1',
                'service-1', 'workload-hash', '{}', '{}',
                '{}', 'payment_pending', 1, 1
             )",
            [],
        )
        .expect("insert parent deal");
        insert_deal_settlement_materialization(
            &conn,
            "deal-1",
            "stripe_payment_reservation",
            "{}",
            1,
        )
        .expect("insert materialization");

        let first = claim_deal_settlement_materialization(&conn, "deal-1", "claim-a", 10, 2)
            .expect("first claim")
            .expect("materialization claimed");
        assert_eq!(first.claim_token.as_deref(), Some("claim-a"));
        assert_eq!(first.claim_expires_at, Some(10));

        let second = claim_deal_settlement_materialization(&conn, "deal-1", "claim-b", 20, 3)
            .expect("second claim");
        assert!(
            second.is_none(),
            "unexpired claim must block a second claimant"
        );

        let third = claim_deal_settlement_materialization(&conn, "deal-1", "claim-c", 30, 11)
            .expect("third claim")
            .expect("expired materialization should be claimable");
        assert_eq!(third.claim_token.as_deref(), Some("claim-c"));
        assert_eq!(third.claim_expires_at, Some(30));
    }

    #[test]
    fn stale_settlement_claim_cannot_delete_a_newer_claim() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute(
            "INSERT INTO deals (
                deal_id, idempotency_key, quote_id, quote_hash, offer_id,
                service_id, workload_hash, spec_json, quote_json,
                deal_artifact_json, status, created_at, updated_at
             ) VALUES (
                'deal-stale-delete', NULL, 'quote-1', 'quote-hash', 'offer-1',
                'service-1', 'workload-hash', '{}', '{}',
                '{}', 'payment_pending', 1, 1
             )",
            [],
        )
        .expect("insert parent deal");
        insert_deal_settlement_materialization(
            &conn,
            "deal-stale-delete",
            "stripe_payment_reservation",
            "{}",
            1,
        )
        .expect("insert materialization");

        claim_deal_settlement_materialization(&conn, "deal-stale-delete", "claim-old", 10, 2)
            .expect("old claim")
            .expect("materialization claimed");
        claim_deal_settlement_materialization(&conn, "deal-stale-delete", "claim-new", 30, 11)
            .expect("new claim")
            .expect("expired materialization reclaimed");

        assert!(
            !delete_deal_settlement_materialization_if_claim_token(
                &conn,
                "deal-stale-delete",
                "claim-old",
            )
            .expect("stale delete attempt"),
            "a stale claimant must not delete the newer claim"
        );
        let stored = get_deal_settlement_materialization(&conn, "deal-stale-delete")
            .expect("read materialization")
            .expect("newer claim remains");
        assert_eq!(stored.claim_token.as_deref(), Some("claim-new"));
    }

    #[test]
    fn exact_settlement_claim_can_delete_its_materialization() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute(
            "INSERT INTO deals (
                deal_id, idempotency_key, quote_id, quote_hash, offer_id,
                service_id, workload_hash, spec_json, quote_json,
                deal_artifact_json, status, created_at, updated_at
             ) VALUES (
                'deal-exact-delete', NULL, 'quote-1', 'quote-hash', 'offer-1',
                'service-1', 'workload-hash', '{}', '{}',
                '{}', 'payment_pending', 1, 1
             )",
            [],
        )
        .expect("insert parent deal");
        insert_deal_settlement_materialization(
            &conn,
            "deal-exact-delete",
            "stripe_payment_reservation",
            "{}",
            1,
        )
        .expect("insert materialization");
        claim_deal_settlement_materialization(&conn, "deal-exact-delete", "claim-owner", 30, 2)
            .expect("claim")
            .expect("materialization claimed");

        let error =
            delete_deal_settlement_materialization_if_claim_token(&conn, "deal-exact-delete", "  ")
                .expect_err("blank claim token must be rejected");
        assert!(error.contains("must not be empty"));
        assert!(
            delete_deal_settlement_materialization_if_claim_token(
                &conn,
                "deal-exact-delete",
                "claim-owner",
            )
            .expect("exact delete"),
            "the exact claim owner must be able to delete its materialization"
        );
        assert!(
            get_deal_settlement_materialization(&conn, "deal-exact-delete")
                .expect("read materialization")
                .is_none()
        );
    }

    #[test]
    fn restart_reset_and_retry_state_preserve_exact_materialization_ownership() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute(
            "INSERT INTO deals (
                deal_id, idempotency_key, quote_id, quote_hash, offer_id,
                service_id, workload_hash, spec_json, quote_json,
                deal_artifact_json, status, created_at, updated_at
             ) VALUES (
                'deal-restart-claim', NULL, 'quote-1', 'quote-hash', 'offer-1',
                'service-1', 'workload-hash', '{}', '{}',
                '{}', 'payment_pending', 1, 1
             )",
            [],
        )
        .expect("insert parent deal");
        insert_deal_settlement_materialization(
            &conn,
            "deal-restart-claim",
            "stripe_payment_reservation",
            "{}",
            1,
        )
        .expect("insert materialization");
        claim_deal_settlement_materialization(
            &conn,
            "deal-restart-claim",
            "dead-process-claim",
            10_000,
            2,
        )
        .expect("claim")
        .expect("materialization claimed");
        assert!(
            list_due_deal_settlement_materializations(&conn, 3, 10)
                .expect("list blocked claims")
                .is_empty(),
            "a future-expiry claim is not due inside the same process"
        );

        assert_eq!(
            reset_deal_settlement_materialization_claims(&conn, 4).expect("restart reset"),
            1
        );
        let due =
            list_due_deal_settlement_materializations(&conn, 4, 10).expect("list restart work");
        assert_eq!(due.len(), 1, "restart must make the dead claim recoverable");
        let claimed = claim_deal_settlement_materialization(
            &conn,
            "deal-restart-claim",
            "current-claim",
            100,
            4,
        )
        .expect("current claim")
        .expect("restart row claimed");
        assert_eq!(claimed.attempt_count, 2);

        assert!(
            !set_deal_settlement_materialization_resource_if_claim_token(
                &conn,
                "deal-restart-claim",
                "dead-process-claim",
                "resource_ready",
                r#"{"payment_intent_id":"pi_stale"}"#,
                5,
            )
            .expect("stale resource update")
        );
        assert!(
            set_deal_settlement_materialization_resource_if_claim_token(
                &conn,
                "deal-restart-claim",
                "current-claim",
                "resource_ready",
                r#"{"payment_intent_id":"pi_current"}"#,
                5,
            )
            .expect("owned resource update")
        );
        assert!(
            !reschedule_deal_settlement_materialization_if_claim_token(
                &conn,
                "deal-restart-claim",
                "dead-process-claim",
                20,
                "stale_retry",
                6,
            )
            .expect("stale reschedule")
        );
        assert!(
            reschedule_deal_settlement_materialization_if_claim_token(
                &conn,
                "deal-restart-claim",
                "current-claim",
                20,
                "backend_retry",
                6,
            )
            .expect("owned reschedule")
        );
        assert!(
            list_due_deal_settlement_materializations(&conn, 19, 10)
                .expect("list before retry")
                .is_empty()
        );
        let retry = list_due_deal_settlement_materializations(&conn, 20, 10)
            .expect("list due retry")
            .pop()
            .expect("retry row");
        assert_eq!(retry.phase, "resource_ready");
        assert_eq!(retry.last_error_code.as_deref(), Some("backend_retry"));
        assert_eq!(
            retry.resource_json.as_deref(),
            Some(r#"{"payment_intent_id":"pi_current"}"#)
        );
    }

    #[test]
    fn settlement_claim_commit_failure_rolls_back_and_connection_is_reusable() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute_batch(
            "INSERT INTO deals (
                deal_id, idempotency_key, quote_id, quote_hash, offer_id,
                service_id, workload_hash, spec_json, quote_json,
                deal_artifact_json, status, created_at, updated_at
             ) VALUES (
                'deal-claim-failure', NULL, 'quote-1', 'quote-hash', 'offer-1',
                'service-1', 'workload-hash', '{}', '{}',
                '{}', 'payment_pending', 1, 1
             );
             CREATE TABLE claim_commit_parent (id INTEGER PRIMARY KEY);
             CREATE TABLE claim_commit_child (
                 parent_id INTEGER NOT NULL,
                 FOREIGN KEY (parent_id) REFERENCES claim_commit_parent(id)
                     DEFERRABLE INITIALLY DEFERRED
             );
             CREATE TRIGGER claim_commit_guard
             AFTER UPDATE OF claim_token ON deal_settlement_materializations
             BEGIN
                 INSERT INTO claim_commit_child (parent_id) VALUES (11);
             END;",
        )
        .expect("deferred claim fixture");
        insert_deal_settlement_materialization(
            &conn,
            "deal-claim-failure",
            "stripe_payment_reservation",
            "{}",
            1,
        )
        .expect("insert materialization");

        let error =
            claim_deal_settlement_materialization(&conn, "deal-claim-failure", "claim-a", 10, 2)
                .expect_err("deferred constraint must reject claim COMMIT");
        assert!(error.contains("FOREIGN KEY constraint failed"));
        assert!(
            conn.is_autocommit(),
            "failed claim must release transaction"
        );
        let materialization = get_deal_settlement_materialization(&conn, "deal-claim-failure")
            .expect("read rolled-back materialization")
            .expect("materialization exists");
        assert_eq!(materialization.claim_token, None);

        conn.execute("INSERT INTO claim_commit_parent (id) VALUES (11)", [])
            .expect("connection remains writable");
        let claimed =
            claim_deal_settlement_materialization(&conn, "deal-claim-failure", "claim-b", 20, 3)
                .expect("claim retry")
                .expect("materialization claimed");
        assert_eq!(claimed.claim_token.as_deref(), Some("claim-b"));
    }

    #[test]
    fn concurrent_materialization_claim_returns_only_the_winners_token() {
        use std::sync::Barrier;

        let db_path = temp_db_path("materialization-claim-race");
        let setup = Connection::open(&db_path).expect("open setup db");
        configure_connection(&setup).expect("configure setup db");
        setup
            .execute(
                "INSERT INTO deals (
                    deal_id, idempotency_key, quote_id, quote_hash, offer_id,
                    service_id, workload_hash, spec_json, quote_json,
                    deal_artifact_json, status, created_at, updated_at
                 ) VALUES (
                    'deal-race', NULL, 'quote-1', 'quote-hash', 'offer-1',
                    'service-1', 'workload-hash', '{}', '{}',
                    '{}', 'payment_pending', 1, 1
                 )",
                [],
            )
            .expect("insert parent deal");
        insert_deal_settlement_materialization(
            &setup,
            "deal-race",
            "stripe_payment_reservation",
            "{}",
            1,
        )
        .expect("insert materialization");
        drop(setup);

        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = ["claim-a", "claim-b"]
            .into_iter()
            .map(|token| {
                let db_path = db_path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let conn = Connection::open(db_path).expect("open claimant db");
                    configure_connection(&conn).expect("configure claimant db");
                    barrier.wait();
                    let result =
                        claim_deal_settlement_materialization(&conn, "deal-race", token, 100, 2)
                            .expect("claim attempt");
                    (token, result)
                })
            })
            .collect();
        barrier.wait();

        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("claimant thread"))
            .collect();
        let winners: Vec<_> = outcomes
            .iter()
            .filter_map(|(attempted_token, result)| {
                result
                    .as_ref()
                    .map(|record| (*attempted_token, record.claim_token.as_deref()))
            })
            .collect();
        assert_eq!(winners.len(), 1, "exactly one claimant must win");
        assert_eq!(
            winners[0].1,
            Some(winners[0].0),
            "the winner must return its own token, never another claimant's row"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|(_, result)| result.is_none())
                .count(),
            1,
            "the losing claimant must return None"
        );

        let conn = Connection::open(&db_path).expect("open verification db");
        let stored = get_deal_settlement_materialization(&conn, "deal-race")
            .expect("read materialization")
            .expect("stored materialization");
        assert_eq!(stored.claim_token.as_deref(), winners[0].1);
        drop(conn);
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn stripe_settlement_outbox_is_durable_until_terminal_delete() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        configure_connection(&conn).expect("configure");
        conn.execute(
            "INSERT INTO deals (
                deal_id, idempotency_key, quote_id, quote_hash, offer_id,
                service_id, workload_hash, spec_json, quote_json,
                deal_artifact_json, status, created_at, updated_at
             ) VALUES (
                'stripe-deal', NULL, 'quote-1', 'quote-hash', 'offer-1',
                'service-1', 'workload-hash', '{}', '{}',
                '{}', 'result_ready', 1, 1
             )",
            [],
        )
        .expect("insert parent deal");

        insert_stripe_settlement_outbox(
            &conn,
            "stripe-deal",
            "capture",
            r#"{"execution_outcome":"succeeded","result_format":"application/json+jcs"}"#,
            2,
        )
        .expect("insert outbox");

        let record = get_stripe_settlement_outbox(&conn, "stripe-deal")
            .expect("read outbox")
            .expect("outbox exists");
        assert_eq!(record.action, "capture");
        assert_eq!(list_stripe_settlement_outbox(&conn).unwrap().len(), 1);

        assert!(delete_stripe_settlement_outbox(&conn, "stripe-deal").unwrap());
        assert!(
            get_stripe_settlement_outbox(&conn, "stripe-deal")
                .unwrap()
                .is_none()
        );
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
