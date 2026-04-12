use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_postgres::error::SqlState;

pub type PgPool = Pool;

pub async fn connect(database_url: &str) -> Result<PgPool, String> {
    // Parse the URL into deadpool config
    let pg_config: tokio_postgres::Config = database_url
        .parse()
        .map_err(|e| format!("invalid database URL: {e}"))?;

    let host_str = pg_config.get_hosts().first().map(|h| match h {
        tokio_postgres::config::Host::Tcp(s) => s.clone(),
        #[cfg(unix)]
        tokio_postgres::config::Host::Unix(p) => p.to_string_lossy().to_string(),
    });

    let mut cfg = Config::new();
    cfg.dbname = pg_config.get_dbname().map(String::from);
    cfg.host = host_str.clone();
    cfg.port = pg_config.get_ports().first().copied();
    cfg.user = pg_config.get_user().map(String::from);
    cfg.password = pg_config
        .get_password()
        .map(|p| String::from_utf8_lossy(p).to_string());
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });

    let use_tls = should_use_tls(host_str.as_deref());
    let pool = if use_tls {
        let tls_config = build_rustls_config()
            .map_err(|e| format!("TLS configuration error: {e}"))?;
        let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
        cfg.create_pool(Some(Runtime::Tokio1), tls)
            .map_err(|e| format!("pool creation (TLS): {e}"))?
    } else {
        cfg.create_pool(Some(Runtime::Tokio1), tokio_postgres::NoTls)
            .map_err(|e| format!("pool creation: {e}"))?
    };

    if use_tls {
        tracing::info!("PostgreSQL connection pool created with TLS");
    } else {
        tracing::info!("PostgreSQL connection pool created without TLS (loopback)");
    }

    wait_for_pool_connectivity(&pool).await?;
    Ok(pool)
}

/// Use TLS for any non-loopback host. Can be overridden with FROGLET_MARKETPLACE_DB_TLS.
fn should_use_tls(host: Option<&str>) -> bool {
    if let Ok(val) = std::env::var("FROGLET_MARKETPLACE_DB_TLS") {
        let disabled = matches!(val.as_str(), "0" | "false" | "off");
        if disabled {
            let is_loopback = host
                .is_some_and(|h| matches!(h, "127.0.0.1" | "localhost" | "::1"));
            if !is_loopback {
                tracing::warn!(
                    "FROGLET_MARKETPLACE_DB_TLS is disabled for non-loopback host {:?} \
                     — database traffic will be unencrypted",
                    host.unwrap_or("(unknown)")
                );
            }
        }
        return !disabled;
    }
    let Some(host) = host else { return false };
    !matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn build_rustls_config() -> Result<rustls::ClientConfig, rustls::Error> {
    froglet::tls::ensure_rustls_crypto_provider();
    let root_store = rustls::RootCertStore::from_iter(
        webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
    );
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(Arc::new(root_store))
        .with_no_client_auth())
}

async fn wait_for_pool_connectivity(pool: &PgPool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);

    loop {
        let last_error = match pool.get().await {
            Ok(client) => match client.simple_query("SELECT 1").await {
                Ok(_) => return Ok(()),
                Err(error) => format!("database ping: {error}"),
            },
            Err(error) => format!("database connection: {error}"),
        };

        if Instant::now() >= deadline {
            return Err(last_error);
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), String> {
    let client = pool
        .get()
        .await
        .map_err(|e| format!("migration connection: {e}"))?;

    let migrations = [
        include_str!("../migrations/001_init.sql"),
        include_str!("../migrations/002_stakes.sql"),
    ];

    for sql in migrations {
        for statement in split_migration_statements(sql) {
            match client.batch_execute(&statement).await {
                Ok(()) => {}
                Err(error) => {
                    let msg = error.to_string();
                    if is_duplicate_migration_error(&error) || msg.contains("already exists") {
                        continue;
                    }
                    return Err(format!("migration failed: {msg}"));
                }
            }
        }
    }

    Ok(())
}

fn is_duplicate_migration_error(error: &tokio_postgres::Error) -> bool {
    error.as_db_error().is_some_and(|db_error| {
        db_error.code() == &SqlState::DUPLICATE_TABLE
            || db_error.code() == &SqlState::DUPLICATE_OBJECT
    })
}

fn split_migration_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double_quote => {
                current.push(ch);
                if in_single_quote {
                    if chars.peek() == Some(&'\'') {
                        current.push(chars.next().expect("peeked escaped quote"));
                    } else {
                        in_single_quote = false;
                    }
                } else {
                    in_single_quote = true;
                }
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                current.push(ch);
            }
            ';' if !in_single_quote && !in_double_quote => {
                push_migration_statement(&mut statements, &mut current, true);
            }
            _ => current.push(ch),
        }
    }

    push_migration_statement(&mut statements, &mut current, false);
    statements
}

fn push_migration_statement(statements: &mut Vec<String>, current: &mut String, terminated: bool) {
    let trimmed = current.trim();
    if statement_has_sql_code(trimmed) {
        let statement = if terminated {
            format!("{trimmed};")
        } else {
            trimmed.to_string()
        };
        statements.push(statement);
    }
    current.clear();
}

fn statement_has_sql_code(statement: &str) -> bool {
    statement.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with("--")
    })
}

#[cfg(test)]
mod tests {
    use super::split_migration_statements;

    #[test]
    fn split_migration_statements_preserves_multiple_statements() {
        let sql = r#"
            -- create provider projection table
            CREATE TABLE marketplace_providers (
                provider_id TEXT PRIMARY KEY,
                source_url TEXT NOT NULL
            );

            CREATE INDEX idx_providers_source ON marketplace_providers (source_url);
        "#;

        let statements = split_migration_statements(sql);

        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("CREATE TABLE marketplace_providers"));
        assert!(statements[1].contains("CREATE INDEX idx_providers_source"));
    }

    #[test]
    fn split_migration_statements_keeps_semicolons_inside_strings() {
        let sql = r#"
            INSERT INTO notes (content) VALUES ('one;two');
            CREATE TABLE marketplace_receipts (receipt_hash TEXT PRIMARY KEY);
        "#;

        let statements = split_migration_statements(sql);

        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("'one;two'"));
        assert!(statements[1].contains("CREATE TABLE marketplace_receipts"));
    }
}
