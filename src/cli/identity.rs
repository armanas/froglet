//! `froglet-node identity` — local identity generation, custody, restore, and
//! independently verifiable rotation continuity.

use super::{CliError, pop_flag, pop_kv};
use crate::{
    config::NodeConfig,
    identity::NodeIdentity,
    identity_custody::{
        BackupStatusState, CustodyAdapter, IdentityPaths, OperatorCommandAdapter,
        RecoveryKeyFileAdapter, backup_status, create_backup, os_keychain_capability,
        read_continuity_record, restore_backup, rotate_node_identity,
    },
};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    let Some(command) = args.first().cloned() else {
        return Err(CliError::BadArgs(usage().to_string()));
    };
    let _ = args.remove(0);
    match command.as_str() {
        "generate" => generate(args, json_mode),
        "status" => status(args, json_mode),
        "capabilities" => capabilities(args, json_mode),
        "backup" => backup(args, json_mode),
        "restore" => restore(args, json_mode),
        "rotate" => rotate(args, json_mode),
        "verify-continuity" => verify_continuity(args, json_mode),
        _ => Err(CliError::BadArgs(usage().to_string())),
    }
}

fn generate(args: Vec<String>, json_mode: bool) -> Result<(), CliError> {
    require_no_args(&args, "identity generate")?;
    let config = load_config()?;
    let identity = NodeIdentity::load_or_create(&config)
        .map_err(|error| CliError::Other(format!("identity generation failed: {error}")))?;
    let paths = IdentityPaths::from(&config);
    let report = IdentityPublicReport {
        status: "ready",
        node_id: identity.node_id(),
        nostr_publication_id: identity.nostr_publication_key_hex(),
        identity_dir: &paths.identity_dir,
        backup_status: backup_status_label(backup_status(&paths).state),
    };
    print_report(&report, json_mode, || {
        println!("node_id:               {}", report.node_id);
        println!("nostr_publication_id:  {}", report.nostr_publication_id);
        println!("identity_dir:          {}", report.identity_dir.display());
        println!("backup_status:         {}", report.backup_status);
    })
}

fn status(args: Vec<String>, json_mode: bool) -> Result<(), CliError> {
    require_no_args(&args, "identity status")?;
    let config = load_config()?;
    let paths = IdentityPaths::from(&config);
    crate::identity_custody::recover_pending_identity_state(&paths)
        .map_err(|error| CliError::Other(format!("identity recovery failed: {error}")))?;
    let status = backup_status(&paths);
    print_report(&status, json_mode, || {
        println!("backup_status: {}", backup_status_label(status.state));
        println!("status_path:   {}", status.status_path.display());
        if let Some(path) = &status.backup_path {
            println!("backup_path:   {}", path.display());
        }
        if let Some(digest) = &status.backup_sha256 {
            println!("backup_sha256: {digest}");
        }
    })
}

fn capabilities(args: Vec<String>, json_mode: bool) -> Result<(), CliError> {
    require_no_args(&args, "identity capabilities")?;
    let report = serde_json::json!({
        "encrypted_file": {
            "status": "available",
            "algorithm": "AES-256-GCM-SIV",
            "key_source": "generated 256-bit recovery-key file"
        },
        "operator_command": {
            "status": "available",
            "protocol": "froglet-custody-command-v1",
            "purpose": "operator-owned KMS/HSM wrapper"
        },
        "os_keychain": {
            "status": "unsupported",
            "reason": os_keychain_capability().expect_err("explicitly unsupported").to_string()
        }
    });
    print_report(&report, json_mode, || {
        println!("encrypted_file:  available (AES-256-GCM-SIV, generated 256-bit key file)");
        println!("operator_command: available (operator-owned KMS/HSM wrapper)");
        println!("os_keychain:      unsupported (no insecure emulation)");
    })
}

fn backup(mut args: Vec<String>, json_mode: bool) -> Result<(), CliError> {
    let output = pop_kv(&mut args, "--output")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("froglet-identity-backup.json"));
    let recovery_key = pop_kv(&mut args, "--recovery-key").map(PathBuf::from);
    let operator_command = pop_kv(&mut args, "--operator-command").map(PathBuf::from);
    let key_id = pop_kv(&mut args, "--key-id");
    require_no_args(&args, "identity backup")?;
    let adapter_choice = match (recovery_key, operator_command, key_id) {
        (Some(_), Some(_), _) => {
            return Err(CliError::BadArgs(
                "choose either --recovery-key or --operator-command, not both".to_string(),
            ));
        }
        (Some(path), None, None) => BackupAdapterChoice::RecoveryKey(path),
        (None, None, None) => {
            BackupAdapterChoice::RecoveryKey(output.with_extension("recovery-key"))
        }
        (None, Some(program), Some(key_id)) => {
            BackupAdapterChoice::OperatorCommand { program, key_id }
        }
        (None, Some(_), None) => {
            return Err(CliError::BadArgs(
                "--operator-command requires --key-id".to_string(),
            ));
        }
        (_, None, Some(_)) => {
            return Err(CliError::BadArgs(
                "--key-id is only valid with --operator-command".to_string(),
            ));
        }
    };
    let config = load_config()?;
    let paths = IdentityPaths::from(&config);
    let recovery_key_path = match &adapter_choice {
        BackupAdapterChoice::RecoveryKey(path) => Some(path.as_path()),
        BackupAdapterChoice::OperatorCommand { .. } => None,
    };
    paths
        .validate_backup_destinations(&output, recovery_key_path)
        .map_err(|error| CliError::Other(error.to_string()))?;
    let _identity = NodeIdentity::load_or_create(&config)
        .map_err(|error| CliError::Other(format!("identity generation failed: {error}")))?;

    match adapter_choice {
        BackupAdapterChoice::OperatorCommand { program, key_id } => {
            let adapter = OperatorCommandAdapter::new(program, key_id)
                .map_err(|error| CliError::Other(error.to_string()))?;
            let report = create_backup(&paths, &output, &adapter, None)
                .map_err(|error| CliError::Other(error.to_string()))?;
            print_backup_report(&report, json_mode)
        }
        BackupAdapterChoice::RecoveryKey(recovery_key_path) => {
            let adapter = RecoveryKeyFileAdapter::create(&recovery_key_path)
                .map_err(|error| CliError::Other(error.to_string()))?;
            let report = create_backup(
                &paths,
                &output,
                &adapter,
                Some(absolute(&recovery_key_path)?),
            )
            .map_err(|error| CliError::Other(error.to_string()))?;
            print_backup_report(&report, json_mode)
        }
    }
}

enum BackupAdapterChoice {
    RecoveryKey(PathBuf),
    OperatorCommand { program: PathBuf, key_id: String },
}

fn restore(mut args: Vec<String>, json_mode: bool) -> Result<(), CliError> {
    let backup_path = required_path(&mut args, "--backup")?;
    let recovery_key = pop_kv(&mut args, "--recovery-key").map(PathBuf::from);
    let operator_command = pop_kv(&mut args, "--operator-command").map(PathBuf::from);
    let key_id = pop_kv(&mut args, "--key-id");
    require_no_args(&args, "identity restore")?;
    let config = load_config()?;
    let paths = IdentityPaths::from(&config);

    let adapter = open_existing_custody_adapter(recovery_key, operator_command, key_id, "restore")?;
    let report = restore_backup(&paths, &backup_path, adapter.as_ref())
        .map_err(|error| CliError::Other(error.to_string()))?;
    print_report(&report, json_mode, || {
        println!("restored_node_id:              {}", report.node_id);
        println!(
            "restored_nostr_publication_id: {}",
            report.nostr_publication_id
        );
        println!(
            "identity_dir:                  {}",
            report.identity_dir.display()
        );
    })
}

fn rotate(mut args: Vec<String>, json_mode: bool) -> Result<(), CliError> {
    let record_path = pop_kv(&mut args, "--continuity-record").map(PathBuf::from);
    let reason =
        pop_kv(&mut args, "--reason").unwrap_or_else(|| "operator-requested rotation".to_string());
    let recovery_key = pop_kv(&mut args, "--recovery-key").map(PathBuf::from);
    let operator_command = pop_kv(&mut args, "--operator-command").map(PathBuf::from);
    let key_id = pop_kv(&mut args, "--key-id");
    require_no_args(&args, "identity rotate")?;
    let adapter = open_existing_custody_adapter(recovery_key, operator_command, key_id, "rotate")?;
    let config = load_config()?;
    let paths = IdentityPaths::from(&config);
    let report = rotate_node_identity(&paths, adapter.as_ref(), record_path.as_deref(), &reason)
        .map_err(|error| CliError::Other(error.to_string()))?;
    print_report(&report, json_mode, || {
        println!("old_node_id:              {}", report.old_node_id);
        println!("new_node_id:              {}", report.new_node_id);
        println!("nostr_publication_id:     {}", report.nostr_publication_id);
        println!(
            "continuity_record:         {}",
            report.continuity_record_path.display()
        );
        println!(
            "continuity_record_sha256: {}",
            report.continuity_record_sha256
        );
        println!("backup_status:            {}", report.backup_status);
        println!("restart_required:         true");
    })
}

fn verify_continuity(mut args: Vec<String>, json_mode: bool) -> Result<(), CliError> {
    let record_path = required_path(&mut args, "--record")?;
    require_no_args(&args, "identity verify-continuity")?;
    let record =
        read_continuity_record(&record_path).map_err(|error| CliError::Other(error.to_string()))?;
    let report = serde_json::json!({
        "valid": true,
        "record_path": absolute(&record_path)?,
        "old_node_id": record.payload.old_node_id,
        "new_node_id": record.payload.new_node_id,
        "nostr_publication_id": record.payload.nostr_publication_id,
        "created_at_unix": record.payload.created_at_unix,
        "reason": record.payload.reason,
    });
    print_report(&report, json_mode, || {
        println!("valid: true");
        println!("old_node_id: {}", record.payload.old_node_id);
        println!("new_node_id: {}", record.payload.new_node_id);
        println!(
            "nostr_publication_id: {}",
            record.payload.nostr_publication_id
        );
    })
}

fn print_backup_report(
    report: &crate::identity_custody::IdentityBackupReport,
    json_mode: bool,
) -> Result<(), CliError> {
    print_report(report, json_mode, || {
        println!("backup_path:           {}", report.backup_path.display());
        if let Some(path) = &report.recovery_key_path {
            println!("recovery_key_path:     {}", path.display());
        }
        println!("backup_sha256:         {}", report.backup_sha256);
        println!("node_id:               {}", report.node_id);
        println!("nostr_publication_id:  {}", report.nostr_publication_id);
        println!();
        println!("{}", report.recovery_instruction);
    })
}

fn print_report<T: Serialize>(
    report: &T,
    json_mode: bool,
    human: impl FnOnce(),
) -> Result<(), CliError> {
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(report)
                .map_err(|error| CliError::Other(format!("JSON output failed: {error}")))?
        );
    } else {
        human();
    }
    Ok(())
}

fn load_config() -> Result<NodeConfig, CliError> {
    NodeConfig::from_env()
        .map_err(|error| CliError::Other(format!("failed to load node config: {error}")))
}

fn required_path(args: &mut Vec<String>, flag: &str) -> Result<PathBuf, CliError> {
    pop_kv(args, flag)
        .map(PathBuf::from)
        .ok_or_else(|| CliError::BadArgs(format!("{flag} requires a path")))
}

fn open_existing_custody_adapter(
    recovery_key: Option<PathBuf>,
    operator_command: Option<PathBuf>,
    key_id: Option<String>,
    operation: &str,
) -> Result<Box<dyn CustodyAdapter>, CliError> {
    match (recovery_key, operator_command, key_id) {
        (Some(path), None, None) => Ok(Box::new(
            RecoveryKeyFileAdapter::open(&path)
                .map_err(|error| CliError::Other(error.to_string()))?,
        )),
        (None, Some(program), Some(key_id)) => Ok(Box::new(
            OperatorCommandAdapter::new(program, key_id)
                .map_err(|error| CliError::Other(error.to_string()))?,
        )),
        _ => Err(CliError::BadArgs(format!(
            "{operation} requires exactly one of --recovery-key PATH or --operator-command ABSOLUTE_PATH --key-id ID"
        ))),
    }
}

fn require_no_args(args: &[String], command: &str) -> Result<(), CliError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(CliError::BadArgs(format!(
            "unexpected arguments for {command}: {:?}",
            args
        )))
    }
}

fn backup_status_label(state: BackupStatusState) -> &'static str {
    match state {
        BackupStatusState::Current => "current",
        BackupStatusState::Missing => "missing",
        BackupStatusState::StaleIdentity => "stale_identity",
        BackupStatusState::BackupMissing => "backup_missing",
        BackupStatusState::BackupDigestMismatch => "backup_digest_mismatch",
        BackupStatusState::InvalidRecord => "invalid_record",
    }
}

fn absolute(path: &Path) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

#[derive(Serialize)]
struct IdentityPublicReport<'a> {
    status: &'static str,
    node_id: &'a str,
    nostr_publication_id: &'a str,
    identity_dir: &'a Path,
    backup_status: &'static str,
}

fn usage() -> &'static str {
    "usage: froglet-node identity generate|status|capabilities|backup|restore|rotate|verify-continuity [--json]\n\
     backup:  [--output FILE] [--recovery-key FILE | --operator-command ABSOLUTE_PATH --key-id ID]\n\
     restore: --backup FILE (--recovery-key FILE | --operator-command ABSOLUTE_PATH --key-id ID)\n\
     rotate:  (--recovery-key FILE | --operator-command ABSOLUTE_PATH --key-id ID) [--continuity-record FILE] [--reason TEXT]\n\
     verify-continuity: --record FILE"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_requires_live_custody_credentials() {
        let error = run(vec!["rotate".to_string()])
            .expect_err("rotation without recovery credentials must fail argument validation");
        match error {
            CliError::BadArgs(message) => assert!(message.contains(
                "rotate requires exactly one of --recovery-key PATH or --operator-command"
            )),
            other => panic!("expected bad arguments, got {other}"),
        }
    }
}
