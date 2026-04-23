use super::*;

pub(crate) fn provider_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/provider/quotes", post(super::create_quote))
        .route("/v1/provider/deals", post(super::create_deal))
        .route("/v1/provider/deals/:deal_id", get(super::get_deal_status))
}

pub(crate) fn runtime_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/runtime/deals", post(super::runtime_create_deal))
        .route("/v1/runtime/deals/:deal_id", get(super::runtime_get_deal))
        .route(
            "/v1/runtime/archive/:subject_kind/:subject_id",
            get(super::runtime_archive_subject),
        )
}

pub(crate) fn hosted_trial_runtime_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/v1/runtime/deals",
            post(super::hosted_trial_runtime_create_deal),
        )
        .route(
            "/v1/runtime/deals/:deal_id",
            get(super::hosted_trial_runtime_get_deal),
        )
}
