use crate::db::PgPool;
use tokio_postgres::error::SqlState;

pub enum ProjectionResult {
    Projected,
    Skipped,
}

pub async fn project_artifact(
    pg: &PgPool,
    source_url: &str,
    kind: &str,
    document: &serde_json::Value,
) -> Result<ProjectionResult, String> {
    match kind {
        "descriptor" => project_descriptor(pg, source_url, document).await,
        "offer" => project_offer(pg, document).await,
        "receipt" => project_receipt(pg, document).await,
        _ => Ok(ProjectionResult::Skipped),
    }
}

async fn project_descriptor(
    pg: &PgPool,
    source_url: &str,
    document: &serde_json::Value,
) -> Result<ProjectionResult, String> {
    let payload = document
        .get("payload")
        .ok_or("descriptor missing payload")?;

    let provider_id = payload
        .get("provider_id")
        .and_then(|v| v.as_str())
        .ok_or("missing provider_id")?;
    let descriptor_seq = payload
        .get("descriptor_seq")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let protocol_version = payload
        .get("protocol_version")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let descriptor_hash = document.get("hash").and_then(|v| v.as_str()).unwrap_or("");
    let transport_endpoints = payload
        .get("transport_endpoints")
        .cloned()
        .unwrap_or(serde_json::json!([]));
    let linked_identities = payload
        .get("linked_identities")
        .cloned()
        .unwrap_or(serde_json::json!([]));
    let capabilities = payload
        .get("capabilities")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let doc_json = document.clone();

    let mut client = pg.get().await.map_err(|e| format!("db: {e}"))?;
    let transaction = client
        .transaction()
        .await
        .map_err(|e| format!("descriptor upsert begin: {e}"))?;
    if !source_url.trim().is_empty() {
        transaction
            .execute(
                "DELETE FROM marketplace_providers
                 WHERE source_url = $1 AND provider_id <> $2",
                &[&source_url, &provider_id],
            )
            .await
            .map_err(|e| format!("descriptor source cleanup: {e}"))?;
    }
    transaction
        .execute(
            "INSERT INTO marketplace_providers
                (provider_id, descriptor_hash, descriptor_seq, protocol_version,
                 transport_endpoints, linked_identities, capabilities,
                 source_url, descriptor_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (provider_id) DO UPDATE
             SET descriptor_hash = $2,
                 descriptor_seq = $3,
                 protocol_version = $4,
                 transport_endpoints = $5,
                 linked_identities = $6,
                 capabilities = $7,
                 source_url = $8,
                 last_seen_at = NOW(),
                 descriptor_json = $9
             WHERE marketplace_providers.descriptor_seq <= $3",
            &[
                &provider_id,
                &descriptor_hash,
                &descriptor_seq,
                &protocol_version,
                &transport_endpoints,
                &linked_identities,
                &capabilities,
                &source_url,
                &doc_json,
            ],
        )
        .await
        .map_err(|e| format!("descriptor upsert: {e}"))?;
    transaction
        .commit()
        .await
        .map_err(|e| format!("descriptor upsert commit: {e}"))?;

    Ok(ProjectionResult::Projected)
}

async fn project_offer(
    pg: &PgPool,
    document: &serde_json::Value,
) -> Result<ProjectionResult, String> {
    let row = offer_projection_row(document)?;
    let client = pg.get().await.map_err(|e| format!("db: {e}"))?;
    upsert_offer_row(&client, &row).await
}

async fn project_receipt(
    pg: &PgPool,
    document: &serde_json::Value,
) -> Result<ProjectionResult, String> {
    let payload = document.get("payload").ok_or("receipt missing payload")?;

    let receipt_hash = document
        .get("hash")
        .and_then(|v| v.as_str())
        .ok_or("receipt missing hash")?;
    let provider_id = payload
        .get("provider_id")
        .and_then(|v| v.as_str())
        .ok_or("receipt missing provider_id")?;
    let deal_hash = payload
        .get("deal_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let quote_hash = payload
        .get("quote_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let requester_id = payload
        .get("requester_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let deal_state = payload
        .get("deal_state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let execution_state = payload
        .get("execution_state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let status = match (deal_state, execution_state) {
        (_, "succeeded") => "succeeded",
        (_, "failed") => "failed",
        ("rejected", _) => "rejected",
        _ => deal_state,
    };

    let doc_json = document.clone();

    let client = pg.get().await.map_err(|e| format!("db: {e}"))?;
    client
        .execute(
            "INSERT INTO marketplace_receipts
                (receipt_hash, provider_id, deal_hash, quote_hash, requester_id,
                 status, receipt_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (receipt_hash) DO NOTHING",
            &[
                &receipt_hash,
                &provider_id,
                &deal_hash,
                &quote_hash,
                &requester_id,
                &status,
                &doc_json,
            ],
        )
        .await
        .map_err(|e| format!("receipt insert: {e}"))?;

    Ok(ProjectionResult::Projected)
}

pub async fn provider_id_for_source_url(
    pg: &PgPool,
    source_url: &str,
) -> Result<Option<String>, String> {
    let client = pg.get().await.map_err(|e| format!("db: {e}"))?;
    let row = client
        .query_opt(
            "SELECT provider_id
             FROM marketplace_providers
             WHERE source_url = $1
             ORDER BY last_seen_at DESC
             LIMIT 1",
            &[&source_url],
        )
        .await
        .map_err(|e| format!("provider source lookup: {e}"))?;
    Ok(row.map(|row| row.get(0)))
}

pub async fn replace_provider_offers(
    pg: &PgPool,
    provider_id: &str,
    offers: &[serde_json::Value],
) -> Result<usize, String> {
    let mut client = pg.get().await.map_err(|e| format!("db: {e}"))?;
    let transaction = client
        .transaction()
        .await
        .map_err(|e| format!("replace offers begin: {e}"))?;
    transaction
        .execute(
            "DELETE FROM marketplace_offers WHERE provider_id = $1",
            &[&provider_id],
        )
        .await
        .map_err(|e| format!("delete provider offers: {e}"))?;

    let mut refreshed = 0_usize;
    for offer in offers {
        let offer_provider_id = offer_provider_id(offer)?;
        if offer_provider_id != provider_id {
            return Err(format!(
                "offer provider_id mismatch: expected {provider_id}, got {offer_provider_id}"
            ));
        }
        let row = offer_projection_row(offer)?;
        match upsert_offer_row_transaction(&transaction, &row).await? {
            ProjectionResult::Projected => refreshed += 1,
            ProjectionResult::Skipped => {}
        }
    }

    transaction
        .commit()
        .await
        .map_err(|e| format!("replace offers commit: {e}"))?;
    Ok(refreshed)
}

fn offer_provider_id(document: &serde_json::Value) -> Result<&str, String> {
    document
        .get("payload")
        .and_then(|payload| payload.get("provider_id"))
        .and_then(|value| value.as_str())
        .ok_or("offer missing provider_id".to_string())
}

struct OfferProjectionRow {
    offer_hash: String,
    provider_id: String,
    offer_id: String,
    descriptor_hash: String,
    offer_kind: String,
    runtime: String,
    package_kind: String,
    contract_version: String,
    settlement_method: String,
    base_fee_msat: i64,
    success_fee_msat: i64,
    exec_profile: serde_json::Value,
    doc_json: serde_json::Value,
}

fn offer_projection_row(document: &serde_json::Value) -> Result<OfferProjectionRow, String> {
    let payload = document.get("payload").ok_or("offer missing payload")?;
    let exec_profile = payload
        .get("execution_profile")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let price_schedule = payload.get("price_schedule");

    Ok(OfferProjectionRow {
        offer_hash: document
            .get("hash")
            .and_then(|v| v.as_str())
            .ok_or("offer missing hash")?
            .to_string(),
        provider_id: offer_provider_id(document)?.to_string(),
        offer_id: payload
            .get("offer_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        descriptor_hash: payload
            .get("descriptor_hash")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        offer_kind: payload
            .get("offer_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        runtime: exec_profile
            .get("runtime")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        package_kind: exec_profile
            .get("package_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        contract_version: exec_profile
            .get("contract_version")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        settlement_method: payload
            .get("settlement_method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        base_fee_msat: price_schedule
            .and_then(|p| p.get("base_fee_msat"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        success_fee_msat: price_schedule
            .and_then(|p| p.get("success_fee_msat"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        exec_profile,
        doc_json: document.clone(),
    })
}

async fn upsert_offer_row(
    client: &deadpool_postgres::Object,
    row: &OfferProjectionRow,
) -> Result<ProjectionResult, String> {
    let result = client
        .execute(
            "INSERT INTO marketplace_offers
                (offer_hash, provider_id, offer_id, descriptor_hash, offer_kind,
                 runtime, package_kind, contract_version, settlement_method,
                 base_fee_msat, success_fee_msat, execution_profile, offer_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
             ON CONFLICT (offer_hash) DO UPDATE SET offer_json = $13",
            &[
                &row.offer_hash,
                &row.provider_id,
                &row.offer_id,
                &row.descriptor_hash,
                &row.offer_kind,
                &row.runtime,
                &row.package_kind,
                &row.contract_version,
                &row.settlement_method,
                &row.base_fee_msat,
                &row.success_fee_msat,
                &row.exec_profile,
                &row.doc_json,
            ],
        )
        .await;

    match result {
        Ok(_) => Ok(ProjectionResult::Projected),
        Err(error) => {
            if error.code() == Some(&SqlState::FOREIGN_KEY_VIOLATION) {
                Ok(ProjectionResult::Skipped)
            } else {
                Err(format!("offer upsert: {error}"))
            }
        }
    }
}

async fn upsert_offer_row_transaction(
    transaction: &deadpool_postgres::Transaction<'_>,
    row: &OfferProjectionRow,
) -> Result<ProjectionResult, String> {
    let result = transaction
        .execute(
            "INSERT INTO marketplace_offers
                (offer_hash, provider_id, offer_id, descriptor_hash, offer_kind,
                 runtime, package_kind, contract_version, settlement_method,
                 base_fee_msat, success_fee_msat, execution_profile, offer_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
             ON CONFLICT (offer_hash) DO UPDATE SET offer_json = $13",
            &[
                &row.offer_hash,
                &row.provider_id,
                &row.offer_id,
                &row.descriptor_hash,
                &row.offer_kind,
                &row.runtime,
                &row.package_kind,
                &row.contract_version,
                &row.settlement_method,
                &row.base_fee_msat,
                &row.success_fee_msat,
                &row.exec_profile,
                &row.doc_json,
            ],
        )
        .await;

    match result {
        Ok(_) => Ok(ProjectionResult::Projected),
        Err(error) => {
            if error.code() == Some(&SqlState::FOREIGN_KEY_VIOLATION) {
                Ok(ProjectionResult::Skipped)
            } else {
                Err(format!("offer upsert: {error}"))
            }
        }
    }
}
