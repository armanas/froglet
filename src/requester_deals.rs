use crate::protocol::{DealPayload, QuotePayload, ReceiptPayload, SignedArtifact, WorkloadSpec};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequesterDealRecord {
    pub deal_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub provider_id: String,
    pub provider_url: String,
    pub status: String,
    pub workload_kind: String,
    pub quote: SignedArtifact<QuotePayload>,
    pub deal: SignedArtifact<DealPayload>,
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
pub struct StoredRequesterDeal {
    pub deal_id: String,
    pub idempotency_key: Option<String>,
    pub provider_id: String,
    pub provider_url: String,
    pub provider_sync_url: Option<String>,
    pub spec: WorkloadSpec,
    pub quote: SignedArtifact<QuotePayload>,
    pub deal: SignedArtifact<DealPayload>,
    pub status: String,
    pub result: Option<Value>,
    pub result_hash: Option<String>,
    pub error: Option<String>,
    pub receipt: Option<SignedArtifact<ReceiptPayload>>,
    pub success_preimage: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl StoredRequesterDeal {
    pub fn sync_provider_url(&self) -> &str {
        self.provider_sync_url
            .as_deref()
            .unwrap_or(self.provider_url.as_str())
    }

    pub fn public_record(&self) -> RequesterDealRecord {
        RequesterDealRecord {
            deal_id: self.deal_id.clone(),
            idempotency_key: self.idempotency_key.clone(),
            provider_id: self.provider_id.clone(),
            provider_url: self.provider_url.clone(),
            status: self.status.clone(),
            workload_kind: self.spec.workload_kind().to_string(),
            quote: self.quote.clone(),
            deal: self.deal.clone(),
            result: self.result.clone(),
            result_hash: self.result_hash.clone(),
            error: self.error.clone(),
            receipt: self.receipt.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewRequesterDeal {
    pub deal_id: String,
    pub idempotency_key: Option<String>,
    pub provider_id: String,
    pub provider_url: String,
    pub provider_sync_url: Option<String>,
    pub spec: WorkloadSpec,
    pub quote: SignedArtifact<QuotePayload>,
    pub deal: SignedArtifact<DealPayload>,
    pub status: String,
    pub success_preimage: String,
    pub created_at: i64,
}

fn decode_json<T: for<'de> Deserialize<'de>>(
    column: usize,
    raw: String,
) -> Result<T, rusqlite::Error> {
    serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn map_requester_deal_row(row: &rusqlite::Row<'_>) -> Result<StoredRequesterDeal, rusqlite::Error> {
    Ok(StoredRequesterDeal {
        deal_id: row.get(0)?,
        idempotency_key: row.get(1)?,
        provider_id: row.get(2)?,
        provider_url: row.get(3)?,
        provider_sync_url: row.get(4)?,
        spec: decode_json(5, row.get(5)?)?,
        quote: decode_json(6, row.get(6)?)?,
        deal: decode_json(7, row.get(7)?)?,
        status: row.get(8)?,
        result: row
            .get::<_, Option<String>>(9)?
            .map(|value| decode_json(9, value))
            .transpose()?,
        result_hash: row.get(10)?,
        error: row.get(11)?,
        receipt: row
            .get::<_, Option<String>>(12)?
            .map(|value| decode_json(12, value))
            .transpose()?,
        success_preimage: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

pub fn get_requester_deal(
    conn: &Connection,
    deal_id: &str,
) -> Result<Option<StoredRequesterDeal>, String> {
    conn.query_row(
        "SELECT
            deal_id,
            idempotency_key,
            provider_id,
            provider_url,
            provider_sync_url,
            spec_json,
            quote_json,
            deal_artifact_json,
            status,
            result_json,
            result_hash,
            error,
            receipt_artifact_json,
            success_preimage,
            created_at,
            updated_at
         FROM requester_deals
         WHERE deal_id = ?1",
        params![deal_id],
        map_requester_deal_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub fn list_recent_requester_deals(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<StoredRequesterDeal>, String> {
    let mut statement = conn
        .prepare(
            "SELECT
                deal_id,
                idempotency_key,
                provider_id,
                provider_url,
                provider_sync_url,
                spec_json,
                quote_json,
                deal_artifact_json,
                status,
                result_json,
                result_hash,
                error,
                receipt_artifact_json,
                success_preimage,
                created_at,
                updated_at
             FROM requester_deals
             ORDER BY updated_at DESC, created_at DESC
             LIMIT ?1",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![limit as i64], map_requester_deal_row)
        .map_err(|error| error.to_string())?;
    let mut deals = Vec::new();
    for row in rows {
        deals.push(row.map_err(|error| error.to_string())?);
    }
    Ok(deals)
}

pub fn find_requester_deal_by_idempotency_key(
    conn: &Connection,
    idempotency_key: &str,
) -> Result<Option<StoredRequesterDeal>, String> {
    conn.query_row(
        "SELECT
            deal_id,
            idempotency_key,
            provider_id,
            provider_url,
            provider_sync_url,
            spec_json,
            quote_json,
            deal_artifact_json,
            status,
            result_json,
            result_hash,
            error,
            receipt_artifact_json,
            success_preimage,
            created_at,
            updated_at
         FROM requester_deals
         WHERE idempotency_key = ?1",
        params![idempotency_key],
        map_requester_deal_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub fn insert_or_get_requester_deal(
    conn: &Connection,
    new_deal: NewRequesterDeal,
) -> Result<StoredRequesterDeal, String> {
    if let Some(idempotency_key) = new_deal.idempotency_key.as_deref()
        && let Some(existing) = find_requester_deal_by_idempotency_key(conn, idempotency_key)?
    {
        if existing.deal.hash != new_deal.deal.hash {
            return Err("idempotency key reused with different requester deal".to_string());
        }
        return Ok(existing);
    }

    if let Some(existing) = get_requester_deal(conn, &new_deal.deal_id)? {
        return Ok(existing);
    }

    let spec_json = serde_json::to_string(&new_deal.spec).map_err(|error| error.to_string())?;
    let quote_json = serde_json::to_string(&new_deal.quote).map_err(|error| error.to_string())?;
    let deal_json = serde_json::to_string(&new_deal.deal).map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO requester_deals (
            deal_id,
            idempotency_key,
            provider_id,
            provider_url,
            provider_sync_url,
            spec_json,
            quote_json,
            deal_artifact_json,
            status,
            result_json,
            result_hash,
            error,
            receipt_artifact_json,
            success_preimage,
            created_at,
            updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL, NULL, NULL, ?10, ?11, ?11)",
        params![
            new_deal.deal_id,
            new_deal.idempotency_key,
            new_deal.provider_id,
            new_deal.provider_url,
            new_deal.provider_sync_url,
            spec_json,
            quote_json,
            deal_json,
            new_deal.status,
            new_deal.success_preimage,
            new_deal.created_at,
        ],
    )
    .map_err(|error| error.to_string())?;

    get_requester_deal(conn, &new_deal.deal_id)?
        .ok_or_else(|| "requester deal disappeared after insert".to_string())
}

#[allow(clippy::too_many_arguments)]
pub fn update_requester_deal_state(
    conn: &Connection,
    deal_id: &str,
    status: &str,
    result: Option<&Value>,
    result_hash: Option<&str>,
    error: Option<&str>,
    receipt: Option<&SignedArtifact<ReceiptPayload>>,
    updated_at: i64,
) -> Result<Option<StoredRequesterDeal>, String> {
    let result_json = result
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| error.to_string())?;
    let receipt_json = receipt
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| error.to_string())?;
    conn.execute(
        "UPDATE requester_deals
         SET status = ?2,
             result_json = ?3,
             result_hash = ?4,
             error = ?5,
             receipt_artifact_json = ?6,
             updated_at = ?7
         WHERE deal_id = ?1",
        params![
            deal_id,
            status,
            result_json,
            result_hash,
            error,
            receipt_json,
            updated_at,
        ],
    )
    .map_err(|error| error.to_string())?;

    get_requester_deal(conn, deal_id)
}
