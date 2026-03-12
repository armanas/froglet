use crate::{config::NodeConfig, crypto};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use std::{fs, path::Path};

#[derive(Debug, Clone)]
pub struct NodeIdentity {
    signing_key: SigningKey,
    public_key_hex: String,
}

impl NodeIdentity {
    pub fn load_or_create(config: &NodeConfig) -> Result<Self, String> {
        ensure_dir(&config.storage.data_dir, 0o700)?;
        ensure_dir(&config.storage.identity_dir, 0o700)?;

        let signing_key = if config.storage.identity_seed_path.exists() {
            load_signing_key(&config.storage.identity_seed_path)?
        } else if config.identity.auto_generate {
            let signing_key = SigningKey::generate(&mut OsRng);
            persist_signing_key(&config.storage.identity_seed_path, &signing_key)?;
            signing_key
        } else {
            return Err(format!(
                "Node identity is required but {} does not exist",
                config.storage.identity_seed_path.display()
            ));
        };

        let public_key_hex = crypto::public_key_hex(&signing_key);

        Ok(Self {
            signing_key,
            public_key_hex,
        })
    }

    pub fn node_id(&self) -> &str {
        &self.public_key_hex
    }

    pub fn public_key_hex(&self) -> &str {
        &self.public_key_hex
    }

    pub fn sign_message_hex(&self, message: &[u8]) -> String {
        crypto::sign_message_hex(&self.signing_key, message)
    }
}

fn load_signing_key(path: &Path) -> Result<SigningKey, String> {
    let seed_hex = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read node identity seed {}: {e}", path.display()))?;
    let seed_hex = seed_hex.trim();
    let bytes = hex::decode(seed_hex)
        .map_err(|e| format!("Invalid hex in node identity seed {}: {e}", path.display()))?;

    if bytes.len() != 32 {
        return Err(format!(
            "Invalid node identity seed length in {}: expected 32 bytes, got {}",
            path.display(),
            bytes.len()
        ));
    }

    let seed: [u8; 32] = bytes.try_into().unwrap();
    Ok(SigningKey::from_bytes(&seed))
}

fn persist_signing_key(path: &Path, signing_key: &SigningKey) -> Result<(), String> {
    let seed_hex = hex::encode(signing_key.to_bytes());

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
        fs::write(path, seed_hex)
            .map_err(|e| format!("Failed to write node identity seed {}: {e}", path.display()))?;
    }

    Ok(())
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
        CashuConfig, DiscoveryMode, IdentityConfig, NetworkMode, NodeConfig, PaymentBackend,
        PricingConfig, StorageConfig,
    };
    use std::path::PathBuf;

    #[test]
    fn test_identity_sign_and_load() {
        let temp_dir = std::env::temp_dir().join(format!("froglet-test-{}", std::process::id()));
        let config = NodeConfig {
            network_mode: NetworkMode::Clearnet,
            listen_addr: "127.0.0.1:8080".into(),
            discovery_mode: DiscoveryMode::None,
            identity: IdentityConfig {
                auto_generate: true,
            },
            marketplace: None,
            pricing: PricingConfig {
                events_query: 0,
                execute_wasm: 0,
            },
            payment_backend: PaymentBackend::None,
            execution_timeout_secs: 10,
            cashu: CashuConfig {
                mint_allowlist: Vec::new(),
                remote_checkstate: false,
                request_timeout_secs: 5,
            },
            storage: StorageConfig {
                data_dir: temp_dir.clone(),
                db_path: temp_dir.join("node.db"),
                identity_dir: temp_dir.join("identity"),
                identity_seed_path: temp_dir.join("identity/ed25519.seed"),
                runtime_dir: temp_dir.join("runtime"),
                runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
                tor_dir: temp_dir.join("tor"),
            },
        };

        let identity = NodeIdentity::load_or_create(&config).unwrap();
        let reloaded = NodeIdentity::load_or_create(&config).unwrap();
        assert_eq!(identity.node_id(), reloaded.node_id());

        let _ = std::fs::remove_dir_all(PathBuf::from(temp_dir));
    }
}
