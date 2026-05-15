//! `froglet-node` CLI subcommands. Phase 2 of the agent-grade publish
//! plan. Each subcommand lives in its own module; the bin entrypoint
//! (`src/bin/froglet-node.rs`) dispatches.
//!
//! Design notes:
//!
//! - **One binary, multiple subcommands.** Per the plan, we extend
//!   `froglet-node` rather than splitting into a separate `froglet`
//!   binary; the migration cost is too high right now. A future v0.3
//!   may split.
//! - **No `clap` dep.** Args are parsed by hand from `std::env::args`
//!   so we don't pull in the macro surface. The shape is small enough
//!   that hand-parsing stays readable.
//! - **Output is human first, JSON second.** Every subcommand supports
//!   `--json` to emit a machine-readable result for MCP / agent
//!   consumers; the default is a tight human-readable summary.
//! - **All `publish`-flavoured commands go through
//!   `froglet_publish_engine`.** The CLI is a thin shell over the
//!   engine; one source of truth for build → host → sign → register.

pub mod build;
pub mod init;
pub mod invoke;
pub mod publish;
pub mod whoami;

use std::fmt;

/// Error returned by a CLI subcommand. Always rendered to stderr with
/// the exit code mapped to a small fixed set; agents parse the exit
/// code as the "did this work" signal.
#[derive(Debug)]
pub enum CliError {
    /// User-supplied arguments were wrong.
    BadArgs(String),
    /// Filesystem / IO failure.
    Io(std::io::Error),
    /// Manifest validation failed.
    Manifest(froglet_protocol::manifest::ManifestError),
    /// The publish engine returned an error.
    Engine(froglet_publish_engine::PublishError),
    /// A daemon HTTP call failed.
    Daemon(String),
    /// Other failure with a string message.
    Other(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadArgs(s) => write!(f, "bad arguments: {s}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Manifest(e) => write!(f, "manifest: {e}"),
            Self::Engine(e) => write!(f, "publish engine: {e}"),
            Self::Daemon(s) => write!(f, "daemon: {s}"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<froglet_protocol::manifest::ManifestError> for CliError {
    fn from(e: froglet_protocol::manifest::ManifestError) -> Self {
        Self::Manifest(e)
    }
}

impl From<froglet_publish_engine::PublishError> for CliError {
    fn from(e: froglet_publish_engine::PublishError) -> Self {
        Self::Engine(e)
    }
}

impl CliError {
    /// Map to a stable exit code so wrapping scripts can branch:
    /// - 0 success
    /// - 2 bad args
    /// - 3 manifest validation
    /// - 4 io
    /// - 5 daemon unreachable
    /// - 6 publish engine
    /// - 1 anything else
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::BadArgs(_) => 2,
            Self::Manifest(_) => 3,
            Self::Io(_) => 4,
            Self::Daemon(_) => 5,
            Self::Engine(_) => 6,
            Self::Other(_) => 1,
        }
    }
}

/// Detect whether the args contain `--json` (consumes it).
pub fn pop_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

/// Pop a `--key value` pair if present.
pub fn pop_kv(args: &mut Vec<String>, key: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == key {
            if i + 1 < args.len() {
                let _ = args.remove(i); // key
                let value = args.remove(i); // value
                return Some(value);
            }
            return None;
        }
        i += 1;
    }
    None
}
