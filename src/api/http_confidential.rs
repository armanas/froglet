use super::*;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/v1/provider/confidential/profiles/:artifact_hash",
            get(super::get_confidential_profile),
        )
        .route(
            "/v1/provider/confidential/sessions",
            post(super::open_confidential_session),
        )
        .route(
            "/v1/provider/confidential/sessions/:session_id",
            get(super::get_confidential_session),
        )
}
