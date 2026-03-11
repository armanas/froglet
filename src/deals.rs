use crate::{
    canonical_json, crypto,
    protocol::{DealPayload, QuotePayload, ReceiptPayload, SignedArtifact, WorkloadSpec},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEAL_STATUS_ACCEPTED: &str = "accepted";
pub const DEAL_STATUS_RUNNING: &str = "running";
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
    pub payment_token_hash: Option<String>,
    pub payment_amount_sats: Option<u64>,
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
            workload_kind: self.spec.kind().to_string(),
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
                kind: "cashu".to_string(),
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
            quote.payload.quote_id,
            quote.hash,
            quote.payload.offer_id,
            quote.payload.service_id,
            quote.payload.workload_hash,
            quote.payload.expires_at,
            quote.payload.price_sats as i64,
            quote_json,
            quote.created_at
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn get_quote(conn: &Connection, quote_id: &str) -> Result<Option<StoredQuote>, String> {
    conn.query_row(
        "SELECT quote_json, created_at
         FROM quotes
         WHERE quote_id = ?1",
        params![quote_id],
        |row| {
            let quote_json: String = row.get(0)?;
            let artifact = serde_json::from_str(&quote_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;

            Ok(StoredQuote {
                artifact,
                created_at: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

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
        payment_token_hash,
        payment_amount_sats,
        created_at,
    } = new_deal;

    let quote_json = serde_json::to_string(&quote).map_err(|e| e.to_string())?;
    let spec_json = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
    let artifact_json = serde_json::to_string(&artifact).map_err(|e| e.to_string())?;

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
            payment_token_hash,
            payment_amount_sats,
            receipt_artifact_json,
            created_at,
            updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, NULL, NULL, ?12, ?13, NULL, ?14, ?14)",
        params![
            &deal_id,
            idempotency_key.as_deref(),
            &quote.payload.quote_id,
            &quote.hash,
            &artifact.payload.offer_id,
            &artifact.payload.service_id,
            &artifact.payload.workload_hash,
            &spec_json,
            &quote_json,
            &artifact_json,
            DEAL_STATUS_ACCEPTED,
            payment_token_hash.as_deref(),
            payment_amount_sats.map(|value| value as i64),
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
            let Some(idempotency_key) = idempotency_key else {
                return Err(e.to_string());
            };

            let existing = find_deal_by_idempotency_key(conn, &idempotency_key)?
                .ok_or_else(|| e.to_string())?;

            if existing.quote.hash != quote.hash
                || existing.artifact.payload.workload_hash != artifact.payload.workload_hash
            {
                return Err("idempotency key reused with different deal payload".to_string());
            }

            Ok(InsertDealOutcome {
                deal: existing,
                created: false,
            })
        }
    }
}

pub fn get_deal(conn: &Connection, deal_id: &str) -> Result<Option<StoredDeal>, String> {
    conn.query_row(
        "SELECT
            deal_id,
            idempotency_key,
            quote_json,
            spec_json,
            deal_artifact_json,
            status,
            result_json,
            result_hash,
            error,
            payment_token_hash,
            payment_amount_sats,
            receipt_artifact_json,
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
            spec_json,
            deal_artifact_json,
            status,
            result_json,
            result_hash,
            error,
            payment_token_hash,
            payment_amount_sats,
            receipt_artifact_json,
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

pub fn complete_deal_success(
    conn: &Connection,
    deal_id: &str,
    result: &Value,
    receipt: &SignedArtifact<ReceiptPayload>,
    now: i64,
) -> Result<(), String> {
    let result_json = serde_json::to_string(result).map_err(|e| e.to_string())?;
    let result_hash =
        crypto::sha256_hex(canonical_json::to_vec(result).map_err(|e| e.to_string())?);
    let receipt_json = serde_json::to_string(receipt).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE deals
         SET status = ?2,
             result_json = ?3,
             result_hash = ?4,
             error = NULL,
             receipt_artifact_json = ?5,
             updated_at = ?6
         WHERE deal_id = ?1",
        params![
            deal_id,
            DEAL_STATUS_SUCCEEDED,
            result_json,
            result_hash,
            receipt_json,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn reject_deal_admission(
    conn: &Connection,
    deal_id: &str,
    error: &str,
    receipt: &SignedArtifact<ReceiptPayload>,
    now: i64,
) -> Result<bool, String> {
    let receipt_json = serde_json::to_string(receipt).map_err(|e| e.to_string())?;

    let updated = conn
        .execute(
            "UPDATE deals
             SET status = ?2,
                 error = ?3,
                 receipt_artifact_json = ?4,
                 updated_at = ?5
             WHERE deal_id = ?1 AND status = ?6",
            params![
                deal_id,
                DEAL_STATUS_REJECTED,
                error,
                receipt_json,
                now,
                DEAL_STATUS_ACCEPTED,
            ],
        )
        .map_err(|e| e.to_string())?;

    Ok(updated > 0)
}

pub fn complete_deal_failure(
    conn: &Connection,
    deal_id: &str,
    error: &str,
    receipt: &SignedArtifact<ReceiptPayload>,
    now: i64,
) -> Result<(), String> {
    let receipt_json = serde_json::to_string(receipt).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE deals
         SET status = ?2,
             error = ?3,
             receipt_artifact_json = ?4,
             updated_at = ?5
         WHERE deal_id = ?1",
        params![deal_id, DEAL_STATUS_FAILED, error, receipt_json, now],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
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
                spec_json,
                deal_artifact_json,
                status,
                result_json,
                result_hash,
                error,
                payment_token_hash,
                payment_amount_sats,
                receipt_artifact_json,
                created_at,
                updated_at
             FROM deals
             WHERE status IN (?1, ?2)
             ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(
            params![DEAL_STATUS_ACCEPTED, DEAL_STATUS_RUNNING],
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
    let quote = serde_json::from_str(&quote_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
    })?;

    let spec_json: String = row.get(3)?;
    let spec = serde_json::from_str(&spec_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
    })?;

    let artifact_json: String = row.get(4)?;
    let artifact = serde_json::from_str(&artifact_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(err))
    })?;

    let result_json: Option<String> = row.get(6)?;
    let result = match result_json {
        Some(json) => Some(serde_json::from_str(&json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(err))
        })?),
        None => None,
    };

    let receipt_json: Option<String> = row.get(11)?;
    let receipt = match receipt_json {
        Some(json) => Some(serde_json::from_str(&json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?),
        None => None,
    };

    let payment_amount_sats: Option<i64> = row.get(10)?;

    Ok(StoredDeal {
        deal_id: row.get(0)?,
        idempotency_key: row.get(1)?,
        quote,
        spec,
        artifact,
        status: row.get(5)?,
        result,
        result_hash: row.get(7)?,
        error: row.get(8)?,
        payment_token_hash: row.get(9)?,
        payment_amount_sats: payment_amount_sats.map(|value| value as u64),
        receipt,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}
