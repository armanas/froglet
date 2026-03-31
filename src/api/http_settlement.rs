use super::*;

pub(crate) fn provider_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/v1/provider/deals/:deal_id/accept",
            post(super::release_deal_preimage),
        )
        .route(
            "/v1/provider/deals/:deal_id/mock-pay",
            post(super::mock_pay_deal),
        )
        .route(
            "/v1/provider/deals/:deal_id/invoice-bundle",
            get(super::get_deal_invoice_bundle),
        )
        .route(
            "/v1/invoice-bundles/verify",
            post(super::verify_invoice_bundle),
        )
        .route("/v1/receipts/verify", post(super::verify_receipt))
}

pub(crate) fn runtime_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/v1/runtime/wallet/balance",
            get(super::runtime_wallet_balance),
        )
        .route(
            "/v1/runtime/deals/:deal_id/payment-intent",
            get(super::runtime_deal_payment_intent),
        )
        .route(
            "/v1/runtime/deals/:deal_id/mock-pay",
            post(super::runtime_mock_pay_deal),
        )
        .route(
            "/v1/runtime/deals/:deal_id/accept",
            post(super::runtime_accept_deal),
        )
}
