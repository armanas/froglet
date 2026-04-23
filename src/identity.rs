use crate::{config::NodeConfig, crypto};
use std::{fs, path::Path, time::UNIX_EPOCH};
use zeroize::Zeroize;

/// Env var that, when set on first boot, seeds the node identity from a hex
/// string instead of auto-generating a fresh one. Ignored if the identity
/// seed file already exists — file wins. Expected format: 64 hex chars
/// (32 bytes).
pub const NODE_IDENTITY_SEED_ENV: &str = "FROGLET_IDENTITY_SEED_HEX";

#[derive(Clone)]
pub struct NodeIdentity {
    signing_key: crypto::NodeSigningKey,
    public_key_hex: String,
    compressed_public_key_hex: String,
    nostr_publication_signing_key: crypto::NodeSigningKey,
    nostr_publication_public_key_hex: String,
    nostr_publication_created_at: i64,
}

impl NodeIdentity {
    pub fn load_or_create(config: &NodeConfig) -> Result<Self, String> {
        ensure_dir(&config.storage.data_dir, config.storage.data_dir_mode())?;
        ensure_dir(&config.storage.identity_dir, 0o700)?;

        let signing_key = load_or_create_signing_key(
            &config.storage.identity_seed_path,
            config.identity.auto_generate,
            "node identity",
            Some(NODE_IDENTITY_SEED_ENV),
        )?;
        let nostr_publication_signing_key = load_or_create_signing_key(
            &config.storage.nostr_publication_seed_path,
            config.identity.auto_generate,
            "Nostr publication identity",
            None,
        )?;

        let public_key_hex = crypto::public_key_hex(&signing_key);
        let compressed_public_key_hex = crypto::compressed_public_key_hex(&signing_key);
        let nostr_publication_public_key_hex =
            crypto::public_key_hex(&nostr_publication_signing_key);
        let nostr_publication_created_at =
            file_modified_timestamp_secs(&config.storage.nostr_publication_seed_path)?;

        Ok(Self {
            signing_key,
            public_key_hex,
            compressed_public_key_hex,
            nostr_publication_signing_key,
            nostr_publication_public_key_hex,
            nostr_publication_created_at,
        })
    }

    pub fn node_id(&self) -> &str {
        &self.public_key_hex
    }

    pub fn public_key_hex(&self) -> &str {
        &self.public_key_hex
    }

    pub fn compressed_public_key_hex(&self) -> &str {
        &self.compressed_public_key_hex
    }

    pub fn sign_message_hex(&self, message: &[u8]) -> String {
        crypto::sign_message_hex(&self.signing_key, message)
    }

    pub fn nostr_publication_key_hex(&self) -> &str {
        &self.nostr_publication_public_key_hex
    }

    pub fn nostr_publication_created_at(&self) -> i64 {
        self.nostr_publication_created_at
    }

    pub fn sign_nostr_publication_message_hex(&self, message: &[u8]) -> String {
        crypto::sign_message_hex(&self.nostr_publication_signing_key, message)
    }

    /// Derives a keyed HMAC-SHA256 hex string using the node's identity seed as key.
    /// This ensures the output is unpredictable without knowledge of the private key.
    pub fn keyed_hmac_hex(&self, message: &[u8]) -> String {
        let mut seed = crypto::signing_key_seed_bytes(&self.signing_key);
        let result = crypto::hmac_sha256_hex(&seed, message);
        seed.zeroize();
        result
    }
}

fn load_signing_key(path: &Path) -> Result<crypto::NodeSigningKey, String> {
    let mut seed_hex = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read node identity seed {}: {e}", path.display()))?;
    let trimmed = seed_hex.trim().to_string();
    seed_hex.zeroize();
    let mut bytes = hex::decode(&trimmed)
        .map_err(|e| format!("Invalid hex in node identity seed {}: {e}", path.display()))?;

    if bytes.len() != 32 {
        bytes.zeroize();
        return Err(format!(
            "Invalid node identity seed length in {}: expected 32 bytes, got {}",
            path.display(),
            bytes.len()
        ));
    }

    let mut seed: [u8; 32] = bytes
        .try_into()
        .expect("length validated as 32 bytes above");
    let result = crypto::signing_key_from_seed_bytes(&seed);
    seed.zeroize();
    result
}

fn persist_signing_key(path: &Path, signing_key: &crypto::NodeSigningKey) -> Result<(), String> {
    let mut seed_bytes = crypto::signing_key_seed_bytes(signing_key);
    let mut seed_hex = hex::encode(seed_bytes.as_slice());
    seed_bytes.zeroize();

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| {
                format!(
                    "Failed to create identity seed file {}: {e}",
                    path.display()
                )
            })?;
        file.write_all(seed_hex.as_bytes())
            .map_err(|e| format!("Failed to write identity seed {}: {e}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, seed_hex.as_bytes())
            .map_err(|e| format!("Failed to write node identity seed {}: {e}", path.display()))?;
    }

    seed_hex.zeroize();
    Ok(())
}

fn load_or_create_signing_key(
    path: &Path,
    auto_generate: bool,
    label: &str,
    env_seed_var: Option<&str>,
) -> Result<crypto::NodeSigningKey, String> {
    // File wins if present. This matches ephemeral-FS deployments (Lightsail
    // Container Service and similar) where every redeploy starts with a fresh
    // FS, so the env-var seed re-creates the file each deploy without drift.
    if path.exists() {
        return load_signing_key(path);
    }

    if let Some(var_name) = env_seed_var
        && let Ok(hex_str) = std::env::var(var_name)
    {
        let mut seed =
            seed_from_hex_str(&hex_str).map_err(|e| format!("Invalid {var_name}: {e}"))?;
        let signing_key = crypto::signing_key_from_seed_bytes(&seed)?;
        seed.zeroize();
        persist_signing_key(path, &signing_key)?;
        tracing::info!(
            identity = %label,
            path = %path.display(),
            env = %var_name,
            "seeded identity from environment",
        );
        return Ok(signing_key);
    }

    if auto_generate {
        let signing_key = crypto::generate_signing_key();
        persist_signing_key(path, &signing_key)?;
        Ok(signing_key)
    } else {
        Err(format!(
            "{label} is required but {} does not exist",
            path.display()
        ))
    }
}

/// Parse a hex-encoded secp256k1 seed (32 bytes / 64 hex chars). Whitespace
/// around the value is trimmed. Zeroized-decode is not needed here: the hex
/// crate does not hold the decoded bytes after return.
fn seed_from_hex_str(hex_str: &str) -> Result<[u8; 32], String> {
    let trimmed = hex_str.trim();
    let bytes = hex::decode(trimmed).map_err(|e| format!("invalid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "expected 32 bytes (64 hex chars), got {} bytes",
            bytes.len()
        ));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(seed)
}

fn file_modified_timestamp_secs(path: &Path) -> Result<i64, String> {
    let modified = fs::metadata(path)
        .map_err(|e| format!("Failed to read metadata for {}: {e}", path.display()))?
        .modified()
        .map_err(|e| format!("Failed to read modified time for {}: {e}", path.display()))?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("Invalid modified time for {}: {e}", path.display()))?;
    i64::try_from(duration.as_secs())
        .map_err(|_| format!("Modified time overflow for {}", path.display()))
}

fn ensure_dir(path: &Path, mode: u32) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|e| format!("Failed to create directory {}: {e}", path.display()))?;
    set_mode(path, mode)
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path)
            .map_err(|e| format!("Failed to read metadata for {}: {e}", path.display()))?;
        let mut perms = metadata.permissions();
        perms.set_mode(mode);
        fs::set_permissions(path, perms)
            .map_err(|e| format!("Failed to set permissions on {}: {e}", path.display()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig, PaymentBackend,
        PricingConfig, StorageConfig, TorSidecarConfig, WasmConfig,
    };
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Serializes env-var mutations across identity tests. Matches the pattern
    /// used in `src/tls.rs::PROXY_ENV_LOCK`. Required because `cargo test`
    /// runs tests in parallel and `std::env::{set_var,remove_var}` mutate
    /// process-global state.
    static IDENTITY_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn test_temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "froglet-identity-test-{}-{tag}",
            std::process::id(),
        ))
    }

    fn test_config(temp_dir: &Path, auto_generate: bool) -> NodeConfig {
        NodeConfig {
            network_mode: NetworkMode::Clearnet,
            listen_addr: "127.0.0.1:8080".into(),
            public_base_url: None,
            runtime_listen_addr: "127.0.0.1:8081".into(),
            runtime_allow_non_loopback: false,
            http_ca_cert_path: None,
            tor: TorSidecarConfig {
                binary_path: "tor".into(),
                backend_listen_addr: "127.0.0.1:8082".into(),
                startup_timeout_secs: 90,
            },
            identity: IdentityConfig { auto_generate },
            pricing: PricingConfig {
                events_query: 0,
                execute_wasm: 0,
            },
            payment_backends: vec![PaymentBackend::None],
            execution_timeout_secs: 10,
            lightning: LightningConfig {
                mode: LightningMode::Mock,
                destination_identity: None,
                base_invoice_expiry_secs: 300,
                success_hold_expiry_secs: 300,
                min_final_cltv_expiry: 18,
                sync_interval_ms: 1_000,
                lnd_rest: None,
            },
            x402: None,
            stripe: None,
            storage: StorageConfig {
                data_dir: temp_dir.to_path_buf(),
                db_path: temp_dir.join("node.db"),
                identity_dir: temp_dir.join("identity"),
                identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
                nostr_publication_seed_path: temp_dir
                    .join("identity/nostr-publication.secp256k1.seed"),
                runtime_dir: temp_dir.join("runtime"),
                runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
                consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
                provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
                tor_dir: temp_dir.join("tor"),
                host_readable_control_token: false,
            },
            wasm: WasmConfig {
                policy_path: None,
                policy: None,
            },
            confidential: crate::confidential::ConfidentialConfig {
                policy_path: None,
                policy: None,
                session_ttl_secs: 300,
            },
            marketplace_url: None,
            postgres_mounts: std::collections::BTreeMap::new(),
            session_pool: Default::default(),
            hosted_trial_origin_secret: None,
        }
    }

    #[test]
    fn test_identity_sign_and_load() {
        let _guard = IDENTITY_ENV_LOCK.lock().unwrap();
        // Defensively clear any leaked env var from an earlier test.
        unsafe {
            std::env::remove_var(NODE_IDENTITY_SEED_ENV);
        }
        let temp_dir = test_temp_dir("sign-and-load");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let config = test_config(&temp_dir, true);

        let identity = NodeIdentity::load_or_create(&config).unwrap();
        let reloaded = NodeIdentity::load_or_create(&config).unwrap();
        assert_eq!(identity.node_id(), reloaded.node_id());
        assert_eq!(
            identity.nostr_publication_key_hex(),
            reloaded.nostr_publication_key_hex()
        );
        assert_ne!(identity.node_id(), identity.nostr_publication_key_hex());

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn identity_seed_env_var_creates_key_on_first_boot() {
        let _guard = IDENTITY_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var(NODE_IDENTITY_SEED_ENV);
        }
        let temp_dir = test_temp_dir("env-first-boot");
        let _ = std::fs::remove_dir_all(&temp_dir);
        // auto_generate=true so the Nostr publication seed (which is NOT
        // env-driven in this feature) can auto-generate. The node identity
        // is still driven by the env var; we verify that by comparing
        // node_id against the deterministic pubkey derived from the seed.
        let config = test_config(&temp_dir, true);

        // Deterministic test seed: 32 bytes, all 0x42.
        let mut expected_seed = [0x42u8; 32];
        let expected_key = crypto::signing_key_from_seed_bytes(&expected_seed)
            .expect("fixed seed must be a valid secp256k1 seed");
        let expected_node_id = crypto::public_key_hex(&expected_key);
        expected_seed.zeroize();

        let seed_hex = "42".repeat(32);
        unsafe {
            std::env::set_var(NODE_IDENTITY_SEED_ENV, &seed_hex);
        }
        let identity_result = NodeIdentity::load_or_create(&config);
        unsafe {
            std::env::remove_var(NODE_IDENTITY_SEED_ENV);
        }

        let identity = identity_result.expect("env-seeded identity should load");

        assert_eq!(
            identity.node_id(),
            expected_node_id,
            "env-seeded node_id must match deterministic pubkey from the fixed seed \
             (proves the env var drove key creation, not auto-generate)"
        );

        assert!(
            config.storage.identity_seed_path.exists(),
            "identity seed file must be written on first boot"
        );

        // Reloading without the env var: the persisted file must reproduce
        // the same identity. This is the "file wins on subsequent boots"
        // guarantee.
        let reloaded = NodeIdentity::load_or_create(&config)
            .expect("reload via persisted file should succeed");
        assert_eq!(
            identity.node_id(),
            reloaded.node_id(),
            "reload must produce the same identity via the seed file"
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn identity_seed_env_var_rejects_invalid_hex() {
        let _guard = IDENTITY_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var(NODE_IDENTITY_SEED_ENV);
        }
        let temp_dir = test_temp_dir("env-invalid-hex");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let config = test_config(&temp_dir, false);

        unsafe {
            std::env::set_var(NODE_IDENTITY_SEED_ENV, "not-hex-at-all");
        }
        let result = NodeIdentity::load_or_create(&config);
        unsafe {
            std::env::remove_var(NODE_IDENTITY_SEED_ENV);
        }

        // `NodeIdentity` does not implement Debug, so we can't use
        // `expect_err` here — pattern-match instead.
        let err = match result {
            Ok(_) => panic!("invalid hex must fail, got Ok"),
            Err(e) => e,
        };
        assert!(
            err.contains(NODE_IDENTITY_SEED_ENV),
            "error must name the env var, got: {err}"
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn identity_seed_env_var_rejects_wrong_length() {
        let _guard = IDENTITY_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var(NODE_IDENTITY_SEED_ENV);
        }
        let temp_dir = test_temp_dir("env-wrong-length");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let config = test_config(&temp_dir, false);

        // 30 bytes — wrong length.
        let seed_hex = "42".repeat(30);
        unsafe {
            std::env::set_var(NODE_IDENTITY_SEED_ENV, &seed_hex);
        }
        let result = NodeIdentity::load_or_create(&config);
        unsafe {
            std::env::remove_var(NODE_IDENTITY_SEED_ENV);
        }

        let err = match result {
            Ok(_) => panic!("wrong length must fail, got Ok"),
            Err(e) => e,
        };
        assert!(
            err.contains("32 bytes") || err.contains("expected 32"),
            "error must mention expected length, got: {err}"
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn identity_file_wins_over_env_var() {
        let _guard = IDENTITY_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var(NODE_IDENTITY_SEED_ENV);
        }
        let temp_dir = test_temp_dir("file-wins");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let config = test_config(&temp_dir, true);

        // First boot without env var: auto-generate and persist.
        let original = NodeIdentity::load_or_create(&config).expect("initial auto-generate");
        let original_node_id = original.node_id().to_string();

        // Second boot with a DIFFERENT env-var seed set: file must win.
        let different_seed_hex = "aa".repeat(32);
        unsafe {
            std::env::set_var(NODE_IDENTITY_SEED_ENV, &different_seed_hex);
        }
        let reloaded_result = NodeIdentity::load_or_create(&config);
        unsafe {
            std::env::remove_var(NODE_IDENTITY_SEED_ENV);
        }

        let reloaded = reloaded_result.expect("reload should succeed");
        assert_eq!(
            reloaded.node_id(),
            original_node_id,
            "file-persisted identity must win over env var",
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
