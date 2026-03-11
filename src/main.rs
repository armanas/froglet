use froglet::{
    api,
    config::NodeConfig,
    db::{self, DbPool},
    identity::NodeIdentity,
    marketplace_client,
    pricing::PricingTable,
    runtime_auth, sandbox,
    state::{AppState, MarketplaceStatus, TransportStatus},
    tor,
};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Mutex as TokioMutex;
use tower::Service;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

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

    sandbox::initialize_engine();

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

    let conn =
        db::initialize_db(&node_config.storage.db_path).expect("Failed to initialize SQLite DB");
    set_mode(&node_config.storage.db_path, 0o600)?;

    let state = Arc::new(AppState {
        db: DbPool::new(conn),
        transport_status: Arc::new(TokioMutex::new(TransportStatus::from_config(&node_config))),
        marketplace_status: Arc::new(TokioMutex::new(MarketplaceStatus::from_config(
            &node_config,
        ))),
        pricing: PricingTable::from_config(node_config.pricing),
        identity,
        config: node_config.clone(),
        http_client: reqwest::Client::new(),
        runtime_auth_token: runtime_auth.token,
        runtime_auth_token_path: node_config.storage.runtime_auth_token_path.clone(),
    });

    api::recover_runtime_state(state.clone())
        .await
        .expect("Failed to recover pending runtime state");

    let app = api::router(state.clone());

    if node_config.network_mode.should_start_tor() {
        let tor_app = app.clone();
        let tor_state = state.clone();
        let tor_required = node_config.network_mode.tor_required();
        let tor_dir = node_config.storage.tor_dir.clone();

        tokio::spawn(async move {
            match tor::start_hidden_service(tor_app, tor_dir).await {
                Ok(onion_url) => {
                    info!("Tor hidden service started successfully: {}", onion_url);
                    let mut status = tor_state.transport_status.lock().await;
                    status.tor_onion_url = Some(onion_url);
                    status.tor_status = "up".to_string();
                }
                Err(e) => {
                    error!("Failed to start Tor hidden service: {}", e);
                    let mut status = tor_state.transport_status.lock().await;
                    status.tor_status = "down".to_string();

                    if tor_required {
                        error!("CRITICAL: Tor is required but failed to start. Exiting.");
                        std::process::exit(1);
                    }
                }
            }
        });
    }

    if node_config
        .marketplace
        .as_ref()
        .map(|marketplace| marketplace.publish)
        .unwrap_or(false)
    {
        match marketplace_client::perform_initial_sync(state.clone()).await {
            Ok(hash) => {
                tokio::spawn(marketplace_client::run_sync_loop(state.clone(), hash));
            }
            Err(e) => {
                warn!("Initial marketplace sync failed: {e}");
                {
                    let mut status = state.marketplace_status.lock().await;
                    status.last_error = Some(e.clone());
                }
                if node_config
                    .marketplace
                    .as_ref()
                    .map(|marketplace| marketplace.required)
                    .unwrap_or(false)
                {
                    error!("Marketplace is required but initial registration failed. Exiting.");
                    std::process::exit(1);
                }
                tokio::spawn(marketplace_client::run_sync_loop(
                    state.clone(),
                    String::new(),
                ));
            }
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
        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let app_clone = app.clone();

            tokio::spawn(async move {
                let mut builder = http1::Builder::new();
                builder.half_close(true);

                let hyper_service = hyper::service::service_fn(
                    move |req: hyper::Request<hyper::body::Incoming>| app_clone.clone().call(req),
                );

                if let Err(e) = builder.serve_connection(io, hyper_service).await {
                    error!("Error serving Axum over TCP: {}", e);
                }
            });
        }
    } else {
        info!("Running in Tor-only mode. No clearnet server started.");
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        }
    }
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "info,arti_client=warn,tor_dirmgr=warn,tor_circmgr=warn,tor_hsservice=warn,tor_proto=warn,tor_chanmgr=warn,tor_guardmgr=warn,arti_core=warn",
        )
    });

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
