//! Optional requester-side A2A transport. Routing and bearer credentials are
//! bound to exact operator-configured origins, never caller-supplied tokens.
use super::*;
use crate::a2a_config::A2aProvider;
use crate::settlement::LightningInvoiceBundleSession;

#[cfg(test)]
#[path = "remote_client_tests.rs"]
mod tests;

fn configured_provider<'a>(state: &'a AppState, origin: &str) -> Option<&'a A2aProvider> {
    state
        .config
        .a2a
        .providers
        .iter()
        .find(|provider| provider.provider_url == origin.trim_end_matches('/'))
}

pub(super) fn uses_a2a(state: &AppState, origin: &str) -> bool {
    configured_provider(state, origin).is_some()
}

pub(super) fn configured_loopback_endpoint(
    state: &AppState,
    raw_url: &str,
) -> Result<Option<provider_resolution::RuntimeProviderEndpoint>, ApiFailure> {
    let Some(provider) =
        configured_provider(state, raw_url).filter(|provider| provider.allow_loopback)
    else {
        return Ok(None);
    };
    let parsed = reqwest::Url::parse(&provider.provider_url)
        .map_err(|_| provider_bad_gateway("Invalid configured A2A loopback origin"))?;
    let address = match parsed.host_str() {
        Some("127.0.0.1") => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        Some("[::1]" | "::1") => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        _ => {
            return Err(provider_bad_gateway(
                "A2A loopback permission requires a literal loopback origin",
            ));
        }
    };
    if parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(provider_bad_gateway(
            "Invalid configured A2A loopback origin",
        ));
    }
    Ok(Some(provider_resolution::RuntimeProviderEndpoint {
        url: provider.provider_url.clone(),
        pinned_public_addresses: vec![address],
    }))
}

pub(super) async fn a2a_request<T, B>(
    state: &AppState,
    method: &reqwest::Method,
    url: &str,
    body: Option<&B>,
    pinned_addresses: &[IpAddr],
    access: Option<(&str, &str)>,
) -> Option<Result<T, ApiFailure>>
where
    T: DeserializeOwned,
    B: Serialize + ?Sized,
{
    let parsed = match reqwest::Url::parse(url) {
        Ok(parsed) => parsed,
        Err(_) => return None,
    };
    let origin = parsed.origin().ascii_serialization();
    let provider = configured_provider(state, &origin)?;
    let path = parsed.path();
    let result=async {
        let payload=body.map(serde_json::to_value).transpose().map_err(|_|provider_bad_gateway("Failed to encode requester A2A operation"))?;
        let value=if matches!(path,"/v1/provider/descriptor"|"/v1/provider/offers") && *method==reqwest::Method::GET {
            let response=send(state,provider,"catalog",&json!({}),None,pinned_addresses,access).await?;
            let catalog=message_payload(response,"catalog")?;
            catalog.get(if path.ends_with("descriptor"){"descriptor"}else{"offers"}).cloned()
                .ok_or_else(||provider_bad_gateway("A2A catalog is missing signed artifacts"))?
        } else if path=="/v1/provider/quotes" && *method==reqwest::Method::POST {
            message_payload(send(state,provider,"quote",&payload.ok_or_else(||provider_bad_gateway("Missing Quote request"))?,None,pinned_addresses,access).await?,"quote")?
        } else if path=="/v1/provider/deals" && *method==reqwest::Method::POST {
            let payload=payload.ok_or_else(||provider_bad_gateway("Missing signed Deal request"))?;
            let expected:CreateDealRequest=serde_json::from_value(payload.clone()).map_err(|_|provider_bad_gateway("Invalid signed Deal request"))?;
            let id=protocol::artifact_hash(&expected.deal).map_err(|_|provider_bad_gateway("Invalid signed Deal hash"))?;
            // No taskId when creating a new Task; the response is deterministic
            // and the provider also handles exact signed replays after expiry.
            task_payload(send(state,provider,"submit",&payload,None,pinned_addresses,access).await?.get("task").cloned().ok_or_else(||provider_bad_gateway("A2A submit did not return a Task"))?,Some(&id))?
        } else if let Some(tail)=path.strip_prefix("/v1/provider/deals/") {
            let (backend_id,operation)=tail.split_once('/').unwrap_or((tail,"status"));
            let id=transport_task_id(state,backend_id).await?;
            if operation=="status" && *method==reqwest::Method::GET {
                task_payload(exchange(state,provider,reqwest::Method::GET,format!("{}/a2a/v1/tasks/{id}",provider.provider_url),None,pinned_addresses,None).await?,Some(&id))?
            } else if operation=="accept" && *method==reqwest::Method::POST {
                let payload=payload.ok_or_else(||provider_bad_gateway("Missing acceptance request"))?;
                let current=exchange(state,provider,reqwest::Method::GET,format!("{}/a2a/v1/tasks/{id}",provider.provider_url),None,pinned_addresses,None).await
                    .and_then(|task|task_payload(task,Some(&id)))?;
                if verified_acceptance_replay(&current,&payload)? {
                    current
                } else {
                match send(state,provider,"accept",&payload,Some(&id),pinned_addresses,None).await {
                    Ok(response)=>task_payload(response.get("task").cloned().ok_or_else(||provider_bad_gateway("A2A acceptance did not return a Task"))?,Some(&id))?,
                    Err(failure)=>{
                        // A2A forbids continuing a terminal Task. After a lost
                        // acceptance response, read and verify its terminal
                        // evidence instead of manufacturing another message.
                        let recovered=exchange(state,provider,reqwest::Method::GET,format!("{}/a2a/v1/tasks/{id}",provider.provider_url),None,pinned_addresses,None).await
                            .and_then(|task|task_payload(task,Some(&id)));
                        match recovered {
                            Ok(record) if verified_acceptance_replay(&record,&payload)?=>record,
                            _=>return Err(failure),
                        }
                    }
                }
                }
            } else if operation=="invoice-bundle" && *method==reqwest::Method::GET {
                invoice_bundle_payload(exchange(state,provider,reqwest::Method::GET,format!("{}/a2a/v1/tasks/{id}",provider.provider_url),None,pinned_addresses,None).await?,&id)?
            } else {
                return Err((StatusCode::BAD_REQUEST,json!({"error":"Configured A2A transport does not support this provider operation"})));
            }
        } else {
            return Err((StatusCode::BAD_REQUEST,json!({"error":"Configured A2A transport does not support this provider path"})));
        };
        serde_json::from_value(value).map_err(|_|provider_bad_gateway("Invalid A2A Froglet payload"))
    }.await;
    Some(result)
}

fn verified_acceptance_replay(record: &Value, request: &Value) -> Result<bool, ApiFailure> {
    let request: ReleaseDealPreimageRequest = serde_json::from_value(request.clone())
        .map_err(|_| provider_bad_gateway("Invalid acceptance request"))?;
    let expected = request
        .expected_result_hash
        .ok_or_else(|| provider_bad_gateway("A2A acceptance requires expected_result_hash"))?;
    let expected = normalize_hex_value("expected_result_hash", expected, 64)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let record: deals::DealRecord = serde_json::from_value(record.clone())
        .map_err(|_| provider_bad_gateway("Invalid acceptance Task"))?;
    if record.quote.payload.settlement_terms.method != "lightning.base_fee_plus_success_fee.v1" {
        return Err(provider_bad_gateway(
            "Acceptance requires a Lightning success-fee Deal",
        ));
    }
    if record.receipt.is_some() && record.status == deals::DEAL_STATUS_SUCCEEDED {
        if record.result_hash.as_deref() != Some(&expected) {
            return Err(provider_bad_gateway(
                "Acceptance result hash conflicts with verified terminal evidence",
            ));
        }
        return Ok(true);
    }
    Ok(false)
}

fn invoice_bundle_payload(task: Value, id: &str) -> Result<Value, ApiFailure> {
    let record: deals::DealRecord =
        serde_json::from_value(task_payload(task.clone(), Some(id))?)
            .map_err(|_| provider_bad_gateway("Invalid invoice-bundle Task"))?;
    let bundle: LightningInvoiceBundleSession = serde_json::from_str(
        task["metadata"]["froglet"]["invoiceBundle"]
            .as_str()
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    json!({"error":"Committed Lightning invoice bundle is not available"}),
                )
            })?,
    )
    .map_err(|_| provider_bad_gateway("Invalid exact Lightning bundle payload"))?;
    if !settlement::validate_lightning_invoice_bundle(
        &bundle.bundle,
        &record.quote,
        &record.deal,
        None,
    )
    .valid
    {
        return Err(provider_bad_gateway(
            "Lightning invoice bundle conflicts with the signed Deal",
        ));
    }
    serde_json::to_value(bundle)
        .map_err(|_| provider_bad_gateway("Failed to decode committed Lightning bundle"))
}

async fn transport_task_id(state: &AppState, local_id: &str) -> Result<String, ApiFailure> {
    if crate::a2a_config::is_hash(local_id) {
        return Ok(local_id.to_string());
    }
    let id = local_id.to_string();
    let stored = state
        .db
        .with_read_conn(move |conn| requester_deals::get_requester_deal(conn, &id))
        .await
        .map_err(|_| provider_bad_gateway("Cannot load durable A2A requester intent"))?
        .ok_or_else(|| provider_bad_gateway("Missing durable A2A requester intent"))?;
    protocol::artifact_hash(&stored.deal)
        .map_err(|_| provider_bad_gateway("Invalid durable A2A Deal hash"))
}

async fn send(
    state: &AppState,
    provider: &A2aProvider,
    operation: &str,
    payload: &Value,
    task_id: Option<&str>,
    addresses: &[IpAddr],
    access: Option<(&str, &str)>,
) -> Result<Value, ApiFailure> {
    let mut message = json!({"messageId":protocol::new_artifact_id(),"role":"ROLE_USER","parts":[a2a::data_part(operation,payload)?],"extensions":[a2a::EXTENSION]});
    if let Some(id) = task_id {
        message["taskId"] = json!(id);
        message["contextId"] = json!(a2a::context_id(id));
    }
    let request = json!({"message":message,"configuration":{"returnImmediately":true,"historyLength":0,"acceptedOutputModes":["application/json"]}});
    exchange(
        state,
        provider,
        reqwest::Method::POST,
        format!("{}/a2a/v1/message:send", provider.provider_url),
        Some(&request),
        addresses,
        access,
    )
    .await
}

async fn exchange(
    state: &AppState,
    provider: &A2aProvider,
    method: reqwest::Method,
    url: String,
    body: Option<&Value>,
    addresses: &[IpAddr],
    access: Option<(&str, &str)>,
) -> Result<Value, ApiFailure> {
    if [
        state.runtime_auth_token.as_str(),
        state.provider_control_auth_token.as_str(),
        state.consumer_control_auth_token.as_str(),
    ]
    .contains(&provider.token.as_str())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({"error":"A2A requires a separate configured bearer credential"}),
        ));
    }
    let client = if addresses.is_empty() {
        state.http_client.clone()
    } else {
        pinned_json_client(state, &url, addresses)
            .map_err(|_| provider_bad_gateway("Failed to pin A2A provider address"))?
    };
    let mut bearer = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", provider.token))
        .map_err(|_| provider_bad_gateway("Invalid configured A2A credential"))?;
    bearer.set_sensitive(true);
    let mut request = client
        .request(method, url)
        .header("authorization", bearer)
        .header("a2a-version", "1.0")
        .header("a2a-extensions", a2a::EXTENSION);
    let invite = access.filter(|(name, _)| *name == "x-froglet-access-token");
    if let Some((name, token)) = invite {
        let mut value = reqwest::header::HeaderValue::from_str(token)
            .map_err(|_| provider_bad_gateway("Invalid invitation credential"))?;
        value.set_sensitive(true);
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|_| {
        provider_bad_gateway(
            "A2A provider request failed; durable submitted intent remains queryable",
        )
    })?;
    let status = response.status();
    let quote_retry = response
        .headers()
        .get(header::RETRY_AFTER)
        .is_some_and(|value| value == "1");
    let bytes = crate::http_body::read_response_bytes_limited(
        response,
        MAX_UPSTREAM_JSON_BYTES,
        "A2A JSON response",
    )
    .await
    .map_err(|_| provider_bad_gateway("A2A provider response exceeded its limit"))?;
    let text = String::from_utf8_lossy(&bytes);
    if text.contains(&provider.token) || invite.is_some_and(|(_, token)| text.contains(token)) {
        return Err(provider_bad_gateway(
            "A2A provider response contained access credentials",
        ));
    }
    // Never reflect an untrusted upstream error body: it may echo acceptance
    // preimages, payment credentials or the caller's Authorization header.
    if !status.is_success() {
        // Recognize only this bounded, pre-admission signal. Reconstruct our
        // own error rather than reflecting any untrusted upstream text.
        if status == StatusCode::SERVICE_UNAVAILABLE
            && quote_retry
            && serde_json::from_slice::<Value>(&bytes)
                .ok()
                .is_some_and(|value| {
                    value["error"]["code"] == 503
                        && value["error"]["status"] == "UNAVAILABLE"
                        && value["error"]["details"].as_array().is_some_and(|details| {
                            details.iter().any(|detail| {
                                detail["@type"] == "type.googleapis.com/google.rpc.ErrorInfo"
                                    && detail["reason"] == "QUOTE_ISSUANCE_COLLISION"
                                    && detail["domain"] == "a2a-protocol.org"
                            })
                        })
                })
        {
            return Err(quote_issuance_collision());
        }
        return Err((
            if status.is_client_error() {
                status
            } else {
                StatusCode::BAD_GATEWAY
            },
            json!({"error":"A2A provider rejected operation","upstream_status":status.as_u16()}),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| provider_bad_gateway("Invalid A2A provider JSON response"))
}

pub(super) fn message_payload(response: Value, operation: &str) -> Result<Value, ApiFailure> {
    if response.get("task").is_some() {
        return Err(provider_bad_gateway(
            "Expected an A2A Message, received a Task",
        ));
    }
    let message = response
        .get("message")
        .ok_or_else(|| provider_bad_gateway("Missing A2A Message response"))?;
    if message["role"] != "ROLE_AGENT"
        || message["messageId"].as_str().is_none_or(str::is_empty)
        || message["contextId"].as_str().is_none_or(str::is_empty)
    {
        return Err(provider_bad_gateway("Invalid A2A Message response"));
    }
    extension_payload(&message["parts"], operation)
}

fn extension_payload(parts: &Value, operation: &str) -> Result<Value, ApiFailure> {
    let parts = parts
        .as_array()
        .filter(|parts| parts.len() == 1)
        .ok_or_else(|| provider_bad_gateway("Expected exactly one Froglet A2A data Part"))?;
    let part = &parts[0];
    if part.get("kind").is_some()
        || ["text", "raw", "url"]
            .iter()
            .any(|key| part.get(*key).is_some())
        || part["data"]["schema"] != a2a::SCHEMA
        || part["data"]["operation"] != operation
    {
        return Err(provider_bad_gateway("Invalid Froglet A2A data Part"));
    }
    serde_json::from_str(
        part["data"]["payload"]
            .as_str()
            .ok_or_else(|| provider_bad_gateway("Froglet A2A payload must be exact JSON text"))?,
    )
    .map_err(|_| provider_bad_gateway("Invalid exact Froglet JSON payload"))
}

pub(super) fn task_payload(value: Value, expected_id: Option<&str>) -> Result<Value, ApiFailure> {
    let artifacts = value["artifacts"]
        .as_array()
        .filter(|artifacts| artifacts.len() == 1)
        .ok_or_else(|| provider_bad_gateway("Missing Froglet A2A Task artifact"))?;
    if artifacts[0]["artifactId"] != "froglet-deal" {
        return Err(provider_bad_gateway("Unknown Froglet A2A Task artifact"));
    }
    let payload = extension_payload(&artifacts[0]["parts"], "deal")?;
    let mut record: deals::DealRecord = serde_json::from_value(payload)
        .map_err(|_| provider_bad_gateway("Invalid A2A Deal record"))?;
    let expected = a2a::task(&record)?;
    if value["id"] != expected["id"]
        || value["contextId"] != expected["contextId"]
        || value["status"]["state"] != expected["status"]["state"]
        || expected_id.is_some_and(|id| value["id"].as_str() != Some(id))
    {
        return Err(provider_bad_gateway(
            "A2A Task projection conflicts with signed Froglet evidence",
        ));
    }
    // The requester uses the stable transport identity. The provider's local
    // random storage ID remains only inside its original wire DealRecord.
    record.deal_id = expected["id"].as_str().unwrap().to_string();
    serde_json::to_value(record).map_err(|_| provider_bad_gateway("Failed to decode A2A Task"))
}
