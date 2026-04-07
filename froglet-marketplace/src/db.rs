use deadpool_postgres::{Config, Pool, Runtime, ManagerConfig, RecyclingMethod};
use tokio_postgres::NoTls;

pub type PgPool = Pool;

pub async fn connect(database_url: &str) -> Result<PgPool, String> {
    // Parse the URL into deadpool config
    let pg_config: tokio_postgres::Config = database_url
        .parse()
        .map_err(|e| format!("invalid database URL: {e}"))?;

    let mut cfg = Config::new();
    cfg.dbname = pg_config.get_dbname().map(String::from);
    cfg.host = pg_config.get_hosts().first().map(|h| match h {
        tokio_postgres::config::Host::Tcp(s) => s.clone(),
        #[cfg(unix)]
        tokio_postgres::config::Host::Unix(p) => p.to_string_lossy().to_string(),
    });
    cfg.port = pg_config.get_ports().first().copied();
    cfg.user = pg_config.get_user().map(String::from);
    cfg.password = pg_config.get_password().map(|p| String::from_utf8_lossy(p).to_string());
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });

    let pool = cfg
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .map_err(|e| format!("pool creation: {e}"))?;

    // Verify the connection works
    let client = pool
        .get()
        .await
        .map_err(|e| format!("database connection: {e}"))?;

    client
        .simple_query("SELECT 1")
        .await
        .map_err(|e| format!("database ping: {e}"))?;

    Ok(pool)
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), String> {
    let client = pool
        .get()
        .await
        .map_err(|e| format!("migration connection: {e}"))?;

    let sql = include_str!("../migrations/001_init.sql");

    // Use batch_execute to send the entire SQL as a single protocol message,
    // letting Postgres handle statement parsing (safe with semicolons in
    // strings, $$ blocks, comments, etc.).
    match client.batch_execute(sql).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let msg = e.to_string();
            // Tables may already exist on restart — treat as success
            if msg.contains("already exists") {
                Ok(())
            } else {
                Err(format!("migration failed: {msg}"))
            }
        }
    }
}
