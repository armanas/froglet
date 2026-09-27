//! Killable, bounded execution for builtins. Only trusted handler registration
//! selects an executable; invocation JSON never chooses a program or a path.
use crate::config::ProcessLimitsConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

pub const SUBCOMMAND: &str = "__builtin-worker";
const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct WorkerSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub handler: String,
    pub config: Value,
}

impl WorkerSpec {
    pub fn local(handler: &str, config: Value) -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        if !cfg!(unix) {
            return Err("supervised builtin workers currently require Unix".into());
        }
        // Unit-test executables do not implement the daemon's worker entrypoint.
        // Integration tests build this exact sibling binary via Cargo.
        let executable = if executable
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|p| p == "deps")
        {
            if let Some(path) = std::env::var_os("FROGLET_TEST_BUILTIN_WORKER") {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err("FROGLET_TEST_BUILTIN_WORKER must be absolute".into());
                }
                path
            } else {
                executable
                    .parent()
                    .and_then(|p| p.parent())
                    .ok_or("invalid executable directory")?
                    .join(format!("froglet-node{}", std::env::consts::EXE_SUFFIX))
            }
        } else {
            executable
        };
        Ok(Self {
            executable,
            args: vec![SUBCOMMAND.into()],
            environment: BTreeMap::new(),
            handler: handler.into(),
            config,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct WorkerRequest {
    pub handler: String,
    pub config: Value,
    pub input: Value,
}

struct ChildGuard(tokio::process::Child, Option<u32>);
impl ChildGuard {
    fn kill_group(&mut self) {
        #[cfg(unix)]
        if let Some(id) = self.1 {
            // SAFETY: each child is started in its own process group.
            unsafe {
                libc::kill(-(id as i32), libc::SIGKILL);
            }
        }
        let _ = self.0.start_kill();
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill_group();
    }
}

async fn read_bounded(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("builtin worker output limit exceeded".into());
    }
    Ok(bytes)
}

pub async fn execute(
    spec: WorkerSpec,
    input: Value,
    timeout: Duration,
    limits: &ProcessLimitsConfig,
) -> Result<Value, String> {
    let request = serde_json::to_vec(&WorkerRequest {
        handler: spec.handler,
        config: spec.config,
        input,
    })
    .map_err(|e| e.to_string())?;
    if request.len() > MAX_REQUEST_BYTES {
        return Err("builtin worker input limit exceeded".into());
    }
    let mut command = tokio::process::Command::new(spec.executable);
    command
        .args(spec.args)
        .env_clear()
        .envs(spec.environment)
        .env("FROGLET_WORKER_PARENT_PID", std::process::id().to_string())
        .env(
            "FROGLET_WORKER_DEADLINE_MS",
            timeout.as_millis().max(1).to_string(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        command.process_group(0);
        let memory = limits.memory_max_bytes;
        let cpu = timeout.as_secs().saturating_add(1);
        // SAFETY: pre_exec uses only async-signal-safe libc calls.
        unsafe {
            command.pre_exec(move || {
                let cpu_limit = libc::rlimit {
                    rlim_cur: cpu as libc::rlim_t,
                    rlim_max: cpu as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_CPU, &cpu_limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                #[cfg(target_os = "linux")]
                {
                    let memory_limit = libc::rlimit {
                        rlim_cur: memory as libc::rlim_t,
                        rlim_max: memory as libc::rlim_t,
                    };
                    if libc::setrlimit(libc::RLIMIT_AS, &memory_limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                #[cfg(not(target_os = "linux"))]
                let _ = memory;
                Ok(())
            });
        }
    }
    let process = command
        .spawn()
        .map_err(|e| format!("builtin worker could not start: {e}"))?;
    let id = process.id();
    let mut child = ChildGuard(process, id);
    let mut stdin = child.0.stdin.take().ok_or("builtin stdin unavailable")?;
    let stdout = child.0.stdout.take().ok_or("builtin stdout unavailable")?;
    let stderr = child.0.stderr.take().ok_or("builtin stderr unavailable")?;
    let operation = async {
        let write = async {
            stdin.write_all(&request).await.map_err(|e| e.to_string())?;
            drop(stdin);
            Ok::<_, String>(())
        };
        let (_, stdout, _) = tokio::try_join!(
            write,
            read_bounded(stdout, limits.output_max_bytes.saturating_add(1024)),
            read_bounded(stderr, 16 * 1024)
        )?;
        let status = child.0.wait().await.map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("builtin worker exited unsuccessfully: {status}"));
        }
        let response: Value = serde_json::from_slice(&stdout)
            .map_err(|e| format!("invalid builtin worker response: {e}"))?;
        if let Some(error) = response.get("error").and_then(Value::as_str) {
            return Err(error.to_string());
        }
        let result = response
            .get("ok")
            .cloned()
            .ok_or("builtin worker omitted result")?;
        if crate::canonical_json::to_vec(&result)
            .map_err(|e| e.to_string())?
            .len()
            > limits.output_max_bytes
        {
            return Err("builtin worker output limit exceeded".into());
        }
        Ok(result)
    };
    match tokio::time::timeout(timeout, operation).await {
        Ok(Ok(value)) => Ok(value),
        outcome => {
            child.kill_group();
            let _ = child.0.wait().await;
            match outcome {
                Ok(Err(error)) => Err(error),
                _ => Err(format!(
                    "builtin execution exceeded runtime deadline after {}ms",
                    timeout.as_millis()
                )),
            }
        }
    }
}

/// Called only by a dedicated worker entrypoint before dispatch. A watchdog
/// also terminates the worker after abrupt provider death, including SIGKILL.
pub fn read_request() -> Result<WorkerRequest, String> {
    use std::io::Read;
    #[cfg(unix)]
    if unsafe { libc::getpgrp() != libc::getpid() } {
        return Err("worker requires a dedicated process group".into());
    }
    let parent: u32 = std::env::var("FROGLET_WORKER_PARENT_PID")
        .map_err(|_| "worker requires supervisor")?
        .parse()
        .map_err(|_| "invalid supervisor")?;
    let duration: u64 = std::env::var("FROGLET_WORKER_DEADLINE_MS")
        .map_err(|_| "worker requires deadline")?
        .parse()
        .map_err(|_| "invalid deadline")?;
    if !(1..=300_000).contains(&duration) {
        return Err("invalid worker deadline".into());
    }
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_millis(duration);
        loop {
            #[cfg(unix)]
            let orphaned = unsafe { libc::getppid() } as u32 != parent;
            #[cfg(not(unix))]
            let orphaned = {
                let _ = parent;
                false
            };
            if orphaned || std::time::Instant::now() >= deadline {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-libc::getpgrp(), libc::SIGKILL);
                }
                std::process::exit(124);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("worker input too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

pub fn write_result(result: Result<Value, String>) {
    let response = match result {
        Ok(value) => serde_json::json!({"ok":value}),
        Err(error) => serde_json::json!({"error":error}),
    };
    println!("{response}");
}

pub async fn dispatch_standard(request: WorkerRequest) -> Result<Value, String> {
    crate::tls::ensure_rustls_crypto_provider();
    use crate::{builtins::*, execution::BuiltinServiceHandler};
    if request.handler == "native.events-query" {
        let database = request.config["database"]
            .as_str()
            .ok_or("missing event database")?;
        let conn = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_millis(100))
            .map_err(|e| e.to_string())?;
        let kinds: Vec<String> =
            serde_json::from_value(request.input["kinds"].clone()).map_err(|e| e.to_string())?;
        let limit: Option<usize> =
            serde_json::from_value(request.input["limit"].clone()).map_err(|e| e.to_string())?;
        let events =
            crate::db::query_events_by_kind(&conn, &kinds, limit).map_err(|e| e.to_string())?;
        return serde_json::to_value(events).map_err(|e| e.to_string());
    }
    let handler: Box<dyn BuiltinServiceHandler> = match request.handler.as_str() {
        "demo.echo" => Box::new(EchoHandler),
        "demo.add" => Box::new(AddHandler),
        "demo.notarize" => Box::new(NotarizeHandler),
        "demo.fetch-witness" => Box::new(FetchWitnessHandler),
        "demo.hash-verify" => Box::new(HashVerifyHandler),
        "data.bound-query" => {
            let config = request.config;
            let root = config["root"].as_str().ok_or("missing root")?;
            let file = config["file"].as_str().ok_or("missing file")?;
            let kind: DataQuerySourceKind =
                serde_json::from_value(config["kind"].clone()).map_err(|e| e.to_string())?;
            let limits =
                serde_json::from_value(config["limits"].clone()).map_err(|e| e.to_string())?;
            if kind == DataQuerySourceKind::Csv {
                let schema =
                    serde_json::from_value(config["schema"].clone()).map_err(|e| e.to_string())?;
                Box::new(DataQueryHandler::open_csv_indexed_with_limits(
                    root,
                    file,
                    schema,
                    config["digest"].as_str().ok_or("missing digest")?,
                    limits,
                )?)
            } else {
                Box::new(DataQueryHandler::open_with_limits(
                    root, file, kind, limits,
                )?)
            }
        }
        "native.data-query" => {
            let root = PathBuf::from(request.config["root"].as_str().ok_or("missing data root")?);
            let digest = request.config["digest"].as_str().ok_or("missing digest")?;
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err("invalid data digest".into());
            }
            let kind: DataQuerySourceKind = request.config["kind"]
                .as_str()
                .ok_or("missing data kind")?
                .parse()?;
            let file = format!("{digest}.{kind}");
            if kind == DataQuerySourceKind::Csv {
                use std::io::Read;
                let mut bytes = Vec::new();
                std::fs::File::open(root.join(format!("{digest}.csv.schema.json")))
                    .map_err(|e| e.to_string())?
                    .take(1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 1024 * 1024 {
                    return Err("CSV schema too large".into());
                }
                let schema = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                if crate::canonical_json::to_vec(&schema).map_err(|e| e.to_string())? != bytes {
                    return Err("CSV schema is not canonical".into());
                }
                Box::new(DataQueryHandler::open_csv_indexed(
                    &root, &file, schema, digest,
                )?)
            } else {
                use sha2::{Digest, Sha256};
                use std::io::Read;
                let mut source =
                    std::fs::File::open(root.join(&file)).map_err(|e| e.to_string())?;
                let mut hash = Sha256::new();
                let mut chunk = [0u8; 65536];
                loop {
                    let n = source.read(&mut chunk).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    hash.update(&chunk[..n]);
                }
                if hex::encode(hash.finalize()) != digest {
                    return Err("data snapshot no longer matches revision binding".into());
                }
                Box::new(DataQueryHandler::open(&root, &file, kind)?)
            }
        }
        _ => return Err("unsupported builtin worker handler".into()),
    };
    handler.execute(request.input).await
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn shell(script: &str, path: &std::path::Path) -> WorkerSpec {
        WorkerSpec {
            executable: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                script.into(),
                "worker-test".into(),
                path.to_string_lossy().into(),
            ],
            environment: BTreeMap::new(),
            handler: "test".into(),
            config: Value::Null,
        }
    }
    async fn pid(path: &std::path::Path) -> i32 {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(s) = std::fs::read_to_string(path)
                    && let Ok(id) = s.trim().parse()
                {
                    return id;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("worker started")
    }
    async fn assert_dead(pid: i32) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker must be killed and reaped");
    }
    #[tokio::test]
    async fn worker_deadline_kills_and_reaps_infinite_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid");
        let result = execute(
            shell("echo $$ > \"$1\"; while :; do :; done", &path),
            Value::Null,
            Duration::from_millis(100),
            &ProcessLimitsConfig::default(),
        )
        .await;
        assert!(result.unwrap_err().contains("deadline"));
        assert_dead(pid(&path).await).await;
    }
    #[tokio::test]
    async fn cancelled_request_terminates_its_worker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid");
        let spec = shell("echo $$ > \"$1\"; sleep 20", &path);
        let running = tokio::spawn(async move {
            execute(
                spec,
                Value::Null,
                Duration::from_secs(30),
                &ProcessLimitsConfig::default(),
            )
            .await
        });
        let id = pid(&path).await;
        running.abort();
        let _ = running.await;
        assert_dead(id).await;
    }
    #[tokio::test]
    async fn worker_pipes_are_bounded_and_normal_results_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let limits = ProcessLimitsConfig {
            output_max_bytes: 1024,
            ..Default::default()
        };
        let flooded = execute(
            shell("head -c 65536 /dev/zero", dir.path()),
            Value::Null,
            Duration::from_secs(2),
            &limits,
        )
        .await;
        assert!(flooded.unwrap_err().contains("output limit"));
        let result = execute(
            shell("cat >/dev/null; printf '{\"ok\":7}'", dir.path()),
            Value::Null,
            Duration::from_secs(2),
            &limits,
        )
        .await
        .unwrap();
        assert_eq!(result, serde_json::json!(7));
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn worker_address_space_limit_is_enforced_before_execution() {
        let spec = WorkerSpec::local("demo.echo", serde_json::json!({})).unwrap();
        let limits = ProcessLimitsConfig {
            memory_max_bytes: 1,
            ..Default::default()
        };
        assert!(
            execute(
                spec.clone(),
                serde_json::json!({"memory_probe": true}),
                Duration::from_secs(2),
                &limits,
            )
            .await
            .is_err(),
            "a native worker cannot execute with one byte of address space"
        );
        let output = execute(
            spec,
            serde_json::json!({"memory_probe": true}),
            Duration::from_secs(2),
            &ProcessLimitsConfig::default(),
        )
        .await
        .expect("the same worker must execute with the configured process budget");
        assert_eq!(output, serde_json::json!({"memory_probe": true}));
    }
    #[tokio::test]
    async fn worker_deadline_kills_descendants_that_keep_pipes_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("child-pid");
        let result = execute(
            shell("sleep 20 & echo $! > \"$1\"; wait", &path),
            Value::Null,
            Duration::from_millis(100),
            &ProcessLimitsConfig::default(),
        )
        .await;
        assert!(result.unwrap_err().contains("deadline"));
        let id = pid(&path).await;
        // A dead orphan may briefly remain as a zombie until the host init reaps it.
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let result = tokio::process::Command::new("ps")
                    .args(["-p", &id.to_string(), "-o", "stat="])
                    .output()
                    .await
                    .unwrap();
                let state = String::from_utf8_lossy(&result.stdout);
                if state.trim().is_empty() || state.trim().starts_with('Z') {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("descendant must stop running");
    }
    #[tokio::test]
    async fn unbounded_sqlite_event_scan_is_killed_at_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("events.db");
        let conn = rusqlite::Connection::open(&database).unwrap();
        conn.execute_batch("CREATE VIEW events AS WITH RECURSIVE infinite(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM infinite) SELECT '' AS id,'' AS pubkey,n AS created_at,'slow' AS kind,'' AS content,'' AS sig,'[]' AS tags FROM infinite;").unwrap();
        drop(conn);
        let spec = WorkerSpec::local(
            "native.events-query",
            serde_json::json!({"database":database}),
        )
        .unwrap();
        let started = std::time::Instant::now();
        let error = execute(
            spec,
            serde_json::json!({"kinds":["slow"],"limit":1}),
            Duration::from_millis(100),
            &ProcessLimitsConfig::default(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("deadline"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
