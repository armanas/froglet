use super::*;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/provider/descriptor", get(super::protocol_descriptor))
        .route("/v1/provider/offers", get(super::list_offers))
        .route("/v1/provider/services", get(super::list_provider_services))
        .route(
            "/v1/provider/services/:service_id",
            get(super::get_provider_service),
        )
        .route(
            "/v1/provider/artifacts/publish",
            post(super::publish_artifact),
        )
        .route("/v1/feed", get(super::get_feed))
        .route("/v1/artifacts/:artifact_hash", get(super::get_artifact))
}
