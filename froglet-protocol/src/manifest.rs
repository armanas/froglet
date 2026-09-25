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

pub use crate::publication::{
    PublicationCsvColumn, PublicationCsvSchema, PublicationLimits as LimitsSection,
    PublicationMount, VerificationFixture,
};

/// Canonical `schema_version` for project manifests.
pub const PROJECT_MANIFEST_SCHEMA_V1: &str = "froglet/v1";

/// Canonical provider-neutral `schema_version` for new service manifests.
pub const SERVICE_MANIFEST_SCHEMA_V4: &str = "froglet-service/v4";

/// Compatibility schema accepted for existing authoring and legacy Fly
/// adapter state.
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
    #[error(
        "settlement.method = {method:?} is not supported; allowed: \"none\", \"lightning\", \"stripe\""
    )]
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
    /// v3 Fly authoring remains readable but new v4 authoring must use the
    /// provider-neutral managed target/profile.
    DeprecatedFlyHosting,
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
    #[serde(default)]
    pub starter: Option<String>,

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
    #[serde(default)]
    pub mounts: Vec<PublicationMount>,
    #[serde(default)]
    pub capabilities: Vec<String>,

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
    /// TOML-safe lossless JSON representation used when the schema contains
    /// values (notably `null`) that TOML cannot express directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema_json: Option<String>,
    #[serde(default)]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema_json: Option<String>,
    #[serde(default)]
    pub verification: Option<VerificationFixture>,

    // Native read-only data-query source (conditional on builtin+builtin).
    #[serde(default)]
    pub data: Option<DataSourceSection>,

    // Locked Python bundle configuration. When omitted for Python inline
    // source, the authoring adapter creates an exact dependency-free lock.
    #[serde(default)]
    pub python: Option<PythonSection>,

    // OCI-specific (conditional)
    #[serde(default)]
    pub oci: Option<OciSection>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostingSection {
    /// One of: "local" | "relay" | "tor" | "self" | "managed" | "fly".
    pub default: String,
    #[serde(default)]
    pub local: Option<LocalHostingConfig>,
    #[serde(default)]
    pub relay: Option<RelayHostingConfig>,
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
pub struct RelayHostingConfig {}

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
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct PythonSection {
    /// JSON lock path relative to the Python entrypoint's directory. The lock
    /// may refer only to hash-pinned wheels in `artifacts/<filename>` beside
    /// the lock file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DataSourceSection {
    /// File path relative to the service manifest directory.
    pub path: String,
    /// One of `json`, `csv`, or `sqlite`.
    pub format: String,
    /// Required for CSV and rejected for self-describing JSON/SQLite sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    /// Ordered CSV header/type declaration. At least one column must be
    /// indexed so equality selection cannot silently degrade into a full-file
    /// scan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<PublicationCsvColumn>,
}

impl DataSourceSection {
    pub fn csv_schema(&self) -> Option<PublicationCsvSchema> {
        (self.format == "csv").then(|| PublicationCsvSchema {
            collection: self.collection.clone().unwrap_or_default(),
            columns: self.columns.clone(),
        })
    }
}

impl ServiceManifest {
    /// Parse a TOML string into a `ServiceManifest` + validate it. Returns
    /// the manifest plus any soft warnings (e.g., legacy-v2 deprecation).
    pub fn from_toml(input: &str) -> Result<(Self, Vec<ManifestWarning>), ManifestError> {
        let mut manifest: Self =
            toml::from_str(input).map_err(|e| ManifestError::Parse(e.to_string()))?;
        resolve_json_alias(
            "input_schema",
            &mut manifest.input_schema,
            &mut manifest.input_schema_json,
        )?;
        resolve_json_alias(
            "output_schema",
            &mut manifest.output_schema,
            &mut manifest.output_schema_json,
        )?;
        let warnings = manifest.validate()?;
        Ok((manifest, warnings))
    }

    fn validate(&self) -> Result<Vec<ManifestWarning>, ManifestError> {
        let mut warnings = Vec::new();
        let is_v2 = self.schema_version == SERVICE_MANIFEST_SCHEMA_V2;
        let is_v3 = self.schema_version == SERVICE_MANIFEST_SCHEMA_V3;
        let is_v4 = self.schema_version == SERVICE_MANIFEST_SCHEMA_V4;
        if !is_v2 && !is_v3 && !is_v4 {
            return Err(ManifestError::SchemaVersion {
                got: self.schema_version.clone(),
                expected: SERVICE_MANIFEST_SCHEMA_V4,
            });
        }

        validate_identifier("service_id", &self.service_id)?;
        if let Some(project_id) = &self.project_id {
            validate_identifier("project_id", project_id)?;
        }
        if let Some(offer_id) = &self.offer_id {
            validate_identifier("offer_id", offer_id)?;
        }

        if let Some(data) = self.data.as_ref() {
            if self.runtime != "builtin" || self.package_kind != "builtin" {
                return Err(ManifestError::RuntimeMismatch {
                    runtime: self.runtime.clone(),
                    package_kind: self.package_kind.clone(),
                    allowed: &["builtin+builtin with [data]"],
                });
            }
            if data.path.trim().is_empty() {
                return Err(ManifestError::MissingRequired {
                    field: "data.path",
                    context: "native read-only data query".to_string(),
                });
            }
            validate_relative_authoring_path("data.path", &data.path)?;
            let expected_contract = match data.format.as_str() {
                "csv" => {
                    let schema = data
                        .csv_schema()
                        .ok_or_else(|| ManifestError::InvalidValue {
                            field: "data.columns",
                            value: "invalid CSV schema".to_string(),
                            reason: "CSV data requires an explicit schema".to_string(),
                        })?;
                    schema
                        .validate()
                        .map_err(|reason| ManifestError::InvalidValue {
                            field: "data.columns",
                            value: "invalid CSV schema".to_string(),
                            reason,
                        })?;
                    "froglet.builtin.data_query.csv.v1"
                }
                "json" | "sqlite" if data.collection.is_some() || !data.columns.is_empty() => {
                    return Err(ManifestError::InvalidValue {
                        field: "data.columns",
                        value: "present".to_string(),
                        reason: "collection and columns are only valid for CSV data".to_string(),
                    });
                }
                "json" => "froglet.builtin.data_query.json.v1",
                "sqlite" => "froglet.builtin.data_query.sqlite.v1",
                _ => {
                    return Err(ManifestError::InvalidValue {
                        field: "data.format",
                        value: data.format.clone(),
                        reason: "must be one of: json, csv, sqlite".to_string(),
                    });
                }
            };
            if self.oci.is_some() {
                return Err(ManifestError::InvalidValue {
                    field: "oci",
                    value: "present".to_string(),
                    reason: "native data publication cannot include an OCI source".to_string(),
                });
            }
            if !self.mounts.is_empty() || !self.capabilities.is_empty() {
                return Err(ManifestError::InvalidValue {
                    field: "capabilities",
                    value: "present".to_string(),
                    reason: "the v1 read-only data lane does not accept mounts or capabilities"
                        .to_string(),
                });
            }
            if self
                .entrypoint_kind
                .as_deref()
                .is_some_and(|value| value != "builtin")
            {
                return Err(ManifestError::InvalidValue {
                    field: "entrypoint_kind",
                    value: self.entrypoint_kind.clone().unwrap_or_default(),
                    reason: "native data publication uses entrypoint_kind=builtin".to_string(),
                });
            }
            if self
                .contract_version
                .as_deref()
                .is_some_and(|value| value != expected_contract)
            {
                return Err(ManifestError::InvalidValue {
                    field: "contract_version",
                    value: self.contract_version.clone().unwrap_or_default(),
                    reason: format!("native data publication uses {expected_contract}"),
                });
            }
        } else {
            validate_runtime(&self.runtime)?;
            validate_package_kind(&self.package_kind)?;
            validate_runtime_package_combo(&self.runtime, &self.package_kind)?;

            // Builtin remains reserved except for the narrowly typed native
            // data source above.
            if self.runtime == "builtin" || self.package_kind == "builtin" {
                return Err(ManifestError::InvalidValue {
                    field: "runtime",
                    value: self.runtime.clone(),
                    reason: "builtin runtime/package_kind is reserved and cannot be published"
                        .to_string(),
                });
            }
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
                validate_relative_authoring_path(
                    "entrypoint",
                    self.entrypoint.as_deref().unwrap_or_default(),
                )?;
                if self.runtime == "python"
                    && let Some(lock) = self.python.as_ref().and_then(|python| python.lock.as_ref())
                {
                    validate_relative_authoring_path("python.lock", lock)?;
                    if !lock.ends_with(".json") {
                        return Err(ManifestError::InvalidValue {
                            field: "python.lock",
                            value: lock.clone(),
                            reason: "must name a JSON lock file".to_string(),
                        });
                    }
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

        if self.python.is_some()
            && !(self.runtime == "python" && self.package_kind == "inline_source")
        {
            return Err(ManifestError::InvalidValue {
                field: "python",
                value: "present".to_string(),
                reason: "[python] is only valid for python+inline_source".to_string(),
            });
        }

        // v3 sections (or v2 deprecation warnings)
        if let Some(hosting) = &self.hosting {
            if is_v4 && (hosting.default == "fly" || hosting.fly.is_some()) {
                return Err(ManifestError::InvalidValue {
                    field: if hosting.default == "fly" {
                        "hosting.default"
                    } else {
                        "hosting.fly"
                    },
                    value: "fly".to_string(),
                    reason: "v4 authoring uses hosting.default=\"managed\" with provider-neutral target/profile; Fly is a v3 compatibility adapter".to_string(),
                });
            }
            if is_v3 && hosting.default == "fly" {
                warnings.push(ManifestWarning::DeprecatedFlyHosting);
            }
            if is_v4 && hosting.default == "managed" {
                let managed = hosting.managed.as_ref().ok_or_else(|| {
                    ManifestError::MissingHostingConfig {
                        choice: "managed".to_string(),
                        missing: "target, profile",
                    }
                })?;
                if managed
                    .target
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
                    || managed
                        .profile
                        .as_deref()
                        .is_none_or(|value| value.trim().is_empty())
                {
                    return Err(ManifestError::MissingHostingConfig {
                        choice: "managed".to_string(),
                        missing: "target, profile",
                    });
                }
            }
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

        // Settlement ↔ currency consistency.
        // "stripe" requires currency="usd"; "lightning" requires currency="sat"
        // (or absent, which defaults to "sat"). Any mismatch is a hard error so
        // the provider cannot publish an offer the settlement backend can't
        // handle.
        if let Some(settlement) = &self.settlement {
            let currency = self.price.as_ref().and_then(|p| p.currency.as_deref());
            match settlement.method.as_str() {
                "stripe" => {
                    if !matches!(currency, Some("usd")) {
                        return Err(ManifestError::InvalidValue {
                            field: "price.currency",
                            value: currency.unwrap_or("<absent>").to_string(),
                            reason:
                                "settlement.method = \"stripe\" requires price.currency = \"usd\""
                                    .to_string(),
                        });
                    }
                }
                "lightning" => {
                    if matches!(currency, Some(c) if !c.eq_ignore_ascii_case("sat")) {
                        return Err(ManifestError::InvalidValue {
                            field: "price.currency",
                            value: currency.unwrap_or("<absent>").to_string(),
                            reason:
                                "settlement.method = \"lightning\" requires price.currency = \"sat\" (or absent)"
                                    .to_string(),
                        });
                    }
                }
                _ => {} // "none" has no currency constraint
            }
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

        crate::publication::PublicationIntent::from_service_manifest(self).map_err(|error| {
            ManifestError::InvalidValue {
                field: "publication",
                value: self.service_id.clone(),
                reason: error.to_string(),
            }
        })?;

        Ok(warnings)
    }

    /// Resolve the offer_id: explicit `offer_id` if present, else `service_id`.
    pub fn resolved_offer_id(&self) -> &str {
        self.offer_id.as_deref().unwrap_or(&self.service_id)
    }
}

fn resolve_json_alias(
    field: &'static str,
    inline: &mut Option<Value>,
    encoded: &mut Option<String>,
) -> Result<(), ManifestError> {
    match (inline.is_some(), encoded.take()) {
        (true, Some(value)) => Err(ManifestError::InvalidValue {
            field,
            value,
            reason: format!("set either {field} or {field}_json, not both"),
        }),
        (false, Some(value)) => {
            *inline = Some(serde_json::from_str(&value).map_err(|error| {
                ManifestError::InvalidValue {
                    field,
                    value,
                    reason: format!("{field}_json is not valid JSON: {error}"),
                }
            })?);
            Ok(())
        }
        (_, None) => Ok(()),
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

fn validate_relative_authoring_path(field: &'static str, value: &str) -> Result<(), ManifestError> {
    use std::path::{Component, Path};

    let path = Path::new(value);
    if value.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ManifestError::InvalidValue {
            field,
            value: value.to_string(),
            reason: "must be a non-empty relative path without parent traversal".to_string(),
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
        "local" | "relay" | "tor" | "self" | "managed" | "fly" => Ok(()),
        other => Err(ManifestError::InvalidValue {
            field: "hosting.default",
            value: other.to_string(),
            reason: "must be one of: local, relay, tor, self, managed, fly".to_string(),
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
        _ => {} // local, relay, tor, managed have no required fields
    }
    Ok(())
}

fn validate_settlement_method(value: &str) -> Result<(), ManifestError> {
    // Allowlist: free ("none"), Lightning (hold-invoice escrow), and Stripe
    // (MPP / Shared Payment Tokens). Keep this an allowlist so unknown methods
    // still fail closed rather than silently publishing an unsettleable offer.
    if matches!(value, "none" | "lightning" | "stripe") {
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
    fn service_manifest_v4_relay_hosting_valid() {
        let toml = r#"
            schema_version = "froglet-service/v4"
            service_id = "relay-echo"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            verification = { input = { message = "ping" } }
            [hosting]
            default = "relay"
            [settlement]
            method = "none"
        "#;
        let (manifest, warnings) = ServiceManifest::from_toml(toml).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(manifest.hosting.as_ref().unwrap().default, "relay");
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
        let intent = crate::publication::PublicationIntent::from_service_manifest(&m).unwrap();
        assert_eq!(
            intent.oci_reference.as_deref(),
            Some("ghcr.io/example/fast-hash:1.0")
        );
        assert_eq!(intent.oci_digest.as_deref(), Some("sha256:abc123"));
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
            schema_version = "froglet-service/v5"
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
    fn rejects_entrypoint_parent_traversal() {
        let toml = r#"
            schema_version = "froglet-service/v4"
            service_id = "x"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "../secret.py"
            [hosting]
            default = "local"
            [settlement]
            method = "none"
        "#;
        assert!(matches!(
            ServiceManifest::from_toml(toml),
            Err(ManifestError::InvalidValue {
                field: "entrypoint",
                ..
            })
        ));
    }

    #[test]
    fn rejects_native_data_parent_traversal() {
        let toml = r#"
            schema_version = "froglet-service/v4"
            service_id = "catalog"
            runtime = "builtin"
            package_kind = "builtin"
            contract_version = "froglet.builtin.data_query.json.v1"
            verification = { input = { op = "describe" } }
            [data]
            path = "../secret.json"
            format = "json"
            [hosting]
            default = "local"
            [settlement]
            method = "none"
        "#;
        assert!(matches!(
            ServiceManifest::from_toml(toml),
            Err(ManifestError::InvalidValue {
                field: "data.path",
                ..
            })
        ));
    }

    #[test]
    fn accepts_only_the_typed_native_data_builtin() {
        let toml = r#"
            schema_version = "froglet-service/v4"
            service_id = "catalog"
            runtime = "builtin"
            package_kind = "builtin"
            contract_version = "froglet.builtin.data_query.json.v1"
            verification = { input = { op = "describe" } }

            [data]
            path = "catalog.json"
            format = "json"

            [hosting]
            default = "relay"

            [settlement]
            method = "none"
        "#;
        let (manifest, warnings) = ServiceManifest::from_toml(toml)
            .expect("typed native data manifest should be accepted");
        assert!(warnings.is_empty());
        assert_eq!(manifest.data.unwrap().path, "catalog.json");

        let arbitrary_builtin = toml.replace("[data]", "[unknown_data]");
        assert!(ServiceManifest::from_toml(&arbitrary_builtin).is_err());
    }

    #[test]
    fn csv_native_data_requires_an_explicit_indexed_schema() {
        let toml = r#"
            schema_version = "froglet-service/v4"
            service_id = "people"
            runtime = "builtin"
            package_kind = "builtin"
            contract_version = "froglet.builtin.data_query.csv.v1"
            verification = { input = { op = "describe" } }

            [data]
            path = "people.csv"
            format = "csv"
            collection = "people"

            [[data.columns]]
            name = "id"
            type = "integer"
            indexed = true

            [[data.columns]]
            name = "name"
            type = "string"

            [hosting]
            default = "relay"

            [settlement]
            method = "none"
        "#;
        let (manifest, warnings) =
            ServiceManifest::from_toml(toml).expect("explicit CSV schema is valid");
        assert!(warnings.is_empty());
        let schema = manifest.data.unwrap().csv_schema().unwrap();
        assert_eq!(schema.collection, "people");
        assert!(schema.columns[0].indexed);

        let without_index = toml.replace("indexed = true", "indexed = false");
        let error = ServiceManifest::from_toml(&without_index)
            .expect_err("CSV without an index must fail closed");
        assert!(error.to_string().contains("indexed"));
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
            method = "lightning"
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
            method = "stripe"
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

    // ── Fix 5: stripe settlement method + consistency checks ──────────────

    #[test]
    fn accepts_stripe_settlement_method() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "stripe-svc"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "stripe"
            [price]
            sats = 500
            currency = "usd"
        "#;
        assert!(
            ServiceManifest::from_toml(toml).is_ok(),
            "stripe settlement with usd currency should be accepted"
        );
    }

    #[test]
    fn stripe_settlement_requires_usd_currency() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "stripe-svc"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "stripe"
            [price]
            sats = 500
            currency = "sat"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(
            matches!(
                err,
                ManifestError::InvalidValue {
                    field: "price.currency",
                    ..
                }
            ),
            "stripe + sat currency must be rejected: {err}"
        );
    }

    #[test]
    fn stripe_settlement_rejects_absent_currency() {
        // stripe requires currency = "usd"; absent currency must be rejected.
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "stripe-svc"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "stripe"
            [price]
            sats = 500
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(
            matches!(
                err,
                ManifestError::InvalidValue {
                    field: "price.currency",
                    ..
                }
            ),
            "stripe without explicit currency must be rejected: {err}"
        );
    }

    #[test]
    fn lightning_settlement_rejects_usd_currency() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "lightning-svc"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "lightning"
            [price]
            sats = 1000
            currency = "usd"
        "#;
        let err = ServiceManifest::from_toml(toml).unwrap_err();
        assert!(
            matches!(
                err,
                ManifestError::InvalidValue {
                    field: "price.currency",
                    ..
                }
            ),
            "lightning + usd currency must be rejected: {err}"
        );
    }

    #[test]
    fn lightning_settlement_accepts_absent_currency() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "lightning-svc"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "tor"
            [settlement]
            method = "lightning"
            [price]
            sats = 1000
        "#;
        assert!(
            ServiceManifest::from_toml(toml).is_ok(),
            "lightning with absent currency (defaults to sat) should be accepted"
        );
    }

    #[test]
    fn service_manifest_preserves_publication_access_and_metadata() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "analytics"
            offer_id = "analytics-read-v2"
            summary = "Read analytics"
            starter = '{"query":"select 1"}'
            runtime = "python"
            package_kind = "inline_source"
            entrypoint_kind = "handler"
            entrypoint = "handler.py"
            contract_version = "froglet.python.handler_json.v1"
            mode = "async"
            publication_state = "hidden"
            mounts = [{ handle = "warehouse", kind = "postgres", read_only = true }]
            capabilities = ["network.http.fetch"]
            limits = { max_input_bytes = 4096, max_runtime_ms = 2500, max_memory_bytes = 8388608, max_output_bytes = 2048, fuel_limit = 50000 }
            input_schema = { type = "object", required = ["query"] }
            output_schema = { type = "object", properties = { rows = { type = "array" } } }
            verification = { input = { query = "select 1" }, expected_output = { rows = [] } }
            [hosting]
            default = "local"
            [settlement]
            method = "none"
            [price]
            sats = 0
            currency = "sat"
        "#;

        let manifest = ServiceManifest::from_toml(toml).unwrap().0;
        assert_eq!(manifest.offer_id.as_deref(), Some("analytics-read-v2"));
        assert_eq!(manifest.starter.as_deref(), Some(r#"{"query":"select 1"}"#));
        assert_eq!(manifest.mounts.len(), 1);
        assert_eq!(manifest.mounts[0].handle, "warehouse");
        assert!(manifest.mounts[0].read_only);
        assert_eq!(manifest.capabilities, vec!["network.http.fetch"]);
        assert_eq!(manifest.limits.unwrap().max_runtime_ms, Some(2500));
        assert_eq!(manifest.input_schema.unwrap()["required"][0], "query");
        assert_eq!(
            manifest.output_schema.unwrap()["properties"]["rows"]["type"],
            "array"
        );
        assert_eq!(
            manifest.verification.unwrap().expected_output.unwrap()["rows"],
            serde_json::json!([])
        );
    }

    #[test]
    fn service_manifest_json_aliases_preserve_null_values() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "nullable"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint_kind = "handler"
            entrypoint = "handler.py"
            contract_version = "froglet.python.handler_json.v1"
            input_schema_json = '{"type":["object","null"],"default":null}'
            output_schema_json = '{"type":["object","null"]}'
            verification = { input_json = 'null', expected_output_json = 'null' }
            [hosting]
            default = "local"
            [settlement]
            method = "none"
            [price]
            sats = 0
            currency = "sat"
        "#;

        let manifest = ServiceManifest::from_toml(toml).unwrap().0;
        assert_eq!(manifest.input_schema.unwrap()["default"], Value::Null);
        assert_eq!(manifest.output_schema.unwrap()["type"][1], "null");
        let verification = manifest.verification.unwrap();
        assert_eq!(verification.input, Value::Null);
        assert_eq!(verification.expected_output, Some(Value::Null));
        assert!(manifest.input_schema_json.is_none());
        assert!(manifest.output_schema_json.is_none());
    }

    #[test]
    fn service_manifest_v4_accepts_provider_neutral_managed_target() {
        let toml = r#"
            schema_version = "froglet-service/v4"
            service_id = "managed-service"
            runtime = "wasm"
            package_kind = "inline_module"
            entrypoint_kind = "handler"
            entrypoint = "run"
            contract_version = "froglet.wasm.run_json.v1"
            [hosting]
            default = "managed"
            [hosting.managed]
            target = "froglet-hosted"
            profile = "small"
            [settlement]
            method = "none"
            [price]
            sats = 0
            currency = "sat"
        "#;

        let (manifest, warnings) = ServiceManifest::from_toml(toml).unwrap();
        assert!(warnings.is_empty());
        let managed = manifest.hosting.unwrap().managed.unwrap();
        assert_eq!(managed.target.as_deref(), Some("froglet-hosted"));
        assert_eq!(managed.profile.as_deref(), Some("small"));
    }

    #[test]
    fn service_manifest_v4_rejects_fly_but_v3_remains_readable() {
        let v3 = r#"
            schema_version = "froglet-service/v3"
            service_id = "legacy-fly"
            runtime = "wasm"
            package_kind = "inline_module"
            entrypoint_kind = "handler"
            entrypoint = "run"
            contract_version = "froglet.wasm.run_json.v1"
            [hosting]
            default = "fly"
            [hosting.fly]
            app = "legacy-app"
            region = "zrh"
            [settlement]
            method = "none"
            [price]
            sats = 0
            currency = "sat"
        "#;
        let (_, warnings) = ServiceManifest::from_toml(v3).unwrap();
        assert!(warnings.contains(&ManifestWarning::DeprecatedFlyHosting));

        let v4 = v3.replacen("froglet-service/v3", "froglet-service/v4", 1);
        let error = ServiceManifest::from_toml(&v4).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::InvalidValue {
                field: "hosting.default",
                ..
            }
        ));

        let v4_with_inactive_fly = r#"
            schema_version = "froglet-service/v4"
            service_id = "managed-service"
            runtime = "wasm"
            package_kind = "inline_module"
            entrypoint_kind = "module"
            entrypoint = "run"
            contract_version = "froglet.wasm.run_json.v1"
            [hosting]
            default = "managed"
            [hosting.managed]
            target = "regional-container"
            profile = "small-public"
            [hosting.fly]
            app = "must-not-leak"
            region = "zrh"
            [settlement]
            method = "none"
            [price]
            sats = 0
            currency = "sat"
        "#;
        let error = ServiceManifest::from_toml(v4_with_inactive_fly).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::InvalidValue {
                field: "hosting.fly",
                ..
            }
        ));
    }
}
