use super::*;
use std::time::Duration;

pub(crate) fn runtime_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/runtime/search", post(runtime_search))
        .route(
            "/v1/runtime/providers/:provider_id",
            get(runtime_provider_details),
        )
}

fn build_marketplace_deal(
    state: &AppState,
    quote: &Value,
    nonce: &str,
) -> Result<SignedArtifact<protocol::DealPayload>, String> {
    let created_at = settlement::current_unix_timestamp();
    let provider_id = quote
        .get("payload")
        .and_then(|p| p.get("provider_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let quote_hash = quote.get("hash").and_then(|v| v.as_str()).unwrap_or("");
    let workload_hash = quote
        .get("payload")
        .and_then(|p| p.get("workload_hash"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let payload = protocol::DealPayload {
        provider_id: provider_id.to_string(),
        requester_id: state.identity.node_id().to_string(),
        quote_hash: quote_hash.to_string(),
        workload_hash: workload_hash.to_string(),
        confidential_session_hash: None,
        extension_refs: Vec::new(),
        authority_ref: None,
        supersedes_deal_hash: None,
        client_nonce: None,
        success_payment_hash: crypto::sha256_hex(format!("{nonce}-{created_at}")),
        admission_deadline: created_at + 60,
        completion_deadline: created_at + 90,
        acceptance_deadline: created_at + 120,
    };
    protocol::sign_artifact(
        state.identity.node_id(),
        |msg| state.identity.sign_message_hex(msg),
        protocol::ARTIFACT_TYPE_DEAL,
        created_at,
        payload,
    )
}

async fn marketplace_deal(
    state: &AppState,
    marketplace_url: &str,
    offer_id: &str,
    execution: &crate::execution::ExecutionWorkload,
    nonce: &str,
) -> Result<Value, (StatusCode, Value)> {
    // Quote
    let quote_url = format!("{marketplace_url}/v1/provider/quotes");
    let quote: Value = remote_json_request(
        state,
        reqwest::Method::POST,
        quote_url,
        Some(&json!({
            "offer_id": offer_id,
            "requester_id": state.identity.node_id(),
            "kind": "execution",
            "execution": execution,
        })),
    )
    .await
    .map_err(|(s, b)| (s, json!({"error":"marketplace quote failed","detail":b})))?;

    // Deal
    let deal = build_marketplace_deal(state, &quote, nonce)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": e})))?;

    let deal_url = format!("{marketplace_url}/v1/provider/deals");
    let response: Value = remote_json_request(
        state,
        reqwest::Method::POST,
        deal_url,
        Some(&json!({
            "quote": quote,
            "deal": deal,
            "kind": "execution",
            "execution": execution,
        })),
    )
    .await
    .map_err(|(s, b)| (s, json!({"error":"marketplace deal failed","detail":b})))?;

    if let Some(result) = response.get("result") {
        return Ok(result.clone());
    }

    let Some(deal_id) = response
        .get("deal_id")
        .and_then(|value| value.as_str())
        .or_else(|| {
            response
                .get("deal")
                .and_then(|value| value.get("deal_id"))
                .and_then(|value| value.as_str())
        })
    else {
        return Err((
            StatusCode::BAD_GATEWAY,
            json!({
                "error": "marketplace deal response missing deal_id",
                "detail": response,
            }),
        ));
    };

    let status_url = format!("{marketplace_url}/v1/provider/deals/{deal_id}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut current = response;
    loop {
        if let Some(result) = current.get("result") {
            return Ok(result.clone());
        }

        match current.get("status").and_then(|value| value.as_str()) {
            Some(deals::DEAL_STATUS_FAILED) | Some(deals::DEAL_STATUS_REJECTED) => {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    json!({
                        "error": "marketplace deal did not succeed",
                        "detail": current,
                    }),
                ));
            }
            Some(deals::DEAL_STATUS_RESULT_READY) | Some(deals::DEAL_STATUS_SUCCEEDED) => {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    json!({
                        "error": "marketplace deal completed without a result payload",
                        "detail": current,
                    }),
                ));
            }
            _ => {}
        }

        if tokio::time::Instant::now() >= deadline {
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                json!({
                    "error": "marketplace deal timed out waiting for a result payload",
                    "detail": current,
                }),
            ));
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        current = remote_json_request(
            state,
            reqwest::Method::GET,
            status_url.clone(),
            None::<&Value>,
        )
        .await
        .map_err(|(s, b)| {
            (
                s,
                json!({"error":"marketplace deal status check failed","detail":b}),
            )
        })?;
    }
}

async fn runtime_search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    let Some(marketplace_url) = state.config.marketplace_url.as_deref() else {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"no marketplace configured — set FROGLET_MARKETPLACE_URL"}),
        );
    };

    let search_input = json!({
        "offer_kind": payload.get("offer_kind").and_then(|v| v.as_str()),
        "runtime": payload.get("runtime").and_then(|v| v.as_str()),
        "limit": payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(20),
    });

    let execution = match crate::execution::ExecutionWorkload::builtin_service(
        "marketplace.search".to_string(),
        search_input,
    ) {
        Ok(e) => e,
        Err(error) => return error_json(StatusCode::BAD_REQUEST, json!({"error": error})),
    };

    match marketplace_deal(
        state.as_ref(),
        marketplace_url,
        "marketplace.search",
        &execution,
        "mkt-search",
    )
    .await
    {
        Ok(result) => (StatusCode::OK, Json(result)),
        Err((status, body)) => error_json(status, body),
    }
}

async fn runtime_provider_details(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = require_runtime_auth(&headers, state.as_ref()) {
        return error_json(error.0, error.1);
    }

    let Some(marketplace_url) = state.config.marketplace_url.as_deref() else {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"no marketplace configured — set FROGLET_MARKETPLACE_URL"}),
        );
    };

    let execution = match crate::execution::ExecutionWorkload::builtin_service(
        "marketplace.provider".to_string(),
        json!({"provider_id": provider_id}),
    ) {
        Ok(e) => e,
        Err(error) => return error_json(StatusCode::BAD_REQUEST, json!({"error": error})),
    };

    match marketplace_deal(
        state.as_ref(),
        marketplace_url,
        "marketplace.provider",
        &execution,
        "mkt-prov",
    )
    .await
    {
        Ok(result) => (StatusCode::OK, Json(result)),
        Err((status, body)) => error_json(status, body),
    }
}
