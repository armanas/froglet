//! Provider-neutral custody for Froglet's two long-lived signing identities.
//!
//! This module deliberately sits above the signed Kernel. It protects the
//! node and Nostr-publication seed files without changing signing bytes,
//! artifact shapes, or protocol transitions.

use crate::{canonical_json, crypto, identity};
use aes_gcm_siv::{
    Aes256GcmSiv, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

pub const BACKUP_SCHEMA_V1: &str = "froglet.identity-backup.v1";
pub const CONTINUITY_SCHEMA_V1: &str = "froglet.identity-continuity.v1";
pub const BACKUP_STATUS_SCHEMA_V1: &str = "froglet.identity-backup-status.v1";
const ROTATION_JOURNAL_SCHEMA_V1: &str = "froglet.identity-rotation-journal.v1";
const ROTATION_JOURNAL_FILE: &str = ".rotation-journal.json";
const ROTATION_NEXT_SEED_FILE: &str = ".rotation-next.seed";
const ROTATION_PREVIOUS_SEED_FILE: &str = ".rotation-previous.seed";
const RESTORE_JOURNAL_SCHEMA_V1: &str = "froglet.identity-restore-journal.v1";
const RESTORE_JOURNAL_FILE: &str = ".identity-restore-journal.json";
const IDENTITY_LOCK_FILE: &str = ".identity-custody.lock";
const RECOVERY_KEY_ALGORITHM: &str = "AES-256-GCM-SIV";
const OPERATOR_COMMAND_PROTOCOL: &str = "froglet-custody-command-v1";
const SECRET_PAYLOAD_MAGIC: &[u8; 16] = b"froglet-seeds-v1";
const SECRET_PAYLOAD_LEN: usize = SECRET_PAYLOAD_MAGIC.len() + 64;
const MAX_BACKUP_BYTES: u64 = 2 * 1024 * 1024;
const MAX_OPERATOR_OUTPUT_BYTES: u64 = 1024 * 1024;
const MAX_ROTATION_JOURNAL_BYTES: u64 = 128 * 1024;
const MAX_RESTORE_JOURNAL_BYTES: u64 = 128 * 1024;
const OPERATOR_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const OPERATOR_COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(5);
const OPERATOR_ENV_ALLOWLIST: &[&str] = &[
    "HOME",
    "PATH",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
];
type ProtectedSeed = Zeroizing<[u8; 32]>;

struct IdentitySeeds {
    node: ProtectedSeed,
    nostr_publication: ProtectedSeed,
}

#[derive(Debug, Clone)]
pub struct IdentityPaths {
    pub data_dir: PathBuf,
    pub identity_dir: PathBuf,
    pub node_seed_path: PathBuf,
    pub nostr_publication_seed_path: PathBuf,
}

impl From<&crate::config::NodeConfig> for IdentityPaths {
    fn from(config: &crate::config::NodeConfig) -> Self {
        Self {
            data_dir: config.storage.data_dir.clone(),
            identity_dir: config.storage.identity_dir.clone(),
            node_seed_path: config.storage.identity_seed_path.clone(),
            nostr_publication_seed_path: config.storage.nostr_publication_seed_path.clone(),
        }
    }
}

impl IdentityPaths {
    pub fn backup_status_path(&self) -> PathBuf {
        self.data_dir.join("identity-backup-status.json")
    }

    pub fn continuity_dir(&self) -> PathBuf {
        self.data_dir.join("identity-continuity")
    }

    fn rotation_journal_path(&self) -> PathBuf {
        self.identity_dir.join(ROTATION_JOURNAL_FILE)
    }

    fn rotation_next_seed_path(&self) -> PathBuf {
        self.identity_dir.join(ROTATION_NEXT_SEED_FILE)
    }

    fn rotation_previous_seed_path(&self) -> PathBuf {
        self.identity_dir.join(ROTATION_PREVIOUS_SEED_FILE)
    }

    fn restore_journal_path(&self) -> PathBuf {
        self.data_dir.join(RESTORE_JOURNAL_FILE)
    }

    fn identity_lock_path(&self) -> PathBuf {
        self.data_dir.join(IDENTITY_LOCK_FILE)
    }

    fn validate_layout(&self) -> Result<(), CustodyError> {
        if self.identity_dir.parent() != Some(self.data_dir.as_path())
            || self.node_seed_path.parent() != Some(self.identity_dir.as_path())
            || self.nostr_publication_seed_path.parent() != Some(self.identity_dir.as_path())
        {
            return Err(CustodyError::Unsupported(
                "identity restore and rotation require both configured seed files to be direct children of FROGLET_DATA_DIR/identity"
                    .to_string(),
            ));
        }
        if self.node_seed_path == self.nostr_publication_seed_path {
            return Err(CustodyError::Invalid(
                "node and Nostr publication seed paths must differ".to_string(),
            ));
        }
        Ok(())
    }

    pub fn validate_backup_destinations(
        &self,
        backup_path: &Path,
        recovery_key_path: Option<&Path>,
    ) -> Result<(), CustodyError> {
        self.validate_layout()?;
        let backup_status_path = self.backup_status_path();
        let restore_journal_path = self.restore_journal_path();
        let identity_lock_path = self.identity_lock_path();
        let reserved = [
            self.node_seed_path.as_path(),
            self.nostr_publication_seed_path.as_path(),
            backup_status_path.as_path(),
            restore_journal_path.as_path(),
            identity_lock_path.as_path(),
        ];
        if reserved
            .iter()
            .any(|reserved| paths_lexically_equal(backup_path, reserved))
        {
            return Err(CustodyError::Invalid(
                "backup output path conflicts with Froglet identity state".to_string(),
            ));
        }
        if let Some(recovery_key_path) = recovery_key_path
            && (paths_lexically_equal(backup_path, recovery_key_path)
                || reserved
                    .iter()
                    .any(|reserved| paths_lexically_equal(recovery_key_path, reserved)))
        {
            return Err(CustodyError::Invalid(
                "recovery-key path conflicts with the backup or Froglet identity state".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CustodyError {
    #[error("invalid identity custody input: {0}")]
    Invalid(String),
    #[error("identity custody authentication failed")]
    Authentication,
    #[error("identity custody operation is unsupported: {0}")]
    Unsupported(String),
    #[error("identity custody I/O failed: {0}")]
    Io(String),
    #[error("identity custody operator command failed: {0}")]
    Operator(String),
}

impl From<std::io::Error> for CustodyError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupProtection {
    RecoveryKeyFile {
        algorithm: String,
        nonce_base64: String,
    },
    OperatorCommand {
        protocol: String,
        key_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupCommonHeader {
    pub schema_version: String,
    pub created_at_unix: u64,
    pub node_id: String,
    pub nostr_publication_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct BackupHeader {
    #[serde(flatten)]
    common: BackupCommonHeader,
    protection: BackupProtection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityBackupBundle {
    pub schema_version: String,
    pub created_at_unix: u64,
    pub node_id: String,
    pub nostr_publication_id: String,
    pub protection: BackupProtection,
    pub ciphertext_base64: String,
}

impl IdentityBackupBundle {
    fn header(&self) -> BackupHeader {
        BackupHeader {
            common: BackupCommonHeader {
                schema_version: self.schema_version.clone(),
                created_at_unix: self.created_at_unix,
                node_id: self.node_id.clone(),
                nostr_publication_id: self.nostr_publication_id.clone(),
            },
            protection: self.protection.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct IdentityBackupReport {
    pub schema_version: &'static str,
    pub backup_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_key_path: Option<PathBuf>,
    pub backup_sha256: String,
    pub node_id: String,
    pub nostr_publication_id: String,
    pub protection: BackupProtection,
    pub recovery_instruction: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IdentityRestoreReport {
    pub schema_version: &'static str,
    pub node_id: String,
    pub nostr_publication_id: String,
    pub identity_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityContinuityPayload {
    pub schema_version: String,
    pub created_at_unix: u64,
    pub old_node_id: String,
    pub new_node_id: String,
    pub nostr_publication_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityContinuityRecord {
    pub payload: IdentityContinuityPayload,
    pub old_node_signature: String,
    pub new_node_signature: String,
    pub nostr_publication_signature: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IdentityRotationReport {
    pub schema_version: &'static str,
    pub old_node_id: String,
    pub new_node_id: String,
    pub nostr_publication_id: String,
    pub continuity_record_path: PathBuf,
    pub continuity_record_sha256: String,
    pub backup_status: &'static str,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct BackupStatusRecord {
    schema_version: String,
    created_at_unix: u64,
    node_id: String,
    nostr_publication_id: String,
    backup_path: PathBuf,
    backup_sha256: String,
    protection_kind: String,
}

/// Durable intent for a node-identity rotation.
///
/// The new seed is deliberately not embedded here. It remains in a separate
/// mode-0600 staging file so parsing or reporting the journal can never expose
/// secret material. The embedded continuity record authenticates both the old
/// and new public identities and lets recovery validate every file it sees.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct IdentityRotationJournal {
    schema_version: String,
    continuity_record_path: PathBuf,
    continuity_record_sha256: String,
    continuity_record: IdentityContinuityRecord,
}

/// Public, authenticated intent for an interrupted identity restore.
///
/// Secret seed bytes remain only in the private staging directory. Both
/// restored identities sign this payload before it becomes durable, so
/// recovery can reject a modified destination, status record, or identity
/// claim without requiring the backup adapter again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct IdentityRestoreIntent {
    schema_version: String,
    identity_dir: PathBuf,
    staging_dir: PathBuf,
    node_id: String,
    nostr_publication_id: String,
    status_record: BackupStatusRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct IdentityRestoreJournal {
    intent: IdentityRestoreIntent,
    node_signature: String,
    nostr_publication_signature: String,
}

#[derive(Debug, Default)]
pub struct IdentityRecoveryReport {
    pub restored_identity: Option<IdentityRestoreReport>,
    pub completed_rotation: Option<IdentityRotationReport>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackupStatusState {
    Current,
    Missing,
    StaleIdentity,
    BackupMissing,
    BackupDigestMismatch,
    InvalidRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupStatus {
    pub state: BackupStatusState,
    pub status_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_sha256: Option<String>,
}

pub struct SealedSecret {
    pub protection: BackupProtection,
    pub ciphertext: Vec<u8>,
}

/// Provider-neutral seam for encrypted-file, KMS, and HSM custody.
///
/// Implementations receive public authenticated metadata separately from the
/// fixed-length secret payload. They must perform authenticated sealing; a
/// decrypt-only or unauthenticated wrapper is not a conforming adapter.
pub trait CustodyAdapter {
    fn seal(
        &self,
        common_header: &BackupCommonHeader,
        plaintext: &[u8],
    ) -> Result<SealedSecret, CustodyError>;

    fn unseal(
        &self,
        bundle: &IdentityBackupBundle,
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CustodyError>;
}

pub struct RecoveryKeyFileAdapter {
    key: Zeroizing<[u8; 32]>,
}

impl RecoveryKeyFileAdapter {
    pub fn create(path: &Path) -> Result<Self, CustodyError> {
        let mut key = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(key.as_mut());
        atomic_write_new(path, key.as_ref(), 0o600)?;
        Ok(Self { key })
    }

    pub fn open(path: &Path) -> Result<Self, CustodyError> {
        reject_symlink(path, "recovery key")?;
        let file = File::open(path).map_err(|error| {
            CustodyError::Io(format!(
                "could not open recovery key {}: {error}",
                path.display()
            ))
        })?;
        require_private_file(&file, path)?;
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(33).read_to_end(&mut bytes).map_err(|error| {
            CustodyError::Io(format!(
                "could not read recovery key {}: {error}",
                path.display()
            ))
        })?;
        if bytes.len() != 32 {
            return Err(CustodyError::Invalid(format!(
                "recovery key {} must contain exactly 32 raw bytes",
                path.display()
            )));
        }
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&bytes);
        Ok(Self { key })
    }
}

impl CustodyAdapter for RecoveryKeyFileAdapter {
    fn seal(
        &self,
        common_header: &BackupCommonHeader,
        plaintext: &[u8],
    ) -> Result<SealedSecret, CustodyError> {
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let protection = BackupProtection::RecoveryKeyFile {
            algorithm: RECOVERY_KEY_ALGORITHM.to_string(),
            nonce_base64: BASE64.encode(nonce),
        };
        let header = BackupHeader {
            common: common_header.clone(),
            protection: protection.clone(),
        };
        let aad = backup_aad(&header)?;
        let cipher = Aes256GcmSiv::new_from_slice(self.key.as_ref())
            .map_err(|_| CustodyError::Invalid("invalid recovery key length".to_string()))?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| CustodyError::Authentication)?;
        nonce.zeroize();
        Ok(SealedSecret {
            protection,
            ciphertext,
        })
    }

    fn unseal(
        &self,
        bundle: &IdentityBackupBundle,
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CustodyError> {
        let header = bundle.header();
        let nonce = match &header.protection {
            BackupProtection::RecoveryKeyFile {
                algorithm,
                nonce_base64,
            } if algorithm == RECOVERY_KEY_ALGORITHM => {
                let decoded = BASE64
                    .decode(nonce_base64)
                    .map_err(|_| CustodyError::Authentication)?;
                if decoded.len() != 12 {
                    return Err(CustodyError::Authentication);
                }
                decoded
            }
            _ => {
                return Err(CustodyError::Invalid(
                    "backup does not use the recovery-key-file adapter".to_string(),
                ));
            }
        };
        let aad = backup_aad(&header)?;
        let cipher = Aes256GcmSiv::new_from_slice(self.key.as_ref())
            .map_err(|_| CustodyError::Invalid("invalid recovery key length".to_string()))?;
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| CustodyError::Authentication)
    }
}

/// Adapter for an operator-owned KMS/HSM wrapper executable.
///
/// No shell is involved. The absolute executable is invoked as
/// `PROGRAM seal|unseal --key-id ID`; binary input arrives on stdin, binary
/// output leaves on stdout, stderr is discarded, and public AAD is supplied in
/// `FROGLET_CUSTODY_AAD_BASE64`. Secret output is capped and zeroized.
pub struct OperatorCommandAdapter {
    program: PathBuf,
    key_id: String,
    timeout: Duration,
}

impl OperatorCommandAdapter {
    pub fn new(program: PathBuf, key_id: String) -> Result<Self, CustodyError> {
        if !program.is_absolute() {
            return Err(CustodyError::Invalid(
                "operator custody command must be an absolute path".to_string(),
            ));
        }
        let metadata = fs::metadata(&program).map_err(|error| {
            CustodyError::Invalid(format!(
                "operator custody command {} is unavailable: {error}",
                program.display()
            ))
        })?;
        if !metadata.is_file() {
            return Err(CustodyError::Invalid(format!(
                "operator custody command {} is not a regular file",
                program.display()
            )));
        }
        if key_id.trim().is_empty() || key_id.len() > 256 {
            return Err(CustodyError::Invalid(
                "operator custody key id must contain 1..=256 bytes".to_string(),
            ));
        }
        Ok(Self {
            program,
            key_id,
            timeout: OPERATOR_COMMAND_TIMEOUT,
        })
    }

    fn invoke(
        &self,
        operation: &str,
        input: &[u8],
        aad: &[u8],
        secret_output: bool,
    ) -> Result<Vec<u8>, CustodyError> {
        let mut command = Command::new(&self.program);
        command
            .arg(operation)
            .arg("--key-id")
            .arg(&self.key_id)
            .env_clear();
        for name in OPERATOR_ENV_ALLOWLIST {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("FROGLET_CUSTODY_AAD_BASE64", BASE64.encode(aad))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|_| {
            CustodyError::Operator("could not start configured adapter".to_string())
        })?;
        let process_group_id = child.id();
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| CustodyError::Operator("adapter stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CustodyError::Operator("adapter stdout unavailable".to_string()))?;
        let deadline = Instant::now() + self.timeout;

        let (status, write_result, read_result, terminal_error) = thread::scope(|scope| {
            let writer = scope.spawn(move || {
                let result = stdin.write_all(input);
                drop(stdin);
                result
            });
            let (reader_event_tx, reader_event_rx) = mpsc::sync_channel(1);
            let reader = scope.spawn(move || {
                let mut output = Vec::new();
                let read_result = stdout
                    .take(MAX_OPERATOR_OUTPUT_BYTES + 1)
                    .read_to_end(&mut output)
                    .map(|_| ());
                let event = match &read_result {
                    Ok(()) if output.len() as u64 > MAX_OPERATOR_OUTPUT_BYTES => {
                        OperatorReaderEvent::TooLarge
                    }
                    Ok(()) => OperatorReaderEvent::Complete,
                    Err(_) => OperatorReaderEvent::Failed,
                };
                let _ = reader_event_tx.send(event);
                (output, read_result)
            });

            let mut status = None;
            let mut reader_event = None;
            let terminal_error = loop {
                if reader_event.is_none() {
                    match reader_event_rx.try_recv() {
                        Ok(event) => reader_event = Some(event),
                        Err(mpsc::TryRecvError::Empty) => {}
                        Err(mpsc::TryRecvError::Disconnected) => {
                            break Some(CustodyError::Operator(
                                "adapter output worker failed".to_string(),
                            ));
                        }
                    }
                }
                if status.is_none() {
                    match child.try_wait() {
                        Ok(Some(exit_status)) => status = Some(exit_status),
                        Ok(None) => {}
                        Err(_) => {
                            break Some(CustodyError::Operator("adapter wait failed".to_string()));
                        }
                    }
                }
                match reader_event {
                    Some(OperatorReaderEvent::TooLarge) => {
                        break Some(CustodyError::Operator(
                            "adapter returned an invalid output length".to_string(),
                        ));
                    }
                    Some(OperatorReaderEvent::Failed) => {
                        break Some(CustodyError::Operator("adapter output failed".to_string()));
                    }
                    _ => {}
                }
                if status.is_some()
                    && reader_event == Some(OperatorReaderEvent::Complete)
                    && writer.is_finished()
                {
                    break None;
                }
                if Instant::now() >= deadline {
                    break Some(CustodyError::Operator(format!(
                        "adapter {operation} timed out after {} seconds",
                        self.timeout.as_secs_f64()
                    )));
                }
                thread::sleep(OPERATOR_COMMAND_POLL_INTERVAL);
            };

            if terminal_error.is_some() {
                let _ = terminate_operator_process_group(
                    &mut child,
                    process_group_id,
                    status.is_some(),
                );
            }
            let write_result = writer
                .join()
                .map_err(|_| CustodyError::Operator("adapter input worker failed".to_string()));
            let read_result = reader
                .join()
                .map_err(|_| CustodyError::Operator("adapter output worker failed".to_string()));
            (status, write_result, read_result, terminal_error)
        });

        let write_result = write_result?;
        let (thread_output, read_result) = read_result?;
        let mut output = thread_output;
        if let Some(error) = terminal_error {
            if secret_output {
                output.zeroize();
            }
            return Err(error);
        }
        write_result.map_err(|_| {
            if secret_output {
                output.zeroize();
            }
            CustodyError::Operator("adapter input failed".to_string())
        })?;
        read_result.map_err(|_| {
            if secret_output {
                output.zeroize();
            }
            CustodyError::Operator("adapter output failed".to_string())
        })?;
        let status = status.ok_or_else(|| {
            CustodyError::Operator("adapter terminated without an exit status".to_string())
        })?;
        if !status.success() {
            if secret_output {
                output.zeroize();
            }
            return Err(CustodyError::Operator(format!(
                "adapter {operation} exited unsuccessfully"
            )));
        }
        if output.is_empty() || output.len() as u64 > MAX_OPERATOR_OUTPUT_BYTES {
            if secret_output {
                output.zeroize();
            }
            return Err(CustodyError::Operator(
                "adapter returned an invalid output length".to_string(),
            ));
        }
        Ok(output)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperatorReaderEvent {
    Complete,
    TooLarge,
    Failed,
}

fn terminate_operator_process_group(
    child: &mut std::process::Child,
    process_group_id: u32,
    child_already_reaped: bool,
) -> Result<(), CustodyError> {
    #[cfg(unix)]
    {
        let process_group_id = i32::try_from(process_group_id).map_err(|_| {
            CustodyError::Operator("adapter process identifier is invalid".to_string())
        })?;
        // SAFETY: a negative, non-zero pid asks `kill(2)` to signal the
        // process group created for this child. No Rust memory is referenced.
        let result = unsafe { libc::kill(-process_group_id, libc::SIGKILL) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(CustodyError::Operator(
                    "could not terminate timed-out adapter".to_string(),
                ));
            }
        }
    }
    #[cfg(not(unix))]
    if !child_already_reaped {
        child.kill().map_err(|_| {
            CustodyError::Operator("could not terminate timed-out adapter".to_string())
        })?;
    }
    if !child_already_reaped {
        child
            .wait()
            .map_err(|_| CustodyError::Operator("could not reap terminated adapter".to_string()))?;
    }
    Ok(())
}

impl CustodyAdapter for OperatorCommandAdapter {
    fn seal(
        &self,
        common_header: &BackupCommonHeader,
        plaintext: &[u8],
    ) -> Result<SealedSecret, CustodyError> {
        let protection = BackupProtection::OperatorCommand {
            protocol: OPERATOR_COMMAND_PROTOCOL.to_string(),
            key_id: self.key_id.clone(),
        };
        let header = BackupHeader {
            common: common_header.clone(),
            protection: protection.clone(),
        };
        let aad = backup_aad(&header)?;
        let ciphertext = self.invoke("seal", plaintext, &aad, false)?;
        Ok(SealedSecret {
            protection,
            ciphertext,
        })
    }

    fn unseal(
        &self,
        bundle: &IdentityBackupBundle,
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CustodyError> {
        let header = bundle.header();
        match &header.protection {
            BackupProtection::OperatorCommand { protocol, key_id }
                if protocol == OPERATOR_COMMAND_PROTOCOL && key_id == &self.key_id => {}
            _ => {
                return Err(CustodyError::Invalid(
                    "backup does not match the configured operator-command adapter".to_string(),
                ));
            }
        }
        let aad = backup_aad(&header)?;
        let output = self.invoke("unseal", ciphertext, &aad, true)?;
        Ok(Zeroizing::new(output))
    }
}

/// OS keychain support is intentionally explicit rather than emulated with a
/// plaintext file or shell invocation.
pub fn os_keychain_capability() -> Result<(), CustodyError> {
    Err(CustodyError::Unsupported(
        "OS keychain adapter is not implemented; use a recovery-key file or an authenticated operator-command KMS/HSM adapter"
            .to_string(),
    ))
}

/// Recover every durable identity operation before loading or generating a
/// live key. Holding the custody lock through `load_or_create_without_recovery`
/// prevents another restore or rotation from exposing an intermediate state.
pub(crate) fn load_or_create_recovered_identity(
    config: &crate::config::NodeConfig,
) -> Result<crate::identity::NodeIdentity, String> {
    let paths = IdentityPaths::from(config);
    ensure_custody_data_dir(&paths, config.storage.data_dir_mode())
        .map_err(|error| format!("failed to prepare identity custody: {error}"))?;
    let _lock = IdentityLock::acquire(&paths.identity_lock_path())
        .map_err(|error| format!("failed to lock identity custody: {error}"))?;
    recover_pending_identity_state_locked(&paths)
        .map_err(|error| format!("failed to recover pending identity operation: {error}"))?;
    crate::identity::NodeIdentity::load_or_create_without_recovery(config)
}

/// Recover a journaled restore first, then a journaled rotation. Restore must
/// win because it owns the entire identity directory; rotation state is only
/// meaningful after that directory is present and authenticated.
pub fn recover_pending_identity_state(
    paths: &IdentityPaths,
) -> Result<IdentityRecoveryReport, CustodyError> {
    paths.validate_layout()?;
    ensure_custody_data_dir(paths, 0o700)?;
    let _lock = IdentityLock::acquire(&paths.identity_lock_path())?;
    recover_pending_identity_state_locked(paths)
}

fn recover_pending_identity_state_locked(
    paths: &IdentityPaths,
) -> Result<IdentityRecoveryReport, CustodyError> {
    let restored_identity = recover_pending_identity_restore_locked(paths)?;
    let completed_rotation = if path_entry_exists(&paths.identity_dir)? {
        require_private_directory(&paths.identity_dir)?;
        recover_pending_identity_rotation_locked(paths)?
    } else {
        None
    };
    Ok(IdentityRecoveryReport {
        restored_identity,
        completed_rotation,
    })
}

pub fn create_backup(
    paths: &IdentityPaths,
    backup_path: &Path,
    adapter: &dyn CustodyAdapter,
    recovery_key_path: Option<PathBuf>,
) -> Result<IdentityBackupReport, CustodyError> {
    paths.validate_layout()?;
    ensure_custody_data_dir(paths, 0o700)?;
    let _lock = IdentityLock::acquire(&paths.identity_lock_path())?;
    recover_pending_identity_state_locked(paths)?;
    create_backup_locked(paths, backup_path, adapter, recovery_key_path)
}

fn create_backup_locked(
    paths: &IdentityPaths,
    backup_path: &Path,
    adapter: &dyn CustodyAdapter,
    recovery_key_path: Option<PathBuf>,
) -> Result<IdentityBackupReport, CustodyError> {
    paths.validate_backup_destinations(backup_path, recovery_key_path.as_deref())?;
    let node_key = identity::load_signing_key(&paths.node_seed_path).map_err(CustodyError::Io)?;
    let nostr_key =
        identity::load_signing_key(&paths.nostr_publication_seed_path).map_err(CustodyError::Io)?;
    let node_id = crypto::public_key_hex(&node_key);
    let nostr_publication_id = crypto::public_key_hex(&nostr_key);
    let created_at_unix = unix_now()?;

    let common_header = BackupCommonHeader {
        schema_version: BACKUP_SCHEMA_V1.to_string(),
        created_at_unix,
        node_id: node_id.clone(),
        nostr_publication_id: nostr_publication_id.clone(),
    };
    let plaintext = encode_secret_payload(&node_key, &nostr_key);
    let SealedSecret {
        protection,
        ciphertext,
    } = adapter.seal(&common_header, plaintext.as_slice())?;
    let ciphertext = Zeroizing::new(ciphertext);
    if ciphertext_exposes_secret(&ciphertext, &plaintext) {
        return Err(CustodyError::Authentication);
    }

    let bundle = IdentityBackupBundle {
        schema_version: BACKUP_SCHEMA_V1.to_string(),
        created_at_unix,
        node_id: node_id.clone(),
        nostr_publication_id: nostr_publication_id.clone(),
        protection: protection.clone(),
        ciphertext_base64: BASE64.encode(ciphertext.as_slice()),
    };
    validate_bundle_shape(&bundle)?;
    let recovered = adapter.unseal(&bundle, ciphertext.as_slice())?;
    verify_recovered_identity_payload(&recovered, &plaintext, &node_id, &nostr_publication_id)?;
    let encoded = canonical_json::to_vec(&bundle)
        .map_err(|error| CustodyError::Invalid(format!("backup serialization failed: {error}")))?;
    atomic_write_new(backup_path, &encoded, 0o600)?;
    let backup_sha256 = crypto::sha256_hex(&encoded);
    let status_record = BackupStatusRecord {
        schema_version: BACKUP_STATUS_SCHEMA_V1.to_string(),
        created_at_unix,
        node_id: node_id.clone(),
        nostr_publication_id: nostr_publication_id.clone(),
        backup_path: absolute_path(backup_path)?,
        backup_sha256: backup_sha256.clone(),
        protection_kind: protection_kind(&protection).to_string(),
    };
    let status_bytes = canonical_json::to_vec(&status_record).map_err(|error| {
        CustodyError::Invalid(format!("backup status serialization failed: {error}"))
    })?;
    atomic_write_replace(&paths.backup_status_path(), &status_bytes, 0o600)?;

    Ok(IdentityBackupReport {
        schema_version: BACKUP_SCHEMA_V1,
        backup_path: absolute_path(backup_path)?,
        recovery_key_path,
        backup_sha256,
        node_id,
        nostr_publication_id,
        protection,
        recovery_instruction: "Store the backup and its recovery key in separate secure locations; Froglet never prints recovery-key bytes."
            .to_string(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreCheckpoint {
    StagingSynced,
    JournalWritten,
    StatusWritten,
    IdentityInstalled,
    JournalRemoved,
}

pub fn restore_backup(
    paths: &IdentityPaths,
    backup_path: &Path,
    adapter: &dyn CustodyAdapter,
) -> Result<IdentityRestoreReport, CustodyError> {
    restore_backup_with_checkpoints(paths, backup_path, adapter, |_| Ok(()))
}

fn restore_backup_with_checkpoints(
    paths: &IdentityPaths,
    backup_path: &Path,
    adapter: &dyn CustodyAdapter,
    mut checkpoint: impl FnMut(RestoreCheckpoint) -> Result<(), CustodyError>,
) -> Result<IdentityRestoreReport, CustodyError> {
    paths.validate_layout()?;
    ensure_custody_data_dir(paths, 0o700)?;
    let _lock = IdentityLock::acquire(&paths.identity_lock_path())?;
    recover_pending_identity_state_locked(paths)?;

    let status_path = paths.backup_status_path();
    let restore_journal_path = paths.restore_journal_path();
    let identity_lock_path = paths.identity_lock_path();
    if [
        status_path.as_path(),
        restore_journal_path.as_path(),
        identity_lock_path.as_path(),
    ]
    .iter()
    .any(|reserved| paths_lexically_equal(backup_path, reserved))
    {
        return Err(CustodyError::Invalid(
            "backup path conflicts with protected identity restore state".to_string(),
        ));
    }
    if path_entry_exists(&paths.identity_dir)?
        || path_entry_exists(&paths.node_seed_path)?
        || path_entry_exists(&paths.nostr_publication_seed_path)?
    {
        return Err(CustodyError::Invalid(format!(
            "restore target {} already exists; refusing to overwrite any identity material",
            paths.identity_dir.display()
        )));
    }

    reject_symlink(backup_path, "identity backup")?;
    let encoded = read_limited(backup_path, MAX_BACKUP_BYTES)?;
    let bundle: IdentityBackupBundle = serde_json::from_slice(&encoded)
        .map_err(|_| CustodyError::Invalid("backup is not a valid v1 bundle".to_string()))?;
    validate_bundle_shape(&bundle)?;
    let ciphertext =
        Zeroizing::new(BASE64.decode(&bundle.ciphertext_base64).map_err(|_| {
            CustodyError::Invalid("backup ciphertext is not valid base64".to_string())
        })?);
    if ciphertext.is_empty() || ciphertext.len() as u64 > MAX_OPERATOR_OUTPUT_BYTES {
        return Err(CustodyError::Invalid(
            "backup ciphertext length is invalid".to_string(),
        ));
    }
    let plaintext = adapter.unseal(&bundle, &ciphertext)?;
    if ciphertext_exposes_secret(&ciphertext, &plaintext) {
        return Err(CustodyError::Authentication);
    }
    let IdentitySeeds {
        node: node_seed,
        nostr_publication: nostr_seed,
    } = decode_secret_payload(&plaintext)?;
    let node_key = crypto::signing_key_from_seed_bytes(&node_seed)
        .map_err(|_| CustodyError::Authentication)?;
    let nostr_key = crypto::signing_key_from_seed_bytes(&nostr_seed)
        .map_err(|_| CustodyError::Authentication)?;
    if crypto::public_key_hex(&node_key) != bundle.node_id
        || crypto::public_key_hex(&nostr_key) != bundle.nostr_publication_id
    {
        return Err(CustodyError::Authentication);
    }

    let staging_dir = absolute_path(&unique_sibling(&paths.identity_dir, "restore"))?;
    if let Err(error) = create_restore_staging_dir(paths, &staging_dir, &node_seed, &nostr_seed) {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(error);
    }
    sync_directory(&paths.data_dir)?;
    checkpoint(RestoreCheckpoint::StagingSynced)?;

    let status_record = BackupStatusRecord {
        schema_version: BACKUP_STATUS_SCHEMA_V1.to_string(),
        created_at_unix: bundle.created_at_unix,
        node_id: bundle.node_id.clone(),
        nostr_publication_id: bundle.nostr_publication_id.clone(),
        backup_path: absolute_path(backup_path)?,
        backup_sha256: crypto::sha256_hex(&encoded),
        protection_kind: protection_kind(&bundle.protection).to_string(),
    };
    let intent = IdentityRestoreIntent {
        schema_version: RESTORE_JOURNAL_SCHEMA_V1.to_string(),
        identity_dir: absolute_path(&paths.identity_dir)?,
        staging_dir: staging_dir.clone(),
        node_id: bundle.node_id.clone(),
        nostr_publication_id: bundle.nostr_publication_id.clone(),
        status_record,
    };
    let intent_bytes = restore_intent_bytes(&intent)?;
    let journal = IdentityRestoreJournal {
        node_signature: crypto::sign_message_hex(&node_key, &intent_bytes),
        nostr_publication_signature: crypto::sign_message_hex(&nostr_key, &intent_bytes),
        intent,
    };
    verify_restore_journal(paths, &journal)?;
    if let Err(error) = install_restore_journal(&paths.restore_journal_path(), &journal) {
        let _ = fs::remove_dir_all(&staging_dir);
        let _ = sync_directory(&paths.data_dir);
        return Err(error);
    }
    checkpoint(RestoreCheckpoint::JournalWritten)?;

    ensure_restore_status(paths, &journal)?;
    checkpoint(RestoreCheckpoint::StatusWritten)?;
    activate_restored_identity(paths, &journal)?;
    checkpoint(RestoreCheckpoint::IdentityInstalled)?;
    remove_file_if_exists(&paths.restore_journal_path())?;
    sync_directory(&paths.data_dir)?;
    checkpoint(RestoreCheckpoint::JournalRemoved)?;

    restore_report(paths, &journal)
}

fn recover_pending_identity_restore_locked(
    paths: &IdentityPaths,
) -> Result<Option<IdentityRestoreReport>, CustodyError> {
    let journal_path = paths.restore_journal_path();
    if !path_entry_exists(&journal_path)? {
        cleanup_unjournaled_restore_directories(paths)?;
        return Ok(None);
    }

    let journal = read_restore_journal(paths, &journal_path)?;
    let identity_exists = path_entry_exists(&paths.identity_dir)?;
    let staging_exists = path_entry_exists(&journal.intent.staging_dir)?;
    match (identity_exists, staging_exists) {
        (false, true) => {
            require_staged_restore_identity(paths, &journal)?;
            ensure_restore_status(paths, &journal)?;
            activate_restored_identity(paths, &journal)?;
        }
        (true, false) => {
            require_installed_restore_identity(paths, &journal)?;
            ensure_restore_status(paths, &journal)?;
        }
        (true, true) => {
            return Err(CustodyError::Invalid(
                "pending restore has both staging and installed identity directories; refusing ambiguous recovery"
                    .to_string(),
            ));
        }
        (false, false) => {
            return Err(CustodyError::Invalid(
                "pending restore has lost both its staging and installed identity directories"
                    .to_string(),
            ));
        }
    }

    remove_file_if_exists(&journal_path)?;
    sync_directory(&paths.data_dir)?;
    restore_report(paths, &journal).map(Some)
}

fn restore_intent_bytes(intent: &IdentityRestoreIntent) -> Result<Vec<u8>, CustodyError> {
    canonical_json::to_vec(intent).map_err(|error| {
        CustodyError::Invalid(format!("restore intent serialization failed: {error}"))
    })
}

fn verify_restore_journal(
    paths: &IdentityPaths,
    journal: &IdentityRestoreJournal,
) -> Result<(), CustodyError> {
    let intent = &journal.intent;
    let expected_identity_dir = absolute_path(&paths.identity_dir)?;
    let expected_data_dir = absolute_path(&paths.data_dir)?;
    if intent.schema_version != RESTORE_JOURNAL_SCHEMA_V1
        || !paths_lexically_equal(&intent.identity_dir, &expected_identity_dir)
        || !intent.staging_dir.is_absolute()
        || intent.staging_dir.parent() != Some(expected_data_dir.as_path())
        || !is_restore_staging_name(paths, &intent.staging_dir)
        || intent.node_id.len() != 64
        || intent.nostr_publication_id.len() != 64
        || hex::decode(&intent.node_id).is_err()
        || hex::decode(&intent.nostr_publication_id).is_err()
    {
        return Err(CustodyError::Invalid(
            "identity restore journal metadata is invalid".to_string(),
        ));
    }
    let status = &intent.status_record;
    if status.schema_version != BACKUP_STATUS_SCHEMA_V1
        || status.created_at_unix == 0
        || status.node_id != intent.node_id
        || status.nostr_publication_id != intent.nostr_publication_id
        || !status.backup_path.is_absolute()
        || status.backup_sha256.len() != 64
        || hex::decode(&status.backup_sha256).is_err()
        || !matches!(
            status.protection_kind.as_str(),
            "recovery_key_file" | "operator_command"
        )
    {
        return Err(CustodyError::Invalid(
            "identity restore journal status is invalid".to_string(),
        ));
    }
    let intent_bytes = restore_intent_bytes(intent)?;
    if !crypto::verify_message(&intent.node_id, &journal.node_signature, &intent_bytes)
        || !crypto::verify_message(
            &intent.nostr_publication_id,
            &journal.nostr_publication_signature,
            &intent_bytes,
        )
    {
        return Err(CustodyError::Authentication);
    }
    Ok(())
}

fn install_restore_journal(
    path: &Path,
    journal: &IdentityRestoreJournal,
) -> Result<(), CustodyError> {
    let bytes = canonical_json::to_vec(journal).map_err(|error| {
        CustodyError::Invalid(format!("restore journal serialization failed: {error}"))
    })?;
    atomic_write_new(path, &bytes, 0o600)
}

fn read_restore_journal(
    paths: &IdentityPaths,
    path: &Path,
) -> Result<IdentityRestoreJournal, CustodyError> {
    reject_symlink(path, "identity restore journal")?;
    let file = File::open(path).map_err(|error| {
        CustodyError::Io(format!(
            "could not open identity restore journal {}: {error}",
            path.display()
        ))
    })?;
    require_private_file(&file, path)?;
    drop(file);
    let bytes = read_limited(path, MAX_RESTORE_JOURNAL_BYTES)?;
    let journal: IdentityRestoreJournal = serde_json::from_slice(&bytes)
        .map_err(|_| CustodyError::Invalid("identity restore journal is invalid".to_string()))?;
    verify_restore_journal(paths, &journal)?;
    Ok(journal)
}

fn ensure_restore_status(
    paths: &IdentityPaths,
    journal: &IdentityRestoreJournal,
) -> Result<(), CustodyError> {
    let path = paths.backup_status_path();
    let expected = canonical_json::to_vec(&journal.intent.status_record).map_err(|error| {
        CustodyError::Invalid(format!("restore status serialization failed: {error}"))
    })?;
    if path_entry_exists(&path)? {
        reject_symlink(&path, "identity backup status")?;
        let file = File::open(&path).map_err(CustodyError::from)?;
        require_private_file(&file, &path)?;
        drop(file);
        if read_limited(&path, 64 * 1024)? == expected {
            sync_directory(&paths.data_dir)?;
            return Ok(());
        }
    }
    atomic_write_replace(&path, &expected, 0o600)
}

fn activate_restored_identity(
    paths: &IdentityPaths,
    journal: &IdentityRestoreJournal,
) -> Result<(), CustodyError> {
    if path_entry_exists(&paths.identity_dir)? {
        return require_installed_restore_identity(paths, journal);
    }
    require_staged_restore_identity(paths, journal)?;
    fs::rename(&journal.intent.staging_dir, &paths.identity_dir).map_err(|error| {
        CustodyError::Io(format!(
            "could not atomically install restored identity directory: {error}"
        ))
    })?;
    sync_directory(&paths.data_dir)?;
    require_installed_restore_identity(paths, journal)
}

fn require_staged_restore_identity(
    paths: &IdentityPaths,
    journal: &IdentityRestoreJournal,
) -> Result<(), CustodyError> {
    require_private_directory(&journal.intent.staging_dir)?;
    let node_name = paths.node_seed_path.file_name().ok_or_else(|| {
        CustodyError::Invalid("node identity seed path has no filename".to_string())
    })?;
    let nostr_name = paths
        .nostr_publication_seed_path
        .file_name()
        .ok_or_else(|| {
            CustodyError::Invalid("Nostr publication seed path has no filename".to_string())
        })?;
    require_seed_identity(
        &journal.intent.staging_dir.join(node_name),
        &journal.intent.node_id,
        "staged restored node identity",
    )?;
    require_seed_identity(
        &journal.intent.staging_dir.join(nostr_name),
        &journal.intent.nostr_publication_id,
        "staged restored Nostr publication identity",
    )
}

fn require_installed_restore_identity(
    paths: &IdentityPaths,
    journal: &IdentityRestoreJournal,
) -> Result<(), CustodyError> {
    require_private_directory(&paths.identity_dir)?;
    require_seed_identity(
        &paths.node_seed_path,
        &journal.intent.node_id,
        "restored node identity",
    )?;
    require_seed_identity(
        &paths.nostr_publication_seed_path,
        &journal.intent.nostr_publication_id,
        "restored Nostr publication identity",
    )
}

fn restore_report(
    paths: &IdentityPaths,
    journal: &IdentityRestoreJournal,
) -> Result<IdentityRestoreReport, CustodyError> {
    require_installed_restore_identity(paths, journal)?;
    Ok(IdentityRestoreReport {
        schema_version: BACKUP_SCHEMA_V1,
        node_id: journal.intent.node_id.clone(),
        nostr_publication_id: journal.intent.nostr_publication_id.clone(),
        identity_dir: paths.identity_dir.clone(),
    })
}

fn is_restore_staging_name(paths: &IdentityPaths, path: &Path) -> bool {
    let identity_name = paths
        .identity_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("froglet-identity");
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(&format!(".{identity_name}.restore.")))
}

fn cleanup_unjournaled_restore_directories(paths: &IdentityPaths) -> Result<(), CustodyError> {
    let entries = match fs::read_dir(&paths.data_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(CustodyError::Io(error.to_string())),
    };
    for entry in entries {
        let entry = entry.map_err(CustodyError::from)?;
        let path = entry.path();
        if !is_restore_staging_name(paths, &path) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(CustodyError::from)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CustodyError::Invalid(format!(
                "orphaned restore state {} is not a directory",
                path.display()
            )));
        }
        fs::remove_dir_all(&path).map_err(|error| {
            CustodyError::Io(format!(
                "could not remove unjournaled restore staging directory {}: {error}",
                path.display()
            ))
        })?;
    }
    sync_directory(&paths.data_dir)
}

pub fn backup_status(paths: &IdentityPaths) -> BackupStatus {
    let status_path = paths.backup_status_path();
    let missing = || BackupStatus {
        state: BackupStatusState::Missing,
        status_path: status_path.clone(),
        backup_path: None,
        backup_sha256: None,
    };
    match fs::symlink_metadata(&status_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return missing(),
        Err(_) => {
            return BackupStatus {
                state: BackupStatusState::InvalidRecord,
                ..missing()
            };
        }
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return BackupStatus {
                state: BackupStatusState::InvalidRecord,
                ..missing()
            };
        }
        Ok(_) => {}
    }
    let Ok(bytes) = read_private_limited(&status_path, 64 * 1024, "identity backup status") else {
        return BackupStatus {
            state: BackupStatusState::InvalidRecord,
            ..missing()
        };
    };
    let Ok(record) = serde_json::from_slice::<BackupStatusRecord>(&bytes) else {
        return BackupStatus {
            state: BackupStatusState::InvalidRecord,
            ..missing()
        };
    };
    if record.schema_version != BACKUP_STATUS_SCHEMA_V1
        || record.created_at_unix == 0
        || !record.backup_path.is_absolute()
        || !is_lower_hex_64(&record.node_id)
        || !is_lower_hex_64(&record.nostr_publication_id)
        || !is_lower_hex_64(&record.backup_sha256)
        || !matches!(
            record.protection_kind.as_str(),
            "recovery_key_file" | "operator_command"
        )
    {
        return BackupStatus {
            state: BackupStatusState::InvalidRecord,
            backup_path: Some(record.backup_path),
            backup_sha256: Some(record.backup_sha256),
            status_path,
        };
    }
    let base = BackupStatus {
        state: BackupStatusState::Current,
        status_path,
        backup_path: Some(record.backup_path.clone()),
        backup_sha256: Some(record.backup_sha256.clone()),
    };
    let (Ok(node_key), Ok(nostr_key)) = (
        identity::load_signing_key(&paths.node_seed_path),
        identity::load_signing_key(&paths.nostr_publication_seed_path),
    ) else {
        return BackupStatus {
            state: BackupStatusState::StaleIdentity,
            ..base
        };
    };
    if crypto::public_key_hex(&node_key) != record.node_id
        || crypto::public_key_hex(&nostr_key) != record.nostr_publication_id
    {
        return BackupStatus {
            state: BackupStatusState::StaleIdentity,
            ..base
        };
    }
    let backup_metadata = match fs::symlink_metadata(&record.backup_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return BackupStatus {
                state: BackupStatusState::BackupMissing,
                ..base
            };
        }
        Err(_) => {
            return BackupStatus {
                state: BackupStatusState::InvalidRecord,
                ..base
            };
        }
    };
    if !backup_metadata.is_file() || backup_metadata.file_type().is_symlink() {
        return BackupStatus {
            state: BackupStatusState::InvalidRecord,
            ..base
        };
    }
    let Ok(backup_bytes) =
        read_private_limited(&record.backup_path, MAX_BACKUP_BYTES, "identity backup")
    else {
        return BackupStatus {
            state: BackupStatusState::InvalidRecord,
            ..base
        };
    };
    if crypto::sha256_hex(&backup_bytes) != record.backup_sha256 {
        return BackupStatus {
            state: BackupStatusState::BackupDigestMismatch,
            ..base
        };
    }
    let Ok(bundle) = serde_json::from_slice::<IdentityBackupBundle>(&backup_bytes) else {
        return BackupStatus {
            state: BackupStatusState::InvalidRecord,
            ..base
        };
    };
    if validate_bundle_shape(&bundle).is_err()
        || bundle.created_at_unix != record.created_at_unix
        || bundle.node_id != record.node_id
        || bundle.nostr_publication_id != record.nostr_publication_id
        || protection_kind(&bundle.protection) != record.protection_kind
        || !canonical_json::to_vec(&bundle)
            .is_ok_and(|canonical| canonical.as_slice() == backup_bytes.as_slice())
    {
        return BackupStatus {
            state: BackupStatusState::InvalidRecord,
            ..base
        };
    }
    let Ok(ciphertext) = BASE64.decode(&bundle.ciphertext_base64) else {
        return BackupStatus {
            state: BackupStatusState::InvalidRecord,
            ..base
        };
    };
    if ciphertext.is_empty() || ciphertext.len() as u64 > MAX_OPERATOR_OUTPUT_BYTES {
        return BackupStatus {
            state: BackupStatusState::InvalidRecord,
            ..base
        };
    }
    base
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RotationCheckpoint {
    NextSeedDurable,
    PreviousSeedDurable,
    JournalDurable,
    ContinuityRecordDurable,
    CanonicalSeedReplaced,
    CanonicalSeedDurable,
    PreviousSeedRemoved,
}

pub fn rotate_node_identity(
    paths: &IdentityPaths,
    adapter: &dyn CustodyAdapter,
    continuity_record_path: Option<&Path>,
    reason: &str,
) -> Result<IdentityRotationReport, CustodyError> {
    rotate_node_identity_with_checkpoints(
        paths,
        adapter,
        continuity_record_path,
        reason,
        |_| Ok(()),
    )
}

/// Completes or rolls back a journaled rotation after an interrupted process.
///
/// Recovery does not require custody credentials: the journal is only made
/// durable after the caller has proved live recovery of the current backup,
/// and its continuity record authenticates the old, new, and Nostr identities.
pub fn recover_pending_identity_rotation(
    paths: &IdentityPaths,
) -> Result<Option<IdentityRotationReport>, CustodyError> {
    Ok(recover_pending_identity_state(paths)?.completed_rotation)
}

fn rotate_node_identity_with_checkpoints(
    paths: &IdentityPaths,
    adapter: &dyn CustodyAdapter,
    continuity_record_path: Option<&Path>,
    reason: &str,
    mut checkpoint: impl FnMut(RotationCheckpoint) -> Result<(), CustodyError>,
) -> Result<IdentityRotationReport, CustodyError> {
    paths.validate_layout()?;
    if reason.trim().is_empty() || reason.len() > 512 {
        return Err(CustodyError::Invalid(
            "rotation reason must contain 1..=512 bytes".to_string(),
        ));
    }

    ensure_custody_data_dir(paths, 0o700)?;
    let _lock = IdentityLock::acquire(&paths.identity_lock_path())?;
    if let Some(report) = recover_pending_identity_state_locked(paths)?.completed_rotation {
        return Ok(report);
    }
    if backup_status(paths).state != BackupStatusState::Current {
        return Err(CustodyError::Invalid(
            "current node and Nostr publication identities require a verified backup before rotation"
                .to_string(),
        ));
    }

    reject_symlink(&paths.node_seed_path, "node identity seed")?;
    let old_node_key =
        identity::load_signing_key(&paths.node_seed_path).map_err(CustodyError::Io)?;
    let nostr_key =
        identity::load_signing_key(&paths.nostr_publication_seed_path).map_err(CustodyError::Io)?;
    verify_current_backup_for_rotation(paths, adapter, &old_node_key, &nostr_key)?;

    let new_node_key = crypto::generate_signing_key();
    let old_node_id = crypto::public_key_hex(&old_node_key);
    let new_node_id = crypto::public_key_hex(&new_node_key);
    let nostr_publication_id = crypto::public_key_hex(&nostr_key);
    let created_at_unix = unix_now()?;
    let payload = IdentityContinuityPayload {
        schema_version: CONTINUITY_SCHEMA_V1.to_string(),
        created_at_unix,
        old_node_id: old_node_id.clone(),
        new_node_id: new_node_id.clone(),
        nostr_publication_id: nostr_publication_id.clone(),
        reason: reason.trim().to_string(),
    };
    let payload_bytes = canonical_json::to_vec(&payload).map_err(|error| {
        CustodyError::Invalid(format!("continuity payload serialization failed: {error}"))
    })?;
    let record = IdentityContinuityRecord {
        old_node_signature: crypto::sign_message_hex(&old_node_key, &payload_bytes),
        new_node_signature: crypto::sign_message_hex(&new_node_key, &payload_bytes),
        nostr_publication_signature: crypto::sign_message_hex(&nostr_key, &payload_bytes),
        payload,
    };
    verify_continuity_record(&record)?;
    let record_bytes = canonical_json::to_vec(&record).map_err(|error| {
        CustodyError::Invalid(format!("continuity record serialization failed: {error}"))
    })?;
    let record_path = absolute_path(
        &continuity_record_path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                paths.continuity_dir().join(format!(
                    "{}-{}-{}.json",
                    created_at_unix,
                    &old_node_id[..12],
                    &new_node_id[..12]
                ))
            }),
    )?;
    validate_rotation_record_path(paths, &record_path)?;
    if path_entry_exists(&record_path)? {
        return Err(CustodyError::Invalid(format!(
            "continuity record {} already exists",
            record_path.display()
        )));
    }

    let next_seed_path = paths.rotation_next_seed_path();
    let previous_seed_path = paths.rotation_previous_seed_path();
    let journal_path = paths.rotation_journal_path();
    let new_seed = Zeroizing::new(crypto::signing_key_seed_bytes(&new_node_key));
    write_seed_new(&next_seed_path, &new_seed)?;
    checkpoint(RotationCheckpoint::NextSeedDurable)?;

    fs::hard_link(&paths.node_seed_path, &previous_seed_path).map_err(|error| {
        CustodyError::Io(format!(
            "could not preserve the previous node identity at {}: {error}",
            previous_seed_path.display()
        ))
    })?;
    sync_directory(&paths.identity_dir)?;
    checkpoint(RotationCheckpoint::PreviousSeedDurable)?;

    let journal = IdentityRotationJournal {
        schema_version: ROTATION_JOURNAL_SCHEMA_V1.to_string(),
        continuity_record_path: record_path.clone(),
        continuity_record_sha256: crypto::sha256_hex(&record_bytes),
        continuity_record: record,
    };
    install_rotation_journal(&journal_path, &journal)?;
    checkpoint(RotationCheckpoint::JournalDurable)?;

    ensure_continuity_record(&journal)?;
    checkpoint(RotationCheckpoint::ContinuityRecordDurable)?;

    fs::rename(&next_seed_path, &paths.node_seed_path).map_err(|error| {
        CustodyError::Io(format!(
            "could not atomically activate the new node identity: {error}"
        ))
    })?;
    checkpoint(RotationCheckpoint::CanonicalSeedReplaced)?;
    sync_directory(&paths.identity_dir)?;
    checkpoint(RotationCheckpoint::CanonicalSeedDurable)?;
    require_seed_identity(
        &paths.node_seed_path,
        &new_node_id,
        "activated node identity",
    )?;

    remove_file_if_exists(&previous_seed_path)?;
    sync_directory(&paths.identity_dir)?;
    checkpoint(RotationCheckpoint::PreviousSeedRemoved)?;
    remove_file_if_exists(&journal_path)?;
    sync_directory(&paths.identity_dir)?;

    rotation_report(&journal)
}

pub fn verify_continuity_record(record: &IdentityContinuityRecord) -> Result<(), CustodyError> {
    let payload = &record.payload;
    if payload.schema_version != CONTINUITY_SCHEMA_V1
        || payload.old_node_id == payload.new_node_id
        || payload.reason.trim().is_empty()
        || payload.reason.len() > 512
    {
        return Err(CustodyError::Invalid(
            "continuity record payload is invalid".to_string(),
        ));
    }
    let bytes = canonical_json::to_vec(payload).map_err(|error| {
        CustodyError::Invalid(format!("continuity payload serialization failed: {error}"))
    })?;
    let signatures_valid =
        crypto::verify_message(&payload.old_node_id, &record.old_node_signature, &bytes)
            && crypto::verify_message(&payload.new_node_id, &record.new_node_signature, &bytes)
            && crypto::verify_message(
                &payload.nostr_publication_id,
                &record.nostr_publication_signature,
                &bytes,
            );
    if !signatures_valid {
        return Err(CustodyError::Authentication);
    }
    Ok(())
}

pub fn read_continuity_record(path: &Path) -> Result<IdentityContinuityRecord, CustodyError> {
    let bytes = read_limited(path, 128 * 1024)?;
    let record = serde_json::from_slice(&bytes)
        .map_err(|_| CustodyError::Invalid("continuity record is not valid v1 JSON".to_string()))?;
    verify_continuity_record(&record)?;
    Ok(record)
}

fn recover_pending_identity_rotation_locked(
    paths: &IdentityPaths,
) -> Result<Option<IdentityRotationReport>, CustodyError> {
    let journal_path = paths.rotation_journal_path();
    if !path_entry_exists(&journal_path)? {
        cleanup_unjournaled_rotation_files(paths)?;
        return Ok(None);
    }

    let journal = read_rotation_journal(&journal_path)?;
    validate_rotation_record_path(paths, &journal.continuity_record_path)?;
    let payload = &journal.continuity_record.payload;
    require_seed_identity(
        &paths.nostr_publication_seed_path,
        &payload.nostr_publication_id,
        "Nostr publication identity",
    )?;

    let canonical_id = optional_seed_identity(&paths.node_seed_path, "canonical node identity")?;
    let next_seed_path = paths.rotation_next_seed_path();
    let next_id = optional_seed_identity(&next_seed_path, "next node identity")?;
    let previous_seed_path = paths.rotation_previous_seed_path();
    let previous_id = optional_seed_identity(&previous_seed_path, "previous node identity")?;

    if let Some(next_id) = &next_id
        && next_id != &payload.new_node_id
    {
        return Err(CustodyError::Invalid(format!(
            "pending rotation next seed has identity {next_id}, expected {}",
            payload.new_node_id
        )));
    }
    if let Some(previous_id) = &previous_id
        && previous_id != &payload.old_node_id
    {
        return Err(CustodyError::Invalid(format!(
            "pending rotation previous seed has identity {previous_id}, expected {}",
            payload.old_node_id
        )));
    }

    match canonical_id.as_deref() {
        Some(current) if current == payload.new_node_id => {
            ensure_continuity_record(&journal)?;
        }
        Some(current) if current == payload.old_node_id => {
            if previous_id.is_none() {
                return Err(CustodyError::Invalid(
                    "pending rotation is missing its durable previous seed".to_string(),
                ));
            }
            if next_id.is_none() {
                rollback_incomplete_rotation(paths, &journal, false)?;
                return Ok(None);
            }
            ensure_continuity_record(&journal)?;
            activate_pending_seed(paths, &next_seed_path, &payload.new_node_id)?;
        }
        None => {
            if previous_id.is_none() {
                return Err(CustodyError::Invalid(
                    "pending rotation has neither a canonical nor previous node seed".to_string(),
                ));
            }
            if next_id.is_none() {
                rollback_incomplete_rotation(paths, &journal, true)?;
                return Ok(None);
            }
            ensure_continuity_record(&journal)?;
            activate_pending_seed(paths, &next_seed_path, &payload.new_node_id)?;
        }
        Some(current) => {
            return Err(CustodyError::Invalid(format!(
                "canonical node identity {current} matches neither side of the pending rotation"
            )));
        }
    }

    remove_file_if_exists(&next_seed_path)?;
    remove_file_if_exists(&previous_seed_path)?;
    sync_directory(&paths.identity_dir)?;
    remove_file_if_exists(&journal_path)?;
    sync_directory(&paths.identity_dir)?;
    rotation_report(&journal).map(Some)
}

fn cleanup_unjournaled_rotation_files(paths: &IdentityPaths) -> Result<(), CustodyError> {
    let next_seed_path = paths.rotation_next_seed_path();
    let previous_seed_path = paths.rotation_previous_seed_path();
    let next_id = optional_seed_identity(&next_seed_path, "unjournaled next node identity")?;
    let previous_id =
        optional_seed_identity(&previous_seed_path, "unjournaled previous node identity")?;
    if next_id.is_none() && previous_id.is_none() {
        return Ok(());
    }

    let mut canonical_id =
        optional_seed_identity(&paths.node_seed_path, "canonical node identity")?;
    if canonical_id.is_none() {
        let Some(previous_id) = previous_id.as_deref() else {
            return Err(CustodyError::Invalid(
                "canonical node seed is missing and no journal or previous seed can recover it"
                    .to_string(),
            ));
        };
        fs::hard_link(&previous_seed_path, &paths.node_seed_path).map_err(|error| {
            CustodyError::Io(format!(
                "could not restore the unjournaled previous node identity: {error}"
            ))
        })?;
        sync_directory(&paths.identity_dir)?;
        canonical_id = Some(previous_id.to_string());
    }
    if let Some(previous_id) = previous_id
        && canonical_id.as_deref() != Some(previous_id.as_str())
    {
        return Err(CustodyError::Invalid(
            "unjournaled previous seed does not match the canonical node identity".to_string(),
        ));
    }

    remove_file_if_exists(&next_seed_path)?;
    remove_file_if_exists(&previous_seed_path)?;
    sync_directory(&paths.identity_dir)
}

fn rollback_incomplete_rotation(
    paths: &IdentityPaths,
    journal: &IdentityRotationJournal,
    restore_canonical: bool,
) -> Result<(), CustodyError> {
    let previous_seed_path = paths.rotation_previous_seed_path();
    if restore_canonical {
        fs::hard_link(&previous_seed_path, &paths.node_seed_path).map_err(|error| {
            CustodyError::Io(format!(
                "could not restore the previous node identity during rotation recovery: {error}"
            ))
        })?;
        sync_directory(&paths.identity_dir)?;
    }
    require_seed_identity(
        &paths.node_seed_path,
        &journal.continuity_record.payload.old_node_id,
        "rolled-back node identity",
    )?;
    remove_continuity_record_if_owned(journal)?;
    remove_file_if_exists(&previous_seed_path)?;
    remove_file_if_exists(&paths.rotation_next_seed_path())?;
    sync_directory(&paths.identity_dir)?;
    remove_file_if_exists(&paths.rotation_journal_path())?;
    sync_directory(&paths.identity_dir)
}

fn activate_pending_seed(
    paths: &IdentityPaths,
    next_seed_path: &Path,
    expected_new_node_id: &str,
) -> Result<(), CustodyError> {
    fs::rename(next_seed_path, &paths.node_seed_path).map_err(|error| {
        CustodyError::Io(format!(
            "could not atomically recover the pending node identity: {error}"
        ))
    })?;
    sync_directory(&paths.identity_dir)?;
    require_seed_identity(
        &paths.node_seed_path,
        expected_new_node_id,
        "recovered node identity",
    )
}

fn install_rotation_journal(
    path: &Path,
    journal: &IdentityRotationJournal,
) -> Result<(), CustodyError> {
    let bytes = canonical_json::to_vec(journal).map_err(|error| {
        CustodyError::Invalid(format!("rotation journal serialization failed: {error}"))
    })?;
    match atomic_write_new(path, &bytes, 0o600) {
        Ok(()) => Ok(()),
        Err(error) if path_entry_exists(path)? => {
            let installed = read_limited(path, MAX_ROTATION_JOURNAL_BYTES)?;
            if installed != bytes {
                return Err(error);
            }
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn read_rotation_journal(path: &Path) -> Result<IdentityRotationJournal, CustodyError> {
    reject_symlink(path, "identity rotation journal")?;
    let file = File::open(path).map_err(|error| {
        CustodyError::Io(format!(
            "could not open identity rotation journal {}: {error}",
            path.display()
        ))
    })?;
    require_private_file(&file, path)?;
    drop(file);
    let bytes = read_limited(path, MAX_ROTATION_JOURNAL_BYTES)?;
    let journal: IdentityRotationJournal = serde_json::from_slice(&bytes)
        .map_err(|_| CustodyError::Invalid("identity rotation journal is invalid".to_string()))?;
    if journal.schema_version != ROTATION_JOURNAL_SCHEMA_V1
        || !journal.continuity_record_path.is_absolute()
    {
        return Err(CustodyError::Invalid(
            "identity rotation journal metadata is invalid".to_string(),
        ));
    }
    verify_continuity_record(&journal.continuity_record)?;
    let record_bytes = canonical_json::to_vec(&journal.continuity_record).map_err(|error| {
        CustodyError::Invalid(format!("continuity record serialization failed: {error}"))
    })?;
    if crypto::sha256_hex(&record_bytes) != journal.continuity_record_sha256 {
        return Err(CustodyError::Invalid(
            "identity rotation journal continuity digest is invalid".to_string(),
        ));
    }
    Ok(journal)
}

fn ensure_continuity_record(journal: &IdentityRotationJournal) -> Result<(), CustodyError> {
    let path = &journal.continuity_record_path;
    let expected = canonical_json::to_vec(&journal.continuity_record).map_err(|error| {
        CustodyError::Invalid(format!("continuity record serialization failed: {error}"))
    })?;
    if path_entry_exists(path)? {
        reject_symlink(path, "identity continuity record")?;
        let actual = read_limited(path, MAX_BACKUP_BYTES)?;
        if actual != expected {
            return Err(CustodyError::Invalid(format!(
                "continuity record {} conflicts with the pending rotation",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        return Ok(());
    }

    match atomic_write_new(path, &expected, 0o600) {
        Ok(()) => Ok(()),
        Err(error) if path_entry_exists(path)? => {
            reject_symlink(path, "identity continuity record")?;
            if read_limited(path, MAX_BACKUP_BYTES)? != expected {
                return Err(error);
            }
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn remove_continuity_record_if_owned(
    journal: &IdentityRotationJournal,
) -> Result<(), CustodyError> {
    let path = &journal.continuity_record_path;
    if !path_entry_exists(path)? {
        return Ok(());
    }
    reject_symlink(path, "identity continuity record")?;
    let expected = canonical_json::to_vec(&journal.continuity_record).map_err(|error| {
        CustodyError::Invalid(format!("continuity record serialization failed: {error}"))
    })?;
    if read_limited(path, MAX_BACKUP_BYTES)? != expected {
        return Err(CustodyError::Invalid(format!(
            "continuity record {} is not owned by the pending rotation",
            path.display()
        )));
    }
    fs::remove_file(path).map_err(|error| {
        CustodyError::Io(format!(
            "could not remove rolled-back continuity record {}: {error}",
            path.display()
        ))
    })?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn validate_rotation_record_path(
    paths: &IdentityPaths,
    record_path: &Path,
) -> Result<(), CustodyError> {
    let journal_path = paths.rotation_journal_path();
    let next_seed_path = paths.rotation_next_seed_path();
    let previous_seed_path = paths.rotation_previous_seed_path();
    let lock_path = paths.identity_lock_path();
    let restore_journal_path = paths.restore_journal_path();
    let backup_status_path = paths.backup_status_path();
    let protected = [
        paths.node_seed_path.as_path(),
        paths.nostr_publication_seed_path.as_path(),
        journal_path.as_path(),
        next_seed_path.as_path(),
        previous_seed_path.as_path(),
        lock_path.as_path(),
        restore_journal_path.as_path(),
        backup_status_path.as_path(),
    ];
    if protected
        .iter()
        .any(|protected| paths_lexically_equal(record_path, protected))
    {
        return Err(CustodyError::Invalid(
            "continuity record path conflicts with protected identity state".to_string(),
        ));
    }
    Ok(())
}

fn optional_seed_identity(path: &Path, label: &str) -> Result<Option<String>, CustodyError> {
    if !path_entry_exists(path)? {
        return Ok(None);
    }
    reject_symlink(path, label)?;
    let file = File::open(path)
        .map_err(|error| CustodyError::Io(format!("could not open {label}: {error}")))?;
    require_private_file(&file, path)?;
    drop(file);
    let key = identity::load_signing_key(path).map_err(CustodyError::Io)?;
    Ok(Some(crypto::public_key_hex(&key)))
}

fn require_seed_identity(path: &Path, expected_id: &str, label: &str) -> Result<(), CustodyError> {
    let actual = optional_seed_identity(path, label)?
        .ok_or_else(|| CustodyError::Invalid(format!("{label} {} is missing", path.display())))?;
    if actual != expected_id {
        return Err(CustodyError::Invalid(format!(
            "{label} has identity {actual}, expected {expected_id}"
        )));
    }
    Ok(())
}

fn rotation_report(
    journal: &IdentityRotationJournal,
) -> Result<IdentityRotationReport, CustodyError> {
    let payload = &journal.continuity_record.payload;
    Ok(IdentityRotationReport {
        schema_version: CONTINUITY_SCHEMA_V1,
        old_node_id: payload.old_node_id.clone(),
        new_node_id: payload.new_node_id.clone(),
        nostr_publication_id: payload.nostr_publication_id.clone(),
        continuity_record_path: absolute_path(&journal.continuity_record_path)?,
        continuity_record_sha256: journal.continuity_record_sha256.clone(),
        backup_status: "stale_after_rotation; create a new encrypted backup before publishing",
        restart_required: true,
    })
}

fn backup_aad(header: &BackupHeader) -> Result<Vec<u8>, CustodyError> {
    canonical_json::to_vec(header)
        .map_err(|error| CustodyError::Invalid(format!("backup AAD failed: {error}")))
}

fn encode_secret_payload(
    node_key: &crypto::NodeSigningKey,
    nostr_key: &crypto::NodeSigningKey,
) -> Zeroizing<Vec<u8>> {
    let mut payload = Zeroizing::new(Vec::with_capacity(SECRET_PAYLOAD_LEN));
    payload.extend_from_slice(SECRET_PAYLOAD_MAGIC);
    let mut node_seed = crypto::signing_key_seed_bytes(node_key);
    let mut nostr_seed = crypto::signing_key_seed_bytes(nostr_key);
    payload.extend_from_slice(&node_seed);
    payload.extend_from_slice(&nostr_seed);
    node_seed.zeroize();
    nostr_seed.zeroize();
    payload
}

fn decode_secret_payload(payload: &[u8]) -> Result<IdentitySeeds, CustodyError> {
    if payload.len() != SECRET_PAYLOAD_LEN
        || &payload[..SECRET_PAYLOAD_MAGIC.len()] != SECRET_PAYLOAD_MAGIC
    {
        return Err(CustodyError::Authentication);
    }
    let mut node_seed = Zeroizing::new([0u8; 32]);
    let mut nostr_seed = Zeroizing::new([0u8; 32]);
    node_seed
        .copy_from_slice(&payload[SECRET_PAYLOAD_MAGIC.len()..SECRET_PAYLOAD_MAGIC.len() + 32]);
    nostr_seed.copy_from_slice(&payload[SECRET_PAYLOAD_MAGIC.len() + 32..]);
    Ok(IdentitySeeds {
        node: node_seed,
        nostr_publication: nostr_seed,
    })
}

fn ciphertext_exposes_secret(ciphertext: &[u8], plaintext: &[u8]) -> bool {
    !plaintext.is_empty()
        && ciphertext.len() >= plaintext.len()
        && ciphertext
            .windows(plaintext.len())
            .any(|window| window.ct_eq(plaintext).into())
}

fn verify_recovered_identity_payload(
    recovered: &[u8],
    expected_payload: &[u8],
    expected_node_id: &str,
    expected_nostr_publication_id: &str,
) -> Result<(), CustodyError> {
    if recovered.ct_eq(expected_payload).unwrap_u8() != 1 {
        return Err(CustodyError::Authentication);
    }
    let IdentitySeeds {
        node,
        nostr_publication,
    } = decode_secret_payload(recovered)?;
    let node_key =
        crypto::signing_key_from_seed_bytes(&node).map_err(|_| CustodyError::Authentication)?;
    let nostr_key = crypto::signing_key_from_seed_bytes(&nostr_publication)
        .map_err(|_| CustodyError::Authentication)?;
    if crypto::public_key_hex(&node_key) != expected_node_id
        || crypto::public_key_hex(&nostr_key) != expected_nostr_publication_id
    {
        return Err(CustodyError::Authentication);
    }
    Ok(())
}

fn validate_bundle_shape(bundle: &IdentityBackupBundle) -> Result<(), CustodyError> {
    if bundle.schema_version != BACKUP_SCHEMA_V1 {
        return Err(CustodyError::Invalid(format!(
            "unsupported backup schema {:?}",
            bundle.schema_version
        )));
    }
    if bundle.created_at_unix == 0 {
        return Err(CustodyError::Invalid(
            "backup creation time must be positive".to_string(),
        ));
    }
    if bundle.node_id.len() != 64
        || bundle.nostr_publication_id.len() != 64
        || hex::decode(&bundle.node_id).is_err()
        || hex::decode(&bundle.nostr_publication_id).is_err()
    {
        return Err(CustodyError::Invalid(
            "backup public identity metadata is invalid".to_string(),
        ));
    }
    match &bundle.protection {
        BackupProtection::RecoveryKeyFile {
            algorithm,
            nonce_base64,
        } if algorithm == RECOVERY_KEY_ALGORITHM
            && BASE64
                .decode(nonce_base64)
                .is_ok_and(|nonce| nonce.len() == 12) => {}
        BackupProtection::OperatorCommand { protocol, key_id }
            if protocol == OPERATOR_COMMAND_PROTOCOL
                && !key_id.trim().is_empty()
                && key_id.len() <= 256
                && !key_id.chars().any(char::is_control) => {}
        _ => {
            return Err(CustodyError::Invalid(
                "backup protection descriptor is unsupported".to_string(),
            ));
        }
    }
    Ok(())
}

fn protection_kind(protection: &BackupProtection) -> &'static str {
    match protection {
        BackupProtection::RecoveryKeyFile { .. } => "recovery_key_file",
        BackupProtection::OperatorCommand { .. } => "operator_command",
    }
}

fn verify_current_backup_for_rotation(
    paths: &IdentityPaths,
    adapter: &dyn CustodyAdapter,
    node_key: &crypto::NodeSigningKey,
    nostr_key: &crypto::NodeSigningKey,
) -> Result<(), CustodyError> {
    let status_path = paths.backup_status_path();
    reject_symlink(&status_path, "identity backup status")?;
    let status_bytes = read_limited(&status_path, 64 * 1024)?;
    let status_record: BackupStatusRecord = serde_json::from_slice(&status_bytes)
        .map_err(|_| CustodyError::Invalid("backup status record is invalid".to_string()))?;
    let node_id = crypto::public_key_hex(node_key);
    let nostr_publication_id = crypto::public_key_hex(nostr_key);
    if status_record.schema_version != BACKUP_STATUS_SCHEMA_V1
        || status_record.created_at_unix == 0
        || status_record.node_id != node_id
        || status_record.nostr_publication_id != nostr_publication_id
        || !matches!(
            status_record.protection_kind.as_str(),
            "recovery_key_file" | "operator_command"
        )
    {
        return Err(CustodyError::Invalid(
            "current identities do not have a valid backup status record".to_string(),
        ));
    }

    reject_symlink(&status_record.backup_path, "identity backup")?;
    let encoded = read_limited(&status_record.backup_path, MAX_BACKUP_BYTES)?;
    if crypto::sha256_hex(&encoded) != status_record.backup_sha256 {
        return Err(CustodyError::Authentication);
    }
    let bundle: IdentityBackupBundle = serde_json::from_slice(&encoded)
        .map_err(|_| CustodyError::Invalid("backup is not a valid v1 bundle".to_string()))?;
    validate_bundle_shape(&bundle)?;
    if bundle.created_at_unix != status_record.created_at_unix
        || bundle.node_id != node_id
        || bundle.nostr_publication_id != nostr_publication_id
        || protection_kind(&bundle.protection) != status_record.protection_kind
    {
        return Err(CustodyError::Authentication);
    }
    let ciphertext = Zeroizing::new(
        BASE64
            .decode(&bundle.ciphertext_base64)
            .map_err(|_| CustodyError::Authentication)?,
    );
    if ciphertext.is_empty() || ciphertext.len() as u64 > MAX_OPERATOR_OUTPUT_BYTES {
        return Err(CustodyError::Authentication);
    }
    let expected_payload = encode_secret_payload(node_key, nostr_key);
    if ciphertext_exposes_secret(&ciphertext, &expected_payload) {
        return Err(CustodyError::Authentication);
    }
    let recovered = adapter.unseal(&bundle, &ciphertext)?;
    verify_recovered_identity_payload(
        &recovered,
        &expected_payload,
        &node_id,
        &nostr_publication_id,
    )
}

fn create_restore_staging_dir(
    paths: &IdentityPaths,
    staging_dir: &Path,
    node_seed: &[u8; 32],
    nostr_seed: &[u8; 32],
) -> Result<(), CustodyError> {
    fs::create_dir(staging_dir).map_err(|error| {
        CustodyError::Io(format!(
            "could not create restore staging directory {}: {error}",
            staging_dir.display()
        ))
    })?;
    set_mode(staging_dir, 0o700)?;
    let node_name = paths.node_seed_path.file_name().ok_or_else(|| {
        CustodyError::Invalid("node identity seed path has no filename".to_string())
    })?;
    let nostr_name = paths
        .nostr_publication_seed_path
        .file_name()
        .ok_or_else(|| {
            CustodyError::Invalid("Nostr publication seed path has no filename".to_string())
        })?;
    write_seed_new(&staging_dir.join(node_name), node_seed)?;
    write_seed_new(&staging_dir.join(nostr_name), nostr_seed)?;
    sync_directory(staging_dir)
}

fn write_seed_new(path: &Path, seed: &[u8; 32]) -> Result<(), CustodyError> {
    let mut encoded = Zeroizing::new(hex::encode(seed));
    let result = atomic_write_new(path, encoded.as_bytes(), 0o600);
    encoded.zeroize();
    result
}

fn atomic_write_new(path: &Path, bytes: &[u8], mode: u32) -> Result<(), CustodyError> {
    if path.exists() {
        return Err(CustodyError::Invalid(format!(
            "{} already exists; refusing to overwrite it",
            path.display()
        )));
    }
    let staging = write_staging_file(path, bytes, mode)?;
    if let Err(error) = install_staging_new(&staging, path) {
        cleanup_file(&staging);
        return Err(error);
    }
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn atomic_write_replace(path: &Path, bytes: &[u8], mode: u32) -> Result<(), CustodyError> {
    let staging = write_staging_file(path, bytes, mode)?;
    if let Err(error) = fs::rename(&staging, path) {
        cleanup_file(&staging);
        return Err(CustodyError::Io(format!(
            "could not atomically replace {}: {error}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn write_staging_file(path: &Path, bytes: &[u8], mode: u32) -> Result<PathBuf, CustodyError> {
    let parent = path.parent().ok_or_else(|| {
        CustodyError::Invalid(format!("{} has no parent directory", path.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CustodyError::Io(format!("could not create {}: {error}", parent.display()))
    })?;
    let staging = unique_sibling(path, "tmp");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options.open(&staging).map_err(|error| {
        CustodyError::Io(format!(
            "could not create staging file {}: {error}",
            staging.display()
        ))
    })?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        cleanup_file(&staging);
        return Err(CustodyError::Io(format!(
            "could not persist staging file {}: {error}",
            staging.display()
        )));
    }
    drop(file);
    if let Err(error) = set_mode(&staging, mode) {
        cleanup_file(&staging);
        return Err(error);
    }
    Ok(staging)
}

fn install_staging_new(staging: &Path, destination: &Path) -> Result<(), CustodyError> {
    #[cfg(unix)]
    {
        fs::hard_link(staging, destination).map_err(|error| {
            CustodyError::Io(format!(
                "could not atomically install {} without overwrite: {error}",
                destination.display()
            ))
        })?;
        fs::remove_file(staging).map_err(|error| {
            CustodyError::Io(format!(
                "could not remove staging link {}: {error}",
                staging.display()
            ))
        })?;
    }
    #[cfg(not(unix))]
    {
        if destination.exists() {
            return Err(CustodyError::Invalid(format!(
                "{} already exists",
                destination.display()
            )));
        }
        fs::rename(staging, destination).map_err(|error| {
            CustodyError::Io(format!(
                "could not install {}: {error}",
                destination.display()
            ))
        })?;
    }
    Ok(())
}

fn unique_sibling(path: &Path, label: &str) -> PathBuf {
    let mut random = [0u8; 8];
    OsRng.fill_bytes(&mut random);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("froglet-identity");
    path.with_file_name(format!(
        ".{name}.{label}.{}.{}",
        std::process::id(),
        hex::encode(random)
    ))
}

fn read_limited(path: &Path, max_bytes: u64) -> Result<Vec<u8>, CustodyError> {
    let file = File::open(path)
        .map_err(|error| CustodyError::Io(format!("could not open {}: {error}", path.display())))?;
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CustodyError::Io(format!("could not read {}: {error}", path.display())))?;
    if bytes.len() as u64 > max_bytes {
        return Err(CustodyError::Invalid(format!(
            "{} exceeds the {} byte limit",
            path.display(),
            max_bytes
        )));
    }
    Ok(bytes)
}

fn read_private_limited(path: &Path, max_bytes: u64, label: &str) -> Result<Vec<u8>, CustodyError> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| {
        CustodyError::Io(format!(
            "could not inspect {label} {}: {error}",
            path.display()
        ))
    })?;
    if !path_metadata.is_file() || path_metadata.file_type().is_symlink() {
        return Err(CustodyError::Invalid(format!(
            "{label} {} must be a regular file, not a symbolic link",
            path.display()
        )));
    }
    let file = File::open(path).map_err(|error| {
        CustodyError::Io(format!(
            "could not open {label} {}: {error}",
            path.display()
        ))
    })?;
    require_private_file(&file, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened_metadata = file.metadata().map_err(|error| {
            CustodyError::Io(format!(
                "could not inspect opened {label} {}: {error}",
                path.display()
            ))
        })?;
        if opened_metadata.dev() != path_metadata.dev()
            || opened_metadata.ino() != path_metadata.ino()
        {
            return Err(CustodyError::Invalid(format!(
                "{label} {} changed while it was being opened",
                path.display()
            )));
        }
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CustodyError::Io(format!(
                "could not read {label} {}: {error}",
                path.display()
            ))
        })?;
    if bytes.len() as u64 > max_bytes {
        return Err(CustodyError::Invalid(format!(
            "{label} {} exceeds the {max_bytes} byte limit",
            path.display()
        )));
    }
    Ok(bytes)
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn reject_symlink(path: &Path, label: &str) -> Result<(), CustodyError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CustodyError::Io(format!(
            "could not inspect {label} {}: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(CustodyError::Invalid(format!(
            "{label} {} must not be a symbolic link",
            path.display()
        )));
    }
    Ok(())
}

fn require_private_file(file: &File, path: &Path) -> Result<(), CustodyError> {
    let metadata = file.metadata().map_err(|error| {
        CustodyError::Io(format!("could not inspect {}: {error}", path.display()))
    })?;
    if !metadata.is_file() {
        return Err(CustodyError::Invalid(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CustodyError::Invalid(format!(
                "{} must not be accessible by group or other users",
                path.display()
            )));
        }
    }
    Ok(())
}

fn require_private_directory(path: &Path) -> Result<(), CustodyError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CustodyError::Io(format!("could not inspect {}: {error}", path.display()))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CustodyError::Invalid(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CustodyError::Invalid(format!(
                "{} must not be accessible by group or other users",
                path.display()
            )));
        }
    }
    Ok(())
}

fn ensure_custody_data_dir(paths: &IdentityPaths, mode: u32) -> Result<(), CustodyError> {
    let existed = path_entry_exists(&paths.data_dir)?;
    fs::create_dir_all(&paths.data_dir).map_err(|error| {
        CustodyError::Io(format!(
            "could not create data directory {}: {error}",
            paths.data_dir.display()
        ))
    })?;
    if existed {
        Ok(())
    } else {
        set_mode(&paths.data_dir, mode)
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), CustodyError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|error| {
                CustodyError::Io(format!("could not inspect {}: {error}", path.display()))
            })?
            .permissions();
        permissions.set_mode(mode);
        fs::set_permissions(path, permissions).map_err(|error| {
            CustodyError::Io(format!("could not protect {}: {error}", path.display()))
        })?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), CustodyError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                CustodyError::Io(format!(
                    "could not sync directory {}: {error}",
                    path.display()
                ))
            })?;
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, CustodyError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(CustodyError::from)
    }
}

fn paths_lexically_equal(left: &Path, right: &Path) -> bool {
    fn normalized(path: &Path) -> PathBuf {
        use std::path::Component;

        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(path)
        } else {
            path.to_path_buf()
        };
        let mut result = PathBuf::new();
        for component in absolute.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    let _ = result.pop();
                }
                other => result.push(other.as_os_str()),
            }
        }
        result
    }

    normalized(left) == normalized(right)
}

fn cleanup_file(path: &Path) {
    let _ = fs::remove_file(path);
}

fn path_entry_exists(path: &Path) -> Result<bool, CustodyError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(CustodyError::Io(format!(
            "could not inspect {}: {error}",
            path.display()
        ))),
    }
}

fn remove_file_if_exists(path: &Path) -> Result<(), CustodyError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CustodyError::Io(format!(
                "could not inspect {} before removal: {error}",
                path.display()
            )));
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CustodyError::Invalid(format!(
            "identity operation state {} is not a regular file",
            path.display()
        )));
    }
    fs::remove_file(path).map_err(|error| {
        CustodyError::Io(format!(
            "could not remove identity operation state {}: {error}",
            path.display()
        ))
    })
}

fn unix_now() -> Result<u64, CustodyError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| CustodyError::Invalid("system clock is before Unix epoch".to_string()))
}

struct IdentityLock {
    #[cfg(unix)]
    _file: File,
    #[cfg(not(unix))]
    path: PathBuf,
}

impl IdentityLock {
    fn acquire(path: &Path) -> Result<Self, CustodyError> {
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};

            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)
                .map_err(|error| {
                    CustodyError::Io(format!(
                        "could not open identity custody lock {}: {error}",
                        path.display()
                    ))
                })?;
            // SAFETY: `file` owns a valid descriptor for the lifetime of the
            // returned guard. `flock` neither retains the pointer nor accesses
            // Rust-managed memory, and closing the descriptor releases it even
            // if the process exits unexpectedly.
            let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if locked != 0 {
                let error = std::io::Error::last_os_error();
                return Err(CustodyError::Invalid(format!(
                    "another identity custody operation may be in progress (lock {}): {error}",
                    path.display()
                )));
            }
            Ok(Self { _file: file })
        }

        #[cfg(not(unix))]
        {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            options.open(path).map_err(|error| {
                CustodyError::Invalid(format!(
                    "another identity custody operation may be in progress (lock {}): {error}",
                    path.display()
                ))
            })?;
            Ok(Self {
                path: path.to_path_buf(),
            })
        }
    }
}

#[cfg(not(unix))]
impl Drop for IdentityLock {
    fn drop(&mut self) {
        cleanup_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig, PaymentBackend,
        PricingConfig, StorageConfig, TorSidecarConfig, WasmConfig,
    };
    use tempfile::TempDir;

    struct UnrecoverableOperatorAdapter;

    impl CustodyAdapter for UnrecoverableOperatorAdapter {
        fn seal(
            &self,
            _common_header: &BackupCommonHeader,
            _plaintext: &[u8],
        ) -> Result<SealedSecret, CustodyError> {
            Ok(SealedSecret {
                protection: BackupProtection::OperatorCommand {
                    protocol: OPERATOR_COMMAND_PROTOCOL.to_string(),
                    key_id: "adversarial-test-key".to_string(),
                },
                ciphertext: b"arbitrary-nonempty-seal-output".to_vec(),
            })
        }

        fn unseal(
            &self,
            _bundle: &IdentityBackupBundle,
            ciphertext: &[u8],
        ) -> Result<Zeroizing<Vec<u8>>, CustodyError> {
            Ok(Zeroizing::new(ciphertext.to_vec()))
        }
    }

    struct PlaintextOperatorAdapter;

    impl CustodyAdapter for PlaintextOperatorAdapter {
        fn seal(
            &self,
            _common_header: &BackupCommonHeader,
            plaintext: &[u8],
        ) -> Result<SealedSecret, CustodyError> {
            Ok(SealedSecret {
                protection: BackupProtection::OperatorCommand {
                    protocol: OPERATOR_COMMAND_PROTOCOL.to_string(),
                    key_id: "plaintext-test-key".to_string(),
                },
                ciphertext: plaintext.to_vec(),
            })
        }

        fn unseal(
            &self,
            _bundle: &IdentityBackupBundle,
            ciphertext: &[u8],
        ) -> Result<Zeroizing<Vec<u8>>, CustodyError> {
            Ok(Zeroizing::new(ciphertext.to_vec()))
        }
    }

    fn paths(root: &Path) -> IdentityPaths {
        IdentityPaths {
            data_dir: root.to_path_buf(),
            identity_dir: root.join("identity"),
            node_seed_path: root.join("identity/secp256k1.seed"),
            nostr_publication_seed_path: root.join("identity/nostr-publication.secp256k1.seed"),
        }
    }

    fn replace_backup_and_rebind_status(paths: &IdentityPaths, backup_path: &Path, bytes: &[u8]) {
        atomic_write_replace(backup_path, bytes, 0o600).expect("replace backup fixture");
        let status_path = paths.backup_status_path();
        let mut status: BackupStatusRecord =
            serde_json::from_slice(&fs::read(&status_path).expect("read backup status"))
                .expect("parse backup status");
        status.backup_sha256 = crypto::sha256_hex(bytes);
        let status_bytes = canonical_json::to_vec(&status).expect("serialize backup status");
        atomic_write_replace(&status_path, &status_bytes, 0o600)
            .expect("replace backup status fixture");
    }

    fn node_config(root: &Path) -> NodeConfig {
        NodeConfig {
            network_mode: NetworkMode::Clearnet,
            listen_addr: "127.0.0.1:8080".to_string(),
            public_base_url: None,
            runtime_listen_addr: "127.0.0.1:8081".to_string(),
            runtime_allow_non_loopback: false,
            http_ca_cert_path: None,
            tor: TorSidecarConfig {
                binary_path: "tor".to_string(),
                backend_listen_addr: "127.0.0.1:8082".to_string(),
                startup_timeout_secs: 90,
            },
            relay: crate::config::RelayConfig::default(),
            identity: IdentityConfig {
                auto_generate: true,
            },
            pricing: PricingConfig {
                events_query: 0,
                execute_wasm: 0,
            },
            payment_backends: vec![PaymentBackend::None],
            execution_timeout_secs: 10,
            process_limits: Default::default(),
            public_quota: Default::default(),
            lightning: LightningConfig {
                mode: LightningMode::Mock,
                destination_identity: None,
                base_invoice_expiry_secs: 300,
                success_hold_expiry_secs: 300,
                min_final_cltv_expiry: 18,
                sync_interval_ms: 1_000,
                lnd_rest: None,
                phoenixd: None,
            },
            x402: None,
            stripe: None,
            buyer_stripe: None,
            buyer_phoenixd: None,
            requester_spend: Default::default(),
            storage: StorageConfig {
                data_dir: root.to_path_buf(),
                db_path: root.join("node.db"),
                identity_dir: root.join("identity"),
                identity_seed_path: root.join("identity/secp256k1.seed"),
                nostr_publication_seed_path: root.join("identity/nostr-publication.secp256k1.seed"),
                runtime_dir: root.join("runtime"),
                runtime_auth_token_path: root.join("runtime/auth.token"),
                consumer_control_auth_token_path: root.join("runtime/consumerctl.token"),
                provider_control_auth_token_path: root.join("runtime/froglet-control.token"),
                tor_dir: root.join("tor"),
                host_readable_control_token: false,
            },
            wasm: WasmConfig {
                policy_path: None,
                policy: None,
            },
            gpu: Default::default(),
            confidential: crate::confidential::ConfidentialConfig {
                policy_path: None,
                policy: None,
                session_ttl_secs: 300,
            },
            marketplace_url: None,
            marketplace_allow_local: false,
            provider_artifact_root: None,
            postgres_mounts: std::collections::BTreeMap::new(),
            session_pool: Default::default(),
            hosted_trial_origin_secret: None,
        }
    }

    fn initialize(paths: &IdentityPaths) -> (String, String) {
        fs::create_dir_all(&paths.identity_dir).expect("identity directory");
        set_mode(&paths.identity_dir, 0o700).expect("private identity directory");
        let node = crypto::generate_signing_key();
        let nostr = crypto::generate_signing_key();
        let mut node_seed = crypto::signing_key_seed_bytes(&node);
        let mut nostr_seed = crypto::signing_key_seed_bytes(&nostr);
        write_seed_new(&paths.node_seed_path, &node_seed).expect("node seed");
        write_seed_new(&paths.nostr_publication_seed_path, &nostr_seed).expect("nostr seed");
        node_seed.zeroize();
        nostr_seed.zeroize();
        (
            crypto::public_key_hex(&node),
            crypto::public_key_hex(&nostr),
        )
    }

    #[test]
    fn encrypted_backup_restores_both_identities_into_second_directory() {
        let source = TempDir::new().expect("source tempdir");
        let destination = TempDir::new().expect("destination parent");
        let source_paths = paths(source.path());
        let destination_paths = paths(&destination.path().join("restored-node"));
        let (node_id, nostr_id) = initialize(&source_paths);
        let backup_path = source.path().join("identities.froglet-backup");
        let recovery_key_path = source.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");

        let report = create_backup(
            &source_paths,
            &backup_path,
            &adapter,
            Some(recovery_key_path.clone()),
        )
        .expect("encrypted backup");
        assert_eq!(report.node_id, node_id);
        assert_eq!(report.nostr_publication_id, nostr_id);
        assert_eq!(
            backup_status(&source_paths).state,
            BackupStatusState::Current
        );

        let restore_adapter = RecoveryKeyFileAdapter::open(&recovery_key_path).expect("open key");
        let restored = restore_backup(&destination_paths, &backup_path, &restore_adapter)
            .expect("restore into second directory");
        assert_eq!(restored.node_id, node_id);
        assert_eq!(restored.nostr_publication_id, nostr_id);
        let restored_node = identity::load_signing_key(&destination_paths.node_seed_path)
            .expect("restored node seed");
        let restored_nostr =
            identity::load_signing_key(&destination_paths.nostr_publication_seed_path)
                .expect("restored nostr seed");
        assert_eq!(crypto::public_key_hex(&restored_node), node_id);
        assert_eq!(crypto::public_key_hex(&restored_nostr), nostr_id);
        assert_eq!(
            backup_status(&destination_paths).state,
            BackupStatusState::Current
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&destination_paths.node_seed_path)
                    .expect("node metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&destination_paths.identity_dir)
                    .expect("identity dir metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn restore_recovers_or_rolls_back_at_every_durable_boundary() {
        let source = TempDir::new().expect("source tempdir");
        let source_paths = paths(source.path());
        let (node_id, nostr_id) = initialize(&source_paths);
        let backup_path = source.path().join("identities.froglet-backup");
        let recovery_key_path = source.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&source_paths, &backup_path, &adapter, None).expect("source backup");

        for crash_at in [
            RestoreCheckpoint::StagingSynced,
            RestoreCheckpoint::JournalWritten,
            RestoreCheckpoint::StatusWritten,
            RestoreCheckpoint::IdentityInstalled,
            RestoreCheckpoint::JournalRemoved,
        ] {
            let destination = TempDir::new().expect("destination tempdir");
            let destination_paths = paths(&destination.path().join("restored"));
            let restore_adapter =
                RecoveryKeyFileAdapter::open(&recovery_key_path).expect("restore key");
            let error = restore_backup_with_checkpoints(
                &destination_paths,
                &backup_path,
                &restore_adapter,
                |checkpoint| {
                    if checkpoint == crash_at {
                        Err(CustodyError::Io(format!(
                            "simulated restore crash at {checkpoint:?}"
                        )))
                    } else {
                        Ok(())
                    }
                },
            )
            .expect_err("injected restore boundary must interrupt the caller");
            assert!(error.to_string().contains("simulated restore crash"));

            let recovered = recover_pending_identity_state(&destination_paths)
                .expect("deterministic restore recovery");
            if crash_at == RestoreCheckpoint::StagingSynced {
                assert!(recovered.restored_identity.is_none());
                assert!(!destination_paths.identity_dir.exists());
                assert!(!destination_paths.backup_status_path().exists());
            } else {
                let current_node = identity::load_signing_key(&destination_paths.node_seed_path)
                    .expect("restored node identity");
                let current_nostr =
                    identity::load_signing_key(&destination_paths.nostr_publication_seed_path)
                        .expect("restored Nostr identity");
                assert_eq!(crypto::public_key_hex(&current_node), node_id);
                assert_eq!(crypto::public_key_hex(&current_nostr), nostr_id);
                assert_eq!(
                    backup_status(&destination_paths).state,
                    BackupStatusState::Current
                );
            }
            assert!(!destination_paths.restore_journal_path().exists());
            let orphaned_staging = fs::read_dir(&destination_paths.data_dir)
                .expect("destination data directory")
                .filter_map(Result::ok)
                .any(|entry| is_restore_staging_name(&destination_paths, &entry.path()));
            assert!(!orphaned_staging, "restore staging must be cleaned up");
        }
    }

    #[cfg(unix)]
    #[test]
    fn restore_recovery_rejects_an_unauthenticated_intended_status() {
        let source = TempDir::new().expect("source tempdir");
        let destination = TempDir::new().expect("destination tempdir");
        let source_paths = paths(source.path());
        initialize(&source_paths);
        let backup_path = source.path().join("identities.froglet-backup");
        let recovery_key_path = source.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&source_paths, &backup_path, &adapter, None).expect("source backup");

        let destination_paths = paths(&destination.path().join("restored"));
        let error = restore_backup_with_checkpoints(
            &destination_paths,
            &backup_path,
            &adapter,
            |checkpoint| {
                if checkpoint == RestoreCheckpoint::JournalWritten {
                    Err(CustodyError::Io("simulated crash".to_string()))
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("leave a durable restore journal");
        assert!(error.to_string().contains("simulated crash"));

        let journal_path = destination_paths.restore_journal_path();
        let mut journal =
            read_restore_journal(&destination_paths, &journal_path).expect("valid restore journal");
        journal.intent.status_record.backup_sha256 = "00".repeat(32);
        let tampered = canonical_json::to_vec(&journal).expect("tampered journal JSON");
        atomic_write_replace(&journal_path, &tampered, 0o600).expect("replace journal");

        assert!(matches!(
            recover_pending_identity_state(&destination_paths),
            Err(CustodyError::Authentication)
        ));
        assert!(!destination_paths.identity_dir.exists());
        assert!(journal_path.exists(), "failed-closed journal must remain");
    }

    #[test]
    fn wrong_key_and_tampering_fail_without_creating_identity_directory() {
        let source = TempDir::new().expect("source tempdir");
        let destination = TempDir::new().expect("destination tempdir");
        let source_paths = paths(source.path());
        initialize(&source_paths);
        let backup_path = source.path().join("identities.froglet-backup");
        let recovery_key_path = source.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&source_paths, &backup_path, &adapter, None).expect("backup");

        let wrong_key_path = source.path().join("wrong.key");
        let wrong_adapter = RecoveryKeyFileAdapter::create(&wrong_key_path).expect("wrong key");
        let wrong_destination = paths(&destination.path().join("wrong-key"));
        assert!(matches!(
            restore_backup(&wrong_destination, &backup_path, &wrong_adapter),
            Err(CustodyError::Authentication)
        ));
        assert!(!wrong_destination.identity_dir.exists());

        let mut bundle: IdentityBackupBundle =
            serde_json::from_slice(&fs::read(&backup_path).expect("read backup"))
                .expect("parse bundle");
        bundle.node_id.replace_range(
            0..1,
            if &bundle.node_id[..1] == "0" {
                "1"
            } else {
                "0"
            },
        );
        let tampered_path = source.path().join("tampered.froglet-backup");
        let tampered = canonical_json::to_vec(&bundle).expect("tampered bundle");
        atomic_write_new(&tampered_path, &tampered, 0o600).expect("write tampered bundle");
        let right_adapter = RecoveryKeyFileAdapter::open(&recovery_key_path).expect("right key");
        let tampered_destination = paths(&destination.path().join("tampered"));
        assert!(matches!(
            restore_backup(&tampered_destination, &tampered_path, &right_adapter),
            Err(CustodyError::Authentication)
        ));
        assert!(!tampered_destination.identity_dir.exists());
    }

    #[test]
    fn backup_requires_adapter_to_recover_the_exact_identity_payload() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        initialize(&paths);
        let backup_path = temp.path().join("unrecoverable.froglet-backup");

        let error = create_backup(&paths, &backup_path, &UnrecoverableOperatorAdapter, None)
            .expect_err("unrecoverable adapter output must not be accepted as a backup");

        assert!(matches!(error, CustodyError::Authentication));
        assert!(!backup_path.exists());
        assert!(!paths.backup_status_path().exists());
        assert_eq!(backup_status(&paths).state, BackupStatusState::Missing);
        assert!(
            rotate_node_identity(
                &paths,
                &UnrecoverableOperatorAdapter,
                None,
                "must remain locked",
            )
            .is_err(),
            "an unverified backup must not unlock identity rotation"
        );
    }

    #[test]
    fn backup_rejects_adapter_that_exposes_plaintext_as_ciphertext() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        initialize(&paths);
        let backup_path = temp.path().join("plaintext.froglet-backup");

        let error = create_backup(&paths, &backup_path, &PlaintextOperatorAdapter, None)
            .expect_err("plaintext adapter output must never be persisted as ciphertext");

        assert!(matches!(error, CustodyError::Authentication));
        assert!(!backup_path.exists());
        assert!(!paths.backup_status_path().exists());
    }

    #[test]
    fn restore_fails_closed_with_journal_when_status_installation_is_blocked() {
        let source = TempDir::new().expect("source tempdir");
        let destination = TempDir::new().expect("destination tempdir");
        let source_paths = paths(source.path());
        initialize(&source_paths);
        let backup_path = source.path().join("identities.froglet-backup");
        let recovery_key_path = source.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&source_paths, &backup_path, &adapter, None).expect("source backup");

        let destination_paths = paths(&destination.path().join("restore-status-failure"));
        fs::create_dir_all(destination_paths.backup_status_path())
            .expect("status path that cannot be atomically replaced by a file");

        let error = restore_backup(&destination_paths, &backup_path, &adapter)
            .expect_err("status installation must fail");

        assert!(matches!(
            error,
            CustodyError::Io(_) | CustodyError::Invalid(_)
        ));
        assert!(
            !destination_paths.identity_dir.exists(),
            "failed restore must not leave installed identity material"
        );
        assert!(!destination_paths.node_seed_path.exists());
        assert!(!destination_paths.nostr_publication_seed_path.exists());
        assert!(
            destination_paths.restore_journal_path().exists(),
            "durable restore intent must remain for fail-closed operator recovery"
        );
    }

    #[test]
    fn rotation_record_requires_old_new_and_nostr_signatures() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        let (old_node_id, nostr_id) = initialize(&paths);
        let backup_path = temp.path().join("identities.froglet-backup");
        let recovery_key_path = temp.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&paths, &backup_path, &adapter, None).expect("backup before rotation");

        let report = rotate_node_identity(&paths, &adapter, None, "scheduled key rotation")
            .expect("rotate node identity");
        assert_eq!(report.old_node_id, old_node_id);
        assert_ne!(report.new_node_id, old_node_id);
        assert_eq!(report.nostr_publication_id, nostr_id);
        assert_eq!(
            backup_status(&paths).state,
            BackupStatusState::StaleIdentity
        );

        let record =
            read_continuity_record(&report.continuity_record_path).expect("continuity verifies");
        assert_eq!(record.payload.old_node_id, old_node_id);
        assert_eq!(record.payload.new_node_id, report.new_node_id);
        let current = identity::load_signing_key(&paths.node_seed_path).expect("current key");
        assert_eq!(crypto::public_key_hex(&current), report.new_node_id);

        let mut bad_old = record.clone();
        bad_old.old_node_signature.replace_range(0..1, "0");
        if bad_old.old_node_signature == record.old_node_signature {
            bad_old.old_node_signature.replace_range(0..1, "1");
        }
        assert!(verify_continuity_record(&bad_old).is_err());

        let mut bad_new = record.clone();
        bad_new.new_node_signature.replace_range(0..1, "0");
        if bad_new.new_node_signature == record.new_node_signature {
            bad_new.new_node_signature.replace_range(0..1, "1");
        }
        assert!(verify_continuity_record(&bad_new).is_err());

        let mut bad_nostr = record.clone();
        bad_nostr
            .nostr_publication_signature
            .replace_range(0..1, "0");
        if bad_nostr.nostr_publication_signature == record.nostr_publication_signature {
            bad_nostr
                .nostr_publication_signature
                .replace_range(0..1, "1");
        }
        assert!(verify_continuity_record(&bad_nostr).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rotation_recovers_deterministically_at_every_durable_boundary() {
        let checkpoints = [
            RotationCheckpoint::NextSeedDurable,
            RotationCheckpoint::PreviousSeedDurable,
            RotationCheckpoint::JournalDurable,
            RotationCheckpoint::ContinuityRecordDurable,
            RotationCheckpoint::CanonicalSeedReplaced,
            RotationCheckpoint::CanonicalSeedDurable,
            RotationCheckpoint::PreviousSeedRemoved,
        ];

        for crash_at in checkpoints {
            let temp = TempDir::new().expect("tempdir");
            let paths = paths(temp.path());
            let (old_node_id, _) = initialize(&paths);
            let backup_path = temp.path().join("identities.froglet-backup");
            let recovery_key_path = temp.path().join("recovery.key");
            let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
            create_backup(&paths, &backup_path, &adapter, None).expect("backup before rotation");

            let error = rotate_node_identity_with_checkpoints(
                &paths,
                &adapter,
                None,
                "failure-injection rotation",
                |checkpoint| {
                    if checkpoint == crash_at {
                        Err(CustodyError::Io(format!(
                            "simulated crash at {checkpoint:?}"
                        )))
                    } else {
                        Ok(())
                    }
                },
            )
            .expect_err("injected boundary must interrupt rotation");
            assert!(error.to_string().contains("simulated crash"));
            assert!(
                paths.node_seed_path.exists(),
                "canonical seed must exist at {crash_at:?}"
            );

            let journal = if path_entry_exists(&paths.rotation_journal_path())
                .expect("inspect journal")
            {
                Some(read_rotation_journal(&paths.rotation_journal_path()).expect("valid journal"))
            } else {
                None
            };
            let recovered =
                recover_pending_identity_rotation(&paths).expect("deterministic recovery");

            match journal {
                Some(journal) => {
                    let report = recovered.expect("journaled rotation must roll forward");
                    assert_eq!(
                        report.new_node_id,
                        journal.continuity_record.payload.new_node_id
                    );
                    assert_eq!(
                        optional_seed_identity(&paths.node_seed_path, "current")
                            .expect("current identity")
                            .as_deref(),
                        Some(report.new_node_id.as_str())
                    );
                    read_continuity_record(&report.continuity_record_path)
                        .expect("durable continuity record");
                }
                None => {
                    assert!(
                        recovered.is_none(),
                        "pre-journal interruption must roll back"
                    );
                    assert_eq!(
                        optional_seed_identity(&paths.node_seed_path, "current")
                            .expect("current identity")
                            .as_deref(),
                        Some(old_node_id.as_str())
                    );
                }
            }
            assert!(!paths.rotation_journal_path().exists());
            assert!(!paths.rotation_next_seed_path().exists());
            assert!(!paths.rotation_previous_seed_path().exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn generate_and_backup_never_create_a_third_key_during_rotation_recovery() {
        for crash_at in [
            RotationCheckpoint::NextSeedDurable,
            RotationCheckpoint::PreviousSeedDurable,
            RotationCheckpoint::JournalDurable,
            RotationCheckpoint::ContinuityRecordDurable,
            RotationCheckpoint::CanonicalSeedReplaced,
            RotationCheckpoint::CanonicalSeedDurable,
            RotationCheckpoint::PreviousSeedRemoved,
        ] {
            let temp = TempDir::new().expect("tempdir");
            let paths = paths(temp.path());
            let config = node_config(temp.path());
            let (old_node_id, nostr_id) = initialize(&paths);
            let backup_path = temp.path().join("identities.froglet-backup");
            let recovery_key_path = temp.path().join("recovery.key");
            let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
            create_backup(&paths, &backup_path, &adapter, None).expect("backup before rotation");

            rotate_node_identity_with_checkpoints(
                &paths,
                &adapter,
                None,
                "failure-injection rotation",
                |checkpoint| {
                    if checkpoint == crash_at {
                        Err(CustodyError::Io(format!(
                            "simulated crash at {checkpoint:?}"
                        )))
                    } else {
                        Ok(())
                    }
                },
            )
            .expect_err("injected boundary must interrupt rotation");
            let intended_new_id = path_entry_exists(&paths.rotation_journal_path())
                .expect("inspect rotation journal")
                .then(|| {
                    read_rotation_journal(&paths.rotation_journal_path())
                        .expect("valid rotation journal")
                        .continuity_record
                        .payload
                        .new_node_id
                });

            // This is the exact entry point used by `identity generate` and
            // daemon startup. It must recover before auto-generation can see
            // a transiently absent canonical seed.
            let generated = crate::identity::NodeIdentity::load_or_create(&config)
                .expect("recovery-aware generate");
            let expected_node_id = intended_new_id.as_deref().unwrap_or(&old_node_id);
            assert_eq!(generated.node_id(), expected_node_id);
            assert_eq!(generated.nostr_publication_key_hex(), nostr_id);

            // Backup reloads under the same custody lock and must bind the
            // recovered identity, never a separately generated key.
            let after_backup = temp.path().join("after-recovery.froglet-backup");
            let report = create_backup(&paths, &after_backup, &adapter, None)
                .expect("backup recovered identity");
            assert_eq!(report.node_id, expected_node_id);
            assert_eq!(report.nostr_publication_id, nostr_id);
            assert!(!paths.rotation_journal_path().exists());
            assert!(!paths.rotation_next_seed_path().exists());
            assert!(!paths.rotation_previous_seed_path().exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_restores_previous_seed_if_unflushed_activation_is_lost() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        let (old_node_id, _) = initialize(&paths);
        let backup_path = temp.path().join("identities.froglet-backup");
        let recovery_key_path = temp.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&paths, &backup_path, &adapter, None).expect("backup before rotation");

        rotate_node_identity_with_checkpoints(
            &paths,
            &adapter,
            None,
            "lost rename durability",
            |checkpoint| {
                if checkpoint == RotationCheckpoint::CanonicalSeedReplaced {
                    Err(CustodyError::Io(
                        "simulated crash before directory sync".to_string(),
                    ))
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("crash before identity-directory sync");
        let journal =
            read_rotation_journal(&paths.rotation_journal_path()).expect("pending journal");
        assert!(journal.continuity_record_path.exists());

        // Model the conservative crash-recovery observation in which the
        // un-synced rename is absent while its source entry is already gone.
        fs::remove_file(&paths.node_seed_path).expect("lose unflushed canonical rename");
        let recovered =
            recover_pending_identity_rotation(&paths).expect("recover missing canonical seed");

        assert!(
            recovered.is_none(),
            "unrecoverable new generation rolls back"
        );
        assert_eq!(
            optional_seed_identity(&paths.node_seed_path, "restored current")
                .expect("restored identity")
                .as_deref(),
            Some(old_node_id.as_str())
        );
        assert!(!journal.continuity_record_path.exists());
        assert!(!paths.rotation_journal_path().exists());
        assert!(!paths.rotation_previous_seed_path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn rotation_lock_is_released_by_descriptor_close_not_file_removal() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        initialize(&paths);
        let lock_path = paths.identity_lock_path();
        atomic_write_new(&lock_path, b"stale lock inode", 0o600).expect("stale lock file");

        let first = IdentityLock::acquire(&lock_path).expect("stale inode is not a held lock");
        assert!(
            IdentityLock::acquire(&lock_path).is_err(),
            "live descriptor lock must exclude a second rotation"
        );
        drop(first);
        IdentityLock::acquire(&lock_path).expect("descriptor close releases lock");
    }

    #[test]
    fn continuity_record_cannot_alias_rotation_state() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        initialize(&paths);
        for protected in [
            paths.rotation_journal_path(),
            paths.rotation_next_seed_path(),
            paths.rotation_previous_seed_path(),
            paths.identity_lock_path(),
            paths.restore_journal_path(),
        ] {
            assert!(validate_rotation_record_path(&paths, &protected).is_err());
        }
    }

    #[test]
    fn rotation_requires_live_recovery_of_the_recorded_current_backup() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        let (node_id, _) = initialize(&paths);
        let backup_path = temp.path().join("identities.froglet-backup");
        let recovery_key_path = temp.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&paths, &backup_path, &adapter, None).expect("backup before rotation");
        let wrong_key_path = temp.path().join("wrong-recovery.key");
        let wrong_adapter =
            RecoveryKeyFileAdapter::create(&wrong_key_path).expect("wrong recovery key");

        let error = rotate_node_identity(&paths, &wrong_adapter, None, "must prove live recovery")
            .expect_err("digest-only status must not authorize rotation");

        assert!(matches!(error, CustodyError::Authentication));
        let current = identity::load_signing_key(&paths.node_seed_path).expect("current node key");
        assert_eq!(crypto::public_key_hex(&current), node_id);
        assert_eq!(backup_status(&paths).state, BackupStatusState::Current);
        if paths.continuity_dir().exists() {
            assert_eq!(
                fs::read_dir(paths.continuity_dir())
                    .expect("continuity directory")
                    .count(),
                0,
                "failed authorization must not install a continuity record"
            );
        }
    }

    #[test]
    fn backup_status_rejects_malformed_bundle_even_when_its_digest_is_rebound() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        initialize(&paths);
        let backup_path = temp.path().join("identities.froglet-backup");
        let recovery_key_path = temp.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&paths, &backup_path, &adapter, None).expect("backup");

        replace_backup_and_rebind_status(&paths, &backup_path, b"{}");

        assert_eq!(
            backup_status(&paths).state,
            BackupStatusState::InvalidRecord
        );
    }

    #[test]
    fn backup_status_binds_bundle_metadata_and_ciphertext_shape() {
        for drift in ["created_at", "node_id", "protection", "nonce", "ciphertext"] {
            let temp = TempDir::new().expect("tempdir");
            let paths = paths(temp.path());
            initialize(&paths);
            let backup_path = temp.path().join("identities.froglet-backup");
            let recovery_key_path = temp.path().join("recovery.key");
            let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
            create_backup(&paths, &backup_path, &adapter, None).expect("backup");
            let mut bundle: IdentityBackupBundle =
                serde_json::from_slice(&fs::read(&backup_path).expect("read backup"))
                    .expect("parse backup");

            match drift {
                "created_at" => bundle.created_at_unix += 1,
                "node_id" => bundle.node_id = "00".repeat(32),
                "protection" => {
                    bundle.protection = BackupProtection::OperatorCommand {
                        protocol: OPERATOR_COMMAND_PROTOCOL.to_string(),
                        key_id: "different-protection".to_string(),
                    };
                }
                "nonce" => {
                    let BackupProtection::RecoveryKeyFile { nonce_base64, .. } =
                        &mut bundle.protection
                    else {
                        unreachable!("test backup uses recovery-key-file protection");
                    };
                    *nonce_base64 = BASE64.encode([0_u8; 11]);
                }
                "ciphertext" => bundle.ciphertext_base64 = "not-base64!".to_string(),
                _ => unreachable!(),
            }
            let bytes = canonical_json::to_vec(&bundle).expect("serialize modified bundle");
            replace_backup_and_rebind_status(&paths, &backup_path, &bytes);

            assert_eq!(
                backup_status(&paths).state,
                BackupStatusState::InvalidRecord,
                "drift={drift}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn backup_status_rejects_symlinked_status_and_backup_files() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        initialize(&paths);
        let backup_path = temp.path().join("identities.froglet-backup");
        let recovery_key_path = temp.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&paths, &backup_path, &adapter, None).expect("backup");

        let status_path = paths.backup_status_path();
        let real_status_path = temp.path().join("real-backup-status.json");
        fs::rename(&status_path, &real_status_path).expect("move status");
        symlink(&real_status_path, &status_path).expect("symlink status");
        assert_eq!(
            backup_status(&paths).state,
            BackupStatusState::InvalidRecord
        );

        fs::remove_file(&status_path).expect("remove status symlink");
        fs::rename(&real_status_path, &status_path).expect("restore status");
        let real_backup_path = temp.path().join("real-identities.froglet-backup");
        fs::rename(&backup_path, &real_backup_path).expect("move backup");
        symlink(&real_backup_path, &backup_path).expect("symlink backup");
        assert_eq!(
            backup_status(&paths).state,
            BackupStatusState::InvalidRecord
        );
    }

    #[cfg(unix)]
    #[test]
    fn backup_status_rejects_dangling_status_symlink_and_public_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dangling = TempDir::new().expect("dangling tempdir");
        let dangling_paths = paths(dangling.path());
        symlink(
            dangling.path().join("missing-status-target"),
            dangling_paths.backup_status_path(),
        )
        .expect("dangling status symlink");
        assert_eq!(
            backup_status(&dangling_paths).state,
            BackupStatusState::InvalidRecord
        );

        let public = TempDir::new().expect("public tempdir");
        let public_paths = paths(public.path());
        initialize(&public_paths);
        let backup_path = public.path().join("identities.froglet-backup");
        let recovery_key_path = public.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&public_paths, &backup_path, &adapter, None).expect("backup");
        fs::set_permissions(&backup_path, fs::Permissions::from_mode(0o644))
            .expect("make backup public");
        assert_eq!(
            backup_status(&public_paths).state,
            BackupStatusState::InvalidRecord
        );
    }

    #[test]
    fn restore_refuses_any_existing_identity_directory() {
        let source = TempDir::new().expect("source tempdir");
        let destination = TempDir::new().expect("destination tempdir");
        let source_paths = paths(source.path());
        initialize(&source_paths);
        let backup_path = source.path().join("identities.froglet-backup");
        let recovery_key_path = source.path().join("recovery.key");
        let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path).expect("recovery key");
        create_backup(&source_paths, &backup_path, &adapter, None).expect("backup");

        let destination_paths = paths(&destination.path().join("existing"));
        fs::create_dir_all(&destination_paths.identity_dir).expect("existing identity dir");
        set_mode(&destination_paths.identity_dir, 0o700).expect("private identity dir");
        let error = restore_backup(&destination_paths, &backup_path, &adapter)
            .expect_err("restore must refuse existing directory");
        assert!(error.to_string().contains("refusing to overwrite"));
    }

    #[test]
    fn backup_paths_cannot_overwrite_identity_state_or_each_other() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(temp.path());
        let output = temp.path().join("backup.json");
        assert!(
            paths
                .validate_backup_destinations(&paths.backup_status_path(), None)
                .is_err()
        );
        assert!(
            paths
                .validate_backup_destinations(&output, Some(&output))
                .is_err()
        );
        assert!(
            paths
                .validate_backup_destinations(
                    &output,
                    Some(&temp.path().join("./identity-backup-status.json")),
                )
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn operator_command_times_out_kills_and_reaps_the_process_group() {
        let temp = TempDir::new().expect("tempdir");
        let wrapper = temp.path().join("hung-custody-wrapper.sh");
        let pid_path = temp.path().join("hung.pid");
        let script = format!(
            "#!/bin/sh\n\
            printf '%s' \"$$\" > '{}'\n\
            exec /bin/sleep 30\n",
            pid_path.display()
        );
        atomic_write_new(&wrapper, script.as_bytes(), 0o700).expect("hung wrapper");
        let mut adapter =
            OperatorCommandAdapter::new(wrapper, "timeout-test".to_string()).expect("adapter");
        // Give the wrapper enough scheduling time to publish its pid even on a
        // loaded CI host; this is still far below the wrapper's 30-second
        // sleep and exercises the adapter's hard timeout.
        adapter.timeout = Duration::from_secs(5);

        let started = Instant::now();
        let error = adapter
            .invoke("seal", b"input", b"public aad", false)
            .expect_err("hung adapter must time out");

        assert!(error.to_string().contains("timed out"));
        assert!(
            started.elapsed() < Duration::from_secs(12),
            "hard timeout must return promptly"
        );
        let pid: i32 = fs::read_to_string(&pid_path)
            .expect("wrapper pid")
            .parse()
            .expect("numeric pid");
        // SAFETY: signal zero performs an existence check and does not signal
        // an extant process. A reaped process must report ESRCH here.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[cfg(unix)]
    #[test]
    fn operator_command_drains_stdout_while_streaming_stdin() {
        const STREAM_BYTES: usize = 256 * 1024;

        let temp = TempDir::new().expect("tempdir");
        let wrapper = temp.path().join("bidirectional-custody-wrapper.sh");
        let script = format!(
            "#!/bin/sh\n\
            [ -n \"$FROGLET_CUSTODY_AAD_BASE64\" ] || exit 2\n\
            [ -z \"${{FROGLET_IDENTITY_SEED_HEX+x}}\" ] || exit 3\n\
            /bin/dd if=/dev/zero bs=65536 count=4 2>/dev/null\n\
            bytes=$(/usr/bin/wc -c)\n\
            [ \"$bytes\" -eq {STREAM_BYTES} ] || exit 4\n"
        );
        atomic_write_new(&wrapper, script.as_bytes(), 0o700).expect("bidirectional wrapper");
        let mut adapter =
            OperatorCommandAdapter::new(wrapper, "stream-test".to_string()).expect("adapter");
        adapter.timeout = Duration::from_secs(3);
        let input = vec![0x5a; STREAM_BYTES];

        let output = adapter
            .invoke("seal", &input, b"public aad", false)
            .expect("concurrent pipe handling");

        assert_eq!(output.len(), STREAM_BYTES);
        assert!(output.iter().all(|byte| *byte == 0));
    }

    #[cfg(unix)]
    #[test]
    fn operator_command_adapter_roundtrips_through_binary_stdio_protocol() {
        let source = TempDir::new().expect("source tempdir");
        let destination = TempDir::new().expect("destination tempdir");
        let source_paths = paths(source.path());
        let (node_id, nostr_id) = initialize(&source_paths);
        let wrapper = source.path().join("test-custody-wrapper.sh");
        let secret_path = source.path().join("test-custody-secret.bin");
        let script = format!(
            "#!/bin/sh\n\
            secret_path='{}'\n\
            case \"$1\" in seal|unseal) ;; *) exit 2 ;; esac\n\
            [ \"$2\" = \"--key-id\" ] || exit 2\n\
            [ \"$3\" = \"test-key\" ] || exit 2\n\
            [ -n \"$FROGLET_CUSTODY_AAD_BASE64\" ] || exit 2\n\
            if [ \"$1\" = seal ]; then\n\
              umask 077\n\
              /bin/cat > \"$secret_path\"\n\
              printf %s sealed-test-handle-v1\n\
            else\n\
              [ \"$(/bin/cat)\" = sealed-test-handle-v1 ] || exit 3\n\
              exec /bin/cat \"$secret_path\"\n\
            fi\n",
            secret_path.display()
        );
        atomic_write_new(&wrapper, script.as_bytes(), 0o700).expect("test wrapper");
        let adapter =
            OperatorCommandAdapter::new(wrapper, "test-key".to_string()).expect("operator adapter");
        let backup_path = source.path().join("operator.froglet-backup");
        let report =
            create_backup(&source_paths, &backup_path, &adapter, None).expect("operator backup");
        assert!(matches!(
            report.protection,
            BackupProtection::OperatorCommand { .. }
        ));

        let destination_paths = paths(&destination.path().join("operator-restored"));
        let restored =
            restore_backup(&destination_paths, &backup_path, &adapter).expect("operator restore");
        assert_eq!(restored.node_id, node_id);
        assert_eq!(restored.nostr_publication_id, nostr_id);

        let rotation = rotate_node_identity(
            &source_paths,
            &adapter,
            None,
            "operator-command recovery proof",
        )
        .expect("operator-authorized rotation");
        assert_eq!(rotation.old_node_id, node_id);
    }

    #[test]
    fn os_keychain_is_explicitly_unsupported() {
        assert!(matches!(
            os_keychain_capability(),
            Err(CustodyError::Unsupported(_))
        ));
    }
}
