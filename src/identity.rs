use crate::{config::NodeConfig, crypto};
use std::{fs, path::Path, time::UNIX_EPOCH};
use zeroize::Zeroize;

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
        )?;
        let nostr_publication_signing_key = load_or_create_signing_key(
            &config.storage.nostr_publication_seed_path,
            config.identity.auto_generate,
            "Nostr publication identity",
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
) -> Result<crypto::NodeSigningKey, String> {
    if path.exists() {
        load_signing_key(path)
    } else if auto_generate {
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
    #[test]
    fn test_identity_sign_and_load() {
        let temp_dir = std::env::temp_dir().join(format!("froglet-test-{}", std::process::id()));
        let config = NodeConfig {
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
            identity: IdentityConfig {
                auto_generate: true,
            },
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
                data_dir: temp_dir.clone(),
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
        };

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
}
