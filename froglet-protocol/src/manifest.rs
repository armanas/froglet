//! Manifest parser + validator for `froglet.toml` (project-level) and
//! `froglet-service.toml` v3 (per-service). See [`docs/MANIFEST.md`] for
//! the normative spec.
//!
//! The protocol crate is the natural home for these types: they're the
//! durable contract between the author and the publish engine, with no
//! runtime / daemon / network dependencies. Both the open-source
//! `froglet-publish-engine` and any future third-party tooling parse the
//! same types.
//!
//! Design rules:
//!
//! - Field names mirror `ProviderManagedOfferDefinition` in
//!   `froglet/src/api/types.rs` 1:1 so the publish engine never has to
//!   translate names.
//! - Validation is a two-pass process: structural deserialization with
//!   `deny_unknown_fields` to catch typos, then a `validate()` method
//!   that enforces conditional requirements (e.g., python +
//!   inline_source requires `entrypoint`).
//! - `v2` schema versions on the service manifest are accepted with
//!   deprecation warnings, never hard errors. v0.4 will drop v2 support.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical `schema_version` for project manifests.
pub const PROJECT_MANIFEST_SCHEMA_V1: &str = "froglet/v1";

/// Canonical `schema_version` for service manifests.
pub const SERVICE_MANIFEST_SCHEMA_V3: &str = "froglet-service/v3";

/// Legacy `schema_version` accepted with deprecation warnings.
pub const SERVICE_MANIFEST_SCHEMA_V2: &str = "froglet-service/v2";

/// Hard rejection error for a manifest that cannot be acted on.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest parse failed: {0}")]
    Parse(String),
    #[error("schema_version mismatch: got {got:?}, expected {expected:?}")]
    SchemaVersion { got: String, expected: &'static str },
    #[error("identifier {field:?} = {value:?} is invalid: {reason}")]
    Identifier {
        field: &'static str,
        value: String,
        reason: &'static str,
    },
    #[error("invalid {field:?} = {value:?}: {reason}")]
    InvalidValue {
        field: &'static str,
        value: String,
        reason: String,
    },
    #[error(
        "runtime + package_kind combo {runtime:?} + {package_kind:?} not supported; allowed: {allowed:?}"
    )]
    RuntimeMismatch {
        runtime: String,
        package_kind: String,
        allowed: &'static [&'static str],
    },
    #[error("missing required field {field:?} for {context:?}")]
    MissingRequired {
        field: &'static str,
        context: String,
    },
    #[error("settlement.method = {method:?} is not supported; allowed: \"none\", \"lightning\"")]
    UnsupportedSettlement { method: String },
    #[error(
        "hosting.default = {choice:?} requires section [hosting.{choice}] with field {missing:?}"
    )]
    MissingHostingConfig {
        choice: String,
        missing: &'static str,
    },
    /// price.currency value is not in the allowed set {"sat", "usd"}.
    #[error("price.currency = {value:?} is not supported; allowed: \"sat\", \"usd\"")]
    InvalidPriceCurrency { value: String },
}

/// Soft warning. Returned alongside a parsed manifest; not a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestWarning {
    /// v2 service manifest loaded; v3 sections defaulted.
    LegacyV2Service { missing_section: &'static str },
}

// ── Project manifest (froglet.toml) ─────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    pub schema_version: String,
    pub project: ProjectSection,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectSection {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub identity: Option<IdentityConfig>,
    #[serde(default)]
    pub marketplace: Option<MarketplaceConfig>,
    #[serde(default)]
    pub defaults: Option<ProjectDefaults>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// One of: "auto", "env:NAME", "file:PATH".
    pub strategy: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct ProjectDefaults {
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub hosting: Option<String>,
    #[serde(default)]
    pub settlement: Option<String>,
}

impl ProjectManifest {
    /// Parse a TOML string into a `ProjectManifest` + validate it.
    pub fn from_toml(input: &str) -> Result<(Self, Vec<ManifestWarning>), ManifestError> {
        let manifest: Self =
            toml::from_str(input).map_err(|e| ManifestError::Parse(e.to_string()))?;
        let warnings = manifest.validate()?;
        Ok((manifest, warnings))
    }

    fn validate(&self) -> Result<Vec<ManifestWarning>, ManifestError> {
        if self.schema_version != PROJECT_MANIFEST_SCHEMA_V1 {
            return Err(ManifestError::SchemaVersion {
                got: self.schema_version.clone(),
                expected: PROJECT_MANIFEST_SCHEMA_V1,
            });
        }
        validate_identifier("project.name", &self.project.name)?;
        if let Some(identity) = &self.project.identity {
            validate_identity_strategy(&identity.strategy)?;
        }
        if let Some(marketplace) = &self.project.marketplace {
            validate_marketplace_url(&marketplace.url)?;
        }
        if let Some(defaults) = &self.project.defaults {
            if let Some(hosting) = &defaults.hosting {
                validate_hosting_choice(hosting)?;
            }
            if let Some(settlement) = &defaults.settlement {
                validate_settlement_method(settlement)?;
            }
            if let Some(runtime) = &defaults.runtime {
                validate_runtime(runtime)?;
            }
        }
        Ok(Vec::new())
    }
}

// ── Service manifest (froglet-service.toml v3) ──────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServiceManifest {
    pub schema_version: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub service_id: String,
    #[serde(default)]
    pub offer_id: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,

    // Runtime / packaging
    pub runtime: String,
    pub package_kind: String,
    #[serde(default)]
    pub entrypoint_kind: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub contract_version: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub publication_state: Option<String>,

    // v3 additions
    #[serde(default)]
    pub hosting: Option<HostingSection>,
    #[serde(default)]
    pub settlement: Option<SettlementSection>,
    #[serde(default)]
    pub marketplace: Option<MarketplaceConfig>,
    #[serde(default)]
    pub limits: Option<LimitsSection>,
    #[serde(default)]
    pub price: Option<PriceSection>,

    // Free-shape JSON schemas (deliberately not deny_unknown_fields)
    #[serde(default)]
    pub input_schema: Option<Value>,
    #[serde(default)]
    pub output_schema: Option<Value>,

    // OCI-specific (conditional)
    #[serde(default)]
    pub oci: Option<OciSection>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostingSection {
    /// One of: "local" | "tor" | "self" | "managed" | "fly".
    pub default: String,
    #[serde(default)]
    pub local: Option<LocalHostingConfig>,
    #[serde(default)]
    pub tor: Option<TorHostingConfig>,
    #[serde(default, rename = "self")]
    pub self_hosted: Option<SelfHostingConfig>,
    #[serde(default)]
    pub managed: Option<ManagedHostingConfig>,
    #[serde(default)]
    pub fly: Option<FlyHostingConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct LocalHostingConfig {}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct TorHostingConfig {}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelfHostingConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedHostingConfig {
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FlyHostingConfig {
    pub app: String,
    pub region: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SettlementSection {
    /// v1: only "none". v2: "lightning", "stripe", etc.
    pub method: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct LimitsSection {
    #[serde(default)]
    pub max_input_bytes: Option<usize>,
    #[serde(default)]
    pub max_runtime_ms: Option<u64>,
    #[serde(default)]
    pub max_memory_bytes: Option<usize>,
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
    #[serde(default)]
    pub fuel_limit: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct PriceSection {
    #[serde(default)]
    pub sats: Option<u64>,
    #[serde(default)]
    pub base_fee_msat: Option<u64>,
    #[serde(default)]
    pub success_fee_msat: Option<u64>,
    /// Unit for the `sats` price integer.
    ///
    /// - `"sat"` (default when omitted): the integer is **satoshis**, settled
    ///   via the Lightning rail.
    /// - `"usd"`: the integer is **US cents** (e.g. `500` = $5.00), settled
    ///   via the Stripe rail.
    ///
    /// Publishing a service with `currency = "usd"` on a node whose
    /// payment backend is not Stripe is a hard error.
    #[serde(default)]
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OciSection {
    pub reference: String,
    pub digest: String,
}

impl ServiceManifest {
    /// Parse a TOML string into a `ServiceManifest` + validate it. Returns
    /// the manifest plus any soft warnings (e.g., legacy-v2 deprecation).
    pub fn from_toml(input: &str) -> Result<(Self, Vec<ManifestWarning>), ManifestError> {
        let manifest: Self =
            toml::from_str(input).map_err(|e| ManifestError::Parse(e.to_string()))?;
        let warnings = manifest.validate()?;
        Ok((manifest, warnings))
    }

    fn validate(&self) -> Result<Vec<ManifestWarning>, ManifestError> {
        let mut warnings = Vec::new();
        let is_v2 = self.schema_version == SERVICE_MANIFEST_SCHEMA_V2;
        if !is_v2 && self.schema_version != SERVICE_MANIFEST_SCHEMA_V3 {
            return Err(ManifestError::SchemaVersion {
                got: self.schema_version.clone(),
                expected: SERVICE_MANIFEST_SCHEMA_V3,
            });
        }

        validate_identifier("service_id", &self.service_id)?;
        if let Some(project_id) = &self.project_id {
            validate_identifier("project_id", project_id)?;
        }
        if let Some(offer_id) = &self.offer_id {
            validate_identifier("offer_id", offer_id)?;
        }

        validate_runtime(&self.runtime)?;
        validate_package_kind(&self.package_kind)?;
        validate_runtime_package_combo(&self.runtime, &self.package_kind)?;

        // Builtin is reserved.
        if self.runtime == "builtin" || self.package_kind == "builtin" {
            return Err(ManifestError::InvalidValue {
                field: "runtime",
                value: self.runtime.clone(),
                reason: "builtin runtime/package_kind is reserved and cannot be published"
                    .to_string(),
            });
        }

        // Conditional fields per runtime + package_kind.
        match (self.runtime.as_str(), self.package_kind.as_str()) {
            ("python", "inline_source") | ("wasm", "inline_module") => {
                if self.entrypoint.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(ManifestError::MissingRequired {
                        field: "entrypoint",
                        context: format!(
                            "runtime={}, package_kind={}",
                            self.runtime, self.package_kind
                        ),
                    });
                }
            }
            (_, "oci_image") => {
                let oci = self
                    .oci
                    .as_ref()
                    .ok_or_else(|| ManifestError::MissingRequired {
                        field: "oci",
                        context: "package_kind=oci_image".to_string(),
                    })?;
                if oci.reference.trim().is_empty() {
                    return Err(ManifestError::MissingRequired {
                        field: "oci.reference",
                        context: "package_kind=oci_image".to_string(),
                    });
                }
                if oci.digest.trim().is_empty() {
                    return Err(ManifestError::MissingRequired {
                        field: "oci.digest",
                        context: "package_kind=oci_image".to_string(),
                    });
                }
            }
            _ => {}
        }

        // v3 sections (or v2 deprecation warnings)
        if let Some(hosting) = &self.hosting {
            validate_hosting_choice(&hosting.default)?;
            validate_hosting_config(hosting)?;
        } else if !is_v2 {
            return Err(ManifestError::MissingRequired {
                field: "hosting",
                context: "v3 service manifest".to_string(),
            });
        } else {
            warnings.push(ManifestWarning::LegacyV2Service {
                missing_section: "[hosting]",
            });
        }

        if let Some(settlement) = &self.settlement {
            validate_settlement_method(&settlement.method)?;
        } else if !is_v2 {
            return Err(ManifestError::MissingRequired {
                field: "settlement",
                context: "v3 service manifest".to_string(),
            });
        } else {
            warnings.push(ManifestWarning::LegacyV2Service {
                missing_section: "[settlement]",
            });
        }

        if let Some(marketplace) = &self.marketplace {
            validate_marketplace_url(&marketplace.url)?;
        }

        if let Some(limits) = &self.limits {
            validate_limits(limits)?;
        }

        if let Some(price) = &self.price {
            validate_price_currency(price)?;
        }

        if let Some(mode) = &self.mode
            && mode != "sync"
            && mode != "async"
        {
            return Err(ManifestError::InvalidValue {
                field: "mode",
                value: mode.clone(),
                reason: "must be \"sync\" or \"async\"".to_string(),
            });
        }
        if let Some(state) = &self.publication_state
            && state != "active"
            && state != "hidden"
        {
            return Err(ManifestError::InvalidValue {
                field: "publication_state",
                value: state.clone(),
                reason: "must be \"active\" or \"hidden\"".to_string(),
            });
        }

        Ok(warnings)
    }

    /// Resolve the offer_id: explicit `offer_id` if present, else `service_id`.
    pub fn resolved_offer_id(&self) -> &str {
        self.offer_id.as_deref().unwrap_or(&self.service_id)
    }
}

// ── Validators ──────────────────────────────────────────────────────────

const IDENTIFIER_MAX_LEN: usize = 63;

fn validate_identifier(field: &'static str, value: &str) -> Result<(), ManifestError> {
    let v = value.trim();
    if v.is_empty() {
        return Err(ManifestError::Identifier {
            field,
            value: value.to_string(),
            reason: "must not be empty",
        });
    }
    if v.len() > IDENTIFIER_MAX_LEN {
        return Err(ManifestError::Identifier {
            field,
            value: value.to_string(),
            reason: "must be at most 63 characters",
        });
    }
    if v.starts_with('-') || v.ends_with('-') {
        return Err(ManifestError::Identifier {
            field,
            value: value.to_string(),
            reason: "must not start or end with a hyphen",
        });
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ManifestError::Identifier {
            field,
            value: value.to_string(),
            reason: "must be lowercase ASCII letters, digits, or interior hyphens",
        });
    }
    Ok(())
}

fn validate_runtime(value: &str) -> Result<(), ManifestError> {
    match value {
        "python" | "wasm" | "container" | "any" => Ok(()),
        "builtin" => Err(ManifestError::InvalidValue {
            field: "runtime",
            value: value.to_string(),
            reason: "builtin runtime is reserved; not publishable".to_string(),
        }),
        other => Err(ManifestError::InvalidValue {
            field: "runtime",
            value: other.to_string(),
            reason: "must be one of: python, wasm, container".to_string(),
        }),
    }
}

fn validate_package_kind(value: &str) -> Result<(), ManifestError> {
    match value {
        "inline_source" | "inline_module" | "oci_image" => Ok(()),
        "builtin" => Err(ManifestError::InvalidValue {
            field: "package_kind",
            value: value.to_string(),
            reason: "builtin package_kind is reserved; not publishable".to_string(),
        }),
        other => Err(ManifestError::InvalidValue {
            field: "package_kind",
            value: other.to_string(),
            reason: "must be one of: inline_source, inline_module, oci_image".to_string(),
        }),
    }
}

fn validate_runtime_package_combo(runtime: &str, package_kind: &str) -> Result<(), ManifestError> {
    const ALLOWED: &[(&str, &str)] = &[
        ("python", "inline_source"),
        ("python", "oci_image"),
        ("wasm", "inline_module"),
        ("wasm", "oci_image"),
        ("container", "oci_image"),
    ];
    if ALLOWED
        .iter()
        .any(|(r, p)| *r == runtime && *p == package_kind)
    {
        return Ok(());
    }
    Err(ManifestError::RuntimeMismatch {
        runtime: runtime.to_string(),
        package_kind: package_kind.to_string(),
        allowed: &[
            "python+inline_source",
            "python+oci_image",
            "wasm+inline_module",
            "wasm+oci_image",
            "container+oci_image",
        ],
    })
}

fn validate_hosting_choice(value: &str) -> Result<(), ManifestError> {
    match value {
        "local" | "tor" | "self" | "managed" | "fly" => Ok(()),
        other => Err(ManifestError::InvalidValue {
            field: "hosting.default",
            value: other.to_string(),
            reason: "must be one of: local, tor, self, managed, fly".to_string(),
        }),
    }
}

fn validate_hosting_config(hosting: &HostingSection) -> Result<(), ManifestError> {
    match hosting.default.as_str() {
        "self" => {
            let self_cfg = hosting.self_hosted.as_ref().ok_or_else(|| {
                ManifestError::MissingHostingConfig {
                    choice: "self".to_string(),
                    missing: "url",
                }
            })?;
            if self_cfg.url.trim().is_empty() {
                return Err(ManifestError::MissingHostingConfig {
                    choice: "self".to_string(),
                    missing: "url",
                });
            }
            // Basic shape check; full validation is at publish time.
            if !self_cfg.url.starts_with("http://") && !self_cfg.url.starts_with("https://") {
                return Err(ManifestError::InvalidValue {
                    field: "hosting.self.url",
                    value: self_cfg.url.clone(),
                    reason: "must be an http:// or https:// URL".to_string(),
                });
            }
        }
        "fly" => {
            let fly = hosting
                .fly
                .as_ref()
                .ok_or_else(|| ManifestError::MissingHostingConfig {
                    choice: "fly".to_string(),
                    missing: "app, region",
                })?;
            if fly.app.trim().is_empty() || fly.region.trim().is_empty() {
                return Err(ManifestError::MissingHostingConfig {
                    choice: "fly".to_string(),
                    missing: "app, region",
                });
            }
        }
        _ => {} // local, tor, managed have no required fields in v1A
    }
    Ok(())
}

fn validate_settlement_method(value: &str) -> Result<(), ManifestError> {
    // v1 publish surface: free ("none") plus Lightning (hold-invoice escrow,
    // settled at the daemon level). Stripe/x402 remain gated pending a
    // verifiable-proof design. Keep this an allowlist so unknown methods still
    // fail closed rather than silently publishing an unsettleable offer.
    if matches!(value, "none" | "lightning") {
        return Ok(());
    }
    Err(ManifestError::UnsupportedSettlement {
        method: value.to_string(),
    })
}

fn validate_price_currency(price: &PriceSection) -> Result<(), ManifestError> {
    if let Some(currency) = price.currency.as_deref() {
        match currency {
            "sat" | "usd" => {}
            other => {
                return Err(ManifestError::InvalidPriceCurrency {
                    value: other.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn validate_identity_strategy(value: &str) -> Result<(), ManifestError> {
    let v = value.trim();
    if v == "auto" {
        return Ok(());
    }
    if v.starts_with("env:") && v.len() > "env:".len() {
        return Ok(());
    }
    if v.starts_with("file:") && v.len() > "file:".len() {
        return Ok(());
    }
    Err(ManifestError::InvalidValue {
        field: "project.identity.strategy",
        value: value.to_string(),
        reason: "must be \"auto\", \"env:NAME\", or \"file:PATH\"".to_string(),
    })
}

fn validate_marketplace_url(value: &str) -> Result<(), ManifestError> {
    let v = value.trim();
    if v.is_empty() {
        return Err(ManifestError::InvalidValue {
            field: "marketplace.url",
            value: value.to_string(),
            reason: "must not be empty".to_string(),
        });
    }
    if !v.starts_with("http://") && !v.starts_with("https://") {
        return Err(ManifestError::InvalidValue {
            field: "marketplace.url",
            value: value.to_string(),
            reason: "must be an http:// or https:// URL".to_string(),
        });
    }
    Ok(())
}

fn validate_limits(limits: &LimitsSection) -> Result<(), ManifestError> {
    if let Some(v) = limits.max_input_bytes
        && v == 0
    {
        return Err(ManifestError::InvalidValue {
            field: "limits.max_input_bytes",
            value: v.to_string(),
            reason: "must be > 0".to_string(),
        });
    }
    if let Some(v) = limits.max_runtime_ms
        && v == 0
    {
        return Err(ManifestError::InvalidValue {
            field: "limits.max_runtime_ms",
            value: v.to_string(),
            reason: "must be > 0".to_string(),
        });
    }
    if let Some(v) = limits.max_memory_bytes
        && v == 0
    {
        return Err(ManifestError::InvalidValue {
            field: "limits.max_memory_bytes",
            value: v.to_string(),
            reason: "must be > 0".to_string(),
        });
    }
    if let Some(v) = limits.max_output_bytes
        && v == 0
    {
        return Err(ManifestError::InvalidValue {
            field: "limits.max_output_bytes",
            value: v.to_string(),
            reason: "must be > 0".to_string(),
        });
    }
    // fuel_limit may be 0 (= unlimited within max_runtime_ms)
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Valid fixtures ─────────────────────────────────────────────────

    #[test]
    fn project_manifest_minimal_valid() {
        let toml = r#"
            schema_version = "froglet/v1"
            [project]
            name = "my-project"
        "#;
        let (m, warnings) = ProjectManifest::from_toml(toml).unwrap();
        assert_eq!(m.project.name, "my-project");
        assert!(warnings.is_empty());
    }

    #[test]
    fn project_manifest_full_valid() {
        let toml = r#"
            schema_version = "froglet/v1"
            [project]
            name = "my-project"
            description = "Multi-service Froglet project"
            [project.identity]
            strategy = "env:MY_SEED"
            [project.marketplace]
            url = "https://marketplace.froglet.dev"
            [project.defaults]
            runtime = "python"
            hosting = "tor"
            settlement = "none"
        "#;
        let (m, warnings) = ProjectManifest::from_toml(toml).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(m.project.identity.as_ref().unwrap().strategy, "env:MY_SEED");
    }

    #[test]
    fn service_manifest_python_inline_valid() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "translator-en-es"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let (m, warnings) = ServiceManifest::from_toml(toml).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(m.resolved_offer_id(), "translator-en-es");
    }

    #[test]
    fn service_manifest_wasm_oci_valid() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "fast-hash"
            runtime = "wasm"
            package_kind = "oci_image"
            [oci]
            reference = "ghcr.io/example/fast-hash:1.0"
            digest = "sha256:abc123"
            [hosting]
            default = "self"
            [hosting.self]
            url = "https://my-host.example.com"
            [settlement]
            method = "none"
        "#;
        let (m, warnings) = ServiceManifest::from_toml(toml).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(m.runtime, "wasm");
    }

    #[test]
    fn service_manifest_fly_hosting_valid() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "echo"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "echo.py"
            [hosting]
            default = "fly"
            [hosting.fly]
            app = "my-echo"
            region = "iad"
            [settlement]
            method = "none"
        "#;
        let (m, _) = ServiceManifest::from_toml(toml).unwrap();
        assert_eq!(m.hosting.as_ref().unwrap().default, "fly");
    }

    #[test]
    fn service_manifest_v2_loads_with_deprecation_warnings() {
        let toml = r#"
            schema_version = "froglet-service/v2"
            service_id = "legacy"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "main.py"
        "#;
        let (m, warnings) = ServiceManifest::from_toml(toml).unwrap();
        assert_eq!(m.schema_version, SERVICE_MANIFEST_SCHEMA_V2);
        // Both [hosting] and [settlement] missing → two warnings.
        assert_eq!(warnings.len(), 2);
        assert!(matches!(
            warnings[0],
            ManifestWarning::LegacyV2Service { .. }
        ));
    }

    // ── Invalid fixtures ───────────────────────────────────────────────

    #[test]
    fn rejects_unknown_top_level_field() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "x.py"
            unknown_field = "boom"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    #[test]
    fn rejects_invalid_schema_version() {
        let toml = r#"
            schema_version = "froglet-service/v4"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "x.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(err, ManifestError::SchemaVersion { .. }));
    }

    #[test]
    fn rejects_identifier_with_uppercase() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "MyService"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "x.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(err, ManifestError::Identifier { .. }));
    }

    #[test]
    fn rejects_identifier_starting_with_hyphen() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "-leading-hyphen"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "x.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(err, ManifestError::Identifier { .. }));
    }

    #[test]
    fn rejects_builtin_runtime() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "builtin"
            package_kind = "inline_source"
            entrypoint = "x.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::InvalidValue {
                field: "runtime",
                ..
            }
        ));
    }

    #[test]
    fn rejects_unsupported_runtime_package_combo() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_module"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(err, ManifestError::RuntimeMismatch { .. }));
    }

    #[test]
    fn rejects_oci_image_without_reference() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "wasm"
            package_kind = "oci_image"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::MissingRequired { field: "oci", .. }
        ));
    }

    #[test]
    fn rejects_self_hosting_without_url() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "x.py"
            [hosting]
            default = "self"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(err, ManifestError::MissingHostingConfig { .. }));
    }

    #[test]
    fn rejects_unsupported_settlement_method() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "x.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "paypal"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(err, ManifestError::UnsupportedSettlement { .. }));
    }

    #[test]
    fn accepts_lightning_settlement_method() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "x.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "lightning"
        "#;
        assert!(
            ServiceManifest::from_toml(toml).is_ok(),
            "lightning settlement should be accepted on the v1 publish surface"
        );
    }

    #[test]
    fn rejects_zero_limit() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "x.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
            [limits]
            max_input_bytes = 0
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::InvalidValue {
                field: "limits.max_input_bytes",
                ..
            }
        ));
    }

    #[test]
    fn rejects_fly_hosting_missing_app() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "x.py"
            [hosting]
            default = "fly"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(err, ManifestError::MissingHostingConfig { .. }));
    }

    #[test]
    fn rejects_python_inline_source_without_entrypoint() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_source"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::MissingRequired {
                field: "entrypoint",
                ..
            }
        ));
    }

    #[test]
    fn project_manifest_rejects_invalid_identity_strategy() {
        let toml = r#"
            schema_version = "froglet/v1"
            [project]
            name = "my-project"
            [project.identity]
            strategy = "yolo"
        "#;
        let err = ProjectManifest::from_toml(toml).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::InvalidValue {
                field: "project.identity.strategy",
                ..
            }
        ));
    }

    #[test]
    fn project_manifest_rejects_bad_marketplace_url() {
        let toml = r#"
            schema_version = "froglet/v1"
            [project]
            name = "my-project"
            [project.marketplace]
            url = "not-a-url"
        "#;
        let err = ProjectManifest::from_toml(toml).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::InvalidValue {
                field: "marketplace.url",
                ..
            }
        ));
    }

    // ── price.currency field tests ─────────────────────────────────────

    /// Absent [price] section → currency is None (treated as "sat").
    #[test]
    fn price_currency_absent_defaults_to_sat() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "my-svc"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
        "#;
        let (m, warnings) = ServiceManifest::from_toml(toml).unwrap();
        assert!(warnings.is_empty());
        assert!(m.price.is_none() || m.price.as_ref().unwrap().currency.is_none());
    }

    /// Explicit currency = "sat" parses and round-trips.
    #[test]
    fn price_currency_sat_parses_and_roundtrips() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "my-svc"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
            [price]
            sats = 500
            currency = "sat"
        "#;
        let (m, warnings) = ServiceManifest::from_toml(toml).unwrap();
        assert!(warnings.is_empty());
        let price = m.price.as_ref().unwrap();
        assert_eq!(price.sats, Some(500));
        assert_eq!(price.currency.as_deref(), Some("sat"));
        // Round-trip through TOML serialization.
        let serialized = toml::to_string(&m).unwrap();
        let (m2, _) = ServiceManifest::from_toml(&serialized).unwrap();
        assert_eq!(m, m2);
    }

    /// Explicit currency = "usd" parses and round-trips.
    #[test]
    fn price_currency_usd_parses_and_roundtrips() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "my-svc"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
            [price]
            sats = 100
            currency = "usd"
        "#;
        let (m, warnings) = ServiceManifest::from_toml(toml).unwrap();
        assert!(warnings.is_empty());
        let price = m.price.as_ref().unwrap();
        assert_eq!(price.sats, Some(100));
        assert_eq!(price.currency.as_deref(), Some("usd"));
        let serialized = toml::to_string(&m).unwrap();
        let (m2, _) = ServiceManifest::from_toml(&serialized).unwrap();
        assert_eq!(m, m2);
    }

    /// An unrecognised currency value is a hard error.
    #[test]
    fn price_currency_invalid_value_is_rejected() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "my-svc"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "none"
            [price]
            sats = 100
            currency = "eur"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(
            matches!(err, ManifestError::InvalidPriceCurrency { ref value } if value == "eur"),
            "unexpected error: {err}"
        );
    }
}
