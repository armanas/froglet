use froglet::{
    db::DbPool,
    discovery_server::{self, DiscoveryAppState},
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let listen_addr = std::env::var("FROGLET_DISCOVERY_LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:9090".to_string());
    let db_path = PathBuf::from(
        std::env::var("FROGLET_DISCOVERY_DB_PATH")
            .unwrap_or_else(|_| "./data/discovery.db".to_string()),
    );
    let stale_after_secs = std::env::var("FROGLET_DISCOVERY_STALE_AFTER_SECS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(300);

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let pool = DbPool::open_with(
        &db_path,
        discovery_server::initialize_discovery_db,
        discovery_server::initialize_discovery_db_reader,
    )?;
    let db_metrics_path = db_path.clone();
    let wal_metrics = pool
        .with_write_conn(move |conn| {
            froglet::db::collect_wal_checkpoint_metrics(conn, &db_metrics_path)
        })
        .await?;
    tracing::info!(
        wal_size_bytes = wal_metrics.wal_size_bytes,
        wal_frames = wal_metrics.log_frames,
        wal_checkpointed_frames = wal_metrics.checkpointed_frames,
        wal_busy = wal_metrics.busy,
        wal_checkpoint_duration_ms = wal_metrics.duration_ms as u64,
        "Reference discovery SQLite WAL checkpoint metrics collected"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&db_path)?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&db_path, perms)?;
    }

    let state = DiscoveryAppState {
        db: pool,
        stale_after_secs,
    };

    let app = discovery_server::router(state);
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    let bound_addr = listener.local_addr()?;
    println!(" 🌐 Reference Discovery API: http://{}", bound_addr);
    tracing::info!("Reference discovery listening on http://{}", bound_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

fn init_logging() {
    froglet::init_logging();
}
