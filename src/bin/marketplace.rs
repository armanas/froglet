use froglet::marketplace_server::{self, MarketplaceAppState};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let listen_addr = std::env::var("FROGLET_MARKETPLACE_LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:9090".to_string());
    let db_path = PathBuf::from(
        std::env::var("FROGLET_MARKETPLACE_DB_PATH")
            .unwrap_or_else(|_| "./data/marketplace.db".to_string()),
    );
    let stale_after_secs = std::env::var("FROGLET_MARKETPLACE_STALE_AFTER_SECS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(300);

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = marketplace_server::initialize_marketplace_db(&db_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&db_path)?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&db_path, perms)?;
    }

    let state = MarketplaceAppState {
        db: Arc::new(Mutex::new(conn)),
        stale_after_secs,
    };

    let app = marketplace_server::router(state);
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    tracing::info!("Marketplace listening on http://{}", listen_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}
