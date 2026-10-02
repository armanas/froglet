//! A2A 1.0.0 HTTP+JSON transport for the existing bounded Deal runtime.
//! This module does not sign for callers, run a second scheduler, or change
//! Kernel artifacts. Exact JSON strings avoid protobuf Struct number loss.
use super::*;
use crate::a2a_config::{A2aClient, is_hash};
use axum::routing::any;

pub(crate) const EXTENSION: &str = "https://froglet.dev/a2a/bounded-deal/v1";
pub(crate) const SCHEMA: &str = "froglet.a2a.v1";
pub(crate) const PREFIX: &str = "/a2a/v1";

#[cfg(test)]
#[path = "a2a_tests.rs"]
mod tests;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/.well-known/agent-card.json", get(agent_card))
        .route("/a2a/v1/message:send", post(send_message))
        .route("/a2a/v1/tasks", get(list_tasks))
        // Axum captures the :cancel suffix as part of the path parameter.
        .route("/a2a/v1/tasks/:task_id", get(get_task).post(task_action))
        .route("/a2a/v1/*unsupported", any(unsupported))
        .route_layer(ConcurrencyLimitLayer::new(16))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(|_: BoxError| async {
                    error(
                        StatusCode::GATEWAY_TIMEOUT,
                        "DEADLINE_EXCEEDED",
                        "A2A operation timed out; an accepted Task remains queryable",
                    )
                }))
                .layer(TimeoutLayer::new(Duration::from_secs(
                    DEAL_MATERIALIZATION_ROUTE_TIMEOUT_SECS,
                ))),
        )
        .layer(middleware::from_fn(
            |request: Request, next: Next| async move {
                let operation = request.uri().path().starts_with(PREFIX);
                let activated = request
                    .headers()
                    .get("a2a-extensions")
                    .and_then(|header| header.to_str().ok())
                    .is_some_and(|value| value.split(',').any(|uri| uri.trim() == EXTENSION));
                let mut response = next.run(request).await;
                if operation && response.status().is_client_error() {
                    response = normalize_error_response(response).await;
                }
                response
                    .headers_mut()
                    .insert("a2a-version", HeaderValue::from_static("1.0"));
                if operation {
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/a2a+json"),
                    );
                }
                if activated {
                    response
                        .headers_mut()
                        .insert("a2a-extensions", HeaderValue::from_static(EXTENSION));
                }
                response
            },
        ))
}

fn error(status: StatusCode, reason: &str, message: &str) -> Response {
    let canonical = match status {
        StatusCode::UNAUTHORIZED => "UNAUTHENTICATED",
        StatusCode::FORBIDDEN => "PERMISSION_DENIED",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        StatusCode::TOO_MANY_REQUESTS => "RESOURCE_EXHAUSTED",
        StatusCode::INTERNAL_SERVER_ERROR | StatusCode::BAD_GATEWAY => "INTERNAL",
        StatusCode::SERVICE_UNAVAILABLE => "UNAVAILABLE",
        StatusCode::GATEWAY_TIMEOUT => "DEADLINE_EXCEEDED",
        _ if matches!(reason, "INVALID_REQUEST" | "INVALID_ARGUMENT") => "INVALID_ARGUMENT",
        _ => "FAILED_PRECONDITION",
    };
    (
        status,
        Json(json!({"error":{"code":status.as_u16(),"status":canonical,
        "message":message,"details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo",
        "reason":reason,"domain":"a2a-protocol.org"}]}})),
    )
        .into_response()
}

pub(super) fn from_failure(failure: ApiFailure) -> Response {
    let quote_collision = is_quote_issuance_collision(&failure);
    let message = failure
        .1
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("Froglet operation rejected");
    let reason = failure
        .1
        .get("a2a_reason")
        .and_then(Value::as_str)
        .unwrap_or(if failure.0 == StatusCode::BAD_REQUEST {
            "INVALID_ARGUMENT"
        } else {
            "FROGLET_OPERATION_REJECTED"
        });
    let mut response = error(failure.0, reason, message);
    if quote_collision {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

// Axum query/body extraction rejects before entering a handler. Keep those
// errors in the same bounded A2A envelope without reflecting parser input.
async fn normalize_error_response(response: Response) -> Response {
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, MAX_UPSTREAM_JSON_BYTES)
        .await
        .ok();
    if let Some(bytes) = bytes
        && serde_json::from_slice::<Value>(&bytes)
            .ok()
            .is_some_and(|value| {
                value["error"]["code"].as_u64() == Some(u64::from(parts.status.as_u16()))
                    && value["error"]["message"].is_string()
                    && value["error"]["details"].is_array()
            })
    {
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    }
    error(parts.status, "INVALID_REQUEST", "Invalid A2A request")
}

fn limited_rejection(state: &AppState, response: Response) -> Response {
    match enforce_identity_quota(&state.public_request_quota, "public", "public request") {
        Ok(()) => response,
        Err(failure) => from_failure((failure.0, failure.1.0)),
    }
}

fn negotiate(headers: &HeaderMap) -> Result<(), Box<Response>> {
    let version = headers
        .get("a2a-version")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("0.3");
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() < 2
        || parts.len() > 3
        || parts[0] != "1"
        || parts[1] != "0"
        || (parts.len() == 3 && parts[2].parse::<u64>().is_err())
    {
        return Err(Box::new(error(
            StatusCode::BAD_REQUEST,
            "VERSION_NOT_SUPPORTED",
            "This interface requires A2A-Version: 1.0",
        )));
    }
    let extensions = headers
        .get("a2a-extensions")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !extensions.split(',').any(|uri| uri.trim() == EXTENSION) {
        return Err(Box::new(error(
            StatusCode::BAD_REQUEST,
            "EXTENSION_SUPPORT_REQUIRED",
            "The Froglet bounded Deal v1 extension is required",
        )));
    }
    Ok(())
}

fn authenticate<'a>(
    state: &'a AppState,
    headers: &HeaderMap,
) -> Result<&'a A2aClient, Box<Response>> {
    if state.config.a2a.clients.is_empty() {
        return Err(Box::new(error(
            StatusCode::NOT_FOUND,
            "TASK_NOT_FOUND",
            "A2A provider transport is disabled",
        )));
    }
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let Some(presented) = presented else {
        return Err(Box::new(error(
            StatusCode::UNAUTHORIZED,
            "UNAUTHENTICATED",
            "A separate A2A bearer credential is required",
        )));
    };
    // Even an accidentally duplicated credential must never grant this surface
    // runtime/operator authority or make operator policy bypass possible.
    if [
        state.runtime_auth_token.as_str(),
        state.provider_control_auth_token.as_str(),
        state.consumer_control_auth_token.as_str(),
    ]
    .iter()
    .any(|secret| secret.as_bytes().ct_eq(presented.as_bytes()).unwrap_u8() == 1)
    {
        return Err(Box::new(error(
            StatusCode::UNAUTHORIZED,
            "UNAUTHENTICATED",
            "A separate A2A bearer credential is required",
        )));
    }
    let client = state
        .config
        .a2a
        .clients
        .iter()
        .find(|client| {
            client
                .token
                .as_bytes()
                .ct_eq(presented.as_bytes())
                .unwrap_u8()
                == 1
        })
        .ok_or_else(|| {
            error(
                StatusCode::UNAUTHORIZED,
                "UNAUTHENTICATED",
                "Invalid A2A bearer credential",
            )
        })?;
    negotiate(headers)?;
    Ok(client)
}

fn permitted(client: &A2aClient, deal: &deals::StoredDeal) -> bool {
    deal.artifact.payload.requester_id == client.requester_id
        && client.offer_hashes.contains(&deal.quote.payload.offer_hash)
}

pub(crate) fn context_id(task_id: &str) -> String {
    format!("froglet-{task_id}")
}

pub(crate) fn data_part(operation: &str, payload: &impl Serialize) -> Result<Value, ApiFailure> {
    let payload = serde_json::to_string(payload).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"failed to encode A2A payload"}),
        )
    })?;
    Ok(
        json!({"mediaType":"application/json","data":{"schema":SCHEMA,"operation":operation,"payload":payload}}),
    )
}

fn message(
    operation: &str,
    payload: &impl Serialize,
    context: String,
) -> Result<Value, ApiFailure> {
    Ok(
        json!({"message":{"messageId":protocol::new_artifact_id(),"contextId":context,
        "role":"ROLE_AGENT","parts":[data_part(operation,payload)?],"extensions":[EXTENSION]}}),
    )
}

pub(crate) fn task(record: &deals::DealRecord) -> Result<Value, ApiFailure> {
    let report = protocol::validate_quote_deal(&record.quote, &record.deal, None);
    if !report.valid {
        return Err(provider_bad_gateway("A2A Deal chain failed validation"));
    }
    let task_state = if let Some(receipt) = &record.receipt {
        verify_provider_receipt_artifact(
            receipt,
            &record.quote,
            &record.deal,
            &record.quote.payload.provider_id,
            &record.deal.payload.requester_id,
            record.result.as_ref(),
            record.result_hash.as_deref(),
        )?;
        if record.status != status_bound_to_receipt(receipt) {
            return Err(provider_bad_gateway(
                "A2A task status conflicts with its signed Receipt",
            ));
        }
        match receipt.payload.deal_state.as_str() {
            "succeeded" => "TASK_STATE_COMPLETED",
            "rejected" => "TASK_STATE_REJECTED",
            "failed" => "TASK_STATE_FAILED",
            "canceled" => "TASK_STATE_CANCELED",
            _ => return Err(provider_bad_gateway("Unknown A2A Receipt state")),
        }
    } else {
        match record.status.as_str() {
            deals::DEAL_STATUS_ACCEPTED => "TASK_STATE_SUBMITTED",
            deals::DEAL_STATUS_PAYMENT_PENDING => "TASK_STATE_INPUT_REQUIRED",
            deals::DEAL_STATUS_RUNNING | deals::DEAL_STATUS_SETTLEMENT_PENDING => {
                "TASK_STATE_WORKING"
            }
            deals::DEAL_STATUS_RESULT_READY => {
                let (Some(result), Some(hash)) = (&record.result, &record.result_hash) else {
                    return Err(provider_bad_gateway(
                        "A2A result_ready is missing its committed result",
                    ));
                };
                if canonical_result_hash(result) != *hash {
                    return Err(provider_bad_gateway("A2A provisional result hash mismatch"));
                }
                if record.quote.payload.settlement_terms.method
                    == "lightning.base_fee_plus_success_fee.v1"
                {
                    "TASK_STATE_INPUT_REQUIRED"
                } else {
                    "TASK_STATE_WORKING"
                }
            }
            // Settlement setup can fail before any economic admission. That is
            // a local failure, deliberately labeled as lacking a signed Receipt.
            deals::DEAL_STATUS_FAILED
                if record.result.is_none() && record.result_hash.is_none() =>
            {
                "TASK_STATE_FAILED"
            }
            _ => {
                return Err(provider_bad_gateway(
                    "A2A terminal state lacks valid terminal evidence",
                ));
            }
        }
    };
    let id = protocol::artifact_hash(&record.deal)
        .map_err(|_| provider_bad_gateway("Invalid A2A Deal hash"))?;
    Ok(
        json!({"id":id,"contextId":context_id(&id),"status":{"state":task_state,"timestamp":timestamp(record.updated_at)},
        "artifacts":[{"artifactId":"froglet-deal","parts":[data_part("deal",record)?],"extensions":[EXTENSION]}],
        "metadata":{"froglet":{"schema":SCHEMA,"dealStatus":record.status,
            "terminalEvidence":if record.receipt.is_some(){"signed_receipt"}else{"none"}}}}),
    )
}

async fn owned_deal(
    state: &AppState,
    client: &A2aClient,
    id: &str,
) -> Result<deals::StoredDeal, Response> {
    if !is_hash(id) {
        return Err(error(
            StatusCode::NOT_FOUND,
            "TASK_NOT_FOUND",
            "Task not found or inaccessible",
        ));
    }
    find_existing_deal_by_artifact_hash(state, id)
        .await
        .map_err(|e| from_failure((e.0, e.1.0)))?
        .filter(|deal| permitted(client, deal))
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                "TASK_NOT_FOUND",
                "Task not found or inaccessible",
            )
        })
}

async fn response_value(response: Response) -> Result<Value, ApiFailure> {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_UPSTREAM_JSON_BYTES)
        .await
        .map_err(|_| provider_bad_gateway("Froglet response exceeded the A2A transport limit"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| provider_bad_gateway("Invalid Froglet response"))?;
    if !status.is_success() {
        return Err((status, value));
    }
    Ok(value)
}

async fn refreshed_record(
    state: Arc<AppState>,
    deal: &deals::StoredDeal,
) -> Result<deals::DealRecord, ApiFailure> {
    let value = response_value(
        get_deal_status(State(state.clone()), None, Path(deal.deal_id.clone()))
            .await
            .into_response(),
    )
    .await?;
    let mut record: deals::DealRecord = serde_json::from_value(value)
        .map_err(|_| provider_bad_gateway("Invalid Froglet Deal response"))?;
    if deal.payment_method.as_deref() == Some("lightning_prepaid") {
        attach_persisted_prepaid_invoice(state.as_ref(), &mut record)
            .await
            .map_err(|_| provider_bad_gateway("Failed to load the committed prepaid invoice"))?;
    }
    Ok(record)
}

async fn replay_record(
    state: Arc<AppState>,
    deal: deals::StoredDeal,
) -> Result<deals::DealRecord, ApiFailure> {
    if deal.status == deals::DEAL_STATUS_PAYMENT_PENDING {
        match deal.payment_method.as_deref() {
            Some("stripe") => {
                let recovered = run_stripe_materialization_detached(state.clone(), deal)
                    .await
                    .map_err(|e| (e.0, e.1.0))?;
                return refreshed_record(state, &recovered).await;
            }
            Some("lightning_prepaid") if deal.payment_token_hash.is_none() => {
                let (recovered, _) = run_prepaid_materialization_detached(state.clone(), deal)
                    .await
                    .map_err(|e| (e.0, e.1.0))?;
                return refreshed_record(state, &recovered).await;
            }
            Some("lightning") => {
                run_lightning_materialization_detached(state.clone(), deal.deal_id.clone())
                    .await
                    .map_err(|_| {
                        provider_bad_gateway("Failed to resume committed Lightning materialization")
                    })?;
            }
            _ => {}
        }
    }
    refreshed_record(state, &deal).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendRequest {
    message: ClientMessage,
    #[serde(default)]
    configuration: SendConfiguration,
    #[serde(default)]
    metadata: Option<Value>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendConfiguration {
    #[serde(default)]
    return_immediately: bool,
    #[serde(default)]
    history_length: Option<u32>,
    #[serde(default)]
    accepted_output_modes: Vec<String>,
    #[serde(default)]
    push_notification_config: Option<Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClientMessage {
    message_id: String,
    role: String,
    parts: Vec<ClientPart>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    context_id: Option<String>,
    #[serde(default)]
    metadata: Option<Value>,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    reference_task_ids: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClientPart {
    data: ExtensionData,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    metadata: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionData {
    schema: String,
    operation: String,
    payload: String,
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    bytes: Bytes,
) -> Response {
    let client = match authenticate(&state, &headers) {
        Ok(client) => client,
        Err(response) => return limited_rejection(&state, *response),
    };
    // matchit 0.7 treats the embedded colon as a parameter. Check the literal
    // operation URI before parsing or dispatching to prevent :stream aliases.
    if uri.path() != "/a2a/v1/message:send" {
        return limited_rejection(
            &state,
            error(
                StatusCode::BAD_REQUEST,
                "UNSUPPORTED_OPERATION",
                "This interface supports SendMessage polling only",
            ),
        );
    }
    let request: SendRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return limited_rejection(
                &state,
                error(
                    StatusCode::BAD_REQUEST,
                    "INVALID_ARGUMENT",
                    "Expected one structured Froglet Message with exact JSON payload text",
                ),
            );
        }
    };
    if request.message.role != "ROLE_USER"
        || request.message.message_id.is_empty()
        || request.message.message_id.len() > 128
        || request.message.parts.len() != 1
        || !request.message.reference_task_ids.is_empty()
        || request
            .message
            .context_id
            .as_ref()
            .is_some_and(|id| id.len() > 128)
        || request.message.parts[0].data.schema != SCHEMA
        || request.message.parts[0]
            .media_type
            .as_deref()
            .is_some_and(|mime| mime != "application/json")
    {
        return limited_rejection(
            &state,
            error(
                StatusCode::BAD_REQUEST,
                "INVALID_ARGUMENT",
                "Invalid Froglet A2A Message",
            ),
        );
    }
    if request.configuration.push_notification_config.is_some() {
        return limited_rejection(
            &state,
            error(
                StatusCode::BAD_REQUEST,
                "PUSH_NOTIFICATION_NOT_SUPPORTED",
                "Push notifications are not supported",
            ),
        );
    }
    if !request.configuration.accepted_output_modes.is_empty()
        && !request
            .configuration
            .accepted_output_modes
            .iter()
            .any(|mime| mime == "application/json")
    {
        return limited_rejection(
            &state,
            error(
                StatusCode::BAD_REQUEST,
                "CONTENT_TYPE_NOT_SUPPORTED",
                "Froglet returns application/json Parts",
            ),
        );
    }
    // History is deliberately omitted, including all payment/preimage Messages.
    let _ = (
        &request.metadata,
        &request.message.metadata,
        &request.message.extensions,
        &request.message.parts[0].metadata,
        request.configuration.history_length,
    );
    let data = &request.message.parts[0].data;
    let recovery = match data.operation.as_str() {
        "accept" | "invoice_bundle" => {
            if let Some(id) = &request.message.task_id {
                owned_deal(&state, client, id).await.is_ok()
            } else {
                false
            }
        }
        "submit" => match serde_json::from_str::<CreateDealRequest>(&data.payload) {
            Ok(submission)
                if protocol::validate_quote_deal(&submission.quote, &submission.deal, None)
                    .valid
                    && submission.deal.payload.requester_id == client.requester_id
                    && submission.quote.payload.provider_id == state.identity.node_id()
                    && client
                        .offer_hashes
                        .contains(&submission.quote.payload.offer_hash) =>
            {
                match protocol::artifact_hash(&submission.deal) {
                    Ok(hash) => find_existing_deal_by_artifact_hash(&state, &hash)
                        .await
                        .ok()
                        .flatten()
                        .is_some_and(|existing| {
                            permitted(client, &existing)
                                && existing.quote.hash == submission.quote.hash
                                && existing.spec == submission.spec
                        }),
                    Err(_) => false,
                }
            }
            _ => false,
        },
        _ => false,
    };
    if let Err(failure) = enforce_identity_quota(
        &state.public_request_quota,
        if recovery { "recovery" } else { "public" },
        "public request",
    ) {
        return from_failure((failure.0, failure.1.0));
    }
    let context = request
        .message
        .context_id
        .clone()
        .unwrap_or_else(protocol::new_artifact_id);
    let result: Result<Value, ApiFailure> = match data.operation.as_str() {
        "catalog" if request.message.task_id.is_none() => {
            if serde_json::from_str::<Value>(&data.payload).ok() != Some(json!({})) {
                Err((
                    StatusCode::BAD_REQUEST,
                    json!({"error":"catalog payload must be {}"}),
                ))
            } else {
                match current_offer_artifacts(&state).await {
                    Ok(offers) => match current_descriptor_artifact(&state).await {
                        Ok(descriptor) => message(
                            "catalog",
                            &json!({"descriptor":descriptor,"offers":offers.into_iter().filter(|offer|client.offer_hashes.contains(&offer.hash)).collect::<Vec<_>>()}),
                            context,
                        ),
                        Err(_) => Err(provider_bad_gateway("Failed to load Froglet Descriptor")),
                    },
                    Err(_) => Err(provider_bad_gateway("Failed to load Froglet Offers")),
                }
            }
        }
        "quote" if request.message.task_id.is_none() => {
            quote_operation(state.clone(), client, &headers, &data.payload, context).await
        }
        "submit" => {
            submit_operation(
                state.clone(),
                client,
                &headers,
                &data.payload,
                request.message.task_id.as_deref(),
                request.message.context_id.as_deref(),
                request.configuration.return_immediately,
            )
            .await
        }
        "accept" | "invoice_bundle" => {
            task_operation(
                state.clone(),
                client,
                data,
                request.message.task_id.as_deref(),
                request.message.context_id.as_deref(),
            )
            .await
        }
        _ => Err((
            StatusCode::BAD_REQUEST,
            json!({"error":"unknown Froglet operation or invalid task association"}),
        )),
    };
    match result {
        Ok(value) if !value.to_string().contains(&client.token) => {
            (StatusCode::OK, Json(value)).into_response()
        }
        Ok(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL",
            "Response contained access credentials",
        ),
        Err(failure) => from_failure(failure),
    }
}

async fn quote_operation(
    state: Arc<AppState>,
    client: &A2aClient,
    headers: &HeaderMap,
    payload: &str,
    context: String,
) -> Result<Value, ApiFailure> {
    let request: CreateQuoteRequest = serde_json::from_str(payload).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            json!({"error":"invalid quote JSON payload"}),
        )
    })?;
    if request.requester_id != client.requester_id {
        return Err((
            StatusCode::FORBIDDEN,
            json!({"error":"requester identity does not match A2A credential"}),
        ));
    }
    let offer = lookup_offer(&state, &request.offer_id)
        .await
        .map_err(|_| provider_bad_gateway("Failed to load Offer"))?
        .filter(|offer| client.offer_hashes.contains(&offer.hash))
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                json!({"error":"Offer not found or inaccessible"}),
            )
        })?;
    let _ = offer;
    admit_new_provider_work(&state, headers, "/v1/provider/quotes").await?;
    enforce_identity_quota(
        &state.quote_create_quota,
        &client.requester_id,
        "quote creation",
    )
    .map_err(|e| (e.0, e.1.0))?;
    let quote = create_quote_record(state, request).await?;
    message("quote", &quote, context)
}

async fn submit_operation(
    state: Arc<AppState>,
    client: &A2aClient,
    headers: &HeaderMap,
    payload: &str,
    task_id: Option<&str>,
    context: Option<&str>,
    immediate: bool,
) -> Result<Value, ApiFailure> {
    let mut request: CreateDealRequest = serde_json::from_str(payload).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            json!({"error":"invalid signed Deal JSON payload"}),
        )
    })?;
    validate_workload_spec(&request.spec).map_err(|e| (e.0, e.1.0))?;
    let report = protocol::validate_quote_deal(&request.quote, &request.deal, None);
    let hash = protocol::artifact_hash(&request.deal).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            json!({"error":"invalid Deal hash"}),
        )
    })?;
    if !report.valid
        || request.quote.payload.provider_id != state.identity.node_id()
        || request.deal.payload.requester_id != client.requester_id
        || !client
            .offer_hashes
            .contains(&request.quote.payload.offer_hash)
        || request.spec.request_hash().ok().as_deref()
            != Some(request.deal.payload.workload_hash.as_str())
        || request.spec.workload_kind() != request.quote.payload.workload_kind
    {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({"error":"signed Deal chain, workload or caller scope does not match"}),
        ));
    }
    if task_id.is_some_and(|id| id != hash) || context.is_some_and(|id| id != context_id(&hash)) {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({"error":"Task or context does not match signed Deal"}),
        ));
    }
    request.idempotency_key = normalize_idempotency_key(request.idempotency_key.clone())
        .map_err(|e| (e.0, e.1.0))?
        .map(|key| {
            format!(
                "a2a:{}",
                crypto::sha256_hex(format!("{}:{key}", client.requester_id).as_bytes())
            )
        });
    let record = if let Some(existing) = find_existing_deal_by_artifact_hash(&state, &hash)
        .await
        .map_err(|e| (e.0, e.1.0))?
    {
        if !permitted(client, &existing)
            || existing.quote.hash != request.quote.hash
            || existing.spec != request.spec
        {
            return Err((
                StatusCode::CONFLICT,
                json!({"error":"existing Task does not match exact signed request"}),
            ));
        }
        if task_id.is_some()
            && (existing.receipt.is_some()
                || matches!(
                    existing.status.as_str(),
                    deals::DEAL_STATUS_FAILED | deals::DEAL_STATUS_REJECTED
                ))
        {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({"error":"Messages cannot continue a terminal Task","a2a_reason":"UNSUPPORTED_OPERATION"}),
            ));
        }
        if let Some(key) = &request.idempotency_key
            && let Some(by_key) = find_existing_deal(&state, Some(key.clone()))
                .await
                .map_err(|e| (e.0, e.1.0))?
            && by_key.artifact.hash != hash
        {
            return Err((
                StatusCode::CONFLICT,
                json!({"error":"idempotency key reused with a different Task"}),
            ));
        }
        replay_record(state.clone(), existing).await?
    } else {
        if task_id.is_some() {
            return Err((
                StatusCode::NOT_FOUND,
                json!({"error":"Task not found or inaccessible","a2a_reason":"TASK_NOT_FOUND"}),
            ));
        }
        admit_new_provider_work(&state, headers, "/v1/provider/deals").await?;
        create_deal_record(state.clone(), request).await?.0
    };
    let mut projected = task(&record)?;
    if !immediate {
        while matches!(
            projected["status"]["state"].as_str(),
            Some("TASK_STATE_SUBMITTED" | "TASK_STATE_WORKING")
        ) {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let existing = find_existing_deal_by_artifact_hash(&state, &hash)
                .await
                .map_err(|e| (e.0, e.1.0))?
                .ok_or_else(|| provider_bad_gateway("Accepted Task disappeared"))?;
            projected = task(&refreshed_record(state.clone(), &existing).await?)?;
        }
    }
    Ok(json!({"task":projected}))
}

async fn task_operation(
    state: Arc<AppState>,
    client: &A2aClient,
    data: &ExtensionData,
    task_id: Option<&str>,
    context: Option<&str>,
) -> Result<Value, ApiFailure> {
    let id = task_id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            json!({"error":"taskId is required"}),
        )
    })?;
    let deal = owned_deal(&state, client, id).await.map_err(|response| {
        (
            response.status(),
            json!({"error":"Task not found or inaccessible","a2a_reason":"TASK_NOT_FOUND"}),
        )
    })?;
    if context.is_some_and(|context| context != context_id(id)) {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({"error":"contextId does not match Task"}),
        ));
    }
    if deal.receipt.is_some()
        || matches!(
            deal.status.as_str(),
            deals::DEAL_STATUS_FAILED | deals::DEAL_STATUS_REJECTED
        )
    {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({"error":"Messages cannot continue a terminal Task","a2a_reason":"UNSUPPORTED_OPERATION"}),
        ));
    }
    if data.operation == "invoice_bundle" {
        if serde_json::from_str::<Value>(&data.payload).ok() != Some(json!({})) {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({"error":"invoice_bundle payload must be {}"}),
            ));
        }
        let value = response_value(
            get_deal_invoice_bundle(State(state), Path(deal.deal_id))
                .await
                .into_response(),
        )
        .await?;
        message("invoice_bundle", &value, context_id(id))
    } else {
        let request: ReleaseDealPreimageRequest =
            serde_json::from_str(&data.payload).map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    json!({"error":"invalid acceptance JSON payload"}),
                )
            })?;
        if request.expected_result_hash.is_none() {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({"error":"A2A acceptance requires expected_result_hash"}),
            ));
        }
        let value = response_value(
            release_deal_preimage(State(state), Path(deal.deal_id), Json(request))
                .await
                .into_response(),
        )
        .await?;
        let record: deals::DealRecord = serde_json::from_value(value)
            .map_err(|_| provider_bad_gateway("Invalid acceptance response"))?;
        Ok(json!({"task":task(&record)?}))
    }
}

async fn get_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let client = match authenticate(&state, &headers) {
        Ok(client) => client,
        Err(response) => return *response,
    };
    if id.ends_with(":subscribe") {
        return error(
            StatusCode::BAD_REQUEST,
            "UNSUPPORTED_OPERATION",
            "This profile supports task polling only",
        );
    }
    let deal = match owned_deal(&state, client, &id).await {
        Ok(deal) => deal,
        Err(response) => return response,
    };
    let projection = async {
        let record = refreshed_record(state.clone(), &deal).await?;
        let mut projection = task(&record)?;
        if record.quote.payload.settlement_terms.method == "lightning.base_fee_plus_success_fee.v1"
        {
            let response =
                get_deal_invoice_bundle(State(state.clone()), Path(deal.deal_id.clone()))
                    .await
                    .into_response();
            if response.status() != StatusCode::NOT_FOUND {
                let bundle = response_value(response).await?;
                projection["metadata"]["froglet"]["invoiceBundle"] = json!(
                    serde_json::to_string(&bundle).map_err(|_| provider_bad_gateway(
                        "Failed to encode committed Lightning bundle"
                    ))?
                );
            }
        }
        Ok::<_, ApiFailure>(projection)
    }
    .await;
    match projection {
        Ok(task) if !task.to_string().contains(&client.token) => Json(task).into_response(),
        Ok(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL",
            "Response contained access credentials",
        ),
        Err(failure) => from_failure(failure),
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    page_size: Option<usize>,
    page_token: Option<String>,
    context_id: Option<String>,
    status: Option<String>,
    history_length: Option<u32>,
    include_artifacts: Option<bool>,
    status_timestamp_after: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    updated_at: i64,
    deal_id: String,
    scope: String,
}

// Civil-date conversion; kernel timestamps are integral UTC Unix seconds.
fn timestamp(seconds: i64) -> String {
    let days = seconds.div_euclid(86400) + 719468;
    let era = days.div_euclid(146097);
    let day_of_era = days - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let second_of_day = seconds.rem_euclid(86400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60
    )
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Response {
    let client = match authenticate(&state, &headers) {
        Ok(client) => client,
        Err(response) => return *response,
    };
    let size = query.page_size.unwrap_or(50);
    if !(1..=100).contains(&size) {
        return error(
            StatusCode::BAD_REQUEST,
            "INVALID_ARGUMENT",
            "pageSize must be 1–100",
        );
    }
    let scope=crypto::sha256_hex(serde_json::to_string(&json!({"requester":client.requester_id,"offers":client.offer_hashes,"context":query.context_id,"status":query.status,"after":query.status_timestamp_after,"artifacts":query.include_artifacts,"history":query.history_length})).unwrap().as_bytes());
    let cursor = match query
        .page_token
        .as_deref()
        .filter(|token| !token.is_empty())
    {
        Some(token) => match hex::decode(token)
            .ok()
            .filter(|bytes| bytes.len() <= 512)
            .and_then(|bytes| serde_json::from_slice::<Cursor>(&bytes).ok())
        {
            Some(cursor) if cursor.scope == scope && cursor.deal_id.len() <= 128 => Some(cursor),
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "INVALID_ARGUMENT",
                    "Invalid pageToken or changed filters",
                );
            }
        },
        None => None,
    };
    let context_hash = match &query.context_id {
        Some(context) => match context
            .strip_prefix("froglet-")
            .filter(|hash| is_hash(hash))
        {
            Some(hash) => Some(hash.to_string()),
            None => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "INVALID_ARGUMENT",
                    "Unknown contextId",
                );
            }
        },
        None => None,
    };
    if query.status.as_deref().is_some_and(|status| {
        !matches!(
            status,
            "TASK_STATE_SUBMITTED"
                | "TASK_STATE_WORKING"
                | "TASK_STATE_COMPLETED"
                | "TASK_STATE_FAILED"
                | "TASK_STATE_CANCELED"
                | "TASK_STATE_INPUT_REQUIRED"
                | "TASK_STATE_REJECTED"
                | "TASK_STATE_AUTH_REQUIRED"
        )
    }) {
        return error(
            StatusCode::BAD_REQUEST,
            "INVALID_ARGUMENT",
            "Unknown Task status",
        );
    }
    let requester = client.requester_id.clone();
    let offers = client.offer_hashes.clone();
    let filter_status = query.status.clone();
    let after = query.status_timestamp_after.clone();
    let (mut records,total)=match state.db.with_read_conn(move |conn| {
        if let Some(after)=&after {
            let valid:bool=conn.query_row("SELECT julianday(?1) IS NOT NULL",[after],|row|row.get(0)).map_err(|e|e.to_string())?;
            if !valid {return Err("invalid statusTimestampAfter".to_string());}
        }
        let placeholders=std::iter::repeat_n("?",offers.len()).collect::<Vec<_>>().join(",");
        let projection="CASE json_extract(r.document_json,'$.payload.deal_state') WHEN 'succeeded' THEN 'TASK_STATE_COMPLETED' WHEN 'failed' THEN 'TASK_STATE_FAILED' WHEN 'canceled' THEN 'TASK_STATE_CANCELED' WHEN 'rejected' THEN 'TASK_STATE_REJECTED' ELSE CASE d.status WHEN 'accepted' THEN 'TASK_STATE_SUBMITTED' WHEN 'payment_pending' THEN 'TASK_STATE_INPUT_REQUIRED' WHEN 'running' THEN 'TASK_STATE_WORKING' WHEN 'settlement_pending' THEN 'TASK_STATE_WORKING' WHEN 'failed' THEN 'TASK_STATE_FAILED' WHEN 'result_ready' THEN CASE json_extract(q.document_json,'$.payload.settlement_terms.method') WHEN 'lightning.base_fee_plus_success_fee.v1' THEN 'TASK_STATE_INPUT_REQUIRED' ELSE 'TASK_STATE_WORKING' END ELSE 'TASK_STATE_UNSPECIFIED' END END";
        let mut from=format!(" FROM deals d JOIN artifact_documents a ON a.artifact_hash=d.deal_artifact_hash JOIN artifact_documents q ON q.artifact_hash=d.quote_hash LEFT JOIN artifact_documents r ON r.artifact_hash=d.receipt_artifact_hash WHERE json_extract(a.document_json,'$.payload.requester_id')=? AND json_extract(q.document_json,'$.payload.offer_hash') IN ({placeholders})");
        let mut args=vec![requester];args.extend(offers);
        if let Some(hash)=context_hash {from.push_str(" AND d.deal_artifact_hash=?");args.push(hash);}
        if let Some(status)=filter_status {from.push_str(&format!(" AND ({projection})=?"));args.push(status);}
        if let Some(after)=after {from.push_str(" AND julianday(d.updated_at,'unixepoch')>julianday(?)");args.push(after);}
        let total:i64=conn.query_row(&format!("SELECT COUNT(*){from}"),rusqlite::params_from_iter(&args),|row|row.get(0)).map_err(|e|e.to_string())?;
        if let Some(cursor)=cursor {from.push_str(" AND (d.updated_at<CAST(? AS INTEGER) OR (d.updated_at=CAST(? AS INTEGER) AND d.deal_id>?))");args.extend([cursor.updated_at.to_string(),cursor.updated_at.to_string(),cursor.deal_id]);}
        let mut statement=conn.prepare(&format!("SELECT d.deal_artifact_hash{from} ORDER BY d.updated_at DESC,d.deal_id ASC LIMIT {}",size+1)).map_err(|e|e.to_string())?;
        let rows=statement.query_map(rusqlite::params_from_iter(&args),|row|row.get::<_,String>(0)).map_err(|e|e.to_string())?;
        let mut result=Vec::new();
        for row in rows {let hash=row.map_err(|e|e.to_string())?;if let Some(deal)=deals::get_deal_by_artifact_hash(conn,&hash)? {result.push(deal);}}
        Ok::<_,String>((result,total))
    }).await { Ok(result)=>result,Err(message)=>return error(if message=="invalid statusTimestampAfter"{StatusCode::BAD_REQUEST}else{StatusCode::INTERNAL_SERVER_ERROR},"INVALID_ARGUMENT","Failed to list Tasks or invalid timestamp filter") };
    let has_more = records.len() > size;
    records.truncate(size);
    let next = if has_more {
        let last = records.last().unwrap();
        hex::encode(
            serde_json::to_vec(&Cursor {
                updated_at: last.updated_at,
                deal_id: last.deal_id.clone(),
                scope,
            })
            .unwrap(),
        )
    } else {
        String::new()
    };
    let mut tasks = Vec::new();
    // List returns a bounded durable snapshot. GetTask performs rail polling.
    for deal in records {
        if !permitted(client, &deal) {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL",
                "Task scope mismatch",
            );
        }
        let mut task = match task(&deal.public_record()) {
            Ok(task) => task,
            Err(failure) => return from_failure(failure),
        };
        if !query.include_artifacts.unwrap_or(false) {
            task.as_object_mut().unwrap().remove("artifacts");
        }
        tasks.push(task);
    }
    Json(json!({"tasks":tasks,"nextPageToken":next,"pageSize":size,"totalSize":total}))
        .into_response()
}

async fn task_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let client = match authenticate(&state, &headers) {
        Ok(client) => client,
        Err(response) => return *response,
    };
    let Some(id) = id.strip_suffix(":cancel") else {
        return error(
            StatusCode::BAD_REQUEST,
            "UNSUPPORTED_OPERATION",
            "Unsupported Task operation",
        );
    };
    if let Err(response) = owned_deal(&state, client, id).await {
        return response;
    }
    error(
        StatusCode::BAD_REQUEST,
        "TASK_NOT_CANCELABLE",
        "Froglet has no provider cancellation operation; execution and payment are unchanged",
    )
}

async fn unsupported(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    if let Err(response) = authenticate(&state, &headers) {
        return *response;
    }
    let push = request.uri().path().contains("pushNotification");
    error(
        StatusCode::BAD_REQUEST,
        if push {
            "PUSH_NOTIFICATION_NOT_SUPPORTED"
        } else {
            "UNSUPPORTED_OPERATION"
        },
        "This polling profile does not support that operation",
    )
}

async fn agent_card(State(state): State<Arc<AppState>>) -> Response {
    if state.config.a2a.clients.is_empty() {
        return error(
            StatusCode::NOT_FOUND,
            "TASK_NOT_FOUND",
            "A2A provider transport is disabled",
        );
    }
    let offers = match current_offer_artifacts(&state).await {
        Ok(offers) => offers
            .into_iter()
            .filter(|offer| {
                state
                    .config
                    .a2a
                    .clients
                    .iter()
                    .any(|client| client.offer_hashes.contains(&offer.hash))
            })
            .collect::<Vec<_>>(),
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "INTERNAL",
                "Failed to resolve admitted A2A Offers",
            );
        }
    };
    if offers.is_empty() {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "INTERNAL",
            "No active A2A-scoped Offers",
        );
    }
    let mut kinds: Vec<_> = offers
        .iter()
        .map(|offer| offer.payload.offer_kind.clone())
        .collect();
    kinds.sort();
    kinds.dedup();
    let skills:Vec<_>=kinds.into_iter().map(|kind|json!({"id":kind,"name":"Bounded Froglet workload","description":format!("Quote, execute and verify currently admitted {kind} workloads"),"tags":["froglet","signed-deal",kind]})).collect();
    let transport = state.transport_status.lock().await;
    let Some(base) = state
        .config
        .public_base_url
        .as_ref()
        .or(transport.clearnet_url.as_ref())
    else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "INTERNAL",
            "Configure a public provider URL before A2A discovery",
        );
    };
    if state
        .config
        .relay
        .planned_public_url(state.identity.node_id())
        .ok()
        .flatten()
        .as_deref()
        == Some(base.as_str())
        || transport.relay_url.as_deref() == Some(base.as_str())
    {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "UNSUPPORTED_OPERATION",
            "This A2A profile requires direct ingress; relay A2A routes are not enabled",
        );
    }
    Json(json!({"name":"Froglet bounded workloads","description":"Caller-signed bounded workloads and verified Deal receipts over A2A 1.0 polling",
        "version":env!("CARGO_PKG_VERSION"),"supportedInterfaces":[{"url":format!("{}{PREFIX}",base.trim_end_matches('/')),"protocolBinding":"HTTP+JSON","protocolVersion":"1.0"}],
        "capabilities":{"streaming":false,"pushNotifications":false,"extendedAgentCard":false,"extensions":[{"uri":EXTENSION,"required":true,"description":"Exact JSON signed Froglet Quote, Deal and Receipt commitments; requester-bound access; no cancellation"}]},
        "defaultInputModes":["application/json"],"defaultOutputModes":["application/json"],
        "securitySchemes":{"caller":{"httpAuthSecurityScheme":{"scheme":"bearer"}}},"securityRequirements":[{"schemes":{"caller":{"list":[]}}}],
        "skills":skills})).into_response()
}
