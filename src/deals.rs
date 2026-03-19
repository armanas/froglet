use crate::{
    canonical_json, crypto,
    protocol::{DealPayload, QuotePayload, ReceiptPayload, SignedArtifact, WorkloadSpec},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEAL_STATUS_ACCEPTED: &str = "accepted";
pub const DEAL_STATUS_PAYMENT_PENDING: &str = "payment_pending";
pub const DEAL_STATUS_RUNNING: &str = "running";
pub const DEAL_STATUS_RESULT_READY: &str = "result_ready";
pub const DEAL_STATUS_SUCCEEDED: &str = "succeeded";
pub const DEAL_STATUS_FAILED: &str = "failed";
pub const DEAL_STATUS_REJECTED: &str = "rejected";

#[derive(Debug, Clone)]
pub struct StoredQuote {
    pub artifact: SignedArtifact<QuotePayload>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewDeal {
    pub deal_id: String,
    pub idempotency_key: Option<String>,
    pub quote: SignedArtifact<QuotePayload>,
    pub spec: WorkloadSpec,
    pub artifact: SignedArtifact<DealPayload>,
    pub workload_evidence_hash: Option<String>,
    pub deal_artifact_hash: String,
    pub payment_method: Option<String>,
    pub payment_token_hash: Option<String>,
    pub payment_amount_sats: Option<u64>,
    pub initial_status: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DealRecord {
    pub deal_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub status: String,
    pub workload_kind: String,
    pub deal: SignedArtifact<DealPayload>,
    pub quote: SignedArtifact<QuotePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<SignedArtifact<ReceiptPayload>>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct StoredDeal {
    pub deal_id: String,
    pub idempotency_key: Option<String>,
    pub quote: SignedArtifact<QuotePayload>,
    pub spec: WorkloadSpec,
    pub artifact: SignedArtifact<DealPayload>,
    pub status: String,
    pub result: Option<Value>,
    pub result_hash: Option<String>,
    pub error: Option<String>,
    pub payment_method: Option<String>,
    pub payment_token_hash: Option<String>,
    pub payment_amount_sats: Option<u64>,
    pub receipt: Option<SignedArtifact<ReceiptPayload>>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl StoredDeal {
    pub fn public_record(&self) -> DealRecord {
        DealRecord {
            deal_id: self.deal_id.clone(),
            idempotency_key: self.idempotency_key.clone(),
            status: self.status.clone(),
            workload_kind: self.spec.workload_kind().to_string(),
            deal: self.artifact.clone(),
            quote: self.quote.clone(),
            result: self.result.clone(),
            result_hash: self.result_hash.clone(),
            error: self.error.clone(),
            receipt: self.receipt.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    pub fn payment_lock(&self) -> Option<crate::protocol::PaymentLock> {
        match (&self.payment_token_hash, self.payment_amount_sats) {
            (Some(token_hash), Some(amount_sats)) => Some(crate::protocol::PaymentLock {
                kind: self
                    .payment_method
                    .clone()
                    .unwrap_or_else(|| "lightning".to_string()),
                token_hash: token_hash.clone(),
                amount_sats,
            }),
            _ => None,
        }
    }
}

pub struct InsertDealOutcome {
    pub deal: StoredDeal,
    pub created: bool,
}

pub fn insert_quote(conn: &Connection, quote: &SignedArtifact<QuotePayload>) -> Result<(), String> {
    let quote_json = serde_json::to_string(quote).map_err(|e| e.to_string())?;
    let quoted_total_sats = (quote.payload.settlement_terms.base_fee_msat
        + quote.payload.settlement_terms.success_fee_msat)
        / 1_000;
    conn.execute(
        "INSERT INTO quotes (
            quote_id,
            artifact_hash,
            offer_id,
            service_id,
            workload_hash,
            expires_at,
            price_sats,
            quote_json,
            created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            &quote.hash,
            quote.hash,
            quote.payload.offer_hash,
            quote.payload.workload_kind,
            quote.payload.workload_hash,
            quote.payload.expires_at,
            quoted_total_sats as i64,
            quote_json,
            quote.created_at
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn get_quote(conn: &Connection, quote_id: &str) -> Result<Option<StoredQuote>, String> {
    conn.query_row(
        "SELECT
            quote_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = quotes.artifact_hash LIMIT 1),
            created_at
         FROM quotes
         WHERE quote_id = ?1",
        params![quote_id],
        |row| {
            let quote_json: String = row.get(0)?;
            let artifact_document_json: Option<String> = row.get(1)?;
            let artifact_source = artifact_document_json.as_deref().unwrap_or(&quote_json);
            let artifact = serde_json::from_str(artifact_source).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;

            Ok(StoredQuote {
                artifact,
                created_at: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Inserts a new deal if it does not already exist under the canonical artifact hash or
/// idempotency key.
///
/// Callers are expected to execute this inside the same `BEGIN IMMEDIATE` transaction that
/// persists any related artifact documents, evidence rows, or settlement side effects. The
/// helper performs read-then-insert admission checks but does not open its own write
/// transaction, so calling it outside a serializing transaction weakens the dedupe guarantees.
pub fn insert_or_get_deal(
    conn: &Connection,
    new_deal: NewDeal,
) -> Result<InsertDealOutcome, String> {
    let NewDeal {
        deal_id,
        idempotency_key,
        quote,
        spec,
        artifact,
        workload_evidence_hash,
        deal_artifact_hash,
        payment_method,
        payment_token_hash,
        payment_amount_sats,
        initial_status,
        created_at,
    } = new_deal;

    let quote_json = serde_json::to_string(&quote).map_err(|e| e.to_string())?;
    let spec_json = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
    let artifact_json = serde_json::to_string(&artifact).map_err(|e| e.to_string())?;

    if let Some(existing) = get_deal_by_artifact_hash(conn, &deal_artifact_hash)? {
        if let Some(idempotency_key) = idempotency_key.as_deref()
            && existing.idempotency_key.as_deref() != Some(idempotency_key)
        {
            return Err(
                "idempotency key conflict: artifact hash already claimed with a different key"
                    .to_string(),
            );
        }
        return Ok(InsertDealOutcome {
            deal: existing,
            created: false,
        });
    }

    if let Some(idempotency_key) = idempotency_key.as_deref()
        && let Some(existing) = find_deal_by_idempotency_key(conn, idempotency_key)?
    {
        if existing.artifact.hash != artifact.hash {
            return Err("idempotency key reused with different deal payload".to_string());
        }
        return Ok(InsertDealOutcome {
            deal: existing,
            created: false,
        });
    }

    let insert_result = conn.execute(
        "INSERT INTO deals (
            deal_id,
            idempotency_key,
            quote_id,
            quote_hash,
            offer_id,
            service_id,
            workload_hash,
            spec_json,
            quote_json,
            deal_artifact_json,
            status,
            result_json,
            result_hash,
            error,
            payment_method,
            payment_token_hash,
            payment_amount_sats,
            receipt_artifact_json,
            workload_evidence_hash,
            deal_artifact_hash,
            result_evidence_hash,
            failure_evidence_hash,
            receipt_artifact_hash,
            created_at,
            updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, NULL, NULL, ?12, ?13, ?14, NULL, ?15, ?16, NULL, NULL, NULL, ?17, ?17)",
        params![
            &deal_id,
            idempotency_key.as_deref(),
            &quote.hash,
            &quote.hash,
            &quote.payload.offer_hash,
            &quote.payload.workload_kind,
            &artifact.payload.workload_hash,
            &spec_json,
            &quote_json,
            &artifact_json,
            &initial_status,
            payment_method.as_deref(),
            payment_token_hash.as_deref(),
            payment_amount_sats.map(|value| value as i64),
            workload_evidence_hash.as_deref(),
            &deal_artifact_hash,
            created_at,
        ],
    );

    match insert_result {
        Ok(_) => {
            let deal = get_deal(conn, &deal_id)?
                .ok_or_else(|| "deal inserted but not readable".to_string())?;
            Ok(InsertDealOutcome {
                deal,
                created: true,
            })
        }
        Err(e) => {
            if let Some(existing) = get_deal_by_artifact_hash(conn, &deal_artifact_hash)? {
                return Ok(InsertDealOutcome {
                    deal: existing,
                    created: false,
                });
            }
            if let Some(idempotency_key) = idempotency_key.as_deref()
                && let Some(existing) = find_deal_by_idempotency_key(conn, idempotency_key)?
            {
                if existing.artifact.hash != artifact.hash {
                    return Err("idempotency key reused with different deal payload".to_string());
                }
                return Ok(InsertDealOutcome {
                    deal: existing,
                    created: false,
                });
            }
            Err(e.to_string())
        }
    }
}

pub fn set_deal_storage_refs(
    conn: &Connection,
    deal_id: &str,
    workload_evidence_hash: &str,
    deal_artifact_hash: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE deals
         SET workload_evidence_hash = ?2,
             deal_artifact_hash = ?3
         WHERE deal_id = ?1",
        params![deal_id, workload_evidence_hash, deal_artifact_hash],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn get_deal(conn: &Connection, deal_id: &str) -> Result<Option<StoredDeal>, String> {
    conn.query_row(
        "SELECT
            deal_id,
            idempotency_key,
            quote_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.quote_hash LIMIT 1),
            spec_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = deals.workload_evidence_hash LIMIT 1),
            deal_artifact_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.deal_artifact_hash LIMIT 1),
            status,
            result_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = deals.result_evidence_hash LIMIT 1),
            result_hash,
            error,
            payment_method,
            payment_token_hash,
            payment_amount_sats,
            receipt_artifact_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.receipt_artifact_hash LIMIT 1),
            created_at,
            updated_at
         FROM deals
         WHERE deal_id = ?1",
        params![deal_id],
        decode_deal_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn find_deal_by_idempotency_key(
    conn: &Connection,
    idempotency_key: &str,
) -> Result<Option<StoredDeal>, String> {
    conn.query_row(
        "SELECT
            deal_id,
            idempotency_key,
            quote_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.quote_hash LIMIT 1),
            spec_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = deals.workload_evidence_hash LIMIT 1),
            deal_artifact_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.deal_artifact_hash LIMIT 1),
            status,
            result_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = deals.result_evidence_hash LIMIT 1),
            result_hash,
            error,
            payment_method,
            payment_token_hash,
            payment_amount_sats,
            receipt_artifact_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.receipt_artifact_hash LIMIT 1),
            created_at,
            updated_at
         FROM deals
         WHERE idempotency_key = ?1",
        params![idempotency_key],
        decode_deal_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn get_deal_by_artifact_hash(
    conn: &Connection,
    deal_artifact_hash: &str,
) -> Result<Option<StoredDeal>, String> {
    conn.query_row(
        "SELECT
            deal_id,
            idempotency_key,
            quote_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.quote_hash LIMIT 1),
            spec_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = deals.workload_evidence_hash LIMIT 1),
            deal_artifact_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.deal_artifact_hash LIMIT 1),
            status,
            result_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = deals.result_evidence_hash LIMIT 1),
            result_hash,
            error,
            payment_method,
            payment_token_hash,
            payment_amount_sats,
            receipt_artifact_json,
            (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.receipt_artifact_hash LIMIT 1),
            created_at,
            updated_at
         FROM deals
         WHERE deal_artifact_hash = ?1",
        params![deal_artifact_hash],
        decode_deal_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn try_mark_deal_running(conn: &Connection, deal_id: &str, now: i64) -> Result<bool, String> {
    let updated = conn
        .execute(
            "UPDATE deals
             SET status = ?2, updated_at = ?3
             WHERE deal_id = ?1 AND status = ?4",
            params![deal_id, DEAL_STATUS_RUNNING, now, DEAL_STATUS_ACCEPTED],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

pub fn reset_running_deal_to_accepted(
    conn: &Connection,
    deal_id: &str,
    now: i64,
) -> Result<bool, String> {
    let updated = conn
        .execute(
            "UPDATE deals
             SET status = ?2, updated_at = ?3
             WHERE deal_id = ?1 AND status = ?4",
            params![deal_id, DEAL_STATUS_ACCEPTED, now, DEAL_STATUS_RUNNING],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

pub fn try_mark_deal_accepted_from_payment_pending(
    conn: &Connection,
    deal_id: &str,
    now: i64,
) -> Result<bool, String> {
    let updated = conn
        .execute(
            "UPDATE deals
             SET status = ?2, updated_at = ?3
             WHERE deal_id = ?1 AND status = ?4",
            params![
                deal_id,
                DEAL_STATUS_ACCEPTED,
                now,
                DEAL_STATUS_PAYMENT_PENDING
            ],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

pub struct DealSuccessPersistence<'a> {
    pub deal_id: &'a str,
    pub result: &'a Value,
    pub explicit_result_hash: Option<&'a str>,
    pub receipt: &'a SignedArtifact<ReceiptPayload>,
    pub result_evidence_hash: Option<&'a str>,
    pub receipt_artifact_hash: Option<&'a str>,
    pub now: i64,
}

pub fn complete_deal_success(
    conn: &Connection,
    update: DealSuccessPersistence<'_>,
) -> Result<(), String> {
    let result_json = serde_json::to_string(update.result).map_err(|e| e.to_string())?;
    let result_hash = update
        .explicit_result_hash
        .map(str::to_string)
        .unwrap_or_else(|| {
            crypto::sha256_hex(canonical_json::to_vec(update.result).unwrap_or_default())
        });
    let receipt_json = serde_json::to_string(update.receipt).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE deals
         SET status = ?2,
             result_json = ?3,
             result_hash = ?4,
             error = NULL,
             receipt_artifact_json = ?5,
             result_evidence_hash = COALESCE(?6, result_evidence_hash),
             failure_evidence_hash = NULL,
             receipt_artifact_hash = COALESCE(?7, receipt_artifact_hash),
             updated_at = ?8
         WHERE deal_id = ?1",
        params![
            update.deal_id,
            DEAL_STATUS_SUCCEEDED,
            result_json,
            result_hash,
            receipt_json,
            update.result_evidence_hash,
            update.receipt_artifact_hash,
            update.now,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub struct DealSuccessCompletion<'a> {
    pub deal_id: &'a str,
    pub expected_status: &'a str,
    pub result: &'a Value,
    pub explicit_result_hash: Option<&'a str>,
    pub receipt: &'a SignedArtifact<ReceiptPayload>,
    pub result_evidence_hash: Option<&'a str>,
    pub receipt_artifact_hash: Option<&'a str>,
    pub now: i64,
}

pub fn complete_deal_success_if_status(
    conn: &Connection,
    update: DealSuccessCompletion<'_>,
) -> Result<bool, String> {
    let result_json = serde_json::to_string(update.result).map_err(|e| e.to_string())?;
    let result_hash = update
        .explicit_result_hash
        .map(str::to_string)
        .unwrap_or_else(|| {
            crypto::sha256_hex(canonical_json::to_vec(update.result).unwrap_or_default())
        });
    let receipt_json = serde_json::to_string(update.receipt).map_err(|e| e.to_string())?;

    let updated = conn
        .execute(
            "UPDATE deals
             SET status = ?2,
                 result_json = ?3,
                 result_hash = ?4,
                 error = NULL,
                 receipt_artifact_json = ?5,
                 result_evidence_hash = COALESCE(?6, result_evidence_hash),
                 failure_evidence_hash = NULL,
                 receipt_artifact_hash = COALESCE(?7, receipt_artifact_hash),
                 updated_at = ?8
            WHERE deal_id = ?1 AND status = ?9",
            params![
                update.deal_id,
                DEAL_STATUS_SUCCEEDED,
                result_json,
                result_hash,
                receipt_json,
                update.result_evidence_hash,
                update.receipt_artifact_hash,
                update.now,
                update.expected_status,
            ],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

pub fn stage_deal_result_ready(
    conn: &Connection,
    deal_id: &str,
    result: &Value,
    explicit_result_hash: Option<&str>,
    result_evidence_hash: Option<&str>,
    now: i64,
) -> Result<bool, String> {
    let result_json = serde_json::to_string(result).map_err(|e| e.to_string())?;
    let result_hash = explicit_result_hash
        .map(str::to_string)
        .unwrap_or_else(|| crypto::sha256_hex(canonical_json::to_vec(result).unwrap_or_default()));

    let updated = conn
        .execute(
            "UPDATE deals
             SET status = ?2,
                 result_json = ?3,
                 result_hash = ?4,
                 error = NULL,
                 receipt_artifact_json = NULL,
                 result_evidence_hash = COALESCE(?5, result_evidence_hash),
                 failure_evidence_hash = NULL,
                 receipt_artifact_hash = NULL,
                 updated_at = ?6
             WHERE deal_id = ?1 AND status = ?7",
            params![
                deal_id,
                DEAL_STATUS_RESULT_READY,
                result_json,
                result_hash,
                result_evidence_hash,
                now,
                DEAL_STATUS_RUNNING,
            ],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

pub struct DealTerminalTransition<'a> {
    pub deal_id: &'a str,
    pub expected_status: &'a str,
    pub error: &'a str,
    pub receipt: &'a SignedArtifact<ReceiptPayload>,
    pub failure_evidence_hash: Option<&'a str>,
    pub receipt_artifact_hash: Option<&'a str>,
    pub now: i64,
}

pub fn reject_deal_if_status(
    conn: &Connection,
    update: DealTerminalTransition<'_>,
) -> Result<bool, String> {
    let receipt_json = serde_json::to_string(update.receipt).map_err(|e| e.to_string())?;

    let updated = conn
        .execute(
            "UPDATE deals
             SET status = ?2,
                 error = ?3,
                 receipt_artifact_json = ?4,
                 result_evidence_hash = NULL,
                 failure_evidence_hash = COALESCE(?5, failure_evidence_hash),
                 receipt_artifact_hash = COALESCE(?6, receipt_artifact_hash),
                 updated_at = ?7
             WHERE deal_id = ?1 AND status = ?8",
            params![
                update.deal_id,
                DEAL_STATUS_REJECTED,
                update.error,
                receipt_json,
                update.failure_evidence_hash,
                update.receipt_artifact_hash,
                update.now,
                update.expected_status,
            ],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

pub fn reject_deal_admission(
    conn: &Connection,
    deal_id: &str,
    error: &str,
    receipt: &SignedArtifact<ReceiptPayload>,
    failure_evidence_hash: Option<&str>,
    receipt_artifact_hash: Option<&str>,
    now: i64,
) -> Result<bool, String> {
    reject_deal_if_status(
        conn,
        DealTerminalTransition {
            deal_id,
            expected_status: DEAL_STATUS_ACCEPTED,
            error,
            receipt,
            failure_evidence_hash,
            receipt_artifact_hash,
            now,
        },
    )
}

pub fn complete_deal_failure(
    conn: &Connection,
    deal_id: &str,
    error: &str,
    receipt: &SignedArtifact<ReceiptPayload>,
    failure_evidence_hash: Option<&str>,
    receipt_artifact_hash: Option<&str>,
    now: i64,
) -> Result<(), String> {
    let receipt_json = serde_json::to_string(receipt).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE deals
         SET status = ?2,
             error = ?3,
             receipt_artifact_json = ?4,
             result_evidence_hash = NULL,
             failure_evidence_hash = COALESCE(?5, failure_evidence_hash),
             receipt_artifact_hash = COALESCE(?6, receipt_artifact_hash),
             updated_at = ?7
         WHERE deal_id = ?1",
        params![
            deal_id,
            DEAL_STATUS_FAILED,
            error,
            receipt_json,
            failure_evidence_hash,
            receipt_artifact_hash,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn complete_deal_failure_if_status(
    conn: &Connection,
    update: DealTerminalTransition<'_>,
) -> Result<bool, String> {
    let receipt_json = serde_json::to_string(update.receipt).map_err(|e| e.to_string())?;

    let updated = conn
        .execute(
            "UPDATE deals
             SET status = ?2,
                 error = ?3,
                 receipt_artifact_json = ?4,
                 failure_evidence_hash = COALESCE(?5, failure_evidence_hash),
                 receipt_artifact_hash = COALESCE(?6, receipt_artifact_hash),
                 updated_at = ?7
             WHERE deal_id = ?1 AND status = ?8",
            params![
                update.deal_id,
                DEAL_STATUS_FAILED,
                update.error,
                receipt_json,
                update.failure_evidence_hash,
                update.receipt_artifact_hash,
                update.now,
                update.expected_status,
            ],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

pub fn fail_incomplete_deals(conn: &Connection, message: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE deals
         SET status = ?1,
             error = ?2,
             updated_at = ?3
         WHERE status IN (?4, ?5)",
        params![
            DEAL_STATUS_FAILED,
            message,
            now,
            DEAL_STATUS_ACCEPTED,
            DEAL_STATUS_RUNNING
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn list_incomplete_deals(conn: &Connection) -> Result<Vec<StoredDeal>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT
                deal_id,
                idempotency_key,
                quote_json,
                (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.quote_hash LIMIT 1),
                spec_json,
                (SELECT content_json FROM execution_evidence WHERE content_hash = deals.workload_evidence_hash LIMIT 1),
                deal_artifact_json,
                (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.deal_artifact_hash LIMIT 1),
                status,
                result_json,
                (SELECT content_json FROM execution_evidence WHERE content_hash = deals.result_evidence_hash LIMIT 1),
                result_hash,
                error,
                payment_method,
                payment_token_hash,
                payment_amount_sats,
                receipt_artifact_json,
                (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.receipt_artifact_hash LIMIT 1),
                created_at,
                updated_at
             FROM deals
             WHERE status IN (?1, ?2, ?3, ?4)
             ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(
            params![
                DEAL_STATUS_ACCEPTED,
                DEAL_STATUS_RUNNING,
                DEAL_STATUS_PAYMENT_PENDING,
                DEAL_STATUS_RESULT_READY
            ],
            decode_deal_row,
        )
        .map_err(|e| e.to_string())?;

    let mut deals = Vec::new();
    for row in rows {
        deals.push(row.map_err(|e| e.to_string())?);
    }

    Ok(deals)
}

pub fn list_lightning_watch_deals(conn: &Connection) -> Result<Vec<StoredDeal>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT
                deal_id,
                idempotency_key,
                quote_json,
                (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.quote_hash LIMIT 1),
                spec_json,
                (SELECT content_json FROM execution_evidence WHERE content_hash = deals.workload_evidence_hash LIMIT 1),
                deal_artifact_json,
                (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.deal_artifact_hash LIMIT 1),
                status,
                result_json,
                (SELECT content_json FROM execution_evidence WHERE content_hash = deals.result_evidence_hash LIMIT 1),
                result_hash,
                error,
                payment_method,
                payment_token_hash,
                payment_amount_sats,
                receipt_artifact_json,
                (SELECT document_json FROM artifact_documents WHERE artifact_hash = deals.receipt_artifact_hash LIMIT 1),
                created_at,
                updated_at
             FROM deals
             WHERE payment_method = ?1 AND status IN (?2, ?3)
             ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(
            params![
                "lightning",
                DEAL_STATUS_PAYMENT_PENDING,
                DEAL_STATUS_RESULT_READY
            ],
            decode_deal_row,
        )
        .map_err(|e| e.to_string())?;

    let mut deals = Vec::new();
    for row in rows {
        deals.push(row.map_err(|e| e.to_string())?);
    }

    Ok(deals)
}

fn decode_deal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredDeal> {
    let quote_json: String = row.get(2)?;
    let quote_document_json: Option<String> = row.get(3)?;
    let quote_source = quote_document_json.as_deref().unwrap_or(&quote_json);
    let quote = serde_json::from_str(quote_source).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
    })?;

    let spec_json: String = row.get(4)?;
    let workload_evidence_json: Option<String> = row.get(5)?;
    let spec_source = workload_evidence_json.as_deref().unwrap_or(&spec_json);
    let spec = serde_json::from_str(spec_source).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(err))
    })?;

    let artifact_json: String = row.get(6)?;
    let deal_document_json: Option<String> = row.get(7)?;
    let artifact_source = deal_document_json.as_deref().unwrap_or(&artifact_json);
    let artifact = serde_json::from_str(artifact_source).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(err))
    })?;

    let result_json: Option<String> = row.get(9)?;
    let result_evidence_json: Option<String> = row.get(10)?;
    let result_source = result_evidence_json.as_deref().or(result_json.as_deref());
    let result = match result_source {
        Some(json) => Some(serde_json::from_str(json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(err))
        })?),
        None => None,
    };

    let receipt_json: Option<String> = row.get(16)?;
    let receipt_document_json: Option<String> = row.get(17)?;
    let receipt_source = receipt_document_json.as_deref().or(receipt_json.as_deref());
    let receipt = match receipt_source {
        Some(json) => Some(serde_json::from_str(json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                16,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?),
        None => None,
    };

    let payment_amount_sats: Option<i64> = row.get(15)?;

    Ok(StoredDeal {
        deal_id: row.get(0)?,
        idempotency_key: row.get(1)?,
        quote,
        spec,
        artifact,
        status: row.get(8)?,
        result,
        result_hash: row.get(11)?,
        error: row.get(12)?,
        payment_method: row.get(13)?,
        payment_token_hash: row.get(14)?,
        payment_amount_sats: payment_amount_sats.map(|value| value as u64),
        receipt,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
    })
}
