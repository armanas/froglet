//! phoenixd (ACINQ) Lightning backend — the easy self-custodial rail.
//!
//! phoenixd is a single-binary, self-custodial Lightning node with automatic
//! liquidity (pay-to-open / splicing) and a small HTTP Basic-auth API.  It is
//! the "run one binary, paste a URL + password" path for self-custodial
//! Lightning.
//!
//! **It does not support hold invoices.**  `createinvoice` picks the payment
//! hash itself, payments settle immediately on receipt, and there is no
//! settle/cancel-by-preimage.  So phoenixd cannot drive the hold-invoice escrow
//! used by `lightning.base_fee_plus_success_fee.v1`.  Instead it backs the
//! prepaid, non-escrow method `lightning.prepaid.v1`: the buyer pays an
//! ordinary invoice upfront and the receipt carries the payment preimage as a
//! cryptographic proof of payment (`sha256(preimage) == payment_hash`).
//!
//! [`PhoenixdClient`] therefore exposes the prepaid primitives
//! (`create_invoice`, `get_incoming_payment`, `pay_invoice`, `get_info`) as
//! inherent methods, reached through the concrete `Option<Arc<PhoenixdClient>>`
//! on `AppState`.  Its [`LightningWallet`] implementation supports only the
//! non-hold operations; the hold operations return a clear error so that the
//! escrow flow — which is gated on `LightningMode::LndRest`/`Mock` and never
//! dispatches to a phoenixd wallet — fails loudly if it ever reaches here.

use crate::config::LightningPhoenixdConfig;
use crate::lnd::{CreatedInvoice, InvoiceDetails, InvoiceState, LndNodeInfo};
use crate::settlement::wallet::{LightningWallet, WalletError};
use futures::future::BoxFuture;
use reqwest::{Client, Url};
use serde::Deserialize;
use std::error::Error as StdError;
use std::time::Duration;
use thiserror::Error;
use zeroize::Zeroizing;

/// Result of looking up an incoming (received) payment on phoenixd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingPayment {
    pub is_paid: bool,
    /// Hex-encoded 32-byte preimage.  Present once the payment is received;
    /// empty otherwise.  `sha256(preimage) == payment_hash`.
    pub preimage_hex: String,
    pub received_sat: u64,
}

/// Result of paying a BOLT11 invoice via phoenixd (buyer side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentPayment {
    pub payment_hash_hex: String,
    /// Hex-encoded 32-byte preimage revealed by the payee on settlement.
    pub preimage_hex: String,
    pub routing_fee_sat: u64,
}

#[derive(Debug, Error)]
pub enum PhoenixdError {
    #[error("invalid phoenixd configuration: {0}")]
    Config(String),
    #[error("failed to build phoenixd client: {0}")]
    Client(String),
    #[error("failed to build phoenixd URL: {0}")]
    Url(String),
    #[error("phoenixd request failed: {0}")]
    Http(String),
    #[error("phoenixd returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("invalid phoenixd response: {0}")]
    Decode(String),
}

/// Map a [`PhoenixdError`] to a [`WalletError`] for the `LightningWallet`
/// trait surface.  HTTP 404 → `NotFound`, everything else → `Backend`.
pub(crate) fn map_phoenixd_error(error: PhoenixdError) -> WalletError {
    match error {
        PhoenixdError::Status { status: 404, .. } => WalletError::NotFound,
        other => WalletError::Backend(other.to_string()),
    }
}

const HOLD_UNSUPPORTED: &str = "phoenixd does not support hold invoices; use lightning.prepaid.v1";

#[derive(Clone)]
pub struct PhoenixdClient {
    base_url: Url,
    client: Client,
    http_password: Zeroizing<String>,
}

impl std::fmt::Debug for PhoenixdClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhoenixdClient")
            .field("base_url", &self.base_url.as_str())
            .field("http_password", &"[REDACTED]")
            .finish()
    }
}

impl PhoenixdClient {
    pub fn from_config(config: &LightningPhoenixdConfig) -> Result<Self, PhoenixdError> {
        crate::tls::ensure_rustls_crypto_provider();
        let base_url = Url::parse(&config.url)
            .map_err(|error| PhoenixdError::Config(format!("invalid phoenixd url: {error}")))?;
        match base_url.scheme() {
            "https" => {}
            "http" if matches!(base_url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) => {}
            "http" => {
                return Err(PhoenixdError::Config(
                    "plain HTTP phoenixd URLs are only allowed on loopback addresses".to_string(),
                ));
            }
            scheme => {
                return Err(PhoenixdError::Config(format!(
                    "unsupported phoenixd URL scheme: {scheme}"
                )));
            }
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .build()
            .map_err(|error| PhoenixdError::Client(error.to_string()))?;
        Ok(Self {
            base_url,
            client,
            http_password: config.http_password.clone(),
        })
    }

    /// Create an ordinary (non-hold) BOLT11 invoice for `amount_sat`.
    ///
    /// phoenixd chooses the payment hash and preimage itself; the returned
    /// `payment_hash_hex` is what the provider then watches via
    /// [`Self::get_incoming_payment`].
    pub async fn create_invoice(
        &self,
        amount_sat: u64,
        description: &str,
        expiry_secs: u64,
    ) -> Result<CreatedInvoice, PhoenixdError> {
        let amount = amount_sat.to_string();
        let expiry = expiry_secs.to_string();
        let response: CreateInvoiceResponse = self
            .post_form(
                "/createinvoice",
                &[
                    ("amountSat", amount.as_str()),
                    ("description", description),
                    ("expirySeconds", expiry.as_str()),
                ],
            )
            .await?;
        Ok(CreatedInvoice {
            payment_request: response.serialized,
            payment_hash_hex: response.payment_hash,
        })
    }

    /// Look up an incoming payment by payment hash.
    pub async fn get_incoming_payment(
        &self,
        payment_hash_hex: &str,
    ) -> Result<IncomingPayment, PhoenixdError> {
        let path = format!("/payments/incoming/{payment_hash_hex}");
        let response: IncomingPaymentResponse = self.get_json(&path).await?;
        Ok(IncomingPayment {
            is_paid: response.is_paid,
            preimage_hex: response.preimage.unwrap_or_default(),
            received_sat: response.received_sat,
        })
    }

    /// Pay a BOLT11 invoice (buyer side).  `amount_sat` overrides the invoice
    /// amount for amountless invoices; pass `None` for fixed-amount invoices.
    pub async fn pay_invoice(
        &self,
        bolt11: &str,
        amount_sat: Option<u64>,
    ) -> Result<SentPayment, PhoenixdError> {
        let mut params: Vec<(&str, String)> = vec![("invoice", bolt11.to_string())];
        if let Some(sat) = amount_sat {
            params.push(("amountSat", sat.to_string()));
        }
        let borrowed: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let response: PayInvoiceResponse = self.post_form("/payinvoice", &borrowed).await?;
        Ok(SentPayment {
            payment_hash_hex: response.payment_hash,
            preimage_hex: response.payment_preimage,
            routing_fee_sat: response.routing_fee_sat,
        })
    }

    pub async fn get_info(&self) -> Result<LndNodeInfo, PhoenixdError> {
        let response: GetInfoResponse = self.get_json("/getinfo").await?;
        Ok(LndNodeInfo {
            identity_pubkey: response.node_id,
            alias: None,
            version: None,
        })
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, PhoenixdError> {
        let url = self.join(path)?;
        let response = self
            .client
            .get(url)
            .basic_auth("", Some(self.http_password.as_str()))
            .send()
            .await
            .map_err(|error| PhoenixdError::Http(format_reqwest_error(&error)))?;
        parse_response(response).await
    }

    async fn post_form<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, PhoenixdError> {
        let url = self.join(path)?;
        let response = self
            .client
            .post(url)
            .basic_auth("", Some(self.http_password.as_str()))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(encode_form_params(params))
            .send()
            .await
            .map_err(|error| PhoenixdError::Http(format_reqwest_error(&error)))?;
        parse_response(response).await
    }

    fn join(&self, path: &str) -> Result<Url, PhoenixdError> {
        self.base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| PhoenixdError::Url(error.to_string()))
    }
}

// ─── LightningWallet impl ─────────────────────────────────────────────────────
//
// phoenixd supports only the non-hold operations.  The prepaid flow uses the
// inherent methods above (via the concrete client on AppState); the trait
// surface exists so phoenixd can populate `state.lightning_wallet` for shared
// helpers like `resolve_lightning_destination_identity` (get_info).  The hold
// operations return a clear error — they are never reached because the escrow
// flow is gated on LndRest/Mock.

impl LightningWallet for PhoenixdClient {
    fn get_info<'a>(&'a self) -> BoxFuture<'a, Result<LndNodeInfo, WalletError>> {
        Box::pin(async move { self.get_info().await.map_err(map_phoenixd_error) })
    }

    fn add_invoice<'a>(
        &'a self,
        value_msat: u64,
        expiry_secs: u64,
        memo: &'a str,
        _private: bool,
    ) -> BoxFuture<'a, Result<CreatedInvoice, WalletError>> {
        Box::pin(async move {
            self.create_invoice(value_msat / 1000, memo, expiry_secs)
                .await
                .map_err(map_phoenixd_error)
        })
    }

    fn add_hold_invoice<'a>(
        &'a self,
        _payment_hash_hex: &'a str,
        _value_msat: u64,
        _expiry_secs: u64,
        _cltv_expiry: u32,
        _memo: &'a str,
        _private: bool,
    ) -> BoxFuture<'a, Result<CreatedInvoice, WalletError>> {
        Box::pin(async move { Err(WalletError::Backend(HOLD_UNSUPPORTED.to_string())) })
    }

    fn settle_invoice<'a>(
        &'a self,
        _preimage_hex: &'a str,
    ) -> BoxFuture<'a, Result<(), WalletError>> {
        Box::pin(async move { Err(WalletError::Backend(HOLD_UNSUPPORTED.to_string())) })
    }

    fn cancel_invoice<'a>(
        &'a self,
        _payment_hash_hex: &'a str,
    ) -> BoxFuture<'a, Result<(), WalletError>> {
        Box::pin(async move { Err(WalletError::Backend(HOLD_UNSUPPORTED.to_string())) })
    }

    fn lookup_invoice<'a>(
        &'a self,
        payment_hash_hex: &'a str,
    ) -> BoxFuture<'a, Result<InvoiceDetails, WalletError>> {
        Box::pin(async move {
            let payment = self
                .get_incoming_payment(payment_hash_hex)
                .await
                .map_err(map_phoenixd_error)?;
            Ok(InvoiceDetails {
                payment_request: String::new(),
                payment_hash_hex: payment_hash_hex.to_string(),
                value_msat: payment.received_sat.saturating_mul(1000),
                expiry_secs: 0,
                state: if payment.is_paid {
                    InvoiceState::Settled
                } else {
                    InvoiceState::Open
                },
            })
        })
    }
}

// ─── Wire types + helpers ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GetInfoResponse {
    #[serde(rename = "nodeId")]
    node_id: String,
}

#[derive(Debug, Deserialize)]
struct CreateInvoiceResponse {
    #[serde(rename = "paymentHash")]
    payment_hash: String,
    serialized: String,
}

#[derive(Debug, Deserialize)]
struct IncomingPaymentResponse {
    #[serde(rename = "isPaid")]
    is_paid: bool,
    #[serde(default)]
    preimage: Option<String>,
    #[serde(rename = "receivedSat", default)]
    received_sat: u64,
}

#[derive(Debug, Deserialize)]
struct PayInvoiceResponse {
    #[serde(rename = "paymentHash")]
    payment_hash: String,
    #[serde(rename = "paymentPreimage")]
    payment_preimage: String,
    #[serde(rename = "routingFeeSat", default)]
    routing_fee_sat: u64,
}

fn format_reqwest_error(error: &reqwest::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source: Option<&(dyn StdError + 'static)> = error.source();
    while let Some(next) = source {
        parts.push(next.to_string());
        source = next.source();
    }
    parts.join(": ")
}

async fn parse_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, PhoenixdError> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| PhoenixdError::Http(error.to_string()))?;
    if !status.is_success() {
        return Err(PhoenixdError::Status {
            status: status.as_u16(),
            body,
        });
    }
    serde_json::from_str(&body).map_err(|error| PhoenixdError::Decode(error.to_string()))
}

/// Encode key-value pairs as `application/x-www-form-urlencoded` without
/// pulling in reqwest's `form` feature (mirrors `stripe::encode_form_params`).
fn encode_form_params(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Path, State},
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::{Value, json};
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    const TEST_PASSWORD: &str = "test-http-password";

    #[derive(Clone, Default)]
    struct TestState {
        requests: Arc<Mutex<Vec<(String, String)>>>,
    }

    fn test_config(base_url: String) -> LightningPhoenixdConfig {
        LightningPhoenixdConfig {
            url: base_url,
            http_password: Zeroizing::new(TEST_PASSWORD.to_string()),
            request_timeout_secs: 5,
        }
    }

    fn assert_basic_auth(headers: &HeaderMap) {
        let expected = format!("Basic {}", STANDARD.encode(format!(":{TEST_PASSWORD}")));
        assert_eq!(
            headers.get("Authorization").unwrap().to_str().unwrap(),
            expected
        );
    }

    async fn getinfo(headers: HeaderMap) -> (StatusCode, Json<Value>) {
        assert_basic_auth(&headers);
        (
            StatusCode::OK,
            Json(json!({ "nodeId": "03".to_string() + &"ab".repeat(32) })),
        )
    }

    async fn createinvoice(
        State(state): State<TestState>,
        headers: HeaderMap,
        body: String,
    ) -> (StatusCode, Json<Value>) {
        assert_basic_auth(&headers);
        state
            .requests
            .lock()
            .unwrap()
            .push(("createinvoice".to_string(), body));
        (
            StatusCode::OK,
            Json(json!({
                "amountSat": 30,
                "paymentHash": "aa".repeat(32),
                "serialized": "lnbc300n1prepaidinvoice"
            })),
        )
    }

    async fn payinvoice(
        State(state): State<TestState>,
        headers: HeaderMap,
        body: String,
    ) -> (StatusCode, Json<Value>) {
        assert_basic_auth(&headers);
        state
            .requests
            .lock()
            .unwrap()
            .push(("payinvoice".to_string(), body));
        (
            StatusCode::OK,
            Json(json!({
                "paymentHash": "aa".repeat(32),
                "paymentPreimage": "bb".repeat(32),
                "routingFeeSat": 1
            })),
        )
    }

    async fn incoming(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(payment_hash): Path<String>,
    ) -> (StatusCode, Json<Value>) {
        assert_basic_auth(&headers);
        state
            .requests
            .lock()
            .unwrap()
            .push(("incoming".to_string(), payment_hash));
        (
            StatusCode::OK,
            Json(json!({
                "paymentHash": "aa".repeat(32),
                "preimage": "bb".repeat(32),
                "isPaid": true,
                "receivedSat": 30
            })),
        )
    }

    async fn start_server() -> (SocketAddr, TestState) {
        let state = TestState::default();
        let router = Router::new()
            .route("/getinfo", get(getinfo))
            .route("/createinvoice", post(createinvoice))
            .route("/payinvoice", post(payinvoice))
            .route("/payments/incoming/:payment_hash", get(incoming))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (addr, state)
    }

    #[tokio::test]
    async fn phoenixd_client_shapes_requests_and_parses_responses() {
        let (addr, state) = start_server().await;
        let client = PhoenixdClient::from_config(&test_config(format!("http://{addr}"))).unwrap();

        let info = client.get_info().await.unwrap();
        assert_eq!(info.identity_pubkey, "03".to_string() + &"ab".repeat(32));

        let invoice = client.create_invoice(30, "deal-123", 300).await.unwrap();
        assert_eq!(invoice.payment_request, "lnbc300n1prepaidinvoice");
        assert_eq!(invoice.payment_hash_hex, "aa".repeat(32));

        let incoming = client.get_incoming_payment(&"aa".repeat(32)).await.unwrap();
        assert!(incoming.is_paid);
        assert_eq!(incoming.preimage_hex, "bb".repeat(32));
        assert_eq!(incoming.received_sat, 30);

        let sent = client
            .pay_invoice("lnbc300n1prepaidinvoice", None)
            .await
            .unwrap();
        assert_eq!(sent.payment_hash_hex, "aa".repeat(32));
        assert_eq!(sent.preimage_hex, "bb".repeat(32));
        assert_eq!(sent.routing_fee_sat, 1);

        let requests = state.requests.lock().unwrap().clone();
        assert_eq!(requests[0].0, "createinvoice");
        assert!(requests[0].1.contains("amountSat=30"));
        assert!(requests[0].1.contains("description=deal-123"));
        assert_eq!(requests[1].0, "incoming");
        assert_eq!(requests[1].1, "aa".repeat(32));
        assert_eq!(requests[2].0, "payinvoice");
        assert!(requests[2].1.contains("invoice=lnbc300n1prepaidinvoice"));
    }

    #[tokio::test]
    async fn lookup_invoice_maps_incoming_to_settled() {
        let (addr, _state) = start_server().await;
        let client = PhoenixdClient::from_config(&test_config(format!("http://{addr}"))).unwrap();
        let details = client.lookup_invoice(&"aa".repeat(32)).await.unwrap();
        assert_eq!(details.state, InvoiceState::Settled);
        assert_eq!(details.value_msat, 30_000);
    }

    #[tokio::test]
    async fn hold_operations_are_unsupported() {
        let (addr, _state) = start_server().await;
        let client = PhoenixdClient::from_config(&test_config(format!("http://{addr}"))).unwrap();
        let err = client
            .add_hold_invoice(&"11".repeat(32), 1000, 300, 80, "x", true)
            .await
            .unwrap_err();
        assert!(matches!(err, WalletError::Backend(msg) if msg.contains("hold invoices")));
        assert!(matches!(
            client.settle_invoice(&"22".repeat(32)).await.unwrap_err(),
            WalletError::Backend(_)
        ));
        assert!(matches!(
            client.cancel_invoice(&"33".repeat(32)).await.unwrap_err(),
            WalletError::Backend(_)
        ));
    }

    #[test]
    fn rejects_non_loopback_http() {
        let err = PhoenixdClient::from_config(&test_config("http://10.0.0.5:9740".to_string()))
            .unwrap_err();
        assert!(err.to_string().contains("loopback"));
    }
}
