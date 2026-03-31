use super::*;

pub(crate) fn publish_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/node/events/publish", post(super::publish_event))
        .route_layer(ConcurrencyLimitLayer::new(32))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(super::handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(
                    super::DEFAULT_ROUTE_TIMEOUT_SECS,
                ))),
        )
}

pub(crate) fn query_routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    let limit = state
        .db
        .read_connection_count()
        .clamp(1, super::DEFAULT_EVENTS_QUERY_ROUTE_CONCURRENCY_LIMIT);
    Router::new()
        .route("/v1/node/events/query", post(super::query_events))
        .route_layer(ConcurrencyLimitLayer::new(limit))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(super::handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(
                    super::DEFAULT_ROUTE_TIMEOUT_SECS,
                ))),
        )
}

pub(crate) fn provider_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/curated-lists/verify", post(super::verify_curated_list))
        .route("/v1/nostr/events/verify", post(super::verify_nostr_event))
}
