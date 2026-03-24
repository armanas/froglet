use reqwest::{Certificate, Client, ClientBuilder, NoProxy, Proxy};
use std::{fs, path::Path, sync::Once, time::Duration};

static INSTALL_RUSTLS_PROVIDER: Once = Once::new();

pub fn ensure_rustls_crypto_provider() {
    INSTALL_RUSTLS_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn env_var(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn apply_env_proxies(builder: ClientBuilder) -> Result<ClientBuilder, String> {
    let no_proxy = NoProxy::from_env();
    let mut builder = builder;
    let mut configured_specific_proxy = false;
    if let Some(proxy_url) = env_var(&["HTTPS_PROXY", "https_proxy"]) {
        let proxy = Proxy::https(&proxy_url)
            .map_err(|error| format!("Failed to parse HTTPS_PROXY {proxy_url}: {error}"))?
            .no_proxy(no_proxy.clone());
        builder = builder.proxy(proxy);
        configured_specific_proxy = true;
    }
    if let Some(proxy_url) = env_var(&["HTTP_PROXY", "http_proxy"]) {
        let proxy = Proxy::http(&proxy_url)
            .map_err(|error| format!("Failed to parse HTTP_PROXY {proxy_url}: {error}"))?
            .no_proxy(no_proxy);
        builder = builder.proxy(proxy);
        configured_specific_proxy = true;
    }

    if !configured_specific_proxy && let Some(proxy_url) = env_var(&["ALL_PROXY", "all_proxy"]) {
        let proxy = Proxy::all(&proxy_url)
            .map_err(|error| format!("Failed to parse ALL_PROXY {proxy_url}: {error}"))?
            .no_proxy(NoProxy::from_env());
        builder = builder.proxy(proxy);
    }

    Ok(builder)
}

pub fn build_reqwest_client(ca_cert_path: Option<&Path>) -> Result<Client, String> {
    ensure_rustls_crypto_provider();
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10));

    if let Some(path) = ca_cert_path {
        let cert_bytes = fs::read(path).map_err(|error| {
            format!(
                "Failed to read HTTP CA certificate {}: {error}",
                path.display()
            )
        })?;
        let cert = Certificate::from_pem(&cert_bytes).map_err(|error| {
            format!(
                "Failed to parse HTTP CA certificate {} as PEM: {error}",
                path.display()
            )
        })?;
        builder = builder.add_root_certificate(cert);
    }

    builder = apply_env_proxies(builder)?;

    builder
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use hyper::{Request, Response, server::conn::http1, service::service_fn};
    use hyper_util::rt::TokioIo;
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, SanType};
    use std::{
        convert::Infallible,
        net::{IpAddr, Ipv4Addr, SocketAddr},
        sync::Mutex,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::{net::TcpListener, task::JoinHandle};
    use tokio_rustls::{TlsAcceptor, rustls};

    static TEST_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);
    static PROXY_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_proxy_env() {
        for name in [
            "ALL_PROXY",
            "all_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            unsafe {
                std::env::remove_var(name);
            }
        }
    }

    fn unique_temp_file(name: &str) -> std::path::PathBuf {
        let counter = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("froglet-{name}-{nanos}-{counter}.pem"))
    }

    async fn spawn_tls_server() -> (SocketAddr, std::path::PathBuf, JoinHandle<()>) {
        ensure_rustls_crypto_provider();
        let mut ca_params = CertificateParams::new(vec!["froglet-test-ca".to_string()]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "froglet test ca");
        let ca_key = KeyPair::generate().unwrap();
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();

        let mut server_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        server_params
            .subject_alt_names
            .push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        server_params
            .distinguished_name
            .push(DnType::CommonName, "127.0.0.1");
        let server_key = KeyPair::generate().unwrap();
        let server_cert = server_params
            .signed_by(&server_key, &ca_cert, &ca_key)
            .unwrap();

        let ca_path = unique_temp_file("http-ca");
        fs::write(&ca_path, ca_cert.pem()).unwrap();

        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![server_cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(server_key.serialize_der().into()),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(std::sync::Arc::new(server_config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let tls_stream = acceptor.accept(stream).await.unwrap();
                    let service =
                        service_fn(|_request: Request<hyper::body::Incoming>| async move {
                            Ok::<_, Infallible>(Response::new(Body::from("ok")))
                        });
                    http1::Builder::new()
                        .serve_connection(TokioIo::new(tls_stream), service)
                        .await
                        .unwrap();
                });
            }
        });

        (addr, ca_path, handle)
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn build_reqwest_client_accepts_custom_ca_certificate() {
        let _guard = PROXY_ENV_LOCK.lock().unwrap();
        clear_proxy_env();
        let (addr, ca_path, handle) = spawn_tls_server().await;
        let client = build_reqwest_client(Some(&ca_path)).unwrap();

        let response = client
            .get(format!("https://{addr}/health"))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());

        handle.abort();
        let _ = fs::remove_file(ca_path);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn build_reqwest_client_uses_default_trust_store_when_no_custom_ca_is_set() {
        let _guard = PROXY_ENV_LOCK.lock().unwrap();
        clear_proxy_env();
        let (addr, ca_path, handle) = spawn_tls_server().await;
        let client = build_reqwest_client(None).unwrap();

        let error = client
            .get(format!("https://{addr}/health"))
            .send()
            .await
            .expect_err("self-signed test CA should not be trusted by default");
        assert!(error.is_builder() || error.is_request() || error.is_connect());

        handle.abort();
        let _ = fs::remove_file(ca_path);
    }

    #[test]
    fn build_reqwest_client_rejects_invalid_ca_certificate_file() {
        let path = unique_temp_file("invalid-http-ca");
        fs::write(&path, "-----BEGIN CERTIFICATE-----\ninvalid").unwrap();

        let error =
            build_reqwest_client(Some(&path)).expect_err("invalid CA file should fail to load");
        assert!(!error.trim().is_empty());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn build_reqwest_client_rejects_invalid_proxy_configuration() {
        let _guard = PROXY_ENV_LOCK.lock().unwrap();
        clear_proxy_env();
        unsafe {
            std::env::set_var("HTTPS_PROXY", "not a valid proxy url");
        }
        let error =
            build_reqwest_client(None).expect_err("invalid HTTPS_PROXY should fail to load");
        assert!(error.contains("HTTPS_PROXY"));
        unsafe {
            std::env::remove_var("HTTPS_PROXY");
        }
    }

    #[test]
    fn build_reqwest_client_prefers_https_proxy_over_all_proxy() {
        let _guard = PROXY_ENV_LOCK.lock().unwrap();
        clear_proxy_env();
        unsafe {
            std::env::set_var("ALL_PROXY", "not a valid proxy url");
            std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:3128");
        }
        let client = build_reqwest_client(None)
            .expect("valid HTTPS_PROXY should take precedence over invalid ALL_PROXY");
        drop(client);
        unsafe {
            std::env::remove_var("ALL_PROXY");
            std::env::remove_var("HTTPS_PROXY");
        }
    }

    #[test]
    fn build_reqwest_client_accepts_socks5h_all_proxy_configuration() {
        let _guard = PROXY_ENV_LOCK.lock().unwrap();
        clear_proxy_env();
        unsafe {
            std::env::set_var("ALL_PROXY", "socks5h://127.0.0.1:9050");
        }
        let client = build_reqwest_client(None)
            .expect("socks5h ALL_PROXY should be accepted when reqwest SOCKS support is enabled");
        drop(client);
        unsafe {
            std::env::remove_var("ALL_PROXY");
        }
    }
}
