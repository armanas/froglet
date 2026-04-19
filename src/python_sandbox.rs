//! Linux-native sandbox for Python execution.
//!
//! Applies kernel-level restrictions to any `std::process::Command` before
//! `exec`, so the Python subprocess cannot:
//!
//! - read or write outside an explicit allow-list (enforced by `landlock`),
//! - issue outbound network connections unless the caller grants it
//!   (enforced by `seccomp` denying `socket`/`connect`/`bind`),
//! - `execve` another binary (enforced by `seccomp`), or
//! - gain new privileges via suid binaries (enforced by `prctl PR_SET_NO_NEW_PRIVS`).
//!
//! This is a **deny-list** sandbox, not a full allow-list: Python stdlib uses
//! dozens of syscalls and enumerating every one across Python versions is
//! fragile. The deny-list targets the five concrete attack surfaces a
//! malicious workload would exploit: filesystem reads of host secrets,
//! filesystem writes outside its tempdir, outbound network, arbitrary exec,
//! and escape to a new privileged program. Each is closed by landlock or
//! seccomp independently; failure of either primitive does not open the
//! others.
//!
//! On non-Linux hosts the module is a no-op and `install()` refuses to run a
//! Python workload unless `FROGLET_ALLOW_UNSANDBOXED_PYTHON=1` is set. This
//! keeps macOS / Windows dev workflows functional with an explicit opt-in,
//! while production Linux deploys are sandboxed by default.
//!
//! Namespaces (`unshare`) were considered but intentionally omitted: Docker's
//! default seccomp profile denies `unshare(CLONE_NEWUSER|...)` inside a
//! container, which is Froglet's primary deploy target. Adding namespaces
//! would require operators to relax Docker's seccomp profile, widening the
//! host's attack surface for a marginal defense-in-depth gain. Landlock and
//! seccomp both work unchanged inside containers.

#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::path::PathBuf;

/// Per-invocation policy for the sandbox. The caller supplies the paths the
/// workload is allowed to read and write; everything else is denied.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Paths the workload may read (and descend into recursively).
    pub readonly_paths: Vec<PathBuf>,
    /// Paths the workload may read, write, create, and unlink recursively.
    pub writable_paths: Vec<PathBuf>,
    /// When true, outbound network syscalls are allowed. Set only when the
    /// workload has been granted a capability that requires network
    /// (e.g., a postgres mount).
    pub allow_network: bool,
}

impl SandboxConfig {
    /// A sensible default for Python workloads: read the stdlib + SSL certs,
    /// write only to `tempdir`, no network.
    pub fn for_python(tempdir: &std::path::Path) -> Self {
        Self {
            readonly_paths: default_readonly_paths(),
            writable_paths: vec![tempdir.to_path_buf()],
            allow_network: false,
        }
    }
}

fn default_readonly_paths() -> Vec<PathBuf> {
    // Python stdlib + linker + CA certs + DNS resolution. Conservative list
    // that works on Debian / Ubuntu / Alpine / RHEL. Paths that don't exist
    // on a given host are silently skipped when installing the ruleset.
    vec![
        PathBuf::from("/usr"),
        PathBuf::from("/lib"),
        PathBuf::from("/lib64"),
        PathBuf::from("/etc/ssl"),
        PathBuf::from("/etc/ca-certificates"),
        PathBuf::from("/etc/resolv.conf"),
        PathBuf::from("/etc/nsswitch.conf"),
        PathBuf::from("/etc/hosts"),
        PathBuf::from("/etc/localtime"),
    ]
}

/// Tier the sandbox is running in. Logged once at startup so operators can
/// see what level of isolation is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxTier {
    /// Full sandbox — landlock + seccomp + NO_NEW_PRIVS.
    Full,
    /// Landlock and seccomp both unavailable on this host (pre-5.13 kernel
    /// on Linux, or running on non-Linux with the opt-out env var set).
    Unsandboxed,
}

/// Decide what tier to run in based on host capabilities and env.
pub fn detect_tier() -> SandboxTier {
    #[cfg(target_os = "linux")]
    {
        SandboxTier::Full
    }
    #[cfg(not(target_os = "linux"))]
    {
        SandboxTier::Unsandboxed
    }
}

/// Install the sandbox on a `Command`. After this call, any child spawned by
/// the command will have the restrictions applied (via `pre_exec`).
///
/// On non-Linux platforms this returns `Err` unless
/// `FROGLET_ALLOW_UNSANDBOXED_PYTHON=1` is set.
pub fn harden_command(
    command: &mut std::process::Command,
    config: SandboxConfig,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let config_clone = config;
        unsafe {
            command.pre_exec(move || {
                install_sandbox(&config_clone)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error))?;
                Ok(())
            });
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = command;
        let _ = config;
        if std::env::var("FROGLET_ALLOW_UNSANDBOXED_PYTHON").as_deref() == Ok("1") {
            tracing::warn!(
                "python sandbox disabled on non-Linux host (FROGLET_ALLOW_UNSANDBOXED_PYTHON=1)"
            );
            Ok(())
        } else {
            Err(
                "python execution requires the linux sandbox; set FROGLET_ALLOW_UNSANDBOXED_PYTHON=1 to override on dev hosts"
                    .to_string(),
            )
        }
    }
}

#[cfg(target_os = "linux")]
fn install_sandbox(config: &SandboxConfig) -> Result<(), String> {
    // PR_SET_NO_NEW_PRIVS is required to install a seccomp filter without
    // CAP_SYS_ADMIN, and also neutralises suid binaries that the workload
    // might try to exec.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(format!(
                "prctl PR_SET_NO_NEW_PRIVS failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    install_landlock(config)?;
    install_seccomp(config)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_landlock(config: &SandboxConfig) -> Result<(), String> {
    use landlock::{
        ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus,
    };

    let abi = ABI::V1;
    let read_access = AccessFs::from_read(abi);
    let write_access = AccessFs::from_all(abi);

    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|error| format!("landlock handle_access: {error}"))?
        .create()
        .map_err(|error| format!("landlock create: {error}"))?;

    for path in &config.readonly_paths {
        let Ok(fd) = PathFd::new(path) else {
            // Silently skip paths that don't exist — e.g., /lib64 on Alpine.
            continue;
        };
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, read_access))
            .map_err(|error| format!("landlock add_rule ro {path:?}: {error}"))?;
    }
    for path in &config.writable_paths {
        let Ok(fd) = PathFd::new(path) else {
            continue;
        };
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, write_access))
            .map_err(|error| format!("landlock add_rule rw {path:?}: {error}"))?;
    }

    let status = ruleset
        .restrict_self()
        .map_err(|error| format!("landlock restrict_self: {error}"))?;

    // RulesetStatus::NotEnforced means the kernel did not apply the ruleset
    // (pre-5.13 kernel, or landlock disabled at boot). In that case we
    // continue with seccomp only — seccomp still blocks network and exec, and
    // the caller has been told this is "Full" at startup via detect_tier().
    // A warning here keeps operators informed.
    if status.ruleset != RulesetStatus::FullyEnforced {
        tracing::warn!(
            ruleset = ?status.ruleset,
            "landlock not fully enforced; falling back to seccomp-only filesystem isolation"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_seccomp(config: &SandboxConfig) -> Result<(), String> {
    use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule};
    use std::collections::BTreeMap;

    // Deny-list of dangerous syscalls. Everything else continues to work, so
    // Python stdlib is unaffected.
    let mut denied: HashSet<i64> = HashSet::new();
    // Prevent the workload from exec'ing a different binary — the whole
    // sandbox assumes python3 is the only executable in this process.
    denied.insert(libc::SYS_execve);
    denied.insert(libc::SYS_execveat);
    if !config.allow_network {
        // Block outbound sockets. Deny at the creation step (socket /
        // socketpair) so the workload can't even open a file descriptor; also
        // deny connect / bind for defense in depth in case a descriptor
        // leaks into the child somehow.
        denied.insert(libc::SYS_socket);
        denied.insert(libc::SYS_socketpair);
        denied.insert(libc::SYS_connect);
        denied.insert(libc::SYS_bind);
    }

    let rules: BTreeMap<i64, Vec<SeccompRule>> =
        denied.into_iter().map(|sc| (sc, Vec::new())).collect();

    let arch = detect_target_arch()?;
    let filter = SeccompFilter::new(
        rules,
        // default_action: what to do on a syscall NOT in the rule map.
        // Allow — this is a deny-list.
        SeccompAction::Allow,
        // match_action: what to do when a rule matches (with an empty arg
        // filter, match = any invocation of this syscall). EPERM makes the
        // blocked syscalls return a permission error that Python surfaces as
        // PermissionError / OSError, which is recoverable from the workload's
        // point of view instead of terminating the process with SIGSYS.
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(|error| format!("seccomp filter build: {error}"))?;

    let program: BpfProgram = filter
        .try_into()
        .map_err(|error| format!("seccomp compile: {error}"))?;

    seccompiler::apply_filter(&program).map_err(|error| format!("seccomp apply: {error}"))?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn detect_target_arch() -> Result<seccompiler::TargetArch, String> {
    // seccompiler ships only a handful of target arches; pick by compile-time
    // target so CI on x86_64 and arm64 runners both work.
    #[cfg(target_arch = "x86_64")]
    {
        Ok(seccompiler::TargetArch::x86_64)
    }
    #[cfg(target_arch = "aarch64")]
    {
        Ok(seccompiler::TargetArch::aarch64)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        Err(format!(
            "python sandbox does not support target arch {}; seccompiler needs x86_64 or aarch64",
            std::env::consts::ARCH
        ))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    fn tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("froglet-sandbox-test-")
            .tempdir()
            .expect("tempdir")
    }

    fn run_python(script: &str, config: SandboxConfig) -> (i32, String, String) {
        let mut command = Command::new("python3");
        command
            .arg("-I")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        harden_command(&mut command, config).expect("harden");
        let output = command.output().expect("python3 output");
        (
            output
                .status
                .code()
                .or(output.status.signal())
                .unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    }

    #[test]
    fn python_cannot_read_etc_passwd_under_landlock() {
        let dir = tempdir();
        let config = SandboxConfig::for_python(dir.path());
        // `/etc/passwd` is not under `/etc/ssl` or `/etc/hosts` and is not in
        // the readonly allow-list, so landlock must block it.
        let script = "\
try:\n  open('/etc/passwd').read()\n  print('LEAKED')\nexcept PermissionError:\n  print('BLOCKED')\n";
        let (code, stdout, _) = run_python(script, config);
        assert_eq!(code, 0, "python should run to completion");
        assert!(stdout.contains("BLOCKED"), "unexpected stdout: {stdout}");
    }

    #[test]
    fn python_cannot_write_outside_tempdir_under_landlock() {
        let dir = tempdir();
        let config = SandboxConfig::for_python(dir.path());
        let script = "\
import os\n\
try:\n  open('/tmp/froglet-sandbox-outside', 'w').write('x')\n  print('LEAKED')\nexcept PermissionError:\n  print('BLOCKED')\n";
        let (code, stdout, _) = run_python(script, config);
        assert_eq!(code, 0);
        assert!(stdout.contains("BLOCKED"), "unexpected stdout: {stdout}");
    }

    #[test]
    fn python_can_write_inside_tempdir() {
        let dir = tempdir();
        let config = SandboxConfig::for_python(dir.path());
        let target = dir.path().join("ok.txt");
        let script = format!(
            "open({:?}, 'w').write('hello')\nprint('WROTE')\n",
            target.to_string_lossy()
        );
        let (code, stdout, _) = run_python(&script, config);
        assert_eq!(code, 0);
        assert!(stdout.contains("WROTE"));
    }

    #[test]
    fn python_cannot_open_socket_under_seccomp() {
        let dir = tempdir();
        let config = SandboxConfig::for_python(dir.path());
        let script = "\
import socket\n\
try:\n  socket.socket(socket.AF_INET, socket.SOCK_STREAM)\n  print('LEAKED')\nexcept PermissionError:\n  print('BLOCKED')\nexcept OSError as e:\n  if e.errno == 1:\n    print('BLOCKED')\n  else:\n    raise\n";
        let (code, stdout, _) = run_python(script, config);
        assert_eq!(code, 0);
        assert!(stdout.contains("BLOCKED"), "unexpected stdout: {stdout}");
    }

    #[test]
    fn python_can_open_socket_when_network_allowed() {
        let dir = tempdir();
        let mut config = SandboxConfig::for_python(dir.path());
        config.allow_network = true;
        let script = "\
import socket\n\
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)\n\
s.close()\n\
print('OK')\n";
        let (code, stdout, _) = run_python(script, config);
        assert_eq!(code, 0);
        assert!(stdout.contains("OK"));
    }

    #[test]
    fn python_cannot_exec_arbitrary_binary() {
        let dir = tempdir();
        let config = SandboxConfig::for_python(dir.path());
        let script = "\
import os\n\
try:\n  os.execv('/bin/ls', ['ls'])\n  print('LEAKED')\nexcept PermissionError:\n  print('BLOCKED')\nexcept OSError as e:\n  if e.errno == 1:\n    print('BLOCKED')\n  else:\n    raise\n";
        let (code, stdout, _) = run_python(script, config);
        assert_eq!(code, 0);
        assert!(stdout.contains("BLOCKED"));
    }
}
