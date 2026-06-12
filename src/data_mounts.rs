use serde_json::{Value, json};
use std::path::PathBuf;

use crate::execution::ExecutionWorkload;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DataMountPlan {
    pub env: Vec<(String, String)>,
    pub readonly_paths: Vec<PathBuf>,
    pub writable_paths: Vec<PathBuf>,
    pub needs_network: bool,
}

pub(crate) fn collect_data_mount_plan(
    execution: &ExecutionWorkload,
    granted_access: &[String],
) -> Result<DataMountPlan, String> {
    let mut plan = DataMountPlan::default();

    for mount in &execution.mounts {
        let kind = mount.kind.to_ascii_lowercase();
        let Some(kind_policy) = MountKindPolicy::for_kind(&kind) else {
            continue;
        };
        let capability = format!(
            "mount.{}.{}.{}",
            mount.kind,
            if mount.read_only { "read" } else { "write" },
            mount.handle
        );
        if !granted_access.iter().any(|c| c == &capability) {
            continue;
        }

        let env_key = format!("FROGLET_MOUNT_{kind}_{}", mount.handle);
        let binding = match std::env::var(&env_key) {
            Ok(binding) if !binding.trim().is_empty() => binding,
            _ if kind_policy.requires_configured_binding => {
                return Err(format!(
                    "granted {kind} mount '{}' is not configured; set {env_key}",
                    mount.handle
                ));
            }
            _ => continue,
        };

        validate_binding(&kind, &binding)?;
        let safe_handle = mount.handle.to_ascii_uppercase();
        plan.env
            .push((format!("FROGLET_MOUNT_{safe_handle}_URL"), binding.clone()));
        plan.env.push((
            format!("FROGLET_MOUNT_{safe_handle}_READ_ONLY"),
            if mount.read_only { "true" } else { "false" }.to_string(),
        ));

        match kind_policy.access {
            MountAccessPolicy::Network => {
                return Err(format!(
                    "{kind} mount '{}' is network-backed and is disabled until endpoint-scoped proxying is available",
                    mount.handle
                ));
            }
            MountAccessPolicy::SqliteFile => {
                let db_path = PathBuf::from(&binding);
                if !db_path.is_file() {
                    return Err(format!(
                        "sqlite mount '{}' must point to an existing database file",
                        mount.handle
                    ));
                }
                if mount.read_only {
                    plan.readonly_paths.push(db_path);
                } else {
                    plan.writable_paths.push(db_path);
                }
            }
        }
    }

    Ok(plan)
}

pub(crate) fn execution_mount_context(
    execution: &ExecutionWorkload,
    granted_access: &[String],
) -> Value {
    let mounts = execution
        .mounts
        .iter()
        .filter(|mount| {
            let handle = format!(
                "mount.{}.{}.{}",
                mount.kind,
                if mount.read_only { "read" } else { "write" },
                mount.handle
            );
            granted_access.iter().any(|value| value == &handle)
        })
        .map(|mount| {
            (
                mount.handle.clone(),
                json!({
                    "kind": mount.kind,
                    "read_only": mount.read_only,
                    "binding": mount.binding,
                }),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    Value::Object(mounts)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MountAccessPolicy {
    Network,
    SqliteFile,
}

#[derive(Debug, Clone, Copy)]
struct MountKindPolicy {
    access: MountAccessPolicy,
    requires_configured_binding: bool,
}

impl MountKindPolicy {
    fn for_kind(kind: &str) -> Option<Self> {
        match kind {
            "postgres" | "s3" | "redis" => Some(Self {
                access: MountAccessPolicy::Network,
                requires_configured_binding: true,
            }),
            "sqlite" => Some(Self {
                access: MountAccessPolicy::SqliteFile,
                requires_configured_binding: true,
            }),
            _ => None,
        }
    }
}

fn validate_binding(kind: &str, binding: &str) -> Result<(), String> {
    match kind {
        "postgres" => {
            if binding.starts_with("postgres://") || binding.starts_with("postgresql://") {
                Ok(())
            } else {
                Err("postgres mount bindings must use postgres:// or postgresql://".to_string())
            }
        }
        "s3" => {
            if binding.starts_with("s3://") {
                Ok(())
            } else {
                Err("s3 mount bindings must use s3://".to_string())
            }
        }
        "redis" => {
            if binding.starts_with("redis://") || binding.starts_with("rediss://") {
                Ok(())
            } else {
                Err("redis mount bindings must use redis:// or rediss://".to_string())
            }
        }
        "sqlite" => Ok(()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{
        ExecutionEntrypoint, ExecutionEntrypointKind, ExecutionPackageKind, ExecutionSecurity,
        ExecutionWorkload,
    };
    use froglet_protocol::ExecutionRuntime;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<String>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            match self.previous.as_deref() {
                Some(value) => unsafe {
                    std::env::set_var(self.key, value);
                },
                None => unsafe {
                    std::env::remove_var(self.key);
                },
            }
        }
    }

    #[test]
    fn network_mount_fails_closed_when_capability_granted_without_binding() {
        let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _env = ScopedEnvVar::unset("FROGLET_MOUNT_postgres_events");
        let execution = ExecutionWorkload {
            mounts: vec![crate::execution::ExecutionMount {
                handle: "events".to_string(),
                kind: "postgres".to_string(),
                read_only: true,
                binding: None,
            }],
            ..execution_for_mount_tests()
        };

        let error =
            collect_data_mount_plan(&execution, &["mount.postgres.read.events".to_string()])
                .expect_err("missing network binding must fail closed");

        assert!(error.contains("FROGLET_MOUNT_postgres_events"));
    }

    #[test]
    fn network_mount_is_ignored_when_capability_not_granted() {
        let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _env = ScopedEnvVar::unset("FROGLET_MOUNT_postgres_events");
        let execution = ExecutionWorkload {
            mounts: vec![crate::execution::ExecutionMount {
                handle: "events".to_string(),
                kind: "postgres".to_string(),
                read_only: true,
                binding: None,
            }],
            ..execution_for_mount_tests()
        };

        let plan = collect_data_mount_plan(&execution, &[]).expect("no granted mount");

        assert!(plan.env.is_empty());
        assert!(!plan.needs_network);
    }

    #[test]
    fn sqlite_writable_mount_grants_only_database_file() {
        let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let tempdir = tempfile::Builder::new()
            .prefix("froglet-sqlite-mount-")
            .tempdir()
            .expect("tempdir");
        let db_path = tempdir.path().join("cache.sqlite");
        std::fs::write(&db_path, b"").expect("sqlite placeholder");
        let db_path = db_path.to_string_lossy().to_string();
        let _env = ScopedEnvVar::set("FROGLET_MOUNT_sqlite_cache", &db_path);
        let execution = ExecutionWorkload {
            mounts: vec![crate::execution::ExecutionMount {
                handle: "cache".to_string(),
                kind: "sqlite".to_string(),
                read_only: false,
                binding: None,
            }],
            ..execution_for_mount_tests()
        };

        let plan = collect_data_mount_plan(&execution, &["mount.sqlite.write.cache".to_string()])
            .expect("sqlite plan");

        assert_eq!(plan.writable_paths, vec![PathBuf::from(&db_path)]);
        assert!(plan.readonly_paths.is_empty());
        assert_ne!(plan.writable_paths[0], tempdir.path());
    }

    #[test]
    fn sqlite_readonly_mount_grants_readonly_database_file() {
        let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let tempdir = tempfile::Builder::new()
            .prefix("froglet-sqlite-mount-")
            .tempdir()
            .expect("tempdir");
        let db_path = tempdir.path().join("cache.sqlite");
        std::fs::write(&db_path, b"").expect("sqlite placeholder");
        let db_path = db_path.to_string_lossy().to_string();
        let _env = ScopedEnvVar::set("FROGLET_MOUNT_sqlite_cache", &db_path);
        let execution = ExecutionWorkload {
            mounts: vec![crate::execution::ExecutionMount {
                handle: "cache".to_string(),
                kind: "sqlite".to_string(),
                read_only: true,
                binding: None,
            }],
            ..execution_for_mount_tests()
        };

        let plan = collect_data_mount_plan(&execution, &["mount.sqlite.read.cache".to_string()])
            .expect("sqlite plan");

        assert_eq!(plan.readonly_paths, vec![PathBuf::from(&db_path)]);
        assert!(plan.writable_paths.is_empty());
    }

    fn execution_for_mount_tests() -> ExecutionWorkload {
        ExecutionWorkload {
            schema_version: "froglet/v1".to_string(),
            workload_kind: "execution.v1".to_string(),
            runtime: ExecutionRuntime::Python,
            package_kind: ExecutionPackageKind::InlineSource,
            entrypoint: ExecutionEntrypoint {
                kind: ExecutionEntrypointKind::Handler,
                value: "handler".to_string(),
            },
            contract_version: "froglet.python.handler_json.v1".to_string(),
            input_format: "application/json".to_string(),
            input_hash: "00".repeat(32),
            requested_access: Vec::new(),
            security: ExecutionSecurity::default(),
            mounts: Vec::new(),
            inline_source: Some("def handler(event, ctx):\n    return event\n".to_string()),
            module_hash: None,
            module_bytes_hex: None,
            source_hash: None,
            oci_reference: None,
            oci_digest: None,
            builtin_name: None,
            input: serde_json::Value::Null,
        }
    }
}
