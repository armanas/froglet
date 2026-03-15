use froglet::{
    api,
    config::NodeConfig,
    db::DbPool,
    identity::NodeIdentity,
    marketplace_client,
    pricing::PricingTable,
    runtime_auth, sandbox,
    state::{AppState, MarketplaceStatus, TransportStatus},
    tls, tor,
};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use std::{future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::Mutex as TokioMutex;
use tower::Service;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

const SUPERVISOR_RESTART_DELAY_SECS: u64 = 2;

type SupervisedTaskFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type SupervisedTask = Arc<dyn Fn() -> SupervisedTaskFuture + Send + Sync>;

#[derive(Clone, Copy)]
enum SupervisionPolicy {
    Fatal,
    Restart { delay: Duration },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();
    tls::ensure_rustls_crypto_provider();

    println!("\n=========================================");
    println!(" 🐸 Froglet Node is Starting...");
    println!("=========================================\n");

    let node_config = match NodeConfig::from_env() {
        Ok(cfg) => {
            info!("Network mode: {}", cfg.network_mode);
            cfg
        }
        Err(e) => {
            error!("{e}");
            std::process::exit(1);
        }
    };

    let wasm_sandbox =
        Arc::new(sandbox::WasmSandbox::from_env().expect("Failed to initialize Wasmtime sandbox"));
    wasm_sandbox.warm_up();

    ensure_dir(&node_config.storage.data_dir)?;
    ensure_dir(&node_config.storage.runtime_dir)?;
    ensure_dir(&node_config.storage.tor_dir)?;

    let identity = match NodeIdentity::load_or_create(&node_config) {
        Ok(identity) => Arc::new(identity),
        Err(e) => {
            error!("{e}");
            std::process::exit(1);
        }
    };
    info!("Node identity: {}", identity.node_id());

    let runtime_auth = match runtime_auth::load_or_create_local_runtime_auth(&node_config) {
        Ok(runtime_auth) => runtime_auth,
        Err(e) => {
            error!("{e}");
            std::process::exit(1);
        }
    };
    info!(
        "Runtime auth token file: {}",
        node_config.storage.runtime_auth_token_path.display()
    );

    let db_pool =
        DbPool::open(&node_config.storage.db_path).expect("Failed to initialize SQLite DB pool");
    set_mode(&node_config.storage.db_path, 0o600)?;

    let http_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()?;

    let state = Arc::new(AppState {
        db: db_pool,
        transport_status: Arc::new(TokioMutex::new(TransportStatus::from_config(&node_config))),
        marketplace_status: Arc::new(TokioMutex::new(MarketplaceStatus::from_config(
            &node_config,
        ))),
        wasm_sandbox,
        pricing: PricingTable::from_config(node_config.pricing),
        identity,
        config: node_config.clone(),
        http_client,
        runtime_auth_token: runtime_auth.token,
        runtime_auth_token_path: node_config.storage.runtime_auth_token_path.clone(),
    });

    api::recover_runtime_state(state.clone())
        .await
        .expect("Failed to recover pending runtime state");

    let restart_delay = Duration::from_secs(SUPERVISOR_RESTART_DELAY_SECS);

    if node_config.payment_backend == froglet::config::PaymentBackend::Lightning {
        let settlement_state = state.clone();
        spawn_supervised_task(
            "lightning-settlement-loop",
            SupervisionPolicy::Fatal,
            Arc::new(move || {
                let settlement_state = settlement_state.clone();
                Box::pin(async move {
                    api::run_lightning_settlement_loop(settlement_state).await;
                    Err("lightning settlement loop exited unexpectedly".to_string())
                })
            }),
        );
    }

    let public_app = api::public_router(state.clone());
    let runtime_app = api::runtime_router(state.clone());
    let tor_backend_addr = if node_config.network_mode.should_start_tor() {
        let tor_backend_addr: SocketAddr = node_config
            .tor
            .backend_listen_addr
            .parse()
            .expect("Invalid Tor backend listen address format");
        if !tor_backend_addr.ip().is_loopback() {
            error!(
                "FROGLET_TOR_BACKEND_LISTEN_ADDR must bind to a loopback address, got {}",
                node_config.tor.backend_listen_addr
            );
            std::process::exit(1);
        }

        let tor_backend_listener = tokio::net::TcpListener::bind(tor_backend_addr).await?;
        let bound_tor_backend_addr = tor_backend_listener.local_addr()?;
        let initial_tor_backend_listener = Arc::new(TokioMutex::new(Some(tor_backend_listener)));
        let tor_backend_app = public_app.clone();
        let tor_backend_policy = if node_config.network_mode.tor_required() {
            SupervisionPolicy::Fatal
        } else {
            SupervisionPolicy::Restart {
                delay: restart_delay,
            }
        };
        spawn_supervised_task(
            "tor-backend-listener",
            tor_backend_policy,
            Arc::new(move || {
                let initial_tor_backend_listener = initial_tor_backend_listener.clone();
                let tor_backend_app = tor_backend_app.clone();
                Box::pin(async move {
                    let listener = take_or_bind_listener(
                        initial_tor_backend_listener,
                        bound_tor_backend_addr,
                        "Tor backend API",
                    )
                    .await?;
                    info!(
                        "Local Tor backend listener: http://{}",
                        bound_tor_backend_addr
                    );
                    serve_http_listener(listener, tor_backend_app)
                        .await
                        .map_err(|error| format!("error serving Tor backend API over TCP: {error}"))
                })
            }),
        );
        Some(bound_tor_backend_addr)
    } else {
        None
    };

    let runtime_addr: SocketAddr = node_config
        .runtime_listen_addr
        .parse()
        .expect("Invalid runtime listen address format");
    if !runtime_addr.ip().is_loopback() {
        error!(
            "FROGLET_RUNTIME_LISTEN_ADDR must bind to a loopback address, got {}",
            node_config.runtime_listen_addr
        );
        std::process::exit(1);
    }

    let runtime_listener = tokio::net::TcpListener::bind(runtime_addr).await?;
    let bound_runtime_addr = runtime_listener.local_addr()?;
    let initial_runtime_listener = Arc::new(TokioMutex::new(Some(runtime_listener)));
    println!(" 🔒 Local Runtime API: http://{}", bound_runtime_addr);
    spawn_supervised_task(
        "runtime-api-listener",
        SupervisionPolicy::Restart {
            delay: restart_delay,
        },
        Arc::new(move || {
            let initial_runtime_listener = initial_runtime_listener.clone();
            let runtime_app = runtime_app.clone();
            Box::pin(async move {
                let listener = take_or_bind_listener(
                    initial_runtime_listener,
                    bound_runtime_addr,
                    "runtime API",
                )
                .await?;
                serve_http_listener(listener, runtime_app)
                    .await
                    .map_err(|error| format!("error serving runtime API over TCP: {error}"))
            })
        }),
    );

    if let Some(tor_backend_addr) = tor_backend_addr {
        let tor_state = state.clone();
        let tor_required = node_config.network_mode.tor_required();
        let tor_binary = node_config.tor.binary_path.clone();
        let tor_dir = node_config.storage.tor_dir.clone();
        let startup_timeout = Duration::from_secs(node_config.tor.startup_timeout_secs);

        let tor_policy = if tor_required {
            SupervisionPolicy::Fatal
        } else {
            SupervisionPolicy::Restart {
                delay: restart_delay,
            }
        };
        spawn_supervised_task(
            "tor-sidecar",
            tor_policy,
            Arc::new(move || {
                let tor_state = tor_state.clone();
                let tor_binary = tor_binary.clone();
                let tor_dir = tor_dir.clone();
                Box::pin(async move {
                    match tor::start_hidden_service(
                        &tor_binary,
                        tor_dir,
                        tor_backend_addr,
                        startup_timeout,
                    )
                    .await
                    {
                        Ok(service) => {
                            let onion_url = service.onion_url.clone();
                            info!("Tor hidden service started successfully: {}", onion_url);
                            let mut status = tor_state.transport_status.lock().await;
                            status.tor_onion_url = Some(onion_url);
                            status.tor_status = "up".to_string();
                            drop(status);

                            let exit_status = service.wait().await?;
                            {
                                let mut status = tor_state.transport_status.lock().await;
                                status.tor_onion_url = None;
                                status.tor_status = "down".to_string();
                            }
                            Err(format!("Tor sidecar exited with status {exit_status}"))
                        }
                        Err(error) => {
                            let mut status = tor_state.transport_status.lock().await;
                            status.tor_onion_url = None;
                            status.tor_status = "down".to_string();
                            Err(format!("failed to start Tor hidden service: {error}"))
                        }
                    }
                })
            }),
        );
    }

    let marketplace_publish_enabled = node_config
        .marketplace
        .as_ref()
        .map(|marketplace| marketplace.publish)
        .unwrap_or(false);
    let marketplace_required = node_config
        .marketplace
        .as_ref()
        .map(|marketplace| marketplace.required)
        .unwrap_or(false);
    if marketplace_publish_enabled {
        if marketplace_required {
            let initial_marketplace_hash =
                match marketplace_client::perform_initial_sync(state.clone()).await {
                    Ok(hash) => hash,
                    Err(e) => {
                        warn!("Initial marketplace sync failed: {e}");
                        {
                            let mut status = state.marketplace_status.lock().await;
                            status.last_error = Some(e.clone());
                        }
                        error!("Marketplace is required but initial registration failed. Exiting.");
                        std::process::exit(1);
                    }
                };
            let sync_state = state.clone();
            let initial_marketplace_hash =
                Arc::new(TokioMutex::new(Some(initial_marketplace_hash)));
            spawn_supervised_task(
                "marketplace-sync-loop",
                SupervisionPolicy::Restart {
                    delay: restart_delay,
                },
                Arc::new(move || {
                    let sync_state = sync_state.clone();
                    let initial_marketplace_hash = initial_marketplace_hash.clone();
                    Box::pin(async move {
                        let last_descriptor_hash = take_initial_hash_or_resync(
                            sync_state.clone(),
                            initial_marketplace_hash,
                        )
                        .await?;
                        marketplace_client::run_sync_loop(sync_state, last_descriptor_hash).await;
                        Err("marketplace sync loop exited unexpectedly".to_string())
                    })
                }),
            );
        } else {
            let sync_state = state.clone();
            spawn_supervised_task(
                "marketplace-sync-loop",
                SupervisionPolicy::Restart {
                    delay: restart_delay,
                },
                Arc::new(move || {
                    let sync_state = sync_state.clone();
                    Box::pin(async move {
                        let last_descriptor_hash = match marketplace_client::perform_initial_sync(
                            sync_state.clone(),
                        )
                        .await
                        {
                            Ok(hash) => hash,
                            Err(error) => {
                                warn!("Initial marketplace sync failed: {error}");
                                let mut status = sync_state.marketplace_status.lock().await;
                                status.last_error = Some(error);
                                String::new()
                            }
                        };
                        marketplace_client::run_sync_loop(sync_state, last_descriptor_hash).await;
                        Err("marketplace sync loop exited unexpectedly".to_string())
                    })
                }),
            );
        }
    }

    if node_config.network_mode.should_start_clearnet() {
        let addr: SocketAddr = node_config
            .listen_addr
            .parse()
            .expect("Invalid listen address format");

        println!(" 🌐 Local API Gateway: http://{}", addr);
        if !node_config.network_mode.should_start_tor() {
            println!("\n=========================================");
            println!(" ✅ Node is now online and accepting traffic.");
            println!("=========================================\n");
        }

        let listener = tokio::net::TcpListener::bind(addr).await?;
        serve_http_listener(listener, public_app).await?;
    } else {
        info!("Running in Tor-only mode. No clearnet server started.");
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        }
    }

    Ok(())
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let subscriber = FmtSubscriber::builder()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}

fn ensure_dir(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(path)?;
    set_mode(path, 0o700)?;
    Ok(())
}

fn set_mode(path: &std::path::Path, mode: u32) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(path)?;
        let mut perms = metadata.permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn spawn_supervised_task(name: &'static str, policy: SupervisionPolicy, task: SupervisedTask) {
    tokio::spawn(async move {
        loop {
            info!("Starting background task: {name}");
            let result = (task)().await;
            match result {
                Ok(()) => warn!("Background task {name} exited cleanly"),
                Err(error) => error!("Background task {name} failed: {error}"),
            }

            match policy {
                SupervisionPolicy::Fatal => {
                    error!("Fatal background task {name} exited. Terminating node.");
                    std::process::exit(1);
                }
                SupervisionPolicy::Restart { delay } => {
                    warn!(
                        "Restarting background task {name} after {}s",
                        delay.as_secs()
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    });
}

async fn take_or_bind_listener(
    initial_listener: Arc<TokioMutex<Option<tokio::net::TcpListener>>>,
    addr: SocketAddr,
    label: &'static str,
) -> Result<tokio::net::TcpListener, String> {
    let existing_listener = {
        let mut slot = initial_listener.lock().await;
        slot.take()
    };

    match existing_listener {
        Some(listener) => Ok(listener),
        None => tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|error| format!("failed to bind {label} listener on {addr}: {error}")),
    }
}

async fn take_initial_hash_or_resync(
    state: Arc<AppState>,
    initial_hash: Arc<TokioMutex<Option<String>>>,
) -> Result<String, String> {
    let pending_hash = {
        let mut slot = initial_hash.lock().await;
        slot.take()
    };

    match pending_hash {
        Some(hash) => Ok(hash),
        None => marketplace_client::perform_initial_sync(state).await,
    }
}

async fn serve_http_listener(
    listener: tokio::net::TcpListener,
    app: axum::Router,
) -> std::io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let app_clone = app.clone();

        tokio::spawn(async move {
            if let Err(error) = serve_http_stream(io, app_clone).await {
                error!("Error serving Axum over TCP stream: {error}");
            }
        });
    }
}

async fn serve_http_stream(
    io: TokioIo<tokio::net::TcpStream>,
    app: axum::Router,
) -> Result<(), hyper::Error> {
    let mut builder = http1::Builder::new();
    builder.half_close(true);

    let hyper_service =
        hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
            app.clone().call(req)
        });

    builder.serve_connection(io, hyper_service).await
}
