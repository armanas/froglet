use std::{
    io::{self, Read},
    process::{ChildStderr, ChildStdout, Command},
    sync::Arc,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const DEFAULT_PROCESS_CONCURRENCY_LIMIT: usize = 8;
const DEFAULT_PROCESS_STDOUT_LIMIT_BYTES: usize = 1024 * 1024;
const DEFAULT_PROCESS_STDERR_LIMIT_BYTES: usize = 1024 * 1024;
const DEFAULT_CONTAINER_MEMORY_LIMIT_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_CONTAINER_PIDS_LIMIT: u64 = 256;
const DEFAULT_CONTAINER_CPU_LIMIT: f64 = 1.0;

#[derive(Debug, Clone)]
pub struct ProcessRuntimeConfig {
    pub concurrency_limit: usize,
    pub stdout_limit_bytes: usize,
    pub stderr_limit_bytes: usize,
    pub container_memory_limit_bytes: u64,
    pub container_pids_limit: u64,
    pub container_cpu_limit: f64,
}

impl Default for ProcessRuntimeConfig {
    fn default() -> Self {
        Self {
            concurrency_limit: DEFAULT_PROCESS_CONCURRENCY_LIMIT,
            stdout_limit_bytes: DEFAULT_PROCESS_STDOUT_LIMIT_BYTES,
            stderr_limit_bytes: DEFAULT_PROCESS_STDERR_LIMIT_BYTES,
            container_memory_limit_bytes: DEFAULT_CONTAINER_MEMORY_LIMIT_BYTES,
            container_pids_limit: DEFAULT_CONTAINER_PIDS_LIMIT,
            container_cpu_limit: DEFAULT_CONTAINER_CPU_LIMIT,
        }
    }
}

impl ProcessRuntimeConfig {
    pub fn from_process_limits(limits: &crate::config::ProcessLimitsConfig) -> Self {
        Self {
            concurrency_limit: limits.concurrency,
            stdout_limit_bytes: limits.output_max_bytes,
            stderr_limit_bytes: limits.output_max_bytes,
            container_memory_limit_bytes: limits.memory_max_bytes,
            container_pids_limit: limits.pids_limit,
            container_cpu_limit: limits.cpu_limit,
        }
    }

    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub fn apply_container_limits(&self, command: &mut Command) {
        command
            .arg("--memory")
            .arg(self.container_memory_limit_bytes.to_string())
            .arg("--pids-limit")
            .arg(self.container_pids_limit.to_string())
            .arg("--cpus")
            .arg(format!("{:.3}", self.container_cpu_limit));
    }

    fn from_lookup<F>(mut lookup: F) -> Result<Self, String>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        Ok(Self {
            concurrency_limit: env_usize(
                &mut lookup,
                "FROGLET_PROCESS_CONCURRENCY",
                DEFAULT_PROCESS_CONCURRENCY_LIMIT,
                1,
                256,
            )?,
            stdout_limit_bytes: env_usize(
                &mut lookup,
                "FROGLET_PROCESS_OUTPUT_MAX_BYTES",
                DEFAULT_PROCESS_STDOUT_LIMIT_BYTES,
                1024,
                64 * 1024 * 1024,
            )?,
            stderr_limit_bytes: env_usize(
                &mut lookup,
                "FROGLET_PROCESS_OUTPUT_MAX_BYTES",
                DEFAULT_PROCESS_STDERR_LIMIT_BYTES,
                1024,
                64 * 1024 * 1024,
            )?,
            container_memory_limit_bytes: env_u64(
                &mut lookup,
                "FROGLET_PROCESS_MEMORY_MAX_BYTES",
                DEFAULT_CONTAINER_MEMORY_LIMIT_BYTES,
                16 * 1024 * 1024,
                64 * 1024 * 1024 * 1024,
            )?,
            container_pids_limit: env_u64(
                &mut lookup,
                "FROGLET_PROCESS_PIDS_LIMIT",
                DEFAULT_CONTAINER_PIDS_LIMIT,
                16,
                4096,
            )?,
            container_cpu_limit: env_f64(
                &mut lookup,
                "FROGLET_PROCESS_CPU_LIMIT",
                DEFAULT_CONTAINER_CPU_LIMIT,
                0.1,
                256.0,
            )?,
        })
    }
}

fn env_usize<F>(
    lookup: &mut F,
    name: &'static str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, String>
where
    F: FnMut(&'static str) -> Option<String>,
{
    match lookup(name) {
        Some(value) => value
            .parse::<usize>()
            .map(|value| value.clamp(min, max))
            .map_err(|_| format!("Invalid {name} value: '{value}'. Expected unsigned integer")),
        None => Ok(default),
    }
}

fn env_u64<F>(
    lookup: &mut F,
    name: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, String>
where
    F: FnMut(&'static str) -> Option<String>,
{
    match lookup(name) {
        Some(value) => value
            .parse::<u64>()
            .map(|value| value.clamp(min, max))
            .map_err(|_| format!("Invalid {name} value: '{value}'. Expected unsigned integer")),
        None => Ok(default),
    }
}

fn env_f64<F>(
    lookup: &mut F,
    name: &'static str,
    default: f64,
    min: f64,
    max: f64,
) -> Result<f64, String>
where
    F: FnMut(&'static str) -> Option<String>,
{
    match lookup(name) {
        Some(value) => {
            let parsed = value
                .parse::<f64>()
                .map_err(|_| format!("Invalid {name} value: '{value}'. Expected number"))?;
            if !parsed.is_finite() || parsed <= 0.0 {
                return Err(format!(
                    "Invalid {name} value: '{value}'. Expected positive number"
                ));
            }
            Ok(parsed.clamp(min, max))
        }
        None => Ok(default),
    }
}

#[derive(Clone)]
pub struct ProcessRuntime {
    config: ProcessRuntimeConfig,
    semaphore: Arc<Semaphore>,
}

impl ProcessRuntime {
    pub fn new(config: ProcessRuntimeConfig) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(config.concurrency_limit.max(1))),
            config,
        }
    }

    pub fn config(&self) -> &ProcessRuntimeConfig {
        &self.config
    }

    pub fn try_acquire(&self) -> Result<ProcessPermit, String> {
        self.semaphore
            .clone()
            .try_acquire_owned()
            .map(|permit| ProcessPermit { _permit: permit })
            .map_err(|_| "process concurrency limit reached".to_string())
    }
}

pub struct ProcessPermit {
    _permit: OwnedSemaphorePermit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedStream {
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub bytes_seen: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedChildOutput {
    pub stdout: BoundedStream,
    pub stderr: BoundedStream,
}

pub fn read_child_output_bounded(
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    config: &ProcessRuntimeConfig,
) -> Result<BoundedChildOutput, String> {
    let stdout_limit = config.stdout_limit_bytes;
    let stderr_limit = config.stderr_limit_bytes;
    let stdout_thread =
        std::thread::spawn(move || read_optional_stream_bounded(stdout, stdout_limit));
    let stderr_thread =
        std::thread::spawn(move || read_optional_stream_bounded(stderr, stderr_limit));

    let stdout = stdout_thread
        .join()
        .map_err(|_| "stdout reader thread panicked".to_string())?
        .map_err(|error| format!("failed to read process stdout: {error}"))?;
    let stderr = stderr_thread
        .join()
        .map_err(|_| "stderr reader thread panicked".to_string())?
        .map_err(|error| format!("failed to read process stderr: {error}"))?;

    Ok(BoundedChildOutput { stdout, stderr })
}

fn read_optional_stream_bounded<R: Read>(
    stream: Option<R>,
    limit: usize,
) -> io::Result<BoundedStream> {
    match stream {
        Some(stream) => read_stream_bounded(stream, limit),
        None => Ok(BoundedStream {
            bytes: Vec::new(),
            truncated: false,
            bytes_seen: 0,
            limit,
        }),
    }
}

fn read_stream_bounded<R: Read>(mut stream: R, limit: usize) -> io::Result<BoundedStream> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut scratch = [0_u8; 8192];
    let mut bytes_seen = 0_usize;
    let mut truncated = false;

    loop {
        let n = stream.read(&mut scratch)?;
        if n == 0 {
            break;
        }
        bytes_seen = bytes_seen.saturating_add(n);
        if bytes.len() < limit {
            let remaining = limit - bytes.len();
            let kept = remaining.min(n);
            bytes.extend_from_slice(&scratch[..kept]);
            if kept < n {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }

    Ok(BoundedStream {
        bytes,
        truncated,
        bytes_seen,
        limit,
    })
}

pub fn stream_limit_error(label: &str, stream: &BoundedStream) -> String {
    format!(
        "{label} exceeded process output limit: saw {} bytes, kept {} byte limit",
        stream.bytes_seen, stream.limit
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, io::Cursor};

    #[test]
    fn bounded_reader_keeps_limit_and_drains_rest() {
        let stream = read_stream_bounded(Cursor::new(b"abcdef"), 4).expect("read stream");

        assert_eq!(stream.bytes, b"abcd");
        assert!(stream.truncated);
        assert_eq!(stream.bytes_seen, 6);
        assert_eq!(stream.limit, 4);
    }

    #[test]
    fn bounded_reader_reports_untruncated_short_stream() {
        let stream = read_stream_bounded(Cursor::new(b"abc"), 4).expect("read stream");

        assert_eq!(stream.bytes, b"abc");
        assert!(!stream.truncated);
        assert_eq!(stream.bytes_seen, 3);
    }

    #[test]
    fn config_from_lookup_applies_defaults_and_clamps() {
        let mut values = HashMap::new();
        values.insert("FROGLET_PROCESS_CONCURRENCY", "0");
        values.insert("FROGLET_PROCESS_OUTPUT_MAX_BYTES", "999999999999");
        values.insert("FROGLET_PROCESS_CPU_LIMIT", "999");

        let config =
            ProcessRuntimeConfig::from_lookup(|name| values.get(name).copied().map(str::to_string))
                .expect("config");

        assert_eq!(config.concurrency_limit, 1);
        assert_eq!(config.stdout_limit_bytes, 64 * 1024 * 1024);
        assert_eq!(config.stderr_limit_bytes, 64 * 1024 * 1024);
        assert_eq!(config.container_cpu_limit, 256.0);
    }

    #[test]
    fn config_from_lookup_rejects_invalid_numbers() {
        let config = ProcessRuntimeConfig::from_lookup(|name| {
            (name == "FROGLET_PROCESS_CPU_LIMIT").then(|| "nan".to_string())
        });

        assert!(
            config
                .expect_err("nan must fail")
                .contains("Expected positive number")
        );
    }

    #[test]
    fn process_runtime_enforces_try_acquire_limit() {
        let runtime = ProcessRuntime::new(ProcessRuntimeConfig {
            concurrency_limit: 1,
            ..ProcessRuntimeConfig::default()
        });
        let _permit = runtime.try_acquire().expect("first permit");

        assert!(runtime.try_acquire().is_err());
    }
}
