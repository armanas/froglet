use crate::config::LightningLndRestConfig;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Client, Url};
use rustls::{
    ClientConfig as RustlsClientConfig, DigitallySignedStruct, Error as RustlsError,
    SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject},
};
use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer, Serialize};
use std::{error::Error as StdError, fs, path::Path, sync::Arc, time::Duration};
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LndNodeInfo {
    pub identity_pubkey: String,
    pub alias: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedInvoice {
    pub payment_request: String,
    pub payment_hash_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvoiceState {
    Open,
    Accepted,
    Settled,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceDetails {
    pub payment_request: String,
    pub payment_hash_hex: String,
    pub value_msat: u64,
    pub expiry_secs: u64,
    pub state: InvoiceState,
}

#[derive(Debug, Error)]
pub enum LndRestError {
    #[error("invalid LND REST configuration: {0}")]
    Config(String),
    #[error("failed to read LND credential file: {0}")]
    Io(String),
    #[error("failed to build LND client: {0}")]
    Client(String),
    #[error("failed to build LND URL: {0}")]
    Url(String),
    #[error("LND request failed: {0}")]
    Http(String),
    #[error("LND returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("invalid LND response: {0}")]
    Decode(String),
    #[error("invalid hex value: {0}")]
    Hex(String),
    #[error("unsupported invoice state: {0}")]
    UnsupportedState(String),
}

#[derive(Clone)]
pub struct LndRestClient {
    base_url: Url,
    client: Client,
    macaroon_hex: Zeroizing<String>,
}

impl LndRestClient {
    pub fn from_config(config: &LightningLndRestConfig) -> Result<Self, LndRestError> {
        crate::tls::ensure_rustls_crypto_provider();
        let base_url = Url::parse(&config.rest_url)
            .map_err(|error| LndRestError::Config(format!("invalid rest url: {error}")))?;
        match base_url.scheme() {
            "https" => {}
            "http" if matches!(base_url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) => {}
            "http" => {
                return Err(LndRestError::Config(
                    "plain HTTP LND REST URLs are only allowed on loopback addresses".to_string(),
                ));
            }
            scheme => {
                return Err(LndRestError::Config(format!(
                    "unsupported LND REST URL scheme: {scheme}"
                )));
            }
        }
        let mut builder =
            Client::builder().timeout(Duration::from_secs(config.request_timeout_secs));

        if base_url.scheme() == "https" {
            let Some(tls_cert_path) = config.tls_cert_path.as_ref() else {
                return Err(LndRestError::Config(
                    "https LND REST URLs require FROGLET_LIGHTNING_TLS_CERT_PATH".to_string(),
                ));
            };
            // LND commonly serves a self-signed admin certificate, so we pin the expected
            // leaf certificate instead of assuming WebPKI-style CA validation.
            let verifier = PinnedLndCertVerifier::from_pem_path(tls_cert_path)?;
            let tls = RustlsClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
                .with_no_client_auth();
            builder = builder.use_preconfigured_tls(tls);
        }

        let client = builder
            .build()
            .map_err(|error| LndRestError::Client(error.to_string()))?;
        let macaroon_hex = Zeroizing::new(hex::encode(fs::read(&config.macaroon_path).map_err(
            |error| LndRestError::Io(format!("{}: {error}", config.macaroon_path.display())),
        )?));

        Ok(Self {
            base_url,
            client,
            macaroon_hex,
        })
    }

    pub async fn get_info(&self) -> Result<LndNodeInfo, LndRestError> {
        let response: GetInfoResponse = self.get_json("/v1/getinfo").await?;
        Ok(LndNodeInfo {
            identity_pubkey: response.identity_pubkey,
            alias: response.alias,
            version: response.version,
        })
    }

    pub async fn add_invoice(
        &self,
        value_msat: u64,
        expiry_secs: u64,
        memo: &str,
        private: bool,
    ) -> Result<CreatedInvoice, LndRestError> {
        let response: AddInvoiceResponse = self
            .post_json(
                "/v1/invoices",
                &AddInvoiceRequest {
                    memo: memo.to_string(),
                    value_msat: value_msat.to_string(),
                    expiry: expiry_secs.to_string(),
                    private,
                },
            )
            .await?;
        Ok(CreatedInvoice {
            payment_request: response.payment_request,
            payment_hash_hex: base64_to_hex(&response.r_hash)?,
        })
    }

    pub async fn add_hold_invoice(
        &self,
        payment_hash_hex: &str,
        value_msat: u64,
        expiry_secs: u64,
        cltv_expiry: u32,
        memo: &str,
        private: bool,
    ) -> Result<CreatedInvoice, LndRestError> {
        let response: AddHoldInvoiceResponse = self
            .post_json(
                "/v2/invoices/hodl",
                &AddHoldInvoiceRequest {
                    memo: memo.to_string(),
                    hash: hex_to_base64(payment_hash_hex)?,
                    value_msat: value_msat.to_string(),
                    expiry: expiry_secs.to_string(),
                    cltv_expiry: cltv_expiry.to_string(),
                    private,
                },
            )
            .await?;
        Ok(CreatedInvoice {
            payment_request: response.payment_request,
            payment_hash_hex: payment_hash_hex.to_string(),
        })
    }

    pub async fn lookup_invoice(
        &self,
        payment_hash_hex: &str,
    ) -> Result<InvoiceDetails, LndRestError> {
        let path = format!("/v1/invoice/{payment_hash_hex}");
        let response: InvoiceResponse = self.get_json(&path).await?;
        Ok(InvoiceDetails {
            payment_request: response.payment_request.unwrap_or_default(),
            payment_hash_hex: base64_to_hex(&response.r_hash)?,
            value_msat: response.value_msat,
            expiry_secs: response.expiry,
            state: parse_invoice_state(&response.state)?,
        })
    }

    pub async fn settle_invoice(&self, preimage_hex: &str) -> Result<(), LndRestError> {
        let _: EmptyResponse = self
            .post_json(
                "/v2/invoices/settle",
                &SettleInvoiceRequest {
                    preimage: hex_to_base64(preimage_hex)?,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn cancel_invoice(&self, payment_hash_hex: &str) -> Result<(), LndRestError> {
        let _: EmptyResponse = self
            .post_json(
                "/v2/invoices/cancel",
                &CancelInvoiceRequest {
                    payment_hash: hex_to_base64(payment_hash_hex)?,
                },
            )
            .await?;
        Ok(())
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, LndRestError> {
        let url = self.join(path)?;
        let response = self
            .client
            .get(url)
            .header("Grpc-Metadata-macaroon", self.macaroon_hex.as_str())
            .send()
            .await
            .map_err(|error| LndRestError::Http(format_reqwest_error(&error)))?;
        parse_response(response).await
    }

    async fn post_json<S: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        payload: &S,
    ) -> Result<T, LndRestError> {
        let url = self.join(path)?;
        let response = self
            .client
            .post(url)
            .header("Grpc-Metadata-macaroon", self.macaroon_hex.as_str())
            .json(payload)
            .send()
            .await
            .map_err(|error| LndRestError::Http(format_reqwest_error(&error)))?;
        parse_response(response).await
    }

    fn join(&self, path: &str) -> Result<Url, LndRestError> {
        self.base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| LndRestError::Url(error.to_string()))
    }
}

#[derive(Debug)]
struct PinnedLndCertVerifier {
    expected_der: Vec<u8>,
    supported_algorithms: WebPkiSupportedAlgorithms,
}

impl PinnedLndCertVerifier {
    fn from_pem_path(path: &Path) -> Result<Self, LndRestError> {
        let bytes = fs::read(path)
            .map_err(|error| LndRestError::Io(format!("{}: {error}", path.display())))?;
        let cert = CertificateDer::from_pem_slice(&bytes)
            .map_err(|error| LndRestError::Config(format!("invalid pem certificate: {error}")))?;
        Ok(Self {
            expected_der: cert.as_ref().to_vec(),
            supported_algorithms: rustls::crypto::ring::default_provider()
                .signature_verification_algorithms,
        })
    }
}

impl ServerCertVerifier for PinnedLndCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        if end_entity.as_ref() != self.expected_der.as_slice() {
            return Err(RustlsError::General(
                "server certificate does not match pinned LND certificate".to_string(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(message, cert, dss, &self.supported_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(message, cert, dss, &self.supported_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_algorithms.supported_schemes()
    }
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

async fn parse_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, LndRestError> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| LndRestError::Http(error.to_string()))?;
    if !status.is_success() {
        return Err(LndRestError::Status {
            status: status.as_u16(),
            body,
        });
    }
    serde_json::from_str(&body).map_err(|error| LndRestError::Decode(error.to_string()))
}

fn parse_invoice_state(value: &str) -> Result<InvoiceState, LndRestError> {
    match value {
        "OPEN" => Ok(InvoiceState::Open),
        "ACCEPTED" => Ok(InvoiceState::Accepted),
        "SETTLED" => Ok(InvoiceState::Settled),
        "CANCELED" => Ok(InvoiceState::Canceled),
        other => Err(LndRestError::UnsupportedState(other.to_string())),
    }
}

fn hex_to_base64(value: &str) -> Result<String, LndRestError> {
    let bytes = hex::decode(value).map_err(|error| LndRestError::Hex(error.to_string()))?;
    Ok(STANDARD.encode(bytes))
}

fn base64_to_hex(value: &str) -> Result<String, LndRestError> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|error| LndRestError::Decode(error.to_string()))?;
    Ok(hex::encode(bytes))
}

fn deserialize_u64_from_str_or_int<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        String(String),
        Number(u64),
    }

    match Value::deserialize(deserializer)? {
        Value::String(value) => value.parse::<u64>().map_err(de::Error::custom),
        Value::Number(value) => Ok(value),
    }
}

#[derive(Debug, Deserialize)]
struct GetInfoResponse {
    identity_pubkey: String,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Serialize)]
struct AddInvoiceRequest {
    memo: String,
    value_msat: String,
    expiry: String,
    private: bool,
}

#[derive(Debug, Deserialize)]
struct AddInvoiceResponse {
    r_hash: String,
    payment_request: String,
}

#[derive(Debug, Serialize)]
struct AddHoldInvoiceRequest {
    memo: String,
    hash: String,
    value_msat: String,
    expiry: String,
    cltv_expiry: String,
    private: bool,
}

#[derive(Debug, Deserialize)]
struct AddHoldInvoiceResponse {
    payment_request: String,
}

#[derive(Debug, Deserialize)]
struct InvoiceResponse {
    r_hash: String,
    #[serde(default)]
    payment_request: Option<String>,
    #[serde(deserialize_with = "deserialize_u64_from_str_or_int")]
    value_msat: u64,
    #[serde(deserialize_with = "deserialize_u64_from_str_or_int")]
    expiry: u64,
    state: String,
}

#[derive(Debug, Serialize)]
struct SettleInvoiceRequest {
    preimage: String,
}

#[derive(Debug, Serialize)]
struct CancelInvoiceRequest {
    payment_hash: String,
}

#[derive(Debug, Deserialize)]
struct EmptyResponse {}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Path, State},
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use serde_json::{Value, json};
    use std::{
        net::SocketAddr,
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::net::TcpListener;

    static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone, Default)]
    struct TestState {
        requests: Arc<Mutex<Vec<(String, Value)>>>,
    }

    fn temp_file_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("froglet-{name}-{unique}-{counter}"))
    }

    fn test_config(base_url: String) -> LightningLndRestConfig {
        let macaroon_path = temp_file_path("macaroon");
        fs::write(&macaroon_path, [0xCA, 0xFE, 0xBA, 0xBE]).unwrap();
        LightningLndRestConfig {
            rest_url: base_url,
            tls_cert_path: None,
            macaroon_path,
            request_timeout_secs: 5,
        }
    }

    async fn start_test_server() -> (SocketAddr, TestState) {
        let state = TestState::default();
        let router = Router::new()
            .route("/v1/getinfo", get(get_info))
            .route("/v1/invoices", post(add_invoice))
            .route("/v2/invoices/hodl", post(add_hold_invoice))
            .route("/v1/invoice/:payment_hash", get(lookup_invoice))
            .route("/v2/invoices/settle", post(settle_invoice))
            .route("/v2/invoices/cancel", post(cancel_invoice))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (addr, state)
    }

    fn assert_macaroon(headers: &HeaderMap) {
        assert_eq!(
            headers
                .get("Grpc-Metadata-macaroon")
                .unwrap()
                .to_str()
                .unwrap(),
            "cafebabe"
        );
    }

    async fn get_info(headers: HeaderMap) -> (StatusCode, Json<Value>) {
        assert_macaroon(&headers);
        (
            StatusCode::OK,
            Json(json!({
                "identity_pubkey": "02".to_string() + &"11".repeat(32),
                "alias": "froglet-lnd",
                "version": "0.18.0-beta"
            })),
        )
    }

    async fn add_invoice(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(payload): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        assert_macaroon(&headers);
        state
            .requests
            .lock()
            .unwrap()
            .push(("add_invoice".to_string(), payload.clone()));
        (
            StatusCode::OK,
            Json(json!({
                "r_hash": STANDARD.encode([0x01; 32]),
                "payment_request": "lnbc1invoice"
            })),
        )
    }

    async fn add_hold_invoice(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(payload): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        assert_macaroon(&headers);
        state
            .requests
            .lock()
            .unwrap()
            .push(("add_hold_invoice".to_string(), payload.clone()));
        (
            StatusCode::OK,
            Json(json!({
                "payment_request": "lnbc1holdinvoice"
            })),
        )
    }

    async fn lookup_invoice(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(payment_hash): Path<String>,
    ) -> (StatusCode, Json<Value>) {
        assert_macaroon(&headers);
        state.requests.lock().unwrap().push((
            "lookup_invoice".to_string(),
            json!({ "payment_hash": payment_hash }),
        ));
        (
            StatusCode::OK,
            Json(json!({
                "r_hash": STANDARD.encode([0x11; 32]),
                "payment_request": "lnbc1holdinvoice",
                "value_msat": "21000",
                "expiry": "300",
                "state": "ACCEPTED"
            })),
        )
    }

    async fn settle_invoice(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(payload): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        assert_macaroon(&headers);
        state
            .requests
            .lock()
            .unwrap()
            .push(("settle_invoice".to_string(), payload));
        (StatusCode::OK, Json(json!({})))
    }

    async fn cancel_invoice(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(payload): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        assert_macaroon(&headers);
        state
            .requests
            .lock()
            .unwrap()
            .push(("cancel_invoice".to_string(), payload));
        (StatusCode::OK, Json(json!({})))
    }

    #[tokio::test]
    async fn lnd_rest_client_shapes_requests_and_parses_responses() {
        let (addr, state) = start_test_server().await;
        let config = test_config(format!("http://{addr}"));
        let client = LndRestClient::from_config(&config).unwrap();

        let info = client.get_info().await.unwrap();
        assert_eq!(info.alias.as_deref(), Some("froglet-lnd"));
        assert_eq!(info.version.as_deref(), Some("0.18.0-beta"));

        let invoice = client.add_invoice(0, 300, "base fee", true).await.unwrap();
        assert_eq!(invoice.payment_request, "lnbc1invoice");
        assert_eq!(invoice.payment_hash_hex, "01".repeat(32));

        let hold_invoice = client
            .add_hold_invoice(&"11".repeat(32), 21_000, 600, 18, "success fee", true)
            .await
            .unwrap();
        assert_eq!(hold_invoice.payment_request, "lnbc1holdinvoice");
        assert_eq!(hold_invoice.payment_hash_hex, "11".repeat(32));

        let looked_up = client.lookup_invoice(&"11".repeat(32)).await.unwrap();
        assert_eq!(looked_up.state, InvoiceState::Accepted);
        assert_eq!(looked_up.value_msat, 21_000);

        client.settle_invoice(&"22".repeat(32)).await.unwrap();
        client.cancel_invoice(&"33".repeat(32)).await.unwrap();

        let requests = state.requests.lock().unwrap().clone();
        assert_eq!(requests[0].0, "add_invoice");
        assert_eq!(requests[0].1["value_msat"], "0");
        assert_eq!(requests[1].0, "add_hold_invoice");
        assert_eq!(requests[1].1["hash"], STANDARD.encode([0x11; 32]));
        assert_eq!(requests[1].1["cltv_expiry"], "18");
        assert_eq!(requests[2].1["payment_hash"], "11".repeat(32));
        assert_eq!(requests[3].0, "settle_invoice");
        assert_eq!(requests[3].1["preimage"], STANDARD.encode([0x22; 32]));
        assert_eq!(requests[4].0, "cancel_invoice");
        assert_eq!(requests[4].1["payment_hash"], STANDARD.encode([0x33; 32]));

        let _ = fs::remove_file(&config.macaroon_path);
    }

    #[test]
    fn lnd_rest_client_rejects_non_loopback_http() {
        let config = test_config("http://10.0.0.5:8080".to_string());
        let error = match LndRestClient::from_config(&config) {
            Ok(_) => panic!("non-loopback plain HTTP should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("loopback"));
        let _ = fs::remove_file(&config.macaroon_path);
    }
}
