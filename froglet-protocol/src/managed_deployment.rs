use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::{BTreeMap, BTreeSet};

pub const DESIRED_STATE_SCHEMA_V1: &str = "froglet.managed-deployment.desired-state.v1";
pub const ADAPTER_CAPABILITIES_SCHEMA_V1: &str =
    "froglet.managed-deployment.adapter-capabilities.v1";
pub const PLAN_SCHEMA_V1: &str = "froglet.managed-deployment.plan.v1";
pub const PLAN_APPROVAL_SCHEMA_V1: &str = "froglet.managed-deployment.plan-approval.v1";
pub const RESULT_SCHEMA_V1: &str = "froglet.managed-deployment.result.v1";
pub const HISTORY_SCHEMA_V1: &str = "froglet.managed-deployment.history.v1";
pub const LOGS_SCHEMA_V1: &str = "froglet.managed-deployment.logs.v1";
pub const MAX_HISTORY_ENTRIES_V1: u32 = 100;
pub const MAX_LOG_ENTRIES_V1: u32 = 1_000;
pub const MAX_LOG_BYTES_V1: u32 = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedDeploymentDesiredStateV1 {
    pub schema_version: String,
    pub deployment_id: String,
    pub revision: DeploymentRevisionV1,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: Vec<LogicalSecretReferenceV1>,
    pub resources: ResourceIntentV1,
    #[serde(default)]
    pub ports: Vec<PortIntentV1>,
    #[serde(default)]
    pub persistent_volumes: Vec<PersistentVolumeIntentV1>,
    pub health_check: HttpHealthCheckV1,
    pub ingress: Option<IngressIntentV1>,
    pub observability: ObservabilityIntentV1,
    pub lifecycle: LifecycleIntentV1,
}

impl ManagedDeploymentDesiredStateV1 {
    pub fn validate(&self) -> Result<(), Vec<ContractViolationV1>> {
        let mut violations = Vec::new();
        require_schema(
            &mut violations,
            "schema_version",
            &self.schema_version,
            DESIRED_STATE_SCHEMA_V1,
        );
        require_identifier(&mut violations, "deployment_id", &self.deployment_id);
        require_digest(
            &mut violations,
            "revision.revision_id",
            &self.revision.revision_id,
        );
        require_digest(
            &mut violations,
            "revision.release_bundle_digest",
            &self.revision.release_bundle_digest,
        );
        if self.revision.image.repository.trim().is_empty()
            || self
                .revision
                .image
                .repository
                .chars()
                .any(char::is_whitespace)
        {
            violations.push(ContractViolationV1::new(
                "revision.image.repository",
                "invalid_repository",
                "OCI repository must be non-empty and contain no whitespace",
            ));
        }
        if self.revision.image.repository.contains('@') {
            violations.push(ContractViolationV1::new(
                "revision.image.repository",
                "digest_must_be_separate",
                "OCI repository must not embed a digest; use image.digest",
            ));
        }
        if self
            .revision
            .image
            .repository
            .rsplit('/')
            .next()
            .is_some_and(|last_segment| last_segment.contains(':'))
        {
            violations.push(ContractViolationV1::new(
                "revision.image.repository",
                "mutable_tag_forbidden",
                "OCI repository must not include a tag; deploy image.digest",
            ));
        }
        require_digest(
            &mut violations,
            "revision.image.digest",
            &self.revision.image.digest,
        );

        if self.resources.cpu_millis == 0 {
            violations.push(ContractViolationV1::new(
                "resources.cpu_millis",
                "must_be_positive",
                "CPU intent must be greater than zero",
            ));
        }
        if self.resources.memory_bytes == 0 {
            violations.push(ContractViolationV1::new(
                "resources.memory_bytes",
                "must_be_positive",
                "memory intent must be greater than zero",
            ));
        }

        let mut names = BTreeSet::new();
        for (index, secret) in self.secrets.iter().enumerate() {
            require_environment_name(
                &mut violations,
                &format!("secrets[{index}].environment_name"),
                &secret.environment_name,
            );
            if secret
                .reference
                .strip_prefix("secret://")
                .is_none_or(|reference| reference.trim_matches('/').is_empty())
            {
                violations.push(ContractViolationV1::new(
                    format!("secrets[{index}].reference"),
                    "invalid_logical_secret_reference",
                    "secret reference must use a non-empty secret:// logical reference",
                ));
            }
            if !names.insert(secret.environment_name.as_str()) {
                violations.push(ContractViolationV1::new(
                    format!("secrets[{index}].environment_name"),
                    "duplicate",
                    "secret environment names must be unique",
                ));
            }
            if self.environment.contains_key(&secret.environment_name) {
                violations.push(ContractViolationV1::new(
                    format!("secrets[{index}].environment_name"),
                    "secret_value_collision",
                    "a secret binding must not also appear in plain environment values",
                ));
            }
        }
        for (name, value) in &self.environment {
            require_environment_name(&mut violations, &format!("environment.{name}"), name);
            if value.starts_with("secret://") {
                violations.push(ContractViolationV1::new(
                    format!("environment.{name}"),
                    "logical_secret_must_use_binding",
                    "logical secret references belong in secrets, not plain environment values",
                ));
            }
        }

        let mut port_names = BTreeSet::new();
        for (index, port) in self.ports.iter().enumerate() {
            require_identifier(&mut violations, &format!("ports[{index}].name"), &port.name);
            if port.container_port == 0 {
                violations.push(ContractViolationV1::new(
                    format!("ports[{index}].container_port"),
                    "must_be_positive",
                    "container port must be greater than zero",
                ));
            }
            if !port_names.insert(port.name.as_str()) {
                violations.push(ContractViolationV1::new(
                    format!("ports[{index}].name"),
                    "duplicate",
                    "port names must be unique",
                ));
            }
        }

        let mut volume_names = BTreeSet::new();
        let mut mount_paths = BTreeSet::new();
        for (index, volume) in self.persistent_volumes.iter().enumerate() {
            require_identifier(
                &mut violations,
                &format!("persistent_volumes[{index}].name"),
                &volume.name,
            );
            if !volume.mount_path.starts_with('/') {
                violations.push(ContractViolationV1::new(
                    format!("persistent_volumes[{index}].mount_path"),
                    "must_be_absolute",
                    "persistent volume mount path must be absolute",
                ));
            }
            if volume.minimum_bytes == 0 {
                violations.push(ContractViolationV1::new(
                    format!("persistent_volumes[{index}].minimum_bytes"),
                    "must_be_positive",
                    "persistent volume size must be greater than zero",
                ));
            }
            if !volume_names.insert(volume.name.as_str()) {
                violations.push(ContractViolationV1::new(
                    format!("persistent_volumes[{index}].name"),
                    "duplicate",
                    "persistent volume names must be unique",
                ));
            }
            if !mount_paths.insert(volume.mount_path.as_str()) {
                violations.push(ContractViolationV1::new(
                    format!("persistent_volumes[{index}].mount_path"),
                    "duplicate",
                    "persistent volume mount paths must be unique",
                ));
            }
        }

        if !port_names.contains(self.health_check.port_name.as_str()) {
            violations.push(ContractViolationV1::new(
                "health_check.port_name",
                "unknown_port",
                "health check must reference a declared port",
            ));
        }
        if !self.health_check.path.starts_with('/') {
            violations.push(ContractViolationV1::new(
                "health_check.path",
                "invalid_http_path",
                "HTTP health-check path must start with /",
            ));
        }
        if self.health_check.interval_seconds == 0
            || self.health_check.timeout_seconds == 0
            || self.health_check.timeout_seconds > self.health_check.interval_seconds
        {
            violations.push(ContractViolationV1::new(
                "health_check",
                "invalid_timing",
                "health-check timeout and interval must be positive and timeout must not exceed interval",
            ));
        }
        if let Some(ingress) = self.ingress.as_ref()
            && !port_names.contains(ingress.port_name.as_str())
        {
            violations.push(ContractViolationV1::new(
                "ingress.port_name",
                "unknown_port",
                "ingress must reference a declared port",
            ));
        }
        if let Some(hostname) = self
            .ingress
            .as_ref()
            .and_then(|ingress| ingress.requested_hostname.as_deref())
            && (hostname.trim().is_empty()
                || hostname.contains('/')
                || hostname.contains(':')
                || hostname.chars().any(char::is_whitespace))
        {
            violations.push(ContractViolationV1::new(
                "ingress.requested_hostname",
                "invalid_hostname",
                "requested hostname must be a bare DNS hostname without scheme, port, path, or whitespace",
            ));
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }

    pub fn required_capabilities(&self) -> BTreeSet<AdapterCapabilityV1> {
        let mut required = BTreeSet::from([
            AdapterCapabilityV1::ImmutableOciImage,
            AdapterCapabilityV1::ResourceIntent,
            AdapterCapabilityV1::HttpHealthCheck,
        ]);
        if !self.ports.is_empty() {
            required.insert(AdapterCapabilityV1::ContainerPorts);
        }
        if !self.secrets.is_empty() {
            required.insert(AdapterCapabilityV1::LogicalSecretReferences);
        }
        if !self.persistent_volumes.is_empty() {
            required.insert(AdapterCapabilityV1::PersistentVolumes);
        }
        if self.ingress.is_some() {
            required.insert(AdapterCapabilityV1::PublicHttpsIngress);
        }
        if self.observability.structured_logs {
            required.insert(AdapterCapabilityV1::StructuredLogs);
        }
        if self.lifecycle.rollback_required {
            required.insert(AdapterCapabilityV1::RevisionRollback);
        }
        required
    }

    pub fn required_capabilities_for_plan(
        &self,
        context: &ManagedDeploymentPlanContextV1,
    ) -> BTreeSet<AdapterCapabilityV1> {
        match context {
            ManagedDeploymentPlanContextV1::Inspection
            | ManagedDeploymentPlanContextV1::Mutation(ManagedDeploymentApprovalScopeV1::Deploy) => {
                self.required_capabilities()
            }
            ManagedDeploymentPlanContextV1::Mutation(
                ManagedDeploymentApprovalScopeV1::Rollback { .. },
            ) => {
                let mut required = self.required_capabilities();
                required.insert(AdapterCapabilityV1::RevisionRollback);
                required
            }
            ManagedDeploymentPlanContextV1::Mutation(
                ManagedDeploymentApprovalScopeV1::Provision,
            ) => {
                let mut required = BTreeSet::from([AdapterCapabilityV1::ResourceIntent]);
                if !self.persistent_volumes.is_empty() {
                    required.insert(AdapterCapabilityV1::PersistentVolumes);
                }
                required
            }
            ManagedDeploymentPlanContextV1::Mutation(
                ManagedDeploymentApprovalScopeV1::Destroy { .. },
            ) => BTreeSet::from([AdapterCapabilityV1::DeploymentDestroy]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeploymentRevisionV1 {
    pub revision_id: String,
    pub release_bundle_digest: String,
    pub image: OciImageV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DestroyConfirmationV1 {
    pub deployment_id: String,
    pub adapter_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OciImageV1 {
    pub repository: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogicalSecretReferenceV1 {
    pub environment_name: String,
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourceIntentV1 {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub architecture: WorkloadArchitectureV1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadArchitectureV1 {
    Amd64,
    Arm64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PortIntentV1 {
    pub name: String,
    pub container_port: u16,
    pub protocol: PortProtocolV1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PortProtocolV1 {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PersistentVolumeIntentV1 {
    pub name: String,
    pub mount_path: String,
    pub minimum_bytes: u64,
    pub access_mode: VolumeAccessModeV1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum VolumeAccessModeV1 {
    SingleWriter,
    MultiReader,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HttpHealthCheckV1 {
    pub port_name: String,
    pub path: String,
    pub interval_seconds: u32,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IngressIntentV1 {
    pub port_name: String,
    pub transport: IngressTransportV1,
    pub requested_hostname: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum IngressTransportV1 {
    Https,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityIntentV1 {
    pub structured_logs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleIntentV1 {
    pub rollback_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterCapabilitiesV1 {
    pub schema_version: String,
    pub adapter_id: String,
    pub capabilities: BTreeSet<AdapterCapabilityV1>,
}

impl AdapterCapabilitiesV1 {
    pub fn new(
        adapter_id: impl Into<String>,
        capabilities: impl IntoIterator<Item = AdapterCapabilityV1>,
    ) -> Self {
        Self {
            schema_version: ADAPTER_CAPABILITIES_SCHEMA_V1.to_string(),
            adapter_id: adapter_id.into(),
            capabilities: capabilities.into_iter().collect(),
        }
    }

    pub fn validate(&self) -> Result<(), Vec<ContractViolationV1>> {
        let mut violations = Vec::new();
        require_schema(
            &mut violations,
            "schema_version",
            &self.schema_version,
            ADAPTER_CAPABILITIES_SCHEMA_V1,
        );
        require_identifier(&mut violations, "adapter_id", &self.adapter_id);
        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCapabilityV1 {
    ImmutableOciImage,
    ResourceIntent,
    ContainerPorts,
    HttpHealthCheck,
    LogicalSecretReferences,
    PersistentVolumes,
    PublicHttpsIngress,
    StructuredLogs,
    RevisionRollback,
    DeploymentHistory,
    DeploymentDestroy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedDeploymentPlanV1 {
    pub schema_version: String,
    pub desired_state_schema_version: String,
    pub deployment_id: String,
    pub revision_id: String,
    pub adapter: AdapterCapabilitiesV1,
    pub mutation_performed: NoMutation,
    pub outcome: ManagedDeploymentPlanOutcomeV1,
}

impl ManagedDeploymentPlanV1 {
    pub fn unsupported_capability(
        desired: &ManagedDeploymentDesiredStateV1,
        adapter: AdapterCapabilitiesV1,
        missing_capabilities: BTreeSet<AdapterCapabilityV1>,
    ) -> Self {
        Self {
            schema_version: PLAN_SCHEMA_V1.to_string(),
            desired_state_schema_version: desired.schema_version.clone(),
            deployment_id: desired.deployment_id.clone(),
            revision_id: desired.revision.revision_id.clone(),
            adapter,
            mutation_performed: NoMutation,
            outcome: ManagedDeploymentPlanOutcomeV1::UnsupportedCapability {
                missing_capabilities,
            },
        }
    }

    pub fn applicable(
        desired: &ManagedDeploymentDesiredStateV1,
        adapter: AdapterCapabilitiesV1,
        applicable: ApplicableDeploymentPlanV1,
        approval: Option<(ManagedDeploymentApprovalScopeV1, PlanApprovalHashV1)>,
    ) -> Self {
        let (approval_scope, approval_hash) = approval
            .map(|(scope, hash)| (Some(scope), Some(hash)))
            .unwrap_or((None, None));
        Self {
            schema_version: PLAN_SCHEMA_V1.to_string(),
            desired_state_schema_version: desired.schema_version.clone(),
            deployment_id: desired.deployment_id.clone(),
            revision_id: desired.revision.revision_id.clone(),
            adapter,
            mutation_performed: NoMutation,
            outcome: ManagedDeploymentPlanOutcomeV1::Applicable {
                steps: applicable.steps,
                warnings: applicable.warnings,
                recurring_cost: applicable.recurring_cost,
                approval_scope,
                approval_hash,
            },
        }
    }

    pub fn approval_hash_for_scope(
        &self,
        scope: &ManagedDeploymentApprovalScopeV1,
    ) -> Option<&PlanApprovalHashV1> {
        match &self.outcome {
            ManagedDeploymentPlanOutcomeV1::Applicable {
                approval_scope: Some(approved),
                approval_hash: Some(hash),
                ..
            } if approved == scope => Some(hash),
            ManagedDeploymentPlanOutcomeV1::UnsupportedCapability { .. } => None,
            ManagedDeploymentPlanOutcomeV1::Applicable { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedDeploymentPlanOutcomeV1 {
    Applicable {
        steps: Vec<PlanStepV1>,
        warnings: Vec<String>,
        /// Adapter-supplied fixed recurring cost disclosure. This is not a
        /// total-cost estimate: usage-priced and separately managed resources
        /// remain outside this value.
        #[serde(default)]
        recurring_cost: RecurringCostDisclosureV1,
        /// Hash of the exact desired state, adapter capability/config
        /// fingerprint, normalized plan, recurring-cost disclosure, and exact
        /// mutation scope. Operation-scoped plans always contain it;
        /// inspection/legacy plans omit it and cannot authorize mutation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_hash: Option<PlanApprovalHashV1>,
        /// Exact provider-neutral mutation scope authorized by
        /// `approval_hash`. Legacy inspection plans omit both fields.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_scope: Option<ManagedDeploymentApprovalScopeV1>,
    },
    UnsupportedCapability {
        missing_capabilities: BTreeSet<AdapterCapabilityV1>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedDeploymentApprovalScopeV1 {
    Provision,
    Deploy,
    Rollback {
        target_revision: DeploymentRevisionV1,
    },
    Destroy {
        confirmation: DestroyConfirmationV1,
    },
}

/// Exact planning context passed through the adapter seam. Inspection retains
/// the legacy full-deployment view but cannot authorize mutation; scoped plans
/// must render only the selected lifecycle operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedDeploymentPlanContextV1 {
    Inspection,
    Mutation(ManagedDeploymentApprovalScopeV1),
}

impl ManagedDeploymentPlanContextV1 {
    pub fn approval_scope(&self) -> Option<&ManagedDeploymentApprovalScopeV1> {
        match self {
            Self::Inspection => None,
            Self::Mutation(scope) => Some(scope),
        }
    }
}

impl ManagedDeploymentApprovalScopeV1 {
    pub fn operation(&self) -> ManagedDeploymentOperationV1 {
        match self {
            Self::Provision => ManagedDeploymentOperationV1::Provision,
            Self::Deploy => ManagedDeploymentOperationV1::Deploy,
            Self::Rollback { .. } => ManagedDeploymentOperationV1::Rollback,
            Self::Destroy { .. } => ManagedDeploymentOperationV1::Destroy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanApprovalHashV1([u8; 32]);

impl PlanApprovalHashV1 {
    /// Construct an approval hash from a coordinator-computed digest.
    ///
    /// Adapter crates compute the domain-separated digest while this shared
    /// type owns parsing, serialization, and constant-time comparison.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        encode_lower_hex(&self.0)
    }

    pub fn constant_time_eq(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
    }
}

impl std::fmt::Display for PlanApprovalHashV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&encode_lower_hex(&self.0))
    }
}

impl std::str::FromStr for PlanApprovalHashV1 {
    type Err = PlanApprovalHashParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = value.as_bytes();
        if bytes.len() != 64
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(PlanApprovalHashParseError);
        }
        let mut decoded = [0_u8; 32];
        for (index, pair) in bytes.chunks_exact(2).enumerate() {
            decoded[index] = (lower_hex_nibble(pair[0]) << 4) | lower_hex_nibble(pair[1]);
        }
        Ok(Self(decoded))
    }
}

impl Serialize for PlanApprovalHashV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_lower_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for PlanApprovalHashV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
#[error("plan approval hash must be exactly 64 lowercase hexadecimal characters")]
pub struct PlanApprovalHashParseError;

pub(crate) fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn lower_hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("validated lowercase hexadecimal nibble"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApplicableDeploymentPlanV1 {
    pub steps: Vec<PlanStepV1>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Fixed recurring cost configured by the operator for this adapter, or a
    /// fail-closed unknown value that requires operator approval.
    #[serde(default)]
    pub recurring_cost: RecurringCostDisclosureV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecurringCostDisclosureV1 {
    Known {
        amount_minor_units: u64,
        currency: String,
        interval: RecurringCostIntervalV1,
    },
    Unknown {
        operator_approval_required: RequiredApproval,
    },
}

impl Default for RecurringCostDisclosureV1 {
    fn default() -> Self {
        Self::Unknown {
            operator_approval_required: RequiredApproval,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecurringCostIntervalV1 {
    pub count: u32,
    pub unit: RecurringCostIntervalUnitV1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecurringCostIntervalUnitV1 {
    Hour,
    Day,
    Month,
    Year,
}

/// Type-level proof that an unknown recurring cost cannot be accepted without
/// an operator decision. Deserialization rejects `false`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequiredApproval;

impl Serialize for RequiredApproval {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for RequiredApproval {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(de::Error::custom(
                "unknown recurring cost must require operator approval",
            ))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlanStepV1 {
    pub sequence: u32,
    pub action: PlanActionV1,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PlanActionV1 {
    AcquireCompute,
    ConfigureEnvironment,
    ResolveSecrets,
    AttachStorage,
    DeployRevision,
    ConfigureIngress,
    VerifyHealth,
    RecordRollbackReference,
    DestroyDeployment,
}

/// A planning response is incapable of representing a mutation. The custom
/// deserializer rejects `true`, so a provider adapter cannot deserialize or
/// emit a conforming plan that claims an unsupported decision mutated state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoMutation;

impl NoMutation {
    pub const fn as_bool(self) -> bool {
        false
    }
}

impl Serialize for NoMutation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for NoMutation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Err(de::Error::custom(
                "managed deployment plans must be mutation-free",
            ))
        } else {
            Ok(Self)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedDeploymentResultV1 {
    pub schema_version: String,
    pub operation: ManagedDeploymentOperationV1,
    pub deployment_id: String,
    pub revision_id: String,
    pub adapter_id: String,
    pub status: ManagedDeploymentOperationStatusV1,
    pub image_digests: Vec<OciImageV1>,
    #[serde(default)]
    pub endpoints: Vec<DeploymentEndpointV1>,
    pub health: DeploymentHealthV1,
    pub rollback: Option<RollbackReferenceV1>,
    pub failure: Option<DeploymentFailureV1>,
    pub observed_at_epoch_seconds: i64,
}

impl ManagedDeploymentResultV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        operation: ManagedDeploymentOperationV1,
        deployment_id: impl Into<String>,
        revision_id: impl Into<String>,
        adapter_id: impl Into<String>,
        status: ManagedDeploymentOperationStatusV1,
        image_digests: Vec<OciImageV1>,
        health: DeploymentHealthV1,
        observed_at_epoch_seconds: i64,
    ) -> Self {
        Self {
            schema_version: RESULT_SCHEMA_V1.to_string(),
            operation,
            deployment_id: deployment_id.into(),
            revision_id: revision_id.into(),
            adapter_id: adapter_id.into(),
            status,
            image_digests,
            endpoints: Vec::new(),
            health,
            rollback: None,
            failure: None,
            observed_at_epoch_seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedDeploymentHistoryV1 {
    pub schema_version: String,
    pub operation: ManagedDeploymentOperationV1,
    pub deployment_id: String,
    pub adapter_id: String,
    pub entries: Vec<DeploymentHistoryEntryV1>,
    pub truncated: bool,
    pub observed_at_epoch_seconds: i64,
}

impl ManagedDeploymentHistoryV1 {
    pub fn new(
        deployment_id: impl Into<String>,
        adapter_id: impl Into<String>,
        entries: Vec<DeploymentHistoryEntryV1>,
        truncated: bool,
        observed_at_epoch_seconds: i64,
    ) -> Self {
        Self {
            schema_version: HISTORY_SCHEMA_V1.to_string(),
            operation: ManagedDeploymentOperationV1::History,
            deployment_id: deployment_id.into(),
            adapter_id: adapter_id.into(),
            entries,
            truncated,
            observed_at_epoch_seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeploymentHistoryEntryV1 {
    pub sequence: u32,
    pub status: DeploymentHistoryStatusV1,
    pub image: Option<OciImageV1>,
    pub recorded_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentHistoryStatusV1 {
    Pending,
    Active,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedDeploymentLogsV1 {
    pub schema_version: String,
    pub operation: ManagedDeploymentOperationV1,
    pub deployment_id: String,
    pub adapter_id: String,
    pub entries: Vec<DeploymentLogEntryV1>,
    pub truncated: bool,
    pub observed_at_epoch_seconds: i64,
}

impl ManagedDeploymentLogsV1 {
    pub fn new(
        deployment_id: impl Into<String>,
        adapter_id: impl Into<String>,
        entries: Vec<DeploymentLogEntryV1>,
        truncated: bool,
        observed_at_epoch_seconds: i64,
    ) -> Self {
        Self {
            schema_version: LOGS_SCHEMA_V1.to_string(),
            operation: ManagedDeploymentOperationV1::Logs,
            deployment_id: deployment_id.into(),
            adapter_id: adapter_id.into(),
            entries,
            truncated,
            observed_at_epoch_seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeploymentLogEntryV1 {
    pub sequence: u32,
    pub recorded_at: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDeploymentOperationV1 {
    Provision,
    Deploy,
    Status,
    History,
    Logs,
    Rollback,
    Destroy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDeploymentOperationStatusV1 {
    InProgress,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeploymentEndpointV1 {
    pub name: String,
    pub url: String,
    pub transport: EndpointTransportV1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndpointTransportV1 {
    Https,
    Tcp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeploymentHealthV1 {
    pub status: DeploymentHealthStatusV1,
    pub checked_url: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentHealthStatusV1 {
    Unknown,
    Starting,
    Healthy,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RollbackReferenceV1 {
    pub revision_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeploymentFailureV1 {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContractViolationV1 {
    pub path: String,
    pub code: String,
    pub message: String,
}

impl ContractViolationV1 {
    fn new(path: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            code: code.into(),
            message: message.into(),
        }
    }
}

fn require_schema(
    violations: &mut Vec<ContractViolationV1>,
    path: &str,
    actual: &str,
    expected: &str,
) {
    if actual != expected {
        violations.push(ContractViolationV1::new(
            path,
            "unsupported_schema_version",
            format!("expected {expected}, got {actual}"),
        ));
    }
}

fn require_identifier(violations: &mut Vec<ContractViolationV1>, path: &str, value: &str) {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        violations.push(ContractViolationV1::new(
            path,
            "invalid_identifier",
            "identifier must contain 1-128 ASCII letters, digits, dots, hyphens, or underscores",
        ));
    }
}

fn require_environment_name(violations: &mut Vec<ContractViolationV1>, path: &str, value: &str) {
    let mut bytes = value.bytes();
    let valid_first = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_uppercase() || byte == b'_');
    if !valid_first
        || value.len() > 128
        || !bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        violations.push(ContractViolationV1::new(
            path,
            "invalid_environment_name",
            "environment name must use uppercase ASCII letters, digits, and underscores",
        ));
    }
}

fn require_digest(violations: &mut Vec<ContractViolationV1>, path: &str, value: &str) {
    let Some(hex) = value.strip_prefix("sha256:") else {
        violations.push(ContractViolationV1::new(
            path,
            "invalid_digest",
            "digest must use sha256:<64 lowercase hexadecimal characters>",
        ));
        return;
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        violations.push(ContractViolationV1::new(
            path,
            "invalid_digest",
            "digest must use sha256:<64 lowercase hexadecimal characters>",
        ));
    }
}
