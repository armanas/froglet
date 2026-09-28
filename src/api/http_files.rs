//! File delivery is a separately admitted operation, never a data-query result.
use super::*;
use crate::provider_policy::{self, AccessMode};
use froglet_protocol::file_download::{CONTRACT, FileMetadata};
use std::sync::atomic::Ordering;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/v1/provider/services/:service_id/files/:revision/download",
            get(download).options(preflight),
        )
        .layer(middleware::from_fn(cors))
}

async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn cors(request: Request, next: Next) -> Response {
    let origin = request.headers().get(header::ORIGIN).cloned();
    if origin.as_ref().is_some_and(|h| {
        !matches!(
            h.to_str(),
            Ok("https://froglet.dev" | "https://candidate.froglet.dev")
        )
    }) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(request).await;
    if let Some(origin) = origin {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, HEAD, OPTIONS"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("x-froglet-access-token"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(
                "Content-Disposition, Content-Length, ETag, X-Froglet-File-Sha256",
            ),
        );
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn fail(status: StatusCode, message: &str) -> Response {
    error_json(status, json!({"error":message})).into_response()
}

async fn download(
    State(state): State<Arc<AppState>>,
    scope: Option<Extension<RelayGrantScope>>,
    Path((service_id, revision)): Path<(String, String)>,
    method: axum::http::Method,
    headers: HeaderMap,
) -> Response {
    if scope.as_ref().is_some_and(|Extension(s)| {
        !s.grants
            .iter()
            .any(|g| g.service_id == service_id && g.revision_hash == revision)
    }) {
        return fail(StatusCode::NOT_FOUND, "file not found");
    }
    let Some(limits) = state.config.provider_policy.file_download.clone() else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "file downloads are disabled",
        );
    };
    if headers.contains_key(header::RANGE) {
        return fail(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "resumable downloads are not supported",
        );
    }
    let service = match provider_service_record(&state, &service_id, false, true).await {
        Ok(Some(s))
            if s.contract_version == CONTRACT
                && s.settlement_method == "none"
                && s.price_sats == 0
                && s.base_fee_msat == 0
                && s.success_fee_msat == 0 =>
        {
            s
        }
        _ => return fail(StatusCode::NOT_FOUND, "file not found"),
    };
    let Some(digest) = service.module_hash.clone() else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "file binding unavailable");
    };
    let metadata: FileMetadata = match service
        .output_schema
        .as_ref()
        .and_then(|s| s.get("const"))
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
    {
        Some(m) => m,
        None => return fail(StatusCode::SERVICE_UNAVAILABLE, "file metadata unavailable"),
    };
    if headers
        .get("x-froglet-relay-response-limit")
        .is_some_and(|v| {
            v.to_str()
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .is_none_or(|cap| metadata.size_bytes > cap)
        })
    {
        return fail(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file exceeds this relay's response limit",
        );
    }
    if metadata.validate().is_err() {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "file metadata invalid");
    }
    if let Err((status, _)) = provider_storage_admission(&state) {
        return fail(status, "provider storage admission refused");
    }
    let owner = require_provider_control_auth(&headers, &state).is_ok();
    let policy = state.config.provider_policy.clone();
    let invitation = headers
        .get("x-froglet-access-token")
        .and_then(|v| v.to_str().ok())
        .filter(|s| (32..=256).contains(&s.len()))
        .map(|s| crypto::sha256_hex(s.as_bytes()));
    if policy.require_payment || policy.access_mode == AccessMode::Paid {
        return fail(StatusCode::FORBIDDEN, "file payments are not supported");
    }
    if !owner
        && (policy.access_mode == AccessMode::Private
            || (policy.access_mode == AccessMode::Invite && invitation.is_none()))
    {
        return fail(
            StatusCode::FORBIDDEN,
            "file download requires an invitation or owner credential",
        );
    }
    let head = method == axum::http::Method::HEAD;
    let permit = if head {
        None
    } else {
        match limits.gate.clone().try_acquire_owned() {
            Ok(p) => Some(p),
            Err(_) => {
                return fail(
                    StatusCode::TOO_MANY_REQUESTS,
                    "a file transfer is already active",
                );
            }
        }
    };
    let generation = limits.abort_generation.load(Ordering::Acquire);
    let now = settlement::current_unix_timestamp();
    let expected_binding = service.binding_hash.clone();
    let sid = service_id.clone();
    let rev = revision.clone();
    let m = metadata.clone();
    let l = limits.clone();
    let scope_key = format!("file:{service_id}:{digest}");
    let admitted = state
        .db
        .with_write_conn(move |conn| {
            db::with_immediate_transaction(conn, |conn| {
                let lifecycle =
                    db::get_publication_lifecycle(conn, &sid)?.ok_or("file not active")?;
                if lifecycle.active_revision_hash.as_deref() != Some(rev.as_str()) {
                    return Err("file revision is not active".into());
                }
                let record = db::get_publication_revision(conn, &sid, &rev)?
                    .ok_or("file revision not found")?;
                if expected_binding.as_deref() != Some(record.binding_hash.as_str()) {
                    return Err("file revision binding changed".into());
                }
                let signed: SignedPublicationRevision =
                    serde_json::from_value(record.signed_revision)
                        .map_err(|_| "file revision is invalid")?;
                signed
                    .verify()
                    .map_err(|_| "file revision signature is invalid")?;
                if signed.revision_hash != rev
                    || signed.payload.binding_hash != record.binding_hash
                    || signed.payload.service_id != sid
                    || signed.payload.service.contract_version != CONTRACT
                    || signed.payload.service.output_schema.as_ref() != Some(&m.output_schema())
                {
                    return Err("file revision metadata changed".into());
                }
                provider_policy::require_not_paused(conn)?;
                if now >= m.expires_at {
                    return Err("file publication has expired".into());
                }
                if !owner && policy.access_mode == AccessMode::Invite {
                    let hash = invitation.as_deref().ok_or("access denied")?;
                    if !provider_policy::authorize_invite_in_transaction(conn, &policy, hash, now)?
                    {
                        return Err("access denied".into());
                    }
                }
                if !head {
                    crate::file_download::reserve(conn, &scope_key, &m, &l, now)?;
                }
                Ok(())
            })
        })
        .await;
    if let Err(error) = admitted {
        let status = if error == "access denied" {
            StatusCode::FORBIDDEN
        } else if error.contains("allowance") {
            StatusCode::TOO_MANY_REQUESTS
        } else {
            StatusCode::GONE
        };
        let message = match status {
            StatusCode::FORBIDDEN => "file access denied",
            StatusCode::TOO_MANY_REQUESTS => "file transfer allowance exhausted",
            _ => "file share is not active, has expired, or is paused",
        };
        return fail(status, message);
    }
    let body = if head {
        axum::body::Body::empty()
    } else {
        let root = state.config.storage.data_dir.join("publication-data");
        let file = tokio::task::spawn_blocking(move || {
            crate::file_download::read_snapshot(&root, &digest)
        })
        .await;
        let package = match file {
            Ok(Ok(b)) => b,
            _ => return fail(StatusCode::SERVICE_UNAVAILABLE, "file snapshot unavailable"),
        };
        let data = match froglet_protocol::file_download::decode(&package) {
            Ok((m, b)) if m == metadata => Bytes::copy_from_slice(b),
            _ => {
                return fail(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "file snapshot does not match publication",
                );
            }
        };
        let deadline = Instant::now() + Duration::from_secs(50);
        let abort = limits.abort_generation.clone();
        axum::body::Body::from_stream(stream::unfold(
            (data, permit),
            move |(mut bytes, permit)| {
                let abort = abort.clone();
                async move {
                    if bytes.is_empty() {
                        return None;
                    }
                    if Instant::now() >= deadline || abort.load(Ordering::Acquire) != generation {
                        return Some((
                            Err::<Bytes, std::io::Error>(std::io::Error::other(
                                "file transfer stopped",
                            )),
                            (Bytes::new(), permit),
                        ));
                    }
                    let chunk = bytes.split_to(bytes.len().min(65536));
                    Some((Ok::<Bytes, std::io::Error>(chunk), (bytes, permit)))
                }
            },
        ))
    };
    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", metadata.filename),
        )
        .header(header::CONTENT_LENGTH, metadata.size_bytes.to_string())
        .header(header::ETAG, format!("\"{}\"", metadata.sha256))
        .header("x-froglet-file-sha256", metadata.sha256)
        .header("x-content-type-options", "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub(super) async fn abort(State(state): State<Arc<AppState>>) -> Response {
    if let Some(limits) = &state.config.provider_policy.file_download {
        limits.abort_generation.fetch_add(1, Ordering::AcqRel);
    }
    Json(json!({"status":"abort_requested","allowances_reset":false})).into_response()
}
