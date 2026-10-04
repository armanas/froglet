//! Operator-only configuration for the optional A2A transport.
//! Credentials never enter signed protocol artifacts or public capability JSON.
use serde::Deserialize;
use std::{fmt, path::Path};

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct A2aConfig {
    #[serde(default)]
    pub clients: Vec<A2aClient>,
    #[serde(default)]
    pub providers: Vec<A2aProvider>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct A2aClient {
    pub requester_id: String,
    pub token: String,
    pub offer_hashes: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct A2aProvider {
    pub provider_url: String,
    pub token: String,
    /// Explicit operator authorization for a literal loopback test origin.
    #[serde(default)]
    pub allow_loopback: bool,
}

impl fmt::Debug for A2aConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("A2aConfig")
            .field("clients", &self.clients.len())
            .field("providers", &self.providers.len())
            .finish_non_exhaustive()
    }
}

impl A2aConfig {
    pub fn from_env() -> Result<Self, String> {
        match std::env::var_os("FROGLET_A2A_CONFIG_PATH") {
            Some(path) => Self::load(Path::new(&path)),
            None => Ok(Self::default()),
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| "cannot read A2A credential configuration".to_string())?;
        if !metadata.is_file() || metadata.len() > 64 * 1024 {
            return Err("A2A configuration must be a regular file of at most 64 KiB".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o777 != 0o600 || metadata.uid() != unsafe { libc::geteuid() } {
                return Err(
                    "A2A credential configuration must be owned by this user with mode 0600".into(),
                );
            }
        }
        let bytes = std::fs::read(path)
            .map_err(|_| "cannot read A2A credential configuration".to_string())?;
        let mut config: Self = serde_json::from_slice(&bytes).map_err(|_| {
            "invalid A2A configuration JSON; source withheld because it contains credentials"
                .to_string()
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&mut self) -> Result<(), String> {
        if self.clients.len() > 128 || self.providers.len() > 128 {
            return Err("A2A configuration supports at most 128 clients and providers".into());
        }
        let mut tokens = std::collections::HashSet::new();
        for client in &self.clients {
            if !is_hash(&client.requester_id)
                || client.offer_hashes.is_empty()
                || client.offer_hashes.len() > 128
                || client.offer_hashes.iter().any(|hash| !is_hash(hash))
            {
                return Err("A2A clients require a lowercase requester public key and 1–128 exact lowercase Offer hashes".into());
            }
            validate_token(&client.token)?;
            if !tokens.insert(&client.token) {
                return Err("A2A client tokens must be unique".into());
            }
        }
        let mut urls = std::collections::HashSet::new();
        for provider in &mut self.providers {
            validate_token(&provider.token)?;
            let parsed = reqwest::Url::parse(&provider.provider_url)
                .map_err(|_| "invalid A2A provider URL".to_string())?;
            let loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "[::1]" | "::1"));
            if (parsed.scheme() != "https"
                && !(parsed.scheme() == "http" && loopback && provider.allow_loopback))
                || (provider.allow_loopback && !loopback)
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.path() != "/"
            {
                return Err("A2A provider URL must be an HTTPS origin (or explicit loopback HTTP for local tests), without credentials, query, fragment or path".into());
            }
            provider.provider_url = parsed.as_str().trim_end_matches('/').to_string();
            if !urls.insert(provider.provider_url.clone()) {
                return Err("A2A provider URLs must be unique".into());
            }
        }
        Ok(())
    }
}

pub(crate) fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn validate_token(value: &str) -> Result<(), String> {
    if !(32..=256).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(
            "A2A bearer tokens must contain 32–256 ASCII letters, digits, '-' or '_'".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2a_configuration_is_disabled_and_redacted_by_default() {
        let mut config = A2aConfig::default();
        config.validate().unwrap();
        assert!(config.clients.is_empty());
        assert!(config.providers.is_empty());
        config.providers.push(A2aProvider {
            provider_url: "https://example.org/".into(),
            token: "private-credential-never-in-debug-output".into(),
            allow_loopback: false,
        });
        config.validate().unwrap();
        assert_eq!(config.providers[0].provider_url, "https://example.org");
        assert!(!format!("{config:?}").contains("private-credential"));
    }

    #[test]
    fn a2a_configuration_rejects_unbounded_scopes_and_credential_urls() {
        let mut config = A2aConfig {
            clients: vec![A2aClient {
                requester_id: "aa".repeat(32),
                token: "b".repeat(32),
                offer_hashes: vec![],
            }],
            providers: vec![],
        };
        assert!(config.validate().is_err());
        config.clients[0].offer_hashes.push("cc".repeat(32));
        config.validate().unwrap();
        for url in [
            "http://example.org",
            "https://user:secret@example.org",
            "https://example.org/a2a",
            "https://example.org?secret=1",
        ] {
            let mut config = A2aConfig {
                clients: vec![],
                providers: vec![A2aProvider {
                    provider_url: url.into(),
                    token: "b".repeat(32),
                    allow_loopback: false,
                }],
            };
            assert!(config.validate().is_err(), "{url}");
        }
    }
}
