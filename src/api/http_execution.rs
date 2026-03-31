use super::*;

pub(crate) fn execute_wasm_routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    let timeout_secs = state
        .config
        .execution_timeout_secs
        .saturating_add(super::BLOCKING_EXECUTION_TIMEOUT_GRACE_SECS)
        .saturating_add(1);
    let concurrency_limit = sandbox::wasm_concurrency_limit();
    Router::new()
        .route("/v1/node/execute/wasm", post(super::execute_wasm))
        .route_layer(ConcurrencyLimitLayer::new(concurrency_limit))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(super::handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(timeout_secs))),
        )
}

pub(crate) fn jobs_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/node/jobs", post(super::create_job))
        .route("/v1/node/jobs/:job_id", get(super::get_job_status))
        .route_layer(ConcurrencyLimitLayer::new(16))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(super::handle_timeout_error))
                .layer(TimeoutLayer::new(Duration::from_secs(
                    super::DEFAULT_ROUTE_TIMEOUT_SECS,
                ))),
        )
}
