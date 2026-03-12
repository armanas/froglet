use crate::{pricing::ServiceId, protocol::SettlementStatus, wasm::WasmSubmission};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JOB_STATUS_QUEUED: &str = "queued";
pub const JOB_STATUS_RUNNING: &str = "running";
pub const JOB_STATUS_SUCCEEDED: &str = "succeeded";
pub const JOB_STATUS_FAILED: &str = "failed";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FaaSDescriptor {
    pub jobs_api: bool,
    pub idempotency_keys: bool,
    pub runtimes: Vec<String>,
}

impl FaaSDescriptor {
    pub fn standard() -> Self {
        Self {
            jobs_api: true,
            idempotency_keys: true,
            runtimes: vec!["wasm".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobSpec {
    Wasm { submission: WasmSubmission },
}

impl JobSpec {
    pub fn service_id(&self) -> ServiceId {
        match self {
            JobSpec::Wasm { .. } => ServiceId::ExecuteWasm,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            JobSpec::Wasm { .. } => "wasm",
        }
    }

    pub fn workload_kind(&self) -> &'static str {
        match self {
            JobSpec::Wasm { .. } => crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_V1,
        }
    }

    pub fn request_hash(&self) -> Result<String, String> {
        match self {
            JobSpec::Wasm { submission } => submission.workload_hash(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewJob {
    pub job_id: String,
    pub idempotency_key: Option<String>,
    pub request_hash: String,
    pub service_id: String,
    pub spec: JobSpec,
    pub payment_token_hash: Option<String>,
    pub payment_amount_sats: Option<u64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobPaymentReceipt {
    pub service_id: String,
    pub amount_sats: u64,
    pub token_hash: String,
    pub settlement_status: SettlementStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub job_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub service_id: String,
    pub kind: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_receipt: Option<JobPaymentReceipt>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct StoredJob {
    pub job_id: String,
    pub idempotency_key: Option<String>,
    pub request_hash: String,
    pub service_id: String,
    pub spec: JobSpec,
    pub status: String,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub payment_token_hash: Option<String>,
    pub payment_amount_sats: Option<u64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl StoredJob {
    pub fn public_record(&self) -> JobRecord {
        JobRecord {
            job_id: self.job_id.clone(),
            idempotency_key: self.idempotency_key.clone(),
            service_id: self.service_id.clone(),
            kind: self.spec.kind().to_string(),
            status: self.status.clone(),
            result: self.result.clone(),
            error: self.error.clone(),
            payment_receipt: self.payment_receipt(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    pub fn payment_receipt(&self) -> Option<JobPaymentReceipt> {
        match (&self.payment_token_hash, self.payment_amount_sats) {
            (Some(token_hash), Some(amount_sats)) if self.status == JOB_STATUS_SUCCEEDED => {
                Some(JobPaymentReceipt {
                    service_id: self.service_id.clone(),
                    amount_sats,
                    token_hash: token_hash.clone(),
                    settlement_status: SettlementStatus::Committed,
                })
            }
            _ => None,
        }
    }
}

pub struct InsertJobOutcome {
    pub job: StoredJob,
    pub created: bool,
}

pub fn new_job_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn insert_or_get_job(conn: &Connection, new_job: NewJob) -> Result<InsertJobOutcome, String> {
    let NewJob {
        job_id,
        idempotency_key,
        request_hash,
        service_id,
        spec,
        payment_token_hash,
        payment_amount_sats,
        created_at,
    } = new_job;

    let payload_json = serde_json::to_string(&spec).map_err(|e| e.to_string())?;

    let insert_result = conn.execute(
        "INSERT INTO jobs (
            job_id,
            idempotency_key,
            request_hash,
            service_id,
            kind,
            payload_json,
            status,
            result_json,
            error,
            payment_token_hash,
            payment_amount_sats,
            workload_evidence_hash,
            result_evidence_hash,
            failure_evidence_hash,
            created_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?8, ?9, NULL, NULL, NULL, ?10, ?10)",
        params![
            &job_id,
            idempotency_key.as_deref(),
            &request_hash,
            &service_id,
            spec.kind(),
            &payload_json,
            JOB_STATUS_QUEUED,
            payment_token_hash.as_deref(),
            payment_amount_sats.map(|v| v as i64),
            created_at,
        ],
    );

    match insert_result {
        Ok(_) => {
            let job = get_job(conn, &job_id)?
                .ok_or_else(|| "job inserted but not readable".to_string())?;
            Ok(InsertJobOutcome { job, created: true })
        }
        Err(e) => {
            let Some(idempotency_key) = idempotency_key else {
                return Err(e.to_string());
            };

            let existing = find_job_by_idempotency_key(conn, &idempotency_key)?
                .ok_or_else(|| e.to_string())?;

            if existing.request_hash != request_hash || existing.service_id != service_id {
                return Err("idempotency key reused with different payload".to_string());
            }

            Ok(InsertJobOutcome {
                job: existing,
                created: false,
            })
        }
    }
}

pub fn set_job_workload_evidence_hash(
    conn: &Connection,
    job_id: &str,
    workload_evidence_hash: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE jobs
         SET workload_evidence_hash = ?2
         WHERE job_id = ?1",
        params![job_id, workload_evidence_hash],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn get_job(conn: &Connection, job_id: &str) -> Result<Option<StoredJob>, String> {
    conn.query_row(
        "SELECT
            job_id,
            idempotency_key,
            request_hash,
            service_id,
            payload_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = jobs.workload_evidence_hash LIMIT 1),
            status,
            result_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = jobs.result_evidence_hash LIMIT 1),
            error,
            payment_token_hash,
            payment_amount_sats,
            created_at,
            updated_at
         FROM jobs
         WHERE job_id = ?1",
        params![job_id],
        decode_job_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn try_start_job(
    conn: &Connection,
    job_id: &str,
    now: i64,
) -> Result<Option<StoredJob>, String> {
    let updated = conn
        .execute(
            "UPDATE jobs
             SET status = ?2, updated_at = ?3
             WHERE job_id = ?1 AND status = ?4",
            params![job_id, JOB_STATUS_RUNNING, now, JOB_STATUS_QUEUED],
        )
        .map_err(|e| e.to_string())?;

    if updated == 0 {
        return Ok(None);
    }

    get_job(conn, job_id)
}

pub fn complete_job_success(
    conn: &Connection,
    job_id: &str,
    result: &Value,
    payment_receipt: Option<&JobPaymentReceipt>,
    result_evidence_hash: Option<&str>,
    now: i64,
) -> Result<(), String> {
    let result_json = serde_json::to_string(result).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE jobs
         SET status = ?2,
             result_json = ?3,
             error = NULL,
             payment_token_hash = COALESCE(?4, payment_token_hash),
             payment_amount_sats = COALESCE(?5, payment_amount_sats),
             result_evidence_hash = COALESCE(?6, result_evidence_hash),
             failure_evidence_hash = NULL,
             updated_at = ?7
         WHERE job_id = ?1",
        params![
            job_id,
            JOB_STATUS_SUCCEEDED,
            result_json,
            payment_receipt.map(|receipt| receipt.token_hash.as_str()),
            payment_receipt.map(|receipt| receipt.amount_sats as i64),
            result_evidence_hash,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn complete_job_failure(
    conn: &Connection,
    job_id: &str,
    error: &str,
    failure_evidence_hash: Option<&str>,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE jobs
         SET status = ?2,
             error = ?3,
             result_evidence_hash = NULL,
             failure_evidence_hash = COALESCE(?4, failure_evidence_hash),
             updated_at = ?5
         WHERE job_id = ?1",
        params![job_id, JOB_STATUS_FAILED, error, failure_evidence_hash, now],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn fail_incomplete_jobs(conn: &Connection, message: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE jobs
         SET status = ?1,
             error = ?2,
             updated_at = ?3
         WHERE status IN (?4, ?5)",
        params![
            JOB_STATUS_FAILED,
            message,
            now,
            JOB_STATUS_QUEUED,
            JOB_STATUS_RUNNING
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn find_job_by_idempotency_key(
    conn: &Connection,
    idempotency_key: &str,
) -> Result<Option<StoredJob>, String> {
    conn.query_row(
        "SELECT
            job_id,
            idempotency_key,
            request_hash,
            service_id,
            payload_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = jobs.workload_evidence_hash LIMIT 1),
            status,
            result_json,
            (SELECT content_json FROM execution_evidence WHERE content_hash = jobs.result_evidence_hash LIMIT 1),
            error,
            payment_token_hash,
            payment_amount_sats,
            created_at,
            updated_at
         FROM jobs
         WHERE idempotency_key = ?1",
        params![idempotency_key],
        decode_job_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn decode_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredJob> {
    let payload_json: String = row.get(4)?;
    let workload_evidence_json: Option<String> = row.get(5)?;
    let spec_source = workload_evidence_json.as_deref().unwrap_or(&payload_json);
    let spec = serde_json::from_str(spec_source).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(err))
    })?;

    let result_json: Option<String> = row.get(7)?;
    let result_evidence_json: Option<String> = row.get(8)?;
    let result_source = result_evidence_json.as_deref().or(result_json.as_deref());
    let result = match result_source {
        Some(json) => Some(serde_json::from_str(json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(err))
        })?),
        None => None,
    };

    let payment_amount_sats: Option<i64> = row.get(11)?;

    Ok(StoredJob {
        job_id: row.get(0)?,
        idempotency_key: row.get(1)?,
        request_hash: row.get(2)?,
        service_id: row.get(3)?,
        spec,
        status: row.get(6)?,
        result,
        error: row.get(9)?,
        payment_token_hash: row.get(10)?,
        payment_amount_sats: payment_amount_sats.map(|value| value as u64),
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}
