use crate::{
    canonical_json, crypto,
    protocol::{DealPayload, QuotePayload, ReceiptPayload, SignedArtifact, WorkloadSpec},
};
use rusqlite::{
    Connection, OptionalExtension, params,
    types::{Type, ValueRef},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEAL_STATUS_ACCEPTED: &str = "accepted";
pub const DEAL_STATUS_PAYMENT_PENDING: &str = "payment_pending";
pub const DEAL_STATUS_RUNNING: &str = "running";
pub const DEAL_STATUS_RESULT_READY: &str = "result_ready";
pub const DEAL_STATUS_SUCCEEDED: &str = "succeeded";
pub const DEAL_STATUS_FAILED: &str = "failed";
pub const DEAL_STATUS_REJECTED: &str = "rejected";
pub const QUOTE_ALREADY_USED_ERROR: &str = "quote already used by a different deal";
pub const QUOTE_ADMISSION_IN_PROGRESS_ERROR: &str =
    "quote admission is already in progress for this deal";

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

/// A prepaid (`lightning.prepaid.v1`) BOLT11 invoice the provider mints at
/// deal-create time and returns to the buyer so it can pay upfront. Persisted
/// outside `StoredDeal` so idempotent retries can return the same payable
/// invoice without changing the signed deal payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepaidInvoice {
    pub bolt11: String,
    pub payment_hash: String,
    pub amount_sat: u64,
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
    /// Set on provider deal-create and idempotent replay responses for prepaid
    /// Lightning deals, so the buyer can pay the invoice.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prepaid_invoice: Option<PrepaidInvoice>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantinedDeal {
    pub source_rowid: i64,
    pub deal_id: Option<String>,
    pub status: Option<String>,
    pub reason: String,
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
            prepaid_invoice: None,
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

    reserve_quote_usage(conn, &quote.hash, &deal_id, &deal_artifact_hash, created_at)?;

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

fn reserve_quote_usage(
    conn: &Connection,
    quote_hash: &str,
    deal_id: &str,
    deal_artifact_hash: &str,
    created_at: i64,
) -> Result<(), String> {
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO quote_usages (
                quote_hash,
                deal_id,
                deal_artifact_hash,
                created_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![quote_hash, deal_id, deal_artifact_hash, created_at],
        )
        .map_err(|e| e.to_string())?;

    if inserted > 0 {
        return Ok(());
    }

    let existing: Option<String> = conn
        .query_row(
            "SELECT deal_artifact_hash FROM quote_usages WHERE quote_hash = ?1",
            params![quote_hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if existing.as_deref() == Some(deal_artifact_hash) {
        return Ok(());
    }

    Err(QUOTE_ALREADY_USED_ERROR.to_string())
}

/// Durably reserves a quote before external settlement side effects.
///
/// This is intentionally stricter than `reserve_quote_usage`: if the same
/// deal hash already reserved the quote but no deal row exists yet, a second
/// caller must not mint a second Stripe PaymentIntent or prepaid invoice.
pub fn reserve_quote_usage_before_side_effects(
    conn: &Connection,
    quote_hash: &str,
    deal_id: &str,
    deal_artifact_hash: &str,
    created_at: i64,
) -> Result<bool, String> {
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO quote_usages (
                quote_hash,
                deal_id,
                deal_artifact_hash,
                created_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![quote_hash, deal_id, deal_artifact_hash, created_at],
        )
        .map_err(|e| e.to_string())?;

    if inserted > 0 {
        return Ok(true);
    }

    let existing: Option<String> = conn
        .query_row(
            "SELECT deal_artifact_hash FROM quote_usages WHERE quote_hash = ?1",
            params![quote_hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if existing.as_deref() != Some(deal_artifact_hash) {
        return Err(QUOTE_ALREADY_USED_ERROR.to_string());
    }

    if get_deal_by_artifact_hash(conn, deal_artifact_hash)?.is_some() {
        return Ok(false);
    }

    Err(QUOTE_ADMISSION_IN_PROGRESS_ERROR.to_string())
}

pub fn release_quote_usage_for_unpersisted_deal(
    conn: &Connection,
    quote_hash: &str,
    deal_artifact_hash: &str,
) -> Result<bool, String> {
    if get_deal_by_artifact_hash(conn, deal_artifact_hash)?.is_some() {
        return Ok(false);
    }

    let deleted = conn
        .execute(
            "DELETE FROM quote_usages
              WHERE quote_hash = ?1 AND deal_artifact_hash = ?2",
            params![quote_hash, deal_artifact_hash],
        )
        .map_err(|e| e.to_string())?;
    Ok(deleted > 0)
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

pub fn materialize_payment_for_pending_deal(
    conn: &Connection,
    deal_id: &str,
    payment_token_hash: &str,
    payment_amount_sats: u64,
    next_status: &str,
    now: i64,
) -> Result<bool, String> {
    if next_status != DEAL_STATUS_ACCEPTED && next_status != DEAL_STATUS_PAYMENT_PENDING {
        return Err(format!(
            "unsupported materialized deal status: {next_status}"
        ));
    }

    let updated = conn
        .execute(
            "UPDATE deals
             SET payment_token_hash = ?2,
                 payment_amount_sats = ?3,
                 status = ?4,
                 updated_at = ?5
             WHERE deal_id = ?1 AND status = ?6",
            params![
                deal_id,
                payment_token_hash,
                payment_amount_sats as i64,
                next_status,
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

/// List prepaid (`lightning.prepaid.v1`) deals awaiting payment confirmation.
///
/// Prepaid deals have `payment_method = 'lightning_prepaid'` and sit in
/// `payment_pending` until the provider's phoenixd reports the invoice paid,
/// at which point they are promoted to `accepted` and executed.  Unlike the
/// hold-invoice bundle path, prepaid deals never enter `result_ready` (the
/// receipt is signed inline at execution), so only `payment_pending` is
/// watched.
pub fn list_prepaid_watch_deals(conn: &Connection) -> Result<Vec<StoredDeal>, String> {
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
             WHERE payment_method = ?1 AND status = ?2
             ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(
            params!["lightning_prepaid", DEAL_STATUS_PAYMENT_PENDING],
            decode_deal_row,
        )
        .map_err(|e| e.to_string())?;

    let mut deals = Vec::new();
    for row in rows {
        deals.push(row.map_err(|e| e.to_string())?);
    }

    Ok(deals)
}

pub fn list_recent_deals(conn: &Connection, limit: usize) -> Result<Vec<StoredDeal>, String> {
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
             ORDER BY updated_at DESC, created_at DESC
             LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![limit.clamp(1, 100) as i64], decode_deal_row)
        .map_err(|e| e.to_string())?;

    let mut deals = Vec::new();
    for row in rows {
        deals.push(row.map_err(|e| e.to_string())?);
    }

    Ok(deals)
}

fn decode_deal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredDeal> {
    decode_deal_row_with_offset(row, 0)
}

fn decode_deal_row_with_offset(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<StoredDeal> {
    let quote_json: String = row.get(offset + 2)?;
    let quote_document_json: Option<String> = row.get(offset + 3)?;
    let quote_source = quote_document_json.as_deref().unwrap_or(&quote_json);
    let quote = serde_json::from_str(quote_source).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(offset + 2, Type::Text, Box::new(err))
    })?;

    let spec_json: String = row.get(offset + 4)?;
    let workload_evidence_json: Option<String> = row.get(offset + 5)?;
    let spec_source = workload_evidence_json.as_deref().unwrap_or(&spec_json);
    let spec = serde_json::from_str(spec_source).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(offset + 4, Type::Text, Box::new(err))
    })?;

    let artifact_json: String = row.get(offset + 6)?;
    let deal_document_json: Option<String> = row.get(offset + 7)?;
    let artifact_source = deal_document_json.as_deref().unwrap_or(&artifact_json);
    let artifact = serde_json::from_str(artifact_source).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(offset + 6, Type::Text, Box::new(err))
    })?;

    let result_json: Option<String> = row.get(offset + 9)?;
    let result_evidence_json: Option<String> = row.get(offset + 10)?;
    let result_source = result_evidence_json.as_deref().or(result_json.as_deref());
    let result = match result_source {
        Some(json) => Some(serde_json::from_str(json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(offset + 9, Type::Text, Box::new(err))
        })?),
        None => None,
    };

    let receipt_json: Option<String> = row.get(offset + 16)?;
    let receipt_document_json: Option<String> = row.get(offset + 17)?;
    let receipt_source = receipt_document_json.as_deref().or(receipt_json.as_deref());
    let receipt = match receipt_source {
        Some(json) => Some(serde_json::from_str(json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(offset + 16, Type::Text, Box::new(err))
        })?),
        None => None,
    };

    let payment_amount_sats: Option<i64> = row.get(offset + 15)?;

    Ok(StoredDeal {
        deal_id: row.get(offset)?,
        idempotency_key: row.get(offset + 1)?,
        quote,
        spec,
        artifact,
        status: row.get(offset + 8)?,
        result,
        result_hash: row.get(offset + 11)?,
        error: row.get(offset + 12)?,
        payment_method: row.get(offset + 13)?,
        payment_token_hash: row.get(offset + 14)?,
        payment_amount_sats: payment_amount_sats.map(|value| value as u64),
        receipt,
        created_at: row.get(offset + 18)?,
        updated_at: row.get(offset + 19)?,
    })
}

pub fn quarantine_invalid_deals(
    conn: &Connection,
    quarantined_at: i64,
) -> Result<Vec<QuarantinedDeal>, String> {
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;

    let result = (|| -> Result<Vec<QuarantinedDeal>, String> {
        let quarantine_candidates = {
            let mut stmt = conn
                .prepare(
                    "SELECT
                        rowid,
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
                     FROM deals NOT INDEXED
                     ORDER BY rowid ASC",
                )
                .map_err(|e| e.to_string())?;

            let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
            let mut candidates = Vec::new();

            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                if let Err(error) = decode_deal_row_with_offset(row, 1) {
                    let source_rowid: i64 = row.get(0).map_err(|e| e.to_string())?;
                    let deal_id = row.get_ref(1).ok().and_then(optional_string_from_value_ref);
                    let status = row.get_ref(9).ok().and_then(optional_string_from_value_ref);
                    let reason = error.to_string();
                    let snapshot_json = serde_json::to_string(&deal_row_snapshot(row))
                        .map_err(|e| e.to_string())?;
                    candidates.push((source_rowid, deal_id, status, reason, snapshot_json));
                }
            }

            candidates
        };

        let mut quarantined = Vec::new();
        for (source_rowid, deal_id, status, reason, snapshot_json) in quarantine_candidates {
            conn.execute(
                "INSERT INTO deal_quarantine (
                    source_rowid,
                    deal_id,
                    status,
                    reason,
                    snapshot_json,
                    quarantined_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    source_rowid,
                    deal_id.as_deref(),
                    status.as_deref(),
                    &reason,
                    &snapshot_json,
                    quarantined_at,
                ],
            )
            .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM deals WHERE rowid = ?1", params![source_rowid])
                .map_err(|e| e.to_string())?;

            quarantined.push(QuarantinedDeal {
                source_rowid,
                deal_id,
                status,
                reason,
            });
        }

        if !quarantined.is_empty() {
            conn.execute_batch("REINDEX deals;")
                .map_err(|e| e.to_string())?;
        }

        Ok(quarantined)
    })();

    match result {
        Ok(quarantined) => {
            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(quarantined)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn optional_string_from_value_ref(value: ValueRef<'_>) -> Option<String> {
    match value {
        ValueRef::Null => None,
        ValueRef::Integer(value) => Some(value.to_string()),
        ValueRef::Real(value) => Some(value.to_string()),
        ValueRef::Text(value) => Some(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(_) => None,
    }
}

fn value_ref_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::from(value),
        ValueRef::Real(value) => Value::from(value),
        ValueRef::Text(value) => Value::String(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => Value::Array(
            value
                .iter()
                .map(|byte| Value::from(u64::from(*byte)))
                .collect(),
        ),
    }
}

fn deal_row_snapshot(row: &rusqlite::Row<'_>) -> Value {
    let columns = [
        "rowid",
        "deal_id",
        "idempotency_key",
        "quote_json",
        "quote_document_json",
        "spec_json",
        "workload_evidence_json",
        "deal_artifact_json",
        "deal_document_json",
        "status",
        "result_json",
        "result_evidence_json",
        "result_hash",
        "error",
        "payment_method",
        "payment_token_hash",
        "payment_amount_sats",
        "receipt_artifact_json",
        "receipt_document_json",
        "created_at",
        "updated_at",
    ];
    let mut snapshot = Map::new();

    for (index, name) in columns.iter().enumerate() {
        let value = row
            .get_ref(index)
            .map(value_ref_to_json)
            .unwrap_or_else(|error| Value::String(error.to_string()));
        snapshot.insert((*name).to_string(), value);
    }

    Value::Object(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        self, ARTIFACT_KIND_DEAL, ARTIFACT_KIND_QUOTE, ExecutionLimits, QuoteSettlementTerms,
    };

    fn test_spec() -> WorkloadSpec {
        WorkloadSpec::EventsQuery {
            kinds: vec!["froglet.test".to_string()],
            limit: Some(10),
        }
    }

    fn signed_test_quote(
        provider_key: &crypto::NodeSigningKey,
        requester_id: &str,
        spec: &WorkloadSpec,
    ) -> SignedArtifact<QuotePayload> {
        let provider_id = crypto::public_key_hex(provider_key);
        protocol::sign_artifact(
            &provider_id,
            |message| crypto::sign_message_hex(provider_key, message),
            ARTIFACT_KIND_QUOTE,
            1_700_000_000,
            QuotePayload {
                provider_id: provider_id.clone(),
                requester_id: requester_id.to_string(),
                descriptor_hash: "aa".repeat(32),
                offer_hash: "bb".repeat(32),
                expires_at: 1_700_000_600,
                workload_kind: spec.workload_kind().to_string(),
                workload_hash: spec.request_hash().expect("workload hash"),
                confidential_session_hash: None,
                capabilities_granted: Vec::new(),
                extension_refs: Vec::new(),
                quote_use: None,
                settlement_terms: QuoteSettlementTerms {
                    method: "none".to_string(),
                    destination_identity: provider_id.clone(),
                    base_fee_msat: 0,
                    success_fee_msat: 0,
                    max_base_invoice_expiry_secs: 0,
                    max_success_hold_expiry_secs: 0,
                    min_final_cltv_expiry: 0,
                },
                execution_limits: ExecutionLimits {
                    max_input_bytes: 1024,
                    max_runtime_ms: 1_000,
                    max_memory_bytes: 1024 * 1024,
                    max_output_bytes: 1024,
                    fuel_limit: 10_000,
                },
            },
        )
        .expect("quote")
    }

    fn signed_test_deal(
        requester_key: &crypto::NodeSigningKey,
        quote: &SignedArtifact<QuotePayload>,
        client_nonce: &str,
    ) -> SignedArtifact<DealPayload> {
        let requester_id = crypto::public_key_hex(requester_key);
        protocol::sign_artifact(
            &requester_id,
            |message| crypto::sign_message_hex(requester_key, message),
            ARTIFACT_KIND_DEAL,
            1_700_000_001,
            DealPayload {
                requester_id: requester_id.clone(),
                provider_id: quote.payload.provider_id.clone(),
                quote_hash: quote.hash.clone(),
                workload_hash: quote.payload.workload_hash.clone(),
                confidential_session_hash: None,
                extension_refs: Vec::new(),
                authority_ref: None,
                supersedes_deal_hash: None,
                client_nonce: Some(client_nonce.to_string()),
                success_payment_hash: "cc".repeat(32),
                admission_deadline: 1_700_000_060,
                completion_deadline: 1_700_000_120,
                acceptance_deadline: 1_700_000_180,
            },
        )
        .expect("deal")
    }

    fn new_test_deal(
        deal_id: &str,
        idempotency_key: Option<&str>,
        quote: &SignedArtifact<QuotePayload>,
        spec: &WorkloadSpec,
        artifact: &SignedArtifact<DealPayload>,
    ) -> NewDeal {
        NewDeal {
            deal_id: deal_id.to_string(),
            idempotency_key: idempotency_key.map(str::to_string),
            quote: quote.clone(),
            spec: spec.clone(),
            artifact: artifact.clone(),
            workload_evidence_hash: None,
            deal_artifact_hash: artifact.hash.clone(),
            payment_method: None,
            payment_token_hash: None,
            payment_amount_sats: None,
            initial_status: DEAL_STATUS_ACCEPTED.to_string(),
            created_at: artifact.created_at,
        }
    }

    fn insert_deal_in_transaction(
        conn: &Connection,
        new_deal: NewDeal,
    ) -> Result<InsertDealOutcome, String> {
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let result = insert_or_get_deal(conn, new_deal);
        match result {
            Ok(outcome) => {
                conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                Ok(outcome)
            }
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    #[test]
    fn insert_or_get_deal_reserves_quote_and_allows_exact_replay() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::initialize_db_for_connection(&conn).expect("configure db");
        let provider_key = crypto::generate_signing_key();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let spec = test_spec();
        let quote = signed_test_quote(&provider_key, &requester_id, &spec);
        let deal = signed_test_deal(&requester_key, &quote, "nonce-1");

        let first = insert_deal_in_transaction(
            &conn,
            new_test_deal("deal-1", Some("idem-1"), &quote, &spec, &deal),
        )
        .expect("first deal insert");
        assert!(first.created);

        let usage_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM quote_usages", [], |row| row.get(0))
            .expect("quote usage count");
        let reserved_deal_hash: String = conn
            .query_row(
                "SELECT deal_artifact_hash FROM quote_usages WHERE quote_hash = ?1",
                params![quote.hash],
                |row| row.get(0),
            )
            .expect("quote usage row");
        assert_eq!(usage_count, 1);
        assert_eq!(reserved_deal_hash, deal.hash);

        let replay = insert_deal_in_transaction(
            &conn,
            new_test_deal("deal-replay", Some("idem-1"), &quote, &spec, &deal),
        )
        .expect("exact replay");
        assert!(!replay.created);
        assert_eq!(replay.deal.deal_id, "deal-1");
        assert_eq!(replay.deal.artifact.hash, deal.hash);

        let replay_with_fresh_key = insert_deal_in_transaction(
            &conn,
            new_test_deal(
                "deal-replay-fresh-key",
                Some("idem-2"),
                &quote,
                &spec,
                &deal,
            ),
        )
        .expect("exact replay with fresh idempotency key");
        assert!(!replay_with_fresh_key.created);
        assert_eq!(replay_with_fresh_key.deal.deal_id, "deal-1");
        assert_eq!(replay_with_fresh_key.deal.artifact.hash, deal.hash);
    }

    #[test]
    fn insert_or_get_deal_rejects_distinct_deal_for_used_quote() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::initialize_db_for_connection(&conn).expect("configure db");
        let provider_key = crypto::generate_signing_key();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let spec = test_spec();
        let quote = signed_test_quote(&provider_key, &requester_id, &spec);
        let first_deal = signed_test_deal(&requester_key, &quote, "nonce-1");
        let second_deal = signed_test_deal(&requester_key, &quote, "nonce-2");

        insert_deal_in_transaction(
            &conn,
            new_test_deal("deal-1", Some("idem-1"), &quote, &spec, &first_deal),
        )
        .expect("first deal insert");

        let same_key_error = match insert_deal_in_transaction(
            &conn,
            new_test_deal("deal-2", Some("idem-1"), &quote, &spec, &second_deal),
        ) {
            Ok(_) => panic!("expected idempotency conflict"),
            Err(error) => error,
        };
        assert_eq!(
            same_key_error,
            "idempotency key reused with different deal payload"
        );

        let distinct_error = match insert_deal_in_transaction(
            &conn,
            new_test_deal("deal-3", Some("idem-3"), &quote, &spec, &second_deal),
        ) {
            Ok(_) => panic!("expected quote reuse conflict"),
            Err(error) => error,
        };
        assert_eq!(distinct_error, QUOTE_ALREADY_USED_ERROR);

        let deal_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM deals", [], |row| row.get(0))
            .expect("deal count");
        let usage_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM quote_usages", [], |row| row.get(0))
            .expect("quote usage count");
        assert_eq!(deal_count, 1);
        assert_eq!(usage_count, 1);
    }

    #[test]
    fn pre_side_effect_quote_reservation_blocks_concurrent_admission_until_deal_exists() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::initialize_db_for_connection(&conn).expect("configure db");
        let provider_key = crypto::generate_signing_key();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let spec = test_spec();
        let quote = signed_test_quote(&provider_key, &requester_id, &spec);
        let deal = signed_test_deal(&requester_key, &quote, "nonce-1");

        assert!(
            reserve_quote_usage_before_side_effects(
                &conn,
                &quote.hash,
                "deal-1",
                &deal.hash,
                deal.created_at,
            )
            .expect("initial reservation")
        );

        let duplicate_error = reserve_quote_usage_before_side_effects(
            &conn,
            &quote.hash,
            "deal-1-retry",
            &deal.hash,
            deal.created_at,
        )
        .expect_err("concurrent side effect must be blocked");
        assert_eq!(duplicate_error, QUOTE_ADMISSION_IN_PROGRESS_ERROR);

        let released = release_quote_usage_for_unpersisted_deal(&conn, &quote.hash, &deal.hash)
            .expect("release unpersisted reservation");
        assert!(released);

        let usage_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM quote_usages", [], |row| row.get(0))
            .expect("quote usage count");
        assert_eq!(usage_count, 0);
    }

    #[test]
    fn pre_side_effect_quote_reservation_survives_after_deal_is_persisted() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::initialize_db_for_connection(&conn).expect("configure db");
        let provider_key = crypto::generate_signing_key();
        let requester_key = crypto::generate_signing_key();
        let requester_id = crypto::public_key_hex(&requester_key);
        let spec = test_spec();
        let quote = signed_test_quote(&provider_key, &requester_id, &spec);
        let deal = signed_test_deal(&requester_key, &quote, "nonce-1");

        reserve_quote_usage_before_side_effects(
            &conn,
            &quote.hash,
            "deal-1",
            &deal.hash,
            deal.created_at,
        )
        .expect("initial reservation");

        insert_deal_in_transaction(
            &conn,
            new_test_deal("deal-1", Some("idem-1"), &quote, &spec, &deal),
        )
        .expect("deal insert after pre-reservation");

        let reused = reserve_quote_usage_before_side_effects(
            &conn,
            &quote.hash,
            "deal-1-replay",
            &deal.hash,
            deal.created_at,
        )
        .expect("exact replay after persistence should not side-effect reserve");
        assert!(!reused);

        let released = release_quote_usage_for_unpersisted_deal(&conn, &quote.hash, &deal.hash)
            .expect("release persisted reservation");
        assert!(!released);

        let usage_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM quote_usages", [], |row| row.get(0))
            .expect("quote usage count");
        assert_eq!(usage_count, 1);
    }

    #[test]
    fn quarantine_invalid_deals_removes_unreadable_rows() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::initialize_db_for_connection(&conn).expect("configure db");
        conn.execute(
            "INSERT INTO deals (
                deal_id,
                quote_id,
                quote_hash,
                offer_id,
                service_id,
                workload_hash,
                spec_json,
                quote_json,
                deal_artifact_json,
                status,
                created_at,
                updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                "invalid-deal",
                "quote-id",
                "quote-hash",
                "offer-id",
                "service-id",
                "workload-hash",
                "{\"schema_version\":\"froglet/v1\"}",
                "{not-json",
                "{\"artifact_type\":\"deal\"}",
                DEAL_STATUS_RUNNING,
                1_i64,
                1_i64,
            ],
        )
        .expect("seed invalid deal");

        let quarantined = quarantine_invalid_deals(&conn, 123).expect("quarantine invalid deals");

        assert_eq!(quarantined.len(), 1);
        assert_eq!(quarantined[0].source_rowid, 1);
        assert_eq!(quarantined[0].deal_id.as_deref(), Some("invalid-deal"));
        assert_eq!(quarantined[0].status.as_deref(), Some(DEAL_STATUS_RUNNING));
        assert!(
            quarantined[0].reason.contains("expected ident") || !quarantined[0].reason.is_empty(),
            "unexpected quarantine reason: {}",
            quarantined[0].reason
        );
        let deals_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM deals", [], |row| row.get(0))
            .expect("remaining deal count");
        let quarantine_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM deal_quarantine", [], |row| row.get(0))
            .expect("quarantine count");

        assert_eq!(deals_count, 0);
        assert_eq!(quarantine_count, 1);
    }
}
