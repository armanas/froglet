use super::*;

pub(crate) fn runtime_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/runtime/search", post(super::runtime_search))
        .route(
            "/v1/runtime/providers/:provider_id",
            get(super::runtime_provider_details),
        )
}
