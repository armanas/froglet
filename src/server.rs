use crate::{
    api,
    config::{NodeConfig, PaymentBackend},
    db, settlement,
    state::{self, AppState},
    tls, tor,
};
use futures::FutureExt;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use rand::Rng;
use std::{
    future::Future,
    net::SocketAddr,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Mutex as TokioMutex, Semaphore};
use tower::Service;
use tracing::{error, info, warn};

const SUPERVISOR_RESTART_MIN_DELAY_SECS: u64 = 1;
const SUPERVISOR_RESTART_MAX_DELAY_SECS: u64 = 30;
const HTTP_MAX_CONNECTIONS: usize = 256;
const HTTP_HEADER_READ_TIMEOUT_SECS: u64 = 10;
/// Maximum time a single read or write operation may remain pending before the
/// connection is closed.  Despite the name "idle timeout", the deadline resets
/// on every completed I/O operation, so it measures the longest stall on a
/// single syscall rather than true connection-level inactivity.
const HTTP_IO_STALL_TIMEOUT_SECS: u64 = 120;
const HTTP_ACCEPT_BACKOFF_MIN_MS: u64 = 50;
const HTTP_ACCEPT_BACKOFF_MAX_MS: u64 = 5_000;
#[cfg(unix)]
const ENFILE_ERRNO: i32 = 23;
#[cfg(unix)]
const EMFILE_ERRNO: i32 = 24;
#[cfg(windows)]
const WSAEMFILE_ERRNO: i32 = 10024;

type SupervisedTaskFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type SupervisedTask = Arc<dyn Fn() -> SupervisedTaskFuture + Send + Sync>;

#[derive(Clone, Copy)]
enum SupervisionPolicy {
    Fatal,
    Restart {
        min_delay: Duration,
        max_delay: Duration,
    },
}

#[derive(Clone, Copy)]
struct HttpServeConfig {
    max_connections: usize,
    header_read_timeout: Duration,
    idle_timeout: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ServiceRole {
    Provider,
    Runtime,
    /// Both provider and runtime listeners on a single node.
    Dual,
}

impl ServiceRole {
    fn is_provider(self) -> bool {
        matches!(self, Self::Provider | Self::Dual)
    }
    fn is_runtime(self) -> bool {
        matches!(self, Self::Runtime | Self::Dual)
    }
}

/// Start a Froglet node with the given role.
pub async fn run_with_role(role: ServiceRole) -> Result<(), Box<dyn std::error::Error>> {
    run(role, None).await
}

/// Start a Froglet provider node with a pre-built `AppState`.
///
/// Used by external marketplace or other higher-layer service implementations
/// to inject custom builtin service handlers before starting the server.
pub async fn run_with_state(state: Arc<AppState>) -> Result<(), Box<dyn std::error::Error>> {
    run(ServiceRole::Provider, Some(state)).await
}

async fn run(
    service_role: ServiceRole,
    prebuilt_state: Option<Arc<AppState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    init_logging();
    tls::ensure_rustls_crypto_provider();

    let (node_config, state) = if let Some(state) = prebuilt_state {
        let config = state.config.clone();
        (config, state)
    } else {
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
        let state = match state::build_app_state(node_config.clone()) {
            Ok(state) => state,
            Err(error) => {
                error!("{error}");
                std::process::exit(1);
            }
        };
        // Initialize the write-coalescing event batch writer now that we have a
        // tokio runtime.  `Arc::get_mut` is safe here because no clones exist yet.
        let mut state = state;
        let db_clone = state.db.clone();
        Arc::get_mut(&mut state)
            .ok_or("unexpected Arc clone before event batch writer init")?
            .event_batch_writer = Some(db::EventBatchWriter::spawn(db_clone));

        // Demo services (opt-in via FROGLET_PUBLISH_DEMO_SERVICES=1). These
        // are published by the hosted reference node at `ai.froglet.dev` but
        // NOT by a normal self-host install — so a self-host doesn't
        // fingerprint as a public demo node.
        if crate::builtins::demo_enabled() {
            Arc::get_mut(&mut state)
                .ok_or("unexpected Arc clone before demo builtin registration")?
                .builtin_services
                .extend(crate::builtins::demo_handlers());
            crate::builtins::register_demo_offers(state.as_ref()).await?;
            info!("Published demo services: demo.echo, demo.add");
        }

        (node_config, state)
    };
    info!("Node identity: {}", state.identity.node_id());
    info!(
        "Runtime auth token file: {}",
        state.runtime_auth_token_path.display()
    );
    info!(
        "Froglet control auth token file: {}",
        state.provider_control_auth_token_path.display()
    );
    set_mode(&node_config.storage.db_path, 0o600)?;

    sanitize_persisted_deals(state.clone()).await?;
    log_startup_db_metrics(state.clone(), &node_config.storage.db_path).await?;
    audit_duplicate_deal_hashes(state.clone()).await?;

    let restart_policy = SupervisionPolicy::Restart {
        min_delay: Duration::from_secs(SUPERVISOR_RESTART_MIN_DELAY_SECS),
        max_delay: Duration::from_secs(SUPERVISOR_RESTART_MAX_DELAY_SECS),
    };
    let http_serve_config = HttpServeConfig {
        max_connections: HTTP_MAX_CONNECTIONS,
        header_read_timeout: Duration::from_secs(HTTP_HEADER_READ_TIMEOUT_SECS),
        idle_timeout: Duration::from_secs(HTTP_IO_STALL_TIMEOUT_SECS),
    };

    let public_app = api::public_router(state.clone());
    let runtime_app = api::runtime_router(state.clone());
    let tor_backend_addr = if service_role.is_provider()
        && node_config.network_mode.should_start_tor()
    {
        let tor_backend_addr: SocketAddr = node_config
            .tor
            .backend_listen_addr
            .parse()
            .map_err(|e| format!("invalid FROGLET_TOR_BACKEND_LISTEN_ADDR: {e}"))?;
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
            restart_policy
        };
        spawn_supervised_task(
            "tor-backend-listener",
            tor_backend_policy,
            Arc::new(move || {
                let initial_tor_backend_listener = initial_tor_backend_listener.clone();
                let tor_backend_app = tor_backend_app.clone();
                let serve_config = http_serve_config;
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
                    serve_http_listener(listener, tor_backend_app, serve_config)
                        .await
                        .map_err(|error| format!("error serving Tor backend API over TCP: {error}"))
                })
            }),
        );
        Some(bound_tor_backend_addr)
    } else {
        None
    };

    if service_role.is_runtime() {
        let runtime_addr: SocketAddr = node_config
            .runtime_listen_addr
            .parse()
            .map_err(|e| format!("invalid FROGLET_RUNTIME_LISTEN_ADDR: {e}"))?;
        if !runtime_addr.ip().is_loopback() && !node_config.runtime_allow_non_loopback {
            error!(
                "FROGLET_RUNTIME_LISTEN_ADDR must bind to a loopback address, got {}",
                node_config.runtime_listen_addr
            );
            std::process::exit(1);
        }
        if !runtime_addr.ip().is_loopback() {
            warn!(
                "Runtime API is binding a non-loopback address ({}) because FROGLET_RUNTIME_ALLOW_NON_LOOPBACK=true; restrict network access to trusted local callers",
                node_config.runtime_listen_addr
            );
        }

        let runtime_listener = tokio::net::TcpListener::bind(runtime_addr).await?;
        let bound_runtime_addr = runtime_listener.local_addr()?;
        let initial_runtime_listener = Arc::new(TokioMutex::new(Some(runtime_listener)));
        if runtime_addr.ip().is_loopback() {
            println!(" 🔒 Local Runtime API: http://{}", bound_runtime_addr);
        } else {
            println!(" 🔒 Runtime API: http://{}", bound_runtime_addr);
        }
        spawn_supervised_task(
            "runtime-api-listener",
            restart_policy,
            Arc::new(move || {
                let initial_runtime_listener = initial_runtime_listener.clone();
                let runtime_app = runtime_app.clone();
                let serve_config = http_serve_config;
                Box::pin(async move {
                    let listener = take_or_bind_listener(
                        initial_runtime_listener,
                        bound_runtime_addr,
                        "runtime API",
                    )
                    .await?;
                    serve_http_listener(listener, runtime_app, serve_config)
                        .await
                        .map_err(|error| format!("error serving runtime API over TCP: {error}"))
                })
            }),
        );
    }

    if service_role.is_provider() && node_config.network_mode.should_start_clearnet() {
        let addr: SocketAddr = node_config
            .listen_addr
            .parse()
            .map_err(|e| format!("invalid FROGLET_LISTEN_ADDR: {e}"))?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound_addr = listener.local_addr()?;
        {
            let mut transport_status = state.transport_status.lock().await;
            if let Err(error) =
                transport_status.update_clearnet_bound_addr(&node_config, bound_addr)
            {
                error!("{error}");
                std::process::exit(1);
            }
        }
        let initial_public_listener = Arc::new(TokioMutex::new(Some(listener)));
        let public_listener_app = public_app.clone();
        println!(" 🌐 Local API Gateway: http://{}", bound_addr);
        if !node_config.network_mode.should_start_tor() {
            println!("\n=========================================");
            println!(" ✅ Node is now online and accepting traffic.");
            println!("=========================================\n");
        }
        spawn_supervised_task(
            "public-api-listener",
            SupervisionPolicy::Fatal,
            Arc::new(move || {
                let initial_public_listener = initial_public_listener.clone();
                let public_listener_app = public_listener_app.clone();
                let serve_config = http_serve_config;
                Box::pin(async move {
                    let listener =
                        take_or_bind_listener(initial_public_listener, bound_addr, "public API")
                            .await?;
                    serve_http_listener(listener, public_listener_app, serve_config)
                        .await
                        .map_err(|error| format!("error serving public API over TCP: {error}"))
                })
            }),
        );
    } else if service_role.is_provider() {
        info!("Running in Tor-only mode. No clearnet server started.");
    }

    if service_role.is_provider() {
        api::recover_runtime_state_local(state.clone()).await?;
    }

    // Register with marketplace if configured (non-blocking — failures are logged, not fatal)
    if service_role.is_provider() && node_config.marketplace_url.is_some() {
        let reg_state = state.clone();
        tokio::spawn(async move {
            // Small delay to let transport endpoints bind first
            tokio::time::sleep(Duration::from_secs(2)).await;
            match api::register_with_marketplace(reg_state).await {
                Ok(()) => {}
                Err(error) => {
                    warn!("Marketplace registration failed (will retry on next start): {error}")
                }
            }
        });
    }

    if service_role.is_provider()
        && node_config
            .payment_backends
            .contains(&PaymentBackend::Lightning)
    {
        let recovery_state = state.clone();
        spawn_supervised_task(
            "lightning-remote-recovery",
            restart_policy,
            Arc::new(move || {
                let recovery_state = recovery_state.clone();
                Box::pin(async move {
                    api::recover_runtime_state_remote(recovery_state).await?;
                    futures::future::pending::<()>().await;
                    #[allow(unreachable_code)]
                    Ok(())
                })
            }),
        );

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

    if service_role.is_provider()
        && let Some(tor_backend_addr) = tor_backend_addr
    {
        let tor_state = state.clone();
        let tor_required = node_config.network_mode.tor_required();
        let tor_binary = node_config.tor.binary_path.clone();
        let tor_dir = node_config.storage.tor_dir.clone();
        let startup_timeout = Duration::from_secs(node_config.tor.startup_timeout_secs);

        let tor_policy = if tor_required {
            SupervisionPolicy::Fatal
        } else {
            restart_policy
        };
        spawn_supervised_task(
            "tor-sidecar",
            tor_policy,
            Arc::new(move || {
                let tor_state = tor_state.clone();
                let tor_binary = tor_binary.clone();
                let tor_dir = tor_dir.clone();
                Box::pin(async move {
                    let started_at = Instant::now();
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
                            info!(
                                startup_duration_ms = started_at.elapsed().as_millis() as u64,
                                "Tor hidden service started successfully: {}", onion_url
                            );
                            let mut status = tor_state.transport_status.lock().await;
                            status.tor_onion_url = Some(onion_url);
                            status.tor_status = "up".to_string();
                            drop(status);

                            let exit_status = service.wait().await?;
                            warn!("Tor sidecar exited with status {exit_status}");
                            {
                                let mut status = tor_state.transport_status.lock().await;
                                status.tor_onion_url = None;
                                status.tor_status = "down".to_string();
                            }
                            Err(format!("Tor sidecar exited with status {exit_status}"))
                        }
                        Err(error) => {
                            error!(
                                startup_duration_ms = started_at.elapsed().as_millis() as u64,
                                "Tor sidecar startup failed: {error}"
                            );
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

    std::future::pending::<()>().await;
    #[allow(unreachable_code)]
    Ok(())
}

fn init_logging() {
    crate::init_logging();
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

async fn log_startup_db_metrics(
    state: Arc<AppState>,
    db_path: &std::path::Path,
) -> Result<(), String> {
    let db_path = db_path.to_path_buf();
    let metrics = state
        .db
        .with_write_conn(move |conn| crate::db::collect_wal_checkpoint_metrics(conn, &db_path))
        .await?;
    info!(
        wal_size_bytes = metrics.wal_size_bytes,
        wal_frames = metrics.log_frames,
        wal_checkpointed_frames = metrics.checkpointed_frames,
        wal_busy = metrics.busy,
        wal_checkpoint_duration_ms = metrics.duration_ms as u64,
        "SQLite WAL checkpoint metrics collected"
    );
    Ok(())
}

async fn sanitize_persisted_deals(state: Arc<AppState>) -> Result<(), String> {
    let quarantined = state
        .db
        .with_write_conn(move |conn| {
            crate::deals::quarantine_invalid_deals(conn, settlement::current_unix_timestamp())
        })
        .await?;
    if quarantined.is_empty() {
        info!("No unreadable persisted deals detected at startup");
        return Ok(());
    }

    warn!(
        quarantined_deals = quarantined.len(),
        "Unreadable persisted deals were quarantined at startup"
    );
    for deal in quarantined {
        warn!(
            source_rowid = deal.source_rowid,
            deal_id = deal.deal_id.as_deref().unwrap_or("unknown"),
            status = deal.status.as_deref().unwrap_or("unknown"),
            reason = %deal.reason,
            "quarantined unreadable persisted deal"
        );
    }
    Ok(())
}

async fn audit_duplicate_deal_hashes(state: Arc<AppState>) -> Result<(), String> {
    let duplicates = state
        .db
        .with_read_conn(move |conn| crate::db::list_duplicate_deal_artifact_hashes(conn, 10))
        .await?;
    if duplicates.is_empty() {
        info!("No duplicate deal artifact hashes detected at startup");
        return Ok(());
    }

    warn!(
        duplicate_hashes = duplicates.len(),
        "Duplicate canonical deal hashes detected at startup"
    );
    for (deal_artifact_hash, duplicate_count) in duplicates {
        warn!(
            %deal_artifact_hash,
            duplicate_count,
            "duplicate deal artifact hash audit hit"
        );
    }
    Ok(())
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn compute_restart_delay(failure_count: u32, min_delay: Duration, max_delay: Duration) -> Duration {
    let exponent = failure_count.saturating_sub(1).min(10);
    let multiplier = 1u32 << exponent;
    let base = min_delay
        .checked_mul(multiplier)
        .unwrap_or(max_delay)
        .min(max_delay);
    let jitter_ms_cap = (base.as_millis() / 4).max(1) as u64;
    let jitter_ms = rand::thread_rng().gen_range(0..=jitter_ms_cap);
    base.saturating_add(Duration::from_millis(jitter_ms))
        .min(max_delay)
}

fn spawn_supervised_task(name: &'static str, policy: SupervisionPolicy, task: SupervisedTask) {
    tokio::spawn(async move {
        let mut consecutive_failures = 0u32;
        loop {
            info!("Starting background task: {name}");
            let started_at = Instant::now();
            let result = match AssertUnwindSafe((task)()).catch_unwind().await {
                Ok(result) => result,
                Err(payload) => Err(format!(
                    "background task panicked: {}",
                    panic_payload_to_string(payload)
                )),
            };
            match result {
                Ok(()) => warn!("Background task {name} exited cleanly"),
                Err(error) => error!("Background task {name} failed: {error}"),
            }

            match policy {
                SupervisionPolicy::Fatal => {
                    error!("Fatal background task {name} exited. Terminating node.");
                    std::process::exit(1);
                }
                SupervisionPolicy::Restart {
                    min_delay,
                    max_delay,
                } => {
                    if started_at.elapsed() >= Duration::from_secs(60) {
                        consecutive_failures = 0;
                    }
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    let delay = compute_restart_delay(consecutive_failures, min_delay, max_delay);
                    warn!(
                        "Restarting background task {name} after {:.3}s",
                        delay.as_secs_f64()
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

fn accept_error_is_transient(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::OutOfMemory
            | std::io::ErrorKind::AddrInUse
    ) {
        return true;
    }

    match error.raw_os_error() {
        #[cfg(unix)]
        Some(code) if matches!(code, ENFILE_ERRNO | EMFILE_ERRNO) => true,
        #[cfg(windows)]
        Some(code) if code == WSAEMFILE_ERRNO => true,
        _ => false,
    }
}

struct IdleTimeoutStream {
    stream: tokio::net::TcpStream,
    idle_timeout: Duration,
    read_deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    write_deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl IdleTimeoutStream {
    fn new(stream: tokio::net::TcpStream, idle_timeout: Duration) -> Self {
        Self {
            stream,
            idle_timeout,
            read_deadline: None,
            write_deadline: None,
        }
    }

    fn poll_deadline(
        deadline: &mut Option<Pin<Box<tokio::time::Sleep>>>,
        idle_timeout: Duration,
        cx: &mut Context<'_>,
        operation: &str,
    ) -> Poll<std::io::Result<()>> {
        if deadline.is_none() {
            *deadline = Some(Box::pin(tokio::time::sleep(idle_timeout)));
        }
        if deadline
            .as_mut()
            .expect("idle timeout deadline must be initialized")
            .as_mut()
            .poll(cx)
            .is_ready()
        {
            *deadline = None;
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "connection idle timeout exceeded during {operation} after {:.3}s",
                    idle_timeout.as_secs_f64()
                ),
            )));
        }
        Poll::Pending
    }
}

impl AsyncRead for IdleTimeoutStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        match Pin::new(&mut this.stream).poll_read(cx, buf) {
            Poll::Ready(result) => {
                this.read_deadline = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                Self::poll_deadline(&mut this.read_deadline, this.idle_timeout, cx, "read")
            }
        }
    }
}

impl AsyncWrite for IdleTimeoutStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.stream).poll_write(cx, buf) {
            Poll::Ready(result) => {
                this.write_deadline = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                Self::poll_deadline(&mut this.write_deadline, this.idle_timeout, cx, "write")
                    .map(|result| result.map(|_| 0))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        match Pin::new(&mut this.stream).poll_flush(cx) {
            Poll::Ready(result) => {
                this.write_deadline = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                Self::poll_deadline(&mut this.write_deadline, this.idle_timeout, cx, "flush")
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        match Pin::new(&mut this.stream).poll_shutdown(cx) {
            Poll::Ready(result) => {
                this.write_deadline = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                Self::poll_deadline(&mut this.write_deadline, this.idle_timeout, cx, "shutdown")
            }
        }
    }
}

async fn serve_http_listener(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    config: HttpServeConfig,
) -> std::io::Result<()> {
    let connection_semaphore = Arc::new(Semaphore::new(config.max_connections.max(1)));
    let mut accept_backoff = Duration::from_millis(HTTP_ACCEPT_BACKOFF_MIN_MS);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                accept_backoff = Duration::from_millis(HTTP_ACCEPT_BACKOFF_MIN_MS);
                let permit = match connection_semaphore.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        warn!("Dropping inbound connection because listener capacity is exhausted");
                        continue;
                    }
                };
                let app_clone = app.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = serve_http_stream(stream, app_clone, config).await {
                        error!("Error serving Axum over TCP stream: {error}");
                    }
                });
            }
            Err(error) if accept_error_is_transient(&error) => {
                warn!(
                    "Listener accept failed transiently: {error}; retrying after {:.3}s",
                    accept_backoff.as_secs_f64()
                );
                tokio::time::sleep(accept_backoff).await;
                accept_backoff =
                    (accept_backoff * 2).min(Duration::from_millis(HTTP_ACCEPT_BACKOFF_MAX_MS));
            }
            Err(error) => return Err(error),
        }
    }
}

async fn serve_http_stream(
    stream: tokio::net::TcpStream,
    app: axum::Router,
    config: HttpServeConfig,
) -> Result<(), String> {
    let mut builder = http1::Builder::new();
    builder.half_close(true);
    builder.keep_alive(true);
    builder.timer(TokioTimer::new());
    builder.header_read_timeout(Some(config.header_read_timeout));

    let hyper_service =
        hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
            app.clone().call(req)
        });

    let io = TokioIo::new(IdleTimeoutStream::new(stream, config.idle_timeout));
    builder
        .serve_connection(io, hyper_service)
        .await
        .map_err(|error| error.to_string())
}
