//! Durable orchestration state for provider-neutral Managed Publications.
//!
//! External adapters may be retried or reconciled, but their progress must
//! never live only in an agent process. Every read revalidates the shared wire
//! contract, and every update is a compare-and-swap over one explicit state
//! transition.

use froglet_protocol::managed_deployment::{
    DESIRED_STATE_SCHEMA_V1, DeploymentRevisionV1, HttpHealthCheckV1, IngressIntentV1,
    LifecycleIntentV1, LogicalSecretReferenceV1, ManagedDeploymentApprovalScopeV1,
    ManagedDeploymentDesiredStateV1, ManagedDeploymentPlanV1, ManagedDeploymentResultV1,
    ObservabilityIntentV1, OciImageV1, PersistentVolumeIntentV1, PlanApprovalHashV1, PortIntentV1,
    ResourceIntentV1,
};
use froglet_protocol::managed_publication::{
    MANAGED_PUBLICATION_OPERATION_SCHEMA_V1, ManagedPublicationCapsuleV1,
    ManagedPublicationOperationV1, ManagedPublicationPhaseV1, ManagedPublicationPlanV1,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
};

use crate::{
    canonical_json, db,
    process_runtime::{ProcessRuntimeConfig, read_child_output_bounded, stream_limit_error},
};

pub const MANAGED_TARGET_REGISTRY_ENV: &str = "FROGLET_MANAGED_TARGETS_FILE";
pub const MANAGED_TARGET_REGISTRY_SCHEMA_V1: &str = "froglet.managed-target-registry.v1";
pub const MANAGED_CAPSULE_ENV: &str = "FROGLET_MANAGED_CAPSULE_BASE64";
pub const MANAGED_IDENTITY_SEED_ENV: &str = "FROGLET_IDENTITY_SEED_HEX";
pub const MANAGED_BUNDLE_PATH_ENV: &str = "FROGLET_MANAGED_BUNDLE_PATH";
pub const MANAGED_BUNDLE_CONTAINER_PATH: &str = "/opt/froglet/managed/bundle.json";
pub const MANAGED_PUBLIC_BASE_URL_ENV: &str = "FROGLET_PUBLIC_BASE_URL";
const MAX_TARGET_REGISTRY_BYTES: u64 = 1024 * 1024;
const MAX_OCI_METADATA_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedTargetRegistryV1 {
    pub schema_version: String,
    pub targets: BTreeMap<String, BTreeMap<String, ManagedTargetProfileV1>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedTargetProfileV1 {
    pub operator_binary: PathBuf,
    pub adapter: String,
    pub adapter_config_path: PathBuf,
    pub output_repository: String,
    pub base_runner_image: OciImageV1,
    pub release_bundle_digest: String,
    pub base_manifest_path: PathBuf,
    pub base_config_path: PathBuf,
    pub public_url: String,
    #[serde(default)]
    pub provision: bool,
    pub deployment: ManagedDeploymentTemplateV1,
    /// Provider-private environment variable that the operator's logical
    /// secret resolver maps to `FROGLET_MANAGED_CAPSULE_BASE64`.
    pub capsule_source_environment: String,
    #[serde(default)]
    pub registry_auth: ManagedRegistryAuthV1,
    #[serde(default)]
    pub registry_insecure_http: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedRegistryAuthV1 {
    #[default]
    Anonymous,
    Basic {
        username_environment: String,
        password_environment: String,
    },
    Bearer {
        token_environment: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedDeploymentTemplateV1 {
    pub deployment_id: String,
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

impl ManagedDeploymentTemplateV1 {
    pub fn desired_state(
        &self,
        package: &froglet_protocol::managed_publication::ManagedPublicationPackageV1,
    ) -> ManagedDeploymentDesiredStateV1 {
        ManagedDeploymentDesiredStateV1 {
            schema_version: DESIRED_STATE_SCHEMA_V1.to_string(),
            deployment_id: self.deployment_id.clone(),
            revision: DeploymentRevisionV1 {
                revision_id: package.bundle_manifest_digest.clone(),
                release_bundle_digest: package.release_bundle_digest.clone(),
                image: package.image.clone(),
            },
            environment: self.environment.clone(),
            secrets: self.secrets.clone(),
            resources: self.resources.clone(),
            ports: self.ports.clone(),
            persistent_volumes: self.persistent_volumes.clone(),
            health_check: self.health_check.clone(),
            ingress: self.ingress.clone(),
            observability: self.observability.clone(),
            lifecycle: self.lifecycle.clone(),
        }
    }
}

fn require_regular_absolute_file(
    path: &Path,
    field: &str,
    max_bytes: Option<u64>,
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{field} must be an absolute path"));
    }
    let metadata = fs::metadata(path).map_err(|_| format!("{field} is unavailable"))?;
    if !metadata.is_file() {
        return Err(format!("{field} must be a regular file"));
    }
    if max_bytes.is_some_and(|limit| metadata.len() > limit) {
        return Err(format!("{field} exceeds its byte limit"));
    }
    Ok(())
}

impl ManagedTargetProfileV1 {
    fn validate(&self) -> Result<(), String> {
        require_regular_absolute_file(&self.operator_binary, "operator_binary", None)?;
        require_regular_absolute_file(
            &self.adapter_config_path,
            "adapter_config_path",
            Some(MAX_TARGET_REGISTRY_BYTES),
        )?;
        require_regular_absolute_file(
            &self.base_manifest_path,
            "base_manifest_path",
            Some(MAX_OCI_METADATA_BYTES),
        )?;
        require_regular_absolute_file(
            &self.base_config_path,
            "base_config_path",
            Some(MAX_OCI_METADATA_BYTES),
        )?;
        if self.adapter.trim().is_empty() || self.adapter.chars().any(char::is_whitespace) {
            return Err("adapter must be non-empty and contain no whitespace".to_string());
        }
        let package_probe = froglet_protocol::managed_publication::ManagedPublicationPackageV1 {
            schema_version: froglet_protocol::managed_publication::MANAGED_PUBLICATION_PACKAGE_SCHEMA_V1.to_string(),
            source_package_digest: "0".repeat(64),
            build_evidence_digest: "0".repeat(64),
            bundle_manifest_digest: format!("sha256:{}", "0".repeat(64)),
            release_bundle_digest: self.release_bundle_digest.clone(),
            base_runner_image: self.base_runner_image.clone(),
            image: OciImageV1 { repository: self.output_repository.clone(), digest: format!("sha256:{}", "0".repeat(64)) },
            content_visibility: froglet_protocol::managed_publication::ManagedPublicationContentVisibilityV1::PrivateRegistryRequired,
            runtime: "probe".to_string(),
            package_kind: "probe".to_string(),
            builder_id: "probe".to_string(),
            builder_fingerprint: "0".repeat(64),
            runner_contract: froglet_protocol::managed_publication::MANAGED_PUBLICATION_RUNNER_CONTRACT_V1.to_string(),
        };
        package_probe
            .validate()
            .map_err(|error| error.to_string())?;
        let parsed = url::Url::parse(&self.public_url)
            .map_err(|_| "public_url must be an absolute HTTPS URL".to_string())?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || parsed.cannot_be_a_base()
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(
                "public_url must be a credential-free absolute HTTPS root origin".to_string(),
            );
        }
        if self.capsule_source_environment.is_empty()
            || !self
                .capsule_source_environment
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Err(
                "capsule_source_environment must be an uppercase environment name".to_string(),
            );
        }
        match &self.registry_auth {
            ManagedRegistryAuthV1::Anonymous => {}
            ManagedRegistryAuthV1::Basic {
                username_environment,
                password_environment,
            } => {
                if username_environment.is_empty() || password_environment.is_empty() {
                    return Err(
                        "registry basic auth environment names must be non-empty".to_string()
                    );
                }
            }
            ManagedRegistryAuthV1::Bearer { token_environment } => {
                if token_environment.is_empty() {
                    return Err(
                        "registry bearer token environment name must be non-empty".to_string()
                    );
                }
            }
        }
        for required in [
            MANAGED_CAPSULE_ENV,
            MANAGED_IDENTITY_SEED_ENV,
            crate::identity::NOSTR_PUBLICATION_IDENTITY_SEED_ENV,
        ] {
            if !self
                .deployment
                .secrets
                .iter()
                .any(|secret| secret.environment_name == required)
            {
                return Err(format!(
                    "deployment template must bind logical secret {required}"
                ));
            }
        }
        if self
            .deployment
            .environment
            .get(MANAGED_BUNDLE_PATH_ENV)
            .map(String::as_str)
            != Some(MANAGED_BUNDLE_CONTAINER_PATH)
        {
            return Err(format!(
                "deployment template must set {MANAGED_BUNDLE_PATH_ENV}={MANAGED_BUNDLE_CONTAINER_PATH}"
            ));
        }
        if self
            .deployment
            .environment
            .get(MANAGED_PUBLIC_BASE_URL_ENV)
            .map(|value| value.trim_end_matches('/'))
            != Some(self.public_url.trim_end_matches('/'))
        {
            return Err(format!(
                "deployment template must set {MANAGED_PUBLIC_BASE_URL_ENV} to the exact public_url"
            ));
        }
        let desired = self.deployment.desired_state(&package_probe);
        desired.validate().map_err(|violations| {
            format!(
                "deployment template has {} contract violation(s)",
                violations.len()
            )
        })?;
        Ok(())
    }

    pub fn validate_package(
        &self,
        package: &froglet_protocol::managed_publication::ManagedPublicationPackageV1,
    ) -> Result<(), String> {
        self.validate()?;
        package.validate().map_err(|error| error.to_string())?;
        if package.base_runner_image != self.base_runner_image
            || package.release_bundle_digest != self.release_bundle_digest
            || package.image.repository != self.output_repository
        {
            return Err("managed package does not match the selected target profile".to_string());
        }
        Ok(())
    }

    pub fn read_base_oci_metadata(&self) -> Result<(Vec<u8>, Vec<u8>), String> {
        self.validate()?;
        let manifest = fs::read(&self.base_manifest_path)
            .map_err(|_| "base runner manifest could not be read".to_string())?;
        let config = fs::read(&self.base_config_path)
            .map_err(|_| "base runner config could not be read".to_string())?;
        let config_document: serde_json::Value = serde_json::from_slice(&config)
            .map_err(|_| "base runner config is invalid JSON".to_string())?;
        let expected_architecture = match self.deployment.resources.architecture {
            froglet_protocol::managed_deployment::WorkloadArchitectureV1::Amd64 => "amd64",
            froglet_protocol::managed_deployment::WorkloadArchitectureV1::Arm64 => "arm64",
        };
        if config_document
            .get("os")
            .and_then(serde_json::Value::as_str)
            != Some("linux")
            || config_document
                .get("architecture")
                .and_then(serde_json::Value::as_str)
                != Some(expected_architecture)
        {
            return Err(format!(
                "base runner config must be linux/{expected_architecture} to match the deployment template"
            ));
        }
        Ok((manifest, config))
    }
}

pub fn load_target_profile(target: &str, profile: &str) -> Result<ManagedTargetProfileV1, String> {
    let path = std::env::var_os(MANAGED_TARGET_REGISTRY_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{MANAGED_TARGET_REGISTRY_ENV} is not configured"))?;
    require_regular_absolute_file(
        &path,
        MANAGED_TARGET_REGISTRY_ENV,
        Some(MAX_TARGET_REGISTRY_BYTES),
    )?;
    let bytes =
        fs::read(&path).map_err(|_| "managed target registry could not be read".to_string())?;
    let registry: ManagedTargetRegistryV1 = serde_json::from_slice(&bytes)
        .map_err(|error| format!("managed target registry is invalid JSON: {error}"))?;
    if registry.schema_version != MANAGED_TARGET_REGISTRY_SCHEMA_V1 {
        return Err(format!(
            "unsupported managed target registry schema {:?}",
            registry.schema_version
        ));
    }
    let selected = registry
        .targets
        .get(target)
        .and_then(|profiles| profiles.get(profile))
        .cloned()
        .ok_or_else(|| format!("managed target/profile {target:?}/{profile:?} was not found"))?;
    selected.validate()?;
    Ok(selected)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorCliErrorV1 {
    code: String,
    message: String,
}

fn invoke_operator<T: DeserializeOwned>(
    profile: &ManagedTargetProfileV1,
    desired: &ManagedDeploymentDesiredStateV1,
    leading_arguments: &[String],
    capsule_base64: Option<&str>,
) -> Result<T, String> {
    profile.validate()?;
    desired.validate().map_err(|violations| {
        format!(
            "desired state has {} contract violation(s)",
            violations.len()
        )
    })?;
    let desired_json = canonical_json::to_vec(desired).map_err(|error| error.to_string())?;
    let mut command = std::process::Command::new(&profile.operator_binary);
    command
        .args(leading_arguments)
        .arg("--adapter")
        .arg(&profile.adapter)
        .arg("--desired-state")
        .arg("-")
        .arg("--adapter-config")
        .arg(&profile.adapter_config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(capsule) = capsule_base64 {
        command.env(&profile.capsule_source_environment, capsule);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("managed operator could not start: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "managed operator stdin was unavailable".to_string())?;
    let stdin_writer = std::thread::spawn(move || stdin.write_all(&desired_json));
    let output = read_child_output_bounded(
        child.stdout.take(),
        child.stderr.take(),
        &ProcessRuntimeConfig::default(),
    )?;
    stdin_writer
        .join()
        .map_err(|_| "managed operator stdin writer panicked".to_string())?
        .map_err(|_| "managed operator could not read desired state".to_string())?;
    let status = child
        .wait()
        .map_err(|error| format!("managed operator wait failed: {error}"))?;
    if output.stdout.truncated {
        return Err(stream_limit_error(
            "managed operator stdout",
            &output.stdout,
        ));
    }
    if output.stderr.truncated {
        return Err(stream_limit_error(
            "managed operator stderr",
            &output.stderr,
        ));
    }
    if !status.success() {
        return match serde_json::from_slice::<OperatorCliErrorV1>(&output.stderr.bytes) {
            Ok(error) => Err(format!(
                "managed operator {}: {}",
                error.code, error.message
            )),
            Err(_) => Err(format!("managed operator exited with status {status}")),
        };
    }
    serde_json::from_slice(&output.stdout.bytes)
        .map_err(|error| format!("managed operator returned invalid normalized JSON: {error}"))
}

pub fn plan_operator_operation(
    profile: &ManagedTargetProfileV1,
    desired: &ManagedDeploymentDesiredStateV1,
    scope: &ManagedDeploymentApprovalScopeV1,
) -> Result<ManagedDeploymentPlanV1, String> {
    let mut arguments = vec!["plan".to_string(), "--for-operation".to_string()];
    match scope {
        ManagedDeploymentApprovalScopeV1::Provision => arguments.push("provision".to_string()),
        ManagedDeploymentApprovalScopeV1::Deploy => arguments.push("deploy".to_string()),
        ManagedDeploymentApprovalScopeV1::Rollback { .. } => return Err("rollback planning requires a target revision file and is not supported by managed target schema v1".to_string()),
        ManagedDeploymentApprovalScopeV1::Destroy { confirmation } => {
            arguments.push("destroy".to_string());
            arguments.extend(["--confirm-deployment".to_string(), confirmation.deployment_id.clone(), "--confirm-adapter".to_string(), confirmation.adapter_id.clone()]);
        }
    }
    let plan: ManagedDeploymentPlanV1 = invoke_operator(profile, desired, &arguments, None)?;
    froglet_protocol::managed_publication::validate_operator_plan(&plan, desired, scope)
        .map_err(|error| error.to_string())?;
    Ok(plan)
}

pub fn execute_operator_operation(
    profile: &ManagedTargetProfileV1,
    desired: &ManagedDeploymentDesiredStateV1,
    scope: &ManagedDeploymentApprovalScopeV1,
    approval_hash: &PlanApprovalHashV1,
    capsule_base64: Option<&str>,
) -> Result<ManagedDeploymentResultV1, String> {
    let mut arguments = Vec::new();
    match scope {
        ManagedDeploymentApprovalScopeV1::Provision => arguments.push("provision".to_string()),
        ManagedDeploymentApprovalScopeV1::Deploy => arguments.push("deploy".to_string()),
        ManagedDeploymentApprovalScopeV1::Rollback { .. } => {
            return Err(
                "rollback execution is not supported by managed target schema v1".to_string(),
            );
        }
        ManagedDeploymentApprovalScopeV1::Destroy { confirmation } => {
            arguments.push("destroy".to_string());
            arguments.extend([
                "--confirm-deployment".to_string(),
                confirmation.deployment_id.clone(),
                "--confirm-adapter".to_string(),
                confirmation.adapter_id.clone(),
            ]);
        }
    }
    arguments.extend(["--approve-plan".to_string(), approval_hash.to_string()]);
    invoke_operator(profile, desired, &arguments, capsule_base64)
}

pub fn inspect_operator_status(
    profile: &ManagedTargetProfileV1,
    desired: &ManagedDeploymentDesiredStateV1,
) -> Result<ManagedDeploymentResultV1, String> {
    invoke_operator(profile, desired, &["status".to_string()], None)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManagedPublicationRecord {
    pub plan: ManagedPublicationPlanV1,
    pub operation: ManagedPublicationOperationV1,
    pub capsule: Option<ManagedPublicationCapsuleV1>,
    pub state_version: u64,
    pub created_at_epoch_seconds: i64,
}

fn canonical_string<T: serde::Serialize>(value: &T) -> Result<String, String> {
    let bytes = canonical_json::to_vec(value).map_err(|error| error.to_string())?;
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

fn phase_name(phase: ManagedPublicationPhaseV1) -> &'static str {
    match phase {
        ManagedPublicationPhaseV1::Planned => "planned",
        ManagedPublicationPhaseV1::LocalRevisionReady => "local_revision_ready",
        ManagedPublicationPhaseV1::Provisioning => "provisioning",
        ManagedPublicationPhaseV1::Deploying => "deploying",
        ManagedPublicationPhaseV1::Reconciling => "reconciling",
        ManagedPublicationPhaseV1::Deployed => "deployed",
        ManagedPublicationPhaseV1::CanaryVerified => "canary_verified",
        ManagedPublicationPhaseV1::Registering => "registering",
        ManagedPublicationPhaseV1::Active => "active",
        ManagedPublicationPhaseV1::Compensating => "compensating",
        ManagedPublicationPhaseV1::Compensated => "compensated",
        ManagedPublicationPhaseV1::ReconciliationRequired => "reconciliation_required",
        ManagedPublicationPhaseV1::Failed => "failed",
    }
}

fn phase_requires_capsule(phase: ManagedPublicationPhaseV1) -> bool {
    matches!(
        phase,
        ManagedPublicationPhaseV1::LocalRevisionReady
            | ManagedPublicationPhaseV1::Provisioning
            | ManagedPublicationPhaseV1::Deploying
            | ManagedPublicationPhaseV1::Reconciling
            | ManagedPublicationPhaseV1::Deployed
            | ManagedPublicationPhaseV1::CanaryVerified
            | ManagedPublicationPhaseV1::Registering
            | ManagedPublicationPhaseV1::Active
            | ManagedPublicationPhaseV1::Compensating
            | ManagedPublicationPhaseV1::Compensated
            | ManagedPublicationPhaseV1::ReconciliationRequired
    )
}

fn decode_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ManagedPublicationRecord> {
    let operation_id: String = row.get(0)?;
    let service_id: String = row.get(1)?;
    let plan_hash: String = row.get(2)?;
    let consent_hash: String = row.get(3)?;
    let phase: String = row.get(4)?;
    let revision_hash: Option<String> = row.get(5)?;
    let plan_json: String = row.get(6)?;
    let operation_json: String = row.get(7)?;
    let capsule_json: Option<String> = row.get(8)?;
    let state_version: i64 = row.get(9)?;
    let created_at_epoch_seconds: i64 = row.get(10)?;
    let updated_at_epoch_seconds: i64 = row.get(11)?;

    let invalid = |message: String| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                message,
            )),
        )
    };
    let plan: ManagedPublicationPlanV1 = serde_json::from_str(&plan_json).map_err(|error| {
        invalid(format!(
            "stored Managed Publication plan is invalid: {error}"
        ))
    })?;
    plan.validate().map_err(|error| {
        invalid(format!(
            "stored Managed Publication plan failed validation: {error}"
        ))
    })?;
    let operation: ManagedPublicationOperationV1 =
        serde_json::from_str(&operation_json).map_err(|error| {
            invalid(format!(
                "stored Managed Publication operation is invalid: {error}"
            ))
        })?;
    operation.validate_against(&plan).map_err(|error| {
        invalid(format!(
            "stored Managed Publication operation failed validation: {error}"
        ))
    })?;
    let capsule = capsule_json
        .map(|json| {
            serde_json::from_str::<ManagedPublicationCapsuleV1>(&json).map_err(|error| {
                invalid(format!(
                    "stored Managed Publication capsule is invalid: {error}"
                ))
            })
        })
        .transpose()?;
    if let Some(capsule) = &capsule {
        capsule.validate_against(&plan).map_err(|error| {
            invalid(format!(
                "stored Managed Publication capsule failed validation: {error}"
            ))
        })?;
    }
    if operation_id != plan.payload.operation_id
        || operation_id != operation.operation_id
        || service_id != plan.payload.service_id
        || plan_hash != plan.plan_hash
        || plan_hash != operation.plan_hash
        || consent_hash != operation.consent_hash
        || phase != phase_name(operation.phase)
        || revision_hash != operation.revision_hash
        || updated_at_epoch_seconds != operation.updated_at_epoch_seconds
        || state_version < 1
        || capsule.as_ref().is_some_and(|capsule| {
            capsule.operation_id != operation_id
                || capsule.plan_hash != plan_hash
                || capsule.consent_hash != consent_hash
                || Some(capsule.revision.revision_hash.as_str()) != revision_hash.as_deref()
                || operation.registration.as_ref().is_some_and(|registration| {
                    registration.validated_offer_hash != capsule.revision.payload.offer_hash
                })
        })
        || (phase_requires_capsule(operation.phase) && capsule.is_none())
    {
        return Err(invalid(
            "stored Managed Publication columns do not match their validated documents".to_string(),
        ));
    }

    Ok(ManagedPublicationRecord {
        plan,
        operation,
        capsule,
        state_version: u64::try_from(state_version)
            .map_err(|_| invalid("stored state_version is out of range".to_string()))?,
        created_at_epoch_seconds,
    })
}

const RECORD_COLUMNS: &str = "operation_id, service_id, plan_hash, consent_hash, phase, revision_hash, plan_json, operation_json, capsule_json, state_version, created_at, updated_at";

pub fn get(
    conn: &Connection,
    operation_id: &str,
) -> Result<Option<ManagedPublicationRecord>, String> {
    conn.query_row(
        &format!(
            "SELECT {RECORD_COLUMNS} FROM managed_publication_operations WHERE operation_id = ?1"
        ),
        params![operation_id],
        decode_record,
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub fn create_planned(
    conn: &Connection,
    plan: &ManagedPublicationPlanV1,
    consent_hash: &str,
    now: i64,
) -> Result<ManagedPublicationRecord, String> {
    plan.validate().map_err(|error| error.to_string())?;
    if now < 0 {
        return Err("Managed Publication timestamp must be non-negative".to_string());
    }
    let operation = ManagedPublicationOperationV1 {
        schema_version: MANAGED_PUBLICATION_OPERATION_SCHEMA_V1.to_string(),
        operation_id: plan.payload.operation_id.clone(),
        plan_hash: plan.plan_hash.clone(),
        consent_hash: consent_hash.to_string(),
        phase: ManagedPublicationPhaseV1::Planned,
        revision_hash: None,
        provision_result: None,
        deploy_result: None,
        compensation_result: None,
        remote_canary: None,
        registration: None,
        last_error: None,
        attempt_count: 0,
        updated_at_epoch_seconds: now,
    };
    operation
        .validate_against(plan)
        .map_err(|error| error.to_string())?;
    let plan_json = canonical_string(plan)?;
    let operation_json = canonical_string(&operation)?;

    db::with_immediate_transaction(conn, |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO managed_publication_operations (
                operation_id, service_id, plan_hash, consent_hash, phase,
                revision_hash, plan_json, operation_json, capsule_json,
                state_version, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, NULL, 1, ?8, ?8)",
            params![
                plan.payload.operation_id,
                plan.payload.service_id,
                plan.plan_hash,
                consent_hash,
                phase_name(operation.phase),
                plan_json,
                operation_json,
                now,
            ],
        )
        .map_err(|error| error.to_string())?;
        let stored = get(conn, &plan.payload.operation_id)?
            .ok_or_else(|| "Managed Publication operation missing after insert".to_string())?;
        if stored.plan != *plan || stored.operation.consent_hash != consent_hash {
            return Err(format!(
                "Managed Publication operation {} conflicts with an existing immutable plan or consent",
                plan.payload.operation_id
            ));
        }
        Ok(stored)
    })
}

pub fn attach_capsule(
    conn: &Connection,
    operation_id: &str,
    expected_state_version: u64,
    capsule: &ManagedPublicationCapsuleV1,
    now: i64,
) -> Result<ManagedPublicationRecord, String> {
    capsule.validate().map_err(|error| error.to_string())?;
    db::with_immediate_transaction(conn, |conn| {
        let current = get(conn, operation_id)?
            .ok_or_else(|| format!("Managed Publication operation {operation_id} was not found"))?;
        if current.state_version != expected_state_version {
            return Err(format!(
                "Managed Publication state version mismatch: expected {expected_state_version}, current {}",
                current.state_version
            ));
        }
        capsule
            .validate_against(&current.plan)
            .map_err(|error| error.to_string())?;
        if capsule.consent_hash != current.operation.consent_hash {
            return Err(
                "Managed Publication capsule consent does not match durable consent".to_string(),
            );
        }
        if current.operation.phase == ManagedPublicationPhaseV1::LocalRevisionReady {
            if current.capsule.as_ref() == Some(capsule) {
                return Ok(current);
            }
            return Err(
                "Managed Publication already has a different immutable capsule".to_string(),
            );
        }
        if current.operation.phase != ManagedPublicationPhaseV1::Planned {
            return Err(format!(
                "Managed Publication capsule cannot be attached in phase {}",
                phase_name(current.operation.phase)
            ));
        }
        let mut operation = current.operation.clone();
        operation.revision_hash = Some(capsule.revision.revision_hash.clone());
        operation
            .transition(ManagedPublicationPhaseV1::LocalRevisionReady, now)
            .map_err(|error| error.to_string())?;
        operation
            .validate_against(&current.plan)
            .map_err(|error| error.to_string())?;
        let operation_json = canonical_string(&operation)?;
        let capsule_json = canonical_string(capsule)?;
        let updated = conn
            .execute(
                "UPDATE managed_publication_operations
                    SET phase = ?3, revision_hash = ?4, operation_json = ?5,
                        capsule_json = ?6, state_version = state_version + 1,
                        updated_at = ?7
                  WHERE operation_id = ?1 AND state_version = ?2",
                params![
                    operation_id,
                    i64::try_from(expected_state_version).map_err(|_| {
                        "Managed Publication state version exceeds SQLite range".to_string()
                    })?,
                    phase_name(operation.phase),
                    operation.revision_hash,
                    operation_json,
                    capsule_json,
                    now,
                ],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("Managed Publication compare-and-swap update failed".to_string());
        }
        get(conn, operation_id)?
            .ok_or_else(|| "Managed Publication operation missing after capsule attach".to_string())
    })
}

pub fn update_operation(
    conn: &Connection,
    operation_id: &str,
    expected_state_version: u64,
    next: &ManagedPublicationOperationV1,
) -> Result<ManagedPublicationRecord, String> {
    db::with_immediate_transaction(conn, |conn| {
        let current = get(conn, operation_id)?
            .ok_or_else(|| format!("Managed Publication operation {operation_id} was not found"))?;
        if current.state_version != expected_state_version {
            return Err(format!(
                "Managed Publication state version mismatch: expected {expected_state_version}, current {}",
                current.state_version
            ));
        }
        if next.operation_id != current.operation.operation_id
            || next.plan_hash != current.operation.plan_hash
            || next.consent_hash != current.operation.consent_hash
            || next.revision_hash != current.operation.revision_hash
        {
            return Err(
                "Managed Publication update changed immutable operation coordinates".to_string(),
            );
        }
        if !current.operation.phase.can_transition_to(next.phase) {
            return Err(format!(
                "Managed Publication invalid durable transition from {} to {}",
                phase_name(current.operation.phase),
                phase_name(next.phase)
            ));
        }
        next.validate_against(&current.plan)
            .map_err(|error| error.to_string())?;
        let operation_json = canonical_string(next)?;
        let updated = conn
            .execute(
                "UPDATE managed_publication_operations
                    SET phase = ?3, revision_hash = ?4, operation_json = ?5,
                        state_version = state_version + 1, updated_at = ?6
                  WHERE operation_id = ?1 AND state_version = ?2",
                params![
                    operation_id,
                    i64::try_from(expected_state_version).map_err(|_| {
                        "Managed Publication state version exceeds SQLite range".to_string()
                    })?,
                    phase_name(next.phase),
                    next.revision_hash,
                    operation_json,
                    next.updated_at_epoch_seconds,
                ],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("Managed Publication compare-and-swap update failed".to_string());
        }
        get(conn, operation_id)?
            .ok_or_else(|| "Managed Publication operation missing after update".to_string())
    })
}

pub fn list_recoverable(conn: &Connection) -> Result<Vec<ManagedPublicationRecord>, String> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {RECORD_COLUMNS}
               FROM managed_publication_operations
              WHERE phase NOT IN ('active', 'compensated', 'failed')
              ORDER BY updated_at ASC, operation_id ASC"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], decode_record)
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Convert phases that may have crossed an external mutation boundary into an
/// explicit reconciliation requirement during startup. Purely local phases
/// remain resumable and terminal phases are excluded by `list_recoverable`.
pub fn recover_interrupted(
    conn: &Connection,
    now: i64,
) -> Result<Vec<ManagedPublicationRecord>, String> {
    let mut recovered = Vec::new();
    for current in list_recoverable(conn)? {
        if !matches!(
            current.operation.phase,
            ManagedPublicationPhaseV1::Provisioning
                | ManagedPublicationPhaseV1::Deploying
                | ManagedPublicationPhaseV1::Reconciling
                | ManagedPublicationPhaseV1::Registering
                | ManagedPublicationPhaseV1::Compensating
        ) {
            continue;
        }
        let mut operation = current.operation.clone();
        operation.last_error = Some(format!(
            "node restarted while Managed Publication was in {}",
            phase_name(operation.phase)
        ));
        operation
            .transition(ManagedPublicationPhaseV1::ReconciliationRequired, now)
            .map_err(|error| error.to_string())?;
        recovered.push(update_operation(
            conn,
            &current.operation.operation_id,
            current.state_version,
            &operation,
        )?);
    }
    Ok(recovered)
}
