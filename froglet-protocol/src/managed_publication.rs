//! Provider-neutral contract joining a Froglet Publication to a Managed
//! Deployment.
//!
//! This module is deliberately outside the Kernel. It binds the exact package,
//! immutable operator plans, expected Provider identity, public endpoint, and
//! compensation scope that the publishing agent presents for approval. Both
//! the Froglet Node and `froglet-services` consume these same wire types.

use crate::{
    canonical_json, crypto,
    managed_deployment::{
        DeploymentHealthStatusV1, ManagedDeploymentApprovalScopeV1,
        ManagedDeploymentDesiredStateV1, ManagedDeploymentOperationStatusV1,
        ManagedDeploymentPlanContextV1, ManagedDeploymentPlanOutcomeV1, ManagedDeploymentPlanV1,
        ManagedDeploymentResultV1, OciImageV1, PLAN_SCHEMA_V1, PlanApprovalHashV1,
    },
    protocol::{
        DescriptorPayload, OfferPayload, SignedArtifact, artifact_hash,
        validate_descriptor_artifact, validate_offer_artifact, verify_artifact,
    },
    publication::{PublicationIntent, SignedPublicationRevision, VerificationFixture},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const MANAGED_PUBLICATION_PLAN_SCHEMA_V1: &str = "froglet.managed-publication.plan.v1";
pub const MANAGED_PUBLICATION_PACKAGE_SCHEMA_V1: &str = "froglet.managed-publication.package.v1";
pub const MANAGED_PUBLICATION_PACKAGE_REQUEST_SCHEMA_V1: &str =
    "froglet.managed-publication.package-request.v1";
pub const MANAGED_PUBLICATION_CAPSULE_SCHEMA_V1: &str = "froglet.managed-publication.capsule.v1";
pub const MANAGED_PUBLICATION_BUNDLE_SCHEMA_V1: &str = "froglet.managed-publication.bundle.v1";
pub const MANAGED_PUBLICATION_OPERATION_SCHEMA_V1: &str =
    "froglet.managed-publication.operation.v1";
pub const MANAGED_PUBLICATION_RUNNER_CONTRACT_V1: &str = "froglet.managed-runner.v1";

const PLAN_HASH_DOMAIN_V1: &str = "froglet.managed-publication.plan-hash.v1";
const OPERATION_ID_DOMAIN_V1: &str = "froglet.managed-publication.operation-id.v1";
const CAPSULE_HASH_DOMAIN_V1: &str = "froglet.managed-publication.capsule-hash.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPublicationContentVisibilityV1 {
    PrivateRegistryRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPublicationPackageRequestV1 {
    pub schema_version: String,
    pub operation_id: String,
    pub bundle: ManagedPublicationBundleManifestV1,
    pub output_repository: String,
    pub base_runner_image: OciImageV1,
    pub release_bundle_digest: String,
}

impl ManagedPublicationPackageRequestV1 {
    pub fn validate(&self) -> Result<(), ManagedPublicationContractError> {
        require_schema(
            "package_request.schema_version",
            &self.schema_version,
            MANAGED_PUBLICATION_PACKAGE_REQUEST_SCHEMA_V1,
        )?;
        require_hash("package_request.operation_id", &self.operation_id)?;
        self.bundle.validate()?;
        require_oci_repository("package_request.output_repository", &self.output_repository)?;
        require_oci_repository(
            "package_request.base_runner_image.repository",
            &self.base_runner_image.repository,
        )?;
        require_digest(
            "package_request.base_runner_image.digest",
            &self.base_runner_image.digest,
        )?;
        require_digest(
            "package_request.release_bundle_digest",
            &self.release_bundle_digest,
        )?;
        Ok(())
    }

    pub fn validate_output(
        &self,
        package: &ManagedPublicationPackageV1,
    ) -> Result<(), ManagedPublicationContractError> {
        self.validate()?;
        package.validate()?;
        let build_evidence = self.bundle.intent.build_evidence.as_ref().ok_or_else(|| {
            ManagedPublicationContractError::InvalidField {
                field: "package_request.bundle.intent.build_evidence",
                reason: "managed bundle omitted build evidence".to_string(),
            }
        })?;
        let runtime = self.bundle.intent.runtime.as_deref().unwrap_or_default();
        let package_kind = self
            .bundle
            .intent
            .package_kind
            .as_deref()
            .unwrap_or_default();
        if package.source_package_digest != self.bundle.source_package_digest
            || package.build_evidence_digest != canonical_hash(build_evidence)?
            || package.bundle_manifest_digest != self.bundle.bundle_manifest_digest()?
            || package.release_bundle_digest != self.release_bundle_digest
            || package.base_runner_image != self.base_runner_image
            || package.image.repository != self.output_repository
            || package.runtime != runtime
            || package.package_kind != package_kind
            || package.content_visibility
                != ManagedPublicationContentVisibilityV1::PrivateRegistryRequired
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "package",
                reason: "packager output does not match its exact immutable request".to_string(),
            });
        }
        Ok(())
    }
}

/// Canonical package document placed in the managed image layer.
///
/// The exact provider-private verification fixture remains on the authoring
/// Froglet Node. Its full request digest is retained, while the runtime bundle
/// contains only the executable or data package needed by the managed Node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPublicationBundleManifestV1 {
    pub schema_version: String,
    pub service_id: String,
    pub publish_request_digest: String,
    pub source_package_digest: String,
    pub content_visibility: ManagedPublicationContentVisibilityV1,
    pub intent: PublicationIntent,
}

impl ManagedPublicationBundleManifestV1 {
    pub fn from_intent(
        intent: &PublicationIntent,
    ) -> Result<Self, ManagedPublicationContractError> {
        let normalized = intent.clone().normalized().map_err(|error| {
            ManagedPublicationContractError::InvalidField {
                field: "bundle.intent",
                reason: error.to_string(),
            }
        })?;
        if normalized.artifact_path.is_some() {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "bundle.intent.artifact_path",
                reason: "managed packages require portable inline or bundled bytes; a daemon-local path cannot be transferred to the managed runner".to_string(),
            });
        }
        let publish_request_digest = canonical_hash(&normalized)?;
        let build_evidence = normalized.build_evidence.as_ref().ok_or_else(|| {
            ManagedPublicationContractError::InvalidField {
                field: "bundle.intent.build_evidence",
                reason: "managed packages require immutable build evidence".to_string(),
            }
        })?;
        build_evidence.validate().map_err(|reason| {
            ManagedPublicationContractError::InvalidField {
                field: "bundle.intent.build_evidence",
                reason,
            }
        })?;

        let source_package_digest = build_evidence.artifact_digest.clone();
        let service_id = normalized.service_id.clone();
        let mut runtime_intent = normalized;
        runtime_intent.verification = None;
        runtime_intent.artifact_path = None;
        runtime_intent.publication_state = None;
        let manifest = Self {
            schema_version: MANAGED_PUBLICATION_BUNDLE_SCHEMA_V1.to_string(),
            service_id,
            publish_request_digest,
            source_package_digest,
            content_visibility: ManagedPublicationContentVisibilityV1::PrivateRegistryRequired,
            intent: runtime_intent,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ManagedPublicationContractError> {
        require_schema(
            "bundle.schema_version",
            &self.schema_version,
            MANAGED_PUBLICATION_BUNDLE_SCHEMA_V1,
        )?;
        require_identifier("bundle.service_id", &self.service_id)?;
        require_hash(
            "bundle.publish_request_digest",
            &self.publish_request_digest,
        )?;
        require_hash("bundle.source_package_digest", &self.source_package_digest)?;
        if self.intent.service_id != self.service_id
            || self.intent.verification.is_some()
            || self.intent.artifact_path.is_some()
            || self.intent.publication_state.is_some()
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "bundle.intent",
                reason: "runtime intent must match the service and omit authoring-only state"
                    .to_string(),
            });
        }
        self.intent.validate_authoring().map_err(|error| {
            ManagedPublicationContractError::InvalidField {
                field: "bundle.intent",
                reason: error.to_string(),
            }
        })?;
        let build_evidence = self.intent.build_evidence.as_ref().ok_or_else(|| {
            ManagedPublicationContractError::InvalidField {
                field: "bundle.intent.build_evidence",
                reason: "runtime intent omitted immutable build evidence".to_string(),
            }
        })?;
        build_evidence.validate().map_err(|reason| {
            ManagedPublicationContractError::InvalidField {
                field: "bundle.intent.build_evidence",
                reason,
            }
        })?;
        if build_evidence.artifact_digest != self.source_package_digest {
            return Err(ManagedPublicationContractError::HashMismatch {
                field: "bundle.source_package_digest",
            });
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ManagedPublicationContractError> {
        self.validate()?;
        canonical_json::to_vec(self).map_err(|_| ManagedPublicationContractError::Canonicalization)
    }

    pub fn bundle_manifest_digest(&self) -> Result<String, ManagedPublicationContractError> {
        self.canonical_bytes()
            .map(crypto::sha256_hex)
            .map(|hash| format!("sha256:{hash}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPublicationPackageV1 {
    pub schema_version: String,
    /// Digest of the package already bound by the Publication Revision.
    pub source_package_digest: String,
    /// Canonical digest of the Publication build evidence.
    pub build_evidence_digest: String,
    /// Digest of the deterministic package manifest placed in the image.
    pub bundle_manifest_digest: String,
    /// Release Bundle containing the generic managed Froglet runner.
    pub release_bundle_digest: String,
    pub base_runner_image: OciImageV1,
    pub image: OciImageV1,
    pub content_visibility: ManagedPublicationContentVisibilityV1,
    pub runtime: String,
    pub package_kind: String,
    pub builder_id: String,
    pub builder_fingerprint: String,
    pub runner_contract: String,
}

impl ManagedPublicationPackageV1 {
    pub fn validate(&self) -> Result<(), ManagedPublicationContractError> {
        require_schema(
            "package.schema_version",
            &self.schema_version,
            MANAGED_PUBLICATION_PACKAGE_SCHEMA_V1,
        )?;
        require_hash("package.source_package_digest", &self.source_package_digest)?;
        require_hash("package.build_evidence_digest", &self.build_evidence_digest)?;
        require_digest(
            "package.bundle_manifest_digest",
            &self.bundle_manifest_digest,
        )?;
        require_digest("package.release_bundle_digest", &self.release_bundle_digest)?;
        require_oci_repository(
            "package.base_runner_image.repository",
            &self.base_runner_image.repository,
        )?;
        require_digest(
            "package.base_runner_image.digest",
            &self.base_runner_image.digest,
        )?;
        require_oci_repository("package.image.repository", &self.image.repository)?;
        require_digest("package.image.digest", &self.image.digest)?;
        for (field, value) in [
            ("package.runtime", self.runtime.as_str()),
            ("package.package_kind", self.package_kind.as_str()),
            ("package.builder_id", self.builder_id.as_str()),
        ] {
            require_nonempty(field, value)?;
        }
        require_hash("package.builder_fingerprint", &self.builder_fingerprint)?;
        if self.runner_contract != MANAGED_PUBLICATION_RUNNER_CONTRACT_V1 {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "package.runner_contract",
                reason: "unsupported managed runner contract".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPublicationPlanPayloadV1 {
    pub schema_version: String,
    /// Stable idempotency key for one service/package/target/profile tuple.
    pub operation_id: String,
    pub service_id: String,
    pub target: String,
    pub profile: String,
    pub expected_provider_id: String,
    pub publish_request_digest: String,
    pub package: ManagedPublicationPackageV1,
    pub desired_state: ManagedDeploymentDesiredStateV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provision_plan: Option<ManagedDeploymentPlanV1>,
    pub deploy_plan: ManagedDeploymentPlanV1,
    /// Pre-approved rollback or destroy plan used only if activation or a
    /// downstream canary/registration step fails.
    pub compensation_plan: ManagedDeploymentPlanV1,
    pub public_url: String,
}

impl ManagedPublicationPlanPayloadV1 {
    pub fn validate(&self) -> Result<(), ManagedPublicationContractError> {
        require_schema(
            "plan.schema_version",
            &self.schema_version,
            MANAGED_PUBLICATION_PLAN_SCHEMA_V1,
        )?;
        require_hash("plan.operation_id", &self.operation_id)?;
        require_identifier("plan.service_id", &self.service_id)?;
        require_identifier("plan.target", &self.target)?;
        require_identifier("plan.profile", &self.profile)?;
        require_hash("plan.expected_provider_id", &self.expected_provider_id)?;
        require_hash("plan.publish_request_digest", &self.publish_request_digest)?;
        let expected_operation_id =
            managed_publication_operation_id(&ManagedPublicationOperationIdentityV1 {
                service_id: &self.service_id,
                provider_id: &self.expected_provider_id,
                publish_request_digest: &self.publish_request_digest,
                source_package_digest: &self.package.source_package_digest,
                base_runner_image: &self.package.base_runner_image,
                release_bundle_digest: &self.package.release_bundle_digest,
                output_repository: &self.package.image.repository,
                target: &self.target,
                profile: &self.profile,
            })?;
        if self.operation_id != expected_operation_id {
            return Err(ManagedPublicationContractError::HashMismatch {
                field: "plan.operation_id",
            });
        }
        require_public_https_url("plan.public_url", &self.public_url)?;
        self.package.validate()?;
        self.desired_state.validate().map_err(|violations| {
            ManagedPublicationContractError::InvalidField {
                field: "plan.desired_state",
                reason: format!("{} contract violation(s)", violations.len()),
            }
        })?;

        if self.desired_state.revision.image != self.package.image
            || self.desired_state.revision.release_bundle_digest
                != self.package.release_bundle_digest
            || self.desired_state.revision.revision_id != self.package.bundle_manifest_digest
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "plan.desired_state.revision",
                reason: "desired revision must exactly match the approved managed package"
                    .to_string(),
            });
        }

        if let Some(plan) = &self.provision_plan {
            validate_operator_plan(
                plan,
                &self.desired_state,
                &ManagedDeploymentApprovalScopeV1::Provision,
            )?;
        }
        validate_operator_plan(
            &self.deploy_plan,
            &self.desired_state,
            &ManagedDeploymentApprovalScopeV1::Deploy,
        )?;
        let compensation_scope = plan_approval_scope(&self.compensation_plan)?;
        if !matches!(
            compensation_scope,
            ManagedDeploymentApprovalScopeV1::Rollback { .. }
                | ManagedDeploymentApprovalScopeV1::Destroy { .. }
        ) {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "plan.compensation_plan",
                reason: "compensation must approve an exact rollback or destroy operation"
                    .to_string(),
            });
        }
        validate_operator_plan(
            &self.compensation_plan,
            &self.desired_state,
            compensation_scope,
        )?;

        let expected_adapter = self.deploy_plan.adapter.adapter_id.as_str();
        if self
            .provision_plan
            .iter()
            .chain(std::iter::once(&self.compensation_plan))
            .any(|plan| plan.adapter.adapter_id != expected_adapter)
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "plan.operator_plans",
                reason: "provision, deploy, and compensation must use one exact adapter"
                    .to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPublicationPlanV1 {
    pub plan_hash: String,
    pub payload: ManagedPublicationPlanPayloadV1,
}

impl ManagedPublicationPlanV1 {
    pub fn new(
        payload: ManagedPublicationPlanPayloadV1,
    ) -> Result<Self, ManagedPublicationContractError> {
        payload.validate()?;
        let plan_hash = domain_separated_hash(PLAN_HASH_DOMAIN_V1, &payload)?;
        Ok(Self { plan_hash, payload })
    }

    pub fn validate(&self) -> Result<(), ManagedPublicationContractError> {
        self.payload.validate()?;
        require_hash("plan_hash", &self.plan_hash)?;
        let expected = domain_separated_hash(PLAN_HASH_DOMAIN_V1, &self.payload)?;
        if expected != self.plan_hash {
            return Err(ManagedPublicationContractError::HashMismatch { field: "plan_hash" });
        }
        Ok(())
    }

    pub fn deploy_approval_hash(
        &self,
    ) -> Result<&PlanApprovalHashV1, ManagedPublicationContractError> {
        self.validate()?;
        plan_approval_hash(&self.payload.deploy_plan).ok_or_else(|| {
            ManagedPublicationContractError::InvalidField {
                field: "plan.deploy_plan",
                reason: "validated deploy plan omitted its approval hash".to_string(),
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPublicationCapsuleV1 {
    pub schema_version: String,
    pub operation_id: String,
    pub plan_hash: String,
    pub consent_hash: String,
    /// Self-contained approved plan required by a remote Froglet Node before
    /// it can import the exact signed revision.
    pub plan: ManagedPublicationPlanV1,
    /// Locally verified authoring revision from which the managed endpoint
    /// specific artifact chain was derived. The managed Revision can differ
    /// because its Offer binds a Descriptor containing the approved endpoint.
    pub source_revision_hash: String,
    pub revision: SignedPublicationRevision,
    /// Provider-private canary input transported separately from the runtime
    /// image. Its canonical input hash is signed by the Publication Revision.
    pub verification_fixture: VerificationFixture,
    pub descriptor: Value,
    pub offer: Value,
}

impl ManagedPublicationCapsuleV1 {
    pub fn validate(&self) -> Result<(), ManagedPublicationContractError> {
        self.validate_against(&self.plan)
    }

    pub fn validate_against(
        &self,
        plan: &ManagedPublicationPlanV1,
    ) -> Result<(), ManagedPublicationContractError> {
        plan.validate()?;
        if &self.plan != plan {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "capsule.plan",
                reason: "embedded plan differs from the expected approved plan".to_string(),
            });
        }
        require_schema(
            "capsule.schema_version",
            &self.schema_version,
            MANAGED_PUBLICATION_CAPSULE_SCHEMA_V1,
        )?;
        require_hash("capsule.consent_hash", &self.consent_hash)?;
        require_hash("capsule.source_revision_hash", &self.source_revision_hash)?;
        if self.operation_id != plan.payload.operation_id || self.plan_hash != plan.plan_hash {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "capsule.plan",
                reason: "capsule does not belong to the approved Managed Publication plan"
                    .to_string(),
            });
        }
        self.revision
            .verify()
            .map_err(|error| ManagedPublicationContractError::InvalidField {
                field: "capsule.revision",
                reason: error.to_string(),
            })?;
        let payload = &self.revision.payload;
        if payload.provider_id != plan.payload.expected_provider_id
            || payload.service_id != plan.payload.service_id
            || payload.package_digest != plan.payload.package.source_package_digest
            || payload.runtime != plan.payload.package.runtime
            || payload.package_kind != plan.payload.package.package_kind
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "capsule.revision",
                reason:
                    "revision does not match the approved Provider, service, package, or runtime"
                        .to_string(),
            });
        }
        let build_evidence = payload.build_evidence.as_ref().ok_or_else(|| {
            ManagedPublicationContractError::InvalidField {
                field: "capsule.revision.payload.build_evidence",
                reason: "managed publication requires current immutable build evidence".to_string(),
            }
        })?;
        if canonical_hash(build_evidence)? != plan.payload.package.build_evidence_digest {
            return Err(ManagedPublicationContractError::HashMismatch {
                field: "capsule.revision.payload.build_evidence",
            });
        }
        let fixture_input_hash = canonical_hash(&self.verification_fixture.input)?;
        if fixture_input_hash != payload.local_verification.input_hash {
            return Err(ManagedPublicationContractError::HashMismatch {
                field: "capsule.verification_fixture.input",
            });
        }

        let descriptor: SignedArtifact<DescriptorPayload> =
            serde_json::from_value(self.descriptor.clone()).map_err(|error| {
                ManagedPublicationContractError::InvalidField {
                    field: "capsule.descriptor",
                    reason: error.to_string(),
                }
            })?;
        let offer: SignedArtifact<OfferPayload> = serde_json::from_value(self.offer.clone())
            .map_err(|error| ManagedPublicationContractError::InvalidField {
                field: "capsule.offer",
                reason: error.to_string(),
            })?;
        verify_bound_artifacts(&descriptor, &offer, &self.revision, plan)?;
        if descriptor.payload.transport_endpoints.len() != 1
            || descriptor.payload.transport_endpoints[0].transport != "https"
            || descriptor.payload.transport_endpoints[0]
                .uri
                .trim_end_matches('/')
                != plan.payload.public_url.trim_end_matches('/')
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "capsule.descriptor.transport_endpoints",
                reason: "managed Descriptor must advertise only the exact approved HTTPS endpoint"
                    .to_string(),
            });
        }
        Ok(())
    }

    pub fn capsule_hash(&self) -> Result<String, ManagedPublicationContractError> {
        domain_separated_hash(CAPSULE_HASH_DOMAIN_V1, self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPublicationRemoteCanaryV1 {
    pub provider_id: String,
    pub revision_hash: String,
    pub public_url: String,
    pub observed_at_epoch_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPublicationRegistrationV1 {
    pub marketplace_url: String,
    pub status: String,
    pub provider_id: String,
    pub validated_offer_hash: String,
    pub validated_revision_hash: String,
    pub registered_at_epoch_seconds: i64,
}

impl ManagedPublicationRegistrationV1 {
    pub fn validate_against(
        &self,
        plan: &ManagedPublicationPlanV1,
        revision_hash: &str,
    ) -> Result<(), ManagedPublicationContractError> {
        require_public_https_url("registration.marketplace_url", &self.marketplace_url)?;
        require_hash("registration.provider_id", &self.provider_id)?;
        require_hash(
            "registration.validated_offer_hash",
            &self.validated_offer_hash,
        )?;
        require_hash(
            "registration.validated_revision_hash",
            &self.validated_revision_hash,
        )?;
        if !matches!(self.status.as_str(), "active" | "pending_review")
            || self.provider_id != plan.payload.expected_provider_id
            || self.validated_revision_hash != revision_hash
            || self.registered_at_epoch_seconds < 0
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "registration",
                reason:
                    "marketplace registration does not match the approved Provider and Revision"
                        .to_string(),
            });
        }
        Ok(())
    }
}

impl ManagedPublicationRemoteCanaryV1 {
    pub fn validate_against(
        &self,
        plan: &ManagedPublicationPlanV1,
        revision_hash: &str,
    ) -> Result<(), ManagedPublicationContractError> {
        require_hash("remote_canary.provider_id", &self.provider_id)?;
        require_hash("remote_canary.revision_hash", &self.revision_hash)?;
        require_public_https_url("remote_canary.public_url", &self.public_url)?;
        if self.observed_at_epoch_seconds < 0
            || self.provider_id != plan.payload.expected_provider_id
            || self.revision_hash != revision_hash
            || self.public_url.trim_end_matches('/')
                != plan.payload.public_url.trim_end_matches('/')
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "remote_canary",
                reason: "remote canary does not prove the exact approved Provider, revision, and endpoint"
                    .to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPublicationPhaseV1 {
    Planned,
    LocalRevisionReady,
    Provisioning,
    Deploying,
    Reconciling,
    Deployed,
    CanaryVerified,
    Registering,
    Active,
    Compensating,
    Compensated,
    ReconciliationRequired,
    Failed,
}

impl ManagedPublicationPhaseV1 {
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Planned => matches!(next, Self::LocalRevisionReady | Self::Failed),
            Self::LocalRevisionReady => matches!(
                next,
                Self::Provisioning | Self::Deploying | Self::Compensating | Self::Failed
            ),
            Self::Provisioning => matches!(
                next,
                Self::Deploying | Self::Compensating | Self::ReconciliationRequired | Self::Failed
            ),
            Self::Deploying | Self::Reconciling => matches!(
                next,
                Self::Reconciling
                    | Self::Deployed
                    | Self::Compensating
                    | Self::ReconciliationRequired
                    | Self::Failed
            ),
            Self::Deployed => matches!(
                next,
                Self::CanaryVerified
                    | Self::Compensating
                    | Self::ReconciliationRequired
                    | Self::Failed
            ),
            Self::CanaryVerified => {
                matches!(next, Self::Registering | Self::Compensating | Self::Failed)
            }
            Self::Registering => matches!(
                next,
                Self::Active | Self::Compensating | Self::ReconciliationRequired | Self::Failed
            ),
            Self::Compensating => matches!(
                next,
                Self::Compensated | Self::ReconciliationRequired | Self::Failed
            ),
            Self::ReconciliationRequired => {
                matches!(next, Self::Reconciling | Self::Compensating | Self::Failed)
            }
            Self::Active | Self::Compensated | Self::Failed => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPublicationOperationV1 {
    pub schema_version: String,
    pub operation_id: String,
    pub plan_hash: String,
    pub consent_hash: String,
    pub phase: ManagedPublicationPhaseV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provision_result: Option<ManagedDeploymentResultV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy_result: Option<ManagedDeploymentResultV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation_result: Option<ManagedDeploymentResultV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_canary: Option<ManagedPublicationRemoteCanaryV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration: Option<ManagedPublicationRegistrationV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub attempt_count: u32,
    pub updated_at_epoch_seconds: i64,
}

impl ManagedPublicationOperationV1 {
    pub fn transition(
        &mut self,
        next: ManagedPublicationPhaseV1,
        updated_at_epoch_seconds: i64,
    ) -> Result<(), ManagedPublicationContractError> {
        if updated_at_epoch_seconds < self.updated_at_epoch_seconds {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "operation.updated_at_epoch_seconds",
                reason: "operation timestamps must be monotonic".to_string(),
            });
        }
        if !self.phase.can_transition_to(next) {
            return Err(ManagedPublicationContractError::InvalidTransition {
                from: self.phase,
                to: next,
            });
        }
        self.phase = next;
        self.updated_at_epoch_seconds = updated_at_epoch_seconds;
        Ok(())
    }

    pub fn validate_against(
        &self,
        plan: &ManagedPublicationPlanV1,
    ) -> Result<(), ManagedPublicationContractError> {
        plan.validate()?;
        require_schema(
            "operation.schema_version",
            &self.schema_version,
            MANAGED_PUBLICATION_OPERATION_SCHEMA_V1,
        )?;
        require_hash("operation.consent_hash", &self.consent_hash)?;
        if self.operation_id != plan.payload.operation_id || self.plan_hash != plan.plan_hash {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "operation.plan",
                reason: "operation does not belong to the approved plan".to_string(),
            });
        }
        if self.updated_at_epoch_seconds < 0 {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "operation.updated_at_epoch_seconds",
                reason: "timestamp must be non-negative".to_string(),
            });
        }
        if let Some(revision_hash) = &self.revision_hash {
            require_hash("operation.revision_hash", revision_hash)?;
        }
        if matches!(
            self.phase,
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
        ) && self.revision_hash.is_none()
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "operation.revision_hash",
                reason: "phase requires the exact managed Publication Revision".to_string(),
            });
        }
        if let Some(result) = &self.deploy_result {
            validate_deploy_result(result, plan)?;
        }
        if let Some(result) = &self.provision_result {
            validate_result_for_scope(result, plan, &ManagedDeploymentApprovalScopeV1::Provision)?;
        }
        if plan.payload.provision_plan.is_none() && self.provision_result.is_some() {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "operation.provision_result",
                reason: "operation carries provision evidence without an approved provision plan"
                    .to_string(),
            });
        }
        if plan.payload.provision_plan.is_some()
            && matches!(
                self.phase,
                ManagedPublicationPhaseV1::Deploying
                    | ManagedPublicationPhaseV1::Reconciling
                    | ManagedPublicationPhaseV1::Deployed
                    | ManagedPublicationPhaseV1::CanaryVerified
                    | ManagedPublicationPhaseV1::Registering
                    | ManagedPublicationPhaseV1::Active
            )
            && self.provision_result.is_none()
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "operation.provision_result",
                reason: "phase requires normalized successful provision evidence".to_string(),
            });
        }
        if matches!(
            self.phase,
            ManagedPublicationPhaseV1::Deployed
                | ManagedPublicationPhaseV1::CanaryVerified
                | ManagedPublicationPhaseV1::Registering
                | ManagedPublicationPhaseV1::Active
        ) && self.deploy_result.is_none()
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "operation.deploy_result",
                reason: "phase requires normalized deployment evidence".to_string(),
            });
        }
        if matches!(
            self.phase,
            ManagedPublicationPhaseV1::CanaryVerified
                | ManagedPublicationPhaseV1::Registering
                | ManagedPublicationPhaseV1::Active
        ) {
            let revision_hash = self.revision_hash.as_deref().ok_or_else(|| {
                ManagedPublicationContractError::InvalidField {
                    field: "operation.revision_hash",
                    reason: "remote canary requires an exact revision".to_string(),
                }
            })?;
            self.remote_canary
                .as_ref()
                .ok_or_else(|| ManagedPublicationContractError::InvalidField {
                    field: "operation.remote_canary",
                    reason: "phase requires exact requester-side canary evidence".to_string(),
                })?
                .validate_against(plan, revision_hash)?;
        }
        if self.phase == ManagedPublicationPhaseV1::Compensated
            && self.compensation_result.is_none()
        {
            return Err(ManagedPublicationContractError::InvalidField {
                field: "operation.compensation_result",
                reason: "compensated phase requires normalized compensation evidence".to_string(),
            });
        }
        if let Some(result) = &self.compensation_result {
            validate_result_for_scope(
                result,
                plan,
                plan_approval_scope(&plan.payload.compensation_plan)?,
            )?;
        }
        if self.phase == ManagedPublicationPhaseV1::Active {
            let revision_hash = self.revision_hash.as_deref().ok_or_else(|| {
                ManagedPublicationContractError::InvalidField {
                    field: "operation.revision_hash",
                    reason: "active registration requires an exact revision".to_string(),
                }
            })?;
            self.registration
                .as_ref()
                .ok_or_else(|| ManagedPublicationContractError::InvalidField {
                    field: "operation.registration",
                    reason: "active phase requires normalized marketplace registration evidence"
                        .to_string(),
                })?
                .validate_against(plan, revision_hash)?;
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManagedPublicationContractError {
    #[error("invalid {field}: {reason}")]
    InvalidField { field: &'static str, reason: String },
    #[error("{field} does not match its canonical digest")]
    HashMismatch { field: &'static str },
    #[error("managed publication cannot transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: ManagedPublicationPhaseV1,
        to: ManagedPublicationPhaseV1,
    },
    #[error("managed publication canonicalization failed")]
    Canonicalization,
}

#[derive(Serialize)]
struct OperationIdentityMaterialV1<'a> {
    schema_version: &'static str,
    service_id: &'a str,
    provider_id: &'a str,
    publish_request_digest: &'a str,
    source_package_digest: &'a str,
    base_runner_image: &'a OciImageV1,
    release_bundle_digest: &'a str,
    output_repository: &'a str,
    target: &'a str,
    profile: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct ManagedPublicationOperationIdentityV1<'a> {
    pub service_id: &'a str,
    pub provider_id: &'a str,
    pub publish_request_digest: &'a str,
    pub source_package_digest: &'a str,
    pub base_runner_image: &'a OciImageV1,
    pub release_bundle_digest: &'a str,
    pub output_repository: &'a str,
    pub target: &'a str,
    pub profile: &'a str,
}

pub fn managed_publication_operation_id(
    identity: &ManagedPublicationOperationIdentityV1<'_>,
) -> Result<String, ManagedPublicationContractError> {
    let ManagedPublicationOperationIdentityV1 {
        service_id,
        provider_id,
        publish_request_digest,
        source_package_digest,
        base_runner_image,
        release_bundle_digest,
        output_repository,
        target,
        profile,
    } = *identity;
    require_identifier("operation_id.service_id", service_id)?;
    require_hash("operation_id.provider_id", provider_id)?;
    require_hash(
        "operation_id.publish_request_digest",
        publish_request_digest,
    )?;
    require_hash("operation_id.source_package_digest", source_package_digest)?;
    require_oci_repository(
        "operation_id.base_runner_image.repository",
        &base_runner_image.repository,
    )?;
    require_digest(
        "operation_id.base_runner_image.digest",
        &base_runner_image.digest,
    )?;
    require_digest("operation_id.release_bundle_digest", release_bundle_digest)?;
    require_oci_repository("operation_id.output_repository", output_repository)?;
    require_identifier("operation_id.target", target)?;
    require_identifier("operation_id.profile", profile)?;
    domain_separated_hash(
        OPERATION_ID_DOMAIN_V1,
        &OperationIdentityMaterialV1 {
            schema_version: MANAGED_PUBLICATION_OPERATION_SCHEMA_V1,
            service_id,
            provider_id,
            publish_request_digest,
            source_package_digest,
            base_runner_image,
            release_bundle_digest,
            output_repository,
            target,
            profile,
        },
    )
}

fn verify_bound_artifacts(
    descriptor: &SignedArtifact<DescriptorPayload>,
    offer: &SignedArtifact<OfferPayload>,
    revision: &SignedPublicationRevision,
    plan: &ManagedPublicationPlanV1,
) -> Result<(), ManagedPublicationContractError> {
    if !verify_artifact(descriptor) {
        return Err(invalid_artifact(
            "capsule.descriptor",
            "signature verification failed",
        ));
    }
    validate_descriptor_artifact(descriptor)
        .map_err(|reason| invalid_artifact("capsule.descriptor", reason))?;
    if !verify_artifact(offer) {
        return Err(invalid_artifact(
            "capsule.offer",
            "signature verification failed",
        ));
    }
    validate_offer_artifact(offer).map_err(|reason| invalid_artifact("capsule.offer", reason))?;

    let descriptor_hash = artifact_hash(descriptor)
        .map_err(|reason| invalid_artifact("capsule.descriptor", reason))?;
    let offer_hash =
        artifact_hash(offer).map_err(|reason| invalid_artifact("capsule.offer", reason))?;
    if descriptor_hash != descriptor.hash
        || offer_hash != offer.hash
        || offer_hash != revision.payload.offer_hash
        || offer.payload.descriptor_hash != descriptor_hash
        || descriptor.payload.provider_id != plan.payload.expected_provider_id
        || offer.payload.provider_id != plan.payload.expected_provider_id
        || offer.payload.offer_id != revision.payload.offer_id
        || offer.payload.execution_profile.runtime.as_str() != revision.payload.runtime
        || offer.payload.execution_profile.package_kind != revision.payload.package_kind
    {
        return Err(ManagedPublicationContractError::InvalidField {
            field: "capsule.artifact_chain",
            reason:
                "Descriptor, Offer, and Publication Revision are not one exact Provider-bound chain"
                    .to_string(),
        });
    }
    Ok(())
}

/// Validate one adapter-produced, mutation-free plan against the exact
/// provider-neutral desired state and intended approval scope.
pub fn validate_operator_plan(
    plan: &ManagedDeploymentPlanV1,
    desired: &ManagedDeploymentDesiredStateV1,
    expected_scope: &ManagedDeploymentApprovalScopeV1,
) -> Result<(), ManagedPublicationContractError> {
    if plan.schema_version != PLAN_SCHEMA_V1
        || plan.deployment_id != desired.deployment_id
        || plan.revision_id != desired.revision.revision_id
        || plan.desired_state_schema_version != desired.schema_version
        || plan.mutation_performed.as_bool()
    {
        return Err(ManagedPublicationContractError::InvalidField {
            field: "plan.operator_plan",
            reason: "operator plan does not match desired state or claims a planning mutation"
                .to_string(),
        });
    }
    plan.adapter.validate().map_err(|violations| {
        ManagedPublicationContractError::InvalidField {
            field: "plan.operator_plan.adapter",
            reason: format!("{} contract violation(s)", violations.len()),
        }
    })?;
    let required = desired.required_capabilities_for_plan(
        &ManagedDeploymentPlanContextV1::Mutation(expected_scope.clone()),
    );
    if !required.is_subset(&plan.adapter.capabilities) {
        return Err(ManagedPublicationContractError::InvalidField {
            field: "plan.operator_plan.adapter.capabilities",
            reason: "operator plan does not declare every capability required by desired state"
                .to_string(),
        });
    }
    match &plan.outcome {
        ManagedDeploymentPlanOutcomeV1::Applicable {
            steps,
            approval_hash: Some(_),
            approval_scope: Some(scope),
            ..
        } if !steps.is_empty() && scope == expected_scope => Ok(()),
        _ => Err(ManagedPublicationContractError::InvalidField {
            field: "plan.operator_plan",
            reason:
                "operator plan must be applicable, operation-scoped, non-empty, and approval-bound"
                    .to_string(),
        }),
    }
}

fn plan_approval_scope(
    plan: &ManagedDeploymentPlanV1,
) -> Result<&ManagedDeploymentApprovalScopeV1, ManagedPublicationContractError> {
    match &plan.outcome {
        ManagedDeploymentPlanOutcomeV1::Applicable {
            approval_scope: Some(scope),
            approval_hash: Some(_),
            ..
        } => Ok(scope),
        _ => Err(ManagedPublicationContractError::InvalidField {
            field: "plan.operator_plan",
            reason: "operator plan has no exact approval scope".to_string(),
        }),
    }
}

fn plan_approval_hash(plan: &ManagedDeploymentPlanV1) -> Option<&PlanApprovalHashV1> {
    match &plan.outcome {
        ManagedDeploymentPlanOutcomeV1::Applicable {
            approval_hash: Some(hash),
            ..
        } => Some(hash),
        _ => None,
    }
}

/// Validate normalized deployment evidence against the exact approved Managed
/// Publication plan.
pub fn validate_deploy_result(
    result: &ManagedDeploymentResultV1,
    plan: &ManagedPublicationPlanV1,
) -> Result<(), ManagedPublicationContractError> {
    let desired = &plan.payload.desired_state;
    if result.schema_version != crate::managed_deployment::RESULT_SCHEMA_V1
        || !matches!(
            result.operation,
            crate::managed_deployment::ManagedDeploymentOperationV1::Deploy
                | crate::managed_deployment::ManagedDeploymentOperationV1::Status
        )
        || result.failure.is_some()
        || result.deployment_id != desired.deployment_id
        || result.revision_id != desired.revision.revision_id
        || result.adapter_id != plan.payload.deploy_plan.adapter.adapter_id
        || result.status != ManagedDeploymentOperationStatusV1::Succeeded
        || result.health.status != DeploymentHealthStatusV1::Healthy
        || result.image_digests.as_slice() != [desired.revision.image.clone()]
        || !result.endpoints.iter().any(|endpoint| {
            endpoint.url.trim_end_matches('/') == plan.payload.public_url.trim_end_matches('/')
        })
    {
        return Err(ManagedPublicationContractError::InvalidField {
            field: "operation.deploy_result",
            reason:
                "deployment result does not prove the exact healthy image and approved endpoint"
                    .to_string(),
        });
    }
    Ok(())
}

/// Validate normalized provision, rollback, or destroy evidence against the
/// same desired state and adapter selected by an approved plan.
pub fn validate_result_for_scope(
    result: &ManagedDeploymentResultV1,
    plan: &ManagedPublicationPlanV1,
    scope: &ManagedDeploymentApprovalScopeV1,
) -> Result<(), ManagedPublicationContractError> {
    use crate::managed_deployment::ManagedDeploymentOperationV1;

    let expected_operation = match scope {
        ManagedDeploymentApprovalScopeV1::Provision => ManagedDeploymentOperationV1::Provision,
        ManagedDeploymentApprovalScopeV1::Deploy => ManagedDeploymentOperationV1::Deploy,
        ManagedDeploymentApprovalScopeV1::Rollback { .. } => ManagedDeploymentOperationV1::Rollback,
        ManagedDeploymentApprovalScopeV1::Destroy { .. } => ManagedDeploymentOperationV1::Destroy,
    };
    let expected_adapter = match scope {
        ManagedDeploymentApprovalScopeV1::Provision => plan
            .payload
            .provision_plan
            .as_ref()
            .map(|plan| plan.adapter.adapter_id.as_str()),
        ManagedDeploymentApprovalScopeV1::Deploy => {
            Some(plan.payload.deploy_plan.adapter.adapter_id.as_str())
        }
        ManagedDeploymentApprovalScopeV1::Rollback { .. }
        | ManagedDeploymentApprovalScopeV1::Destroy { .. } => {
            Some(plan.payload.compensation_plan.adapter.adapter_id.as_str())
        }
    };
    if result.schema_version != crate::managed_deployment::RESULT_SCHEMA_V1
        || result.operation != expected_operation
        || result.deployment_id != plan.payload.desired_state.deployment_id
        || result.adapter_id != expected_adapter.unwrap_or_default()
        || result.status != ManagedDeploymentOperationStatusV1::Succeeded
        || result.failure.is_some()
    {
        return Err(ManagedPublicationContractError::InvalidField {
            field: "operation.operator_result",
            reason: "operator result does not prove the exact approved successful operation"
                .to_string(),
        });
    }
    Ok(())
}

fn canonical_hash<T: Serialize>(value: &T) -> Result<String, ManagedPublicationContractError> {
    canonical_json::to_vec(value)
        .map(crypto::sha256_hex)
        .map_err(|_| ManagedPublicationContractError::Canonicalization)
}

fn domain_separated_hash<T: Serialize>(
    domain: &str,
    value: &T,
) -> Result<String, ManagedPublicationContractError> {
    let canonical = canonical_json::to_vec(value)
        .map_err(|_| ManagedPublicationContractError::Canonicalization)?;
    let mut material = Vec::with_capacity(domain.len() + 1 + canonical.len());
    material.extend_from_slice(domain.as_bytes());
    material.push(b'\n');
    material.extend_from_slice(&canonical);
    Ok(crypto::sha256_hex(material))
}

fn invalid_artifact(
    field: &'static str,
    reason: impl Into<String>,
) -> ManagedPublicationContractError {
    ManagedPublicationContractError::InvalidField {
        field,
        reason: reason.into(),
    }
}

fn require_schema(
    field: &'static str,
    actual: &str,
    expected: &str,
) -> Result<(), ManagedPublicationContractError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ManagedPublicationContractError::InvalidField {
            field,
            reason: format!("expected {expected}"),
        })
    }
}

fn require_nonempty(
    field: &'static str,
    value: &str,
) -> Result<(), ManagedPublicationContractError> {
    if !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(ManagedPublicationContractError::InvalidField {
            field,
            reason: "must be non-empty, bounded text without control characters".to_string(),
        })
    }
}

fn require_identifier(
    field: &'static str,
    value: &str,
) -> Result<(), ManagedPublicationContractError> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Ok(())
    } else {
        Err(ManagedPublicationContractError::InvalidField {
            field,
            reason: "must contain 1-128 ASCII letters, digits, dots, hyphens, or underscores"
                .to_string(),
        })
    }
}

fn require_hash(field: &'static str, value: &str) -> Result<(), ManagedPublicationContractError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(ManagedPublicationContractError::InvalidField {
            field,
            reason: "must be exactly 64 lowercase hexadecimal characters".to_string(),
        })
    }
}

fn require_digest(field: &'static str, value: &str) -> Result<(), ManagedPublicationContractError> {
    value.strip_prefix("sha256:").map_or_else(
        || {
            Err(ManagedPublicationContractError::InvalidField {
                field,
                reason: "must use sha256:<64 lowercase hexadecimal characters>".to_string(),
            })
        },
        |hash| require_hash(field, hash),
    )
}

fn require_oci_repository(
    field: &'static str,
    value: &str,
) -> Result<(), ManagedPublicationContractError> {
    let valid = value.len() <= 512
        && !value.contains('@')
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
        && value.split_once('/').is_some_and(|(registry, repository)| {
            !registry.is_empty()
                && !repository.is_empty()
                && repository.split('/').all(|segment| {
                    !segment.is_empty()
                        && segment != "."
                        && segment != ".."
                        && segment.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                        })
                })
                && url::Url::parse(&format!("https://{registry}/")).is_ok_and(|url| {
                    url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.path() == "/"
                        && url.query().is_none()
                        && url.fragment().is_none()
                })
        });
    if valid {
        Ok(())
    } else {
        Err(ManagedPublicationContractError::InvalidField {
            field,
            reason: "must include an explicit credential-free registry and repository path without a tag or digest"
                .to_string(),
        })
    }
}

fn require_public_https_url(
    field: &'static str,
    value: &str,
) -> Result<(), ManagedPublicationContractError> {
    let valid = value.len() <= 2048
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
        && url::Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
        });
    valid
        .then_some(())
        .ok_or_else(|| ManagedPublicationContractError::InvalidField {
            field,
            reason: "must be a bounded credential-free HTTPS root origin".to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_deployment::{
        AdapterCapabilitiesV1, AdapterCapabilityV1, ApplicableDeploymentPlanV1,
        DESIRED_STATE_SCHEMA_V1, DeploymentEndpointV1, DeploymentHealthStatusV1,
        DeploymentHealthV1, DeploymentRevisionV1, DestroyConfirmationV1, EndpointTransportV1,
        HttpHealthCheckV1, IngressIntentV1, IngressTransportV1, LifecycleIntentV1,
        ManagedDeploymentApprovalScopeV1, ManagedDeploymentOperationStatusV1,
        ManagedDeploymentOperationV1, ManagedDeploymentResultV1, ObservabilityIntentV1,
        PlanActionV1, PlanStepV1, PortIntentV1, PortProtocolV1, RecurringCostDisclosureV1,
        ResourceIntentV1, WorkloadArchitectureV1,
    };
    use crate::protocol::{
        ARTIFACT_KIND_DESCRIPTOR, ARTIFACT_KIND_OFFER, DescriptorCapabilities, DescriptorPayload,
        OfferExecutionProfile, OfferPayload, OfferPriceSchedule, sign_artifact,
    };
    use crate::publication::{
        LocalVerificationEvidence, PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1,
        PUBLICATION_INTENT_SCHEMA_V1, PUBLICATION_REVISION_SCHEMA_V1, PublicationBuildEvidence,
        PublicationCurrency, PublicationRevisionPayload, PublicationRevisionPrice,
        PublicationRevisionService, PublicationSettlement, ResolvedPublicationLimits,
        VerificationFixture, sign_publication_revision,
    };

    fn hex(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn digest(byte: char) -> String {
        format!("sha256:{}", hex(byte))
    }

    fn desired_state() -> ManagedDeploymentDesiredStateV1 {
        ManagedDeploymentDesiredStateV1 {
            schema_version: DESIRED_STATE_SCHEMA_V1.to_string(),
            deployment_id: "analytics-managed".to_string(),
            revision: DeploymentRevisionV1 {
                revision_id: digest('a'),
                release_bundle_digest: digest('b'),
                image: OciImageV1 {
                    repository: "ghcr.io/example/froglet-analytics".to_string(),
                    digest: digest('c'),
                },
            },
            environment: Default::default(),
            secrets: Vec::new(),
            resources: ResourceIntentV1 {
                cpu_millis: 500,
                memory_bytes: 512 * 1024 * 1024,
                architecture: WorkloadArchitectureV1::Amd64,
            },
            ports: vec![PortIntentV1 {
                name: "froglet".to_string(),
                container_port: 8080,
                protocol: PortProtocolV1::Tcp,
            }],
            persistent_volumes: Vec::new(),
            health_check: HttpHealthCheckV1 {
                port_name: "froglet".to_string(),
                path: "/healthz".to_string(),
                interval_seconds: 30,
                timeout_seconds: 5,
            },
            ingress: Some(IngressIntentV1 {
                port_name: "froglet".to_string(),
                transport: IngressTransportV1::Https,
                requested_hostname: Some("analytics.example.test".to_string()),
            }),
            observability: ObservabilityIntentV1 {
                structured_logs: true,
            },
            lifecycle: LifecycleIntentV1 {
                rollback_required: true,
            },
        }
    }

    fn capabilities() -> AdapterCapabilitiesV1 {
        AdapterCapabilitiesV1::new(
            "ssh-oci",
            [
                AdapterCapabilityV1::ImmutableOciImage,
                AdapterCapabilityV1::ResourceIntent,
                AdapterCapabilityV1::ContainerPorts,
                AdapterCapabilityV1::HttpHealthCheck,
                AdapterCapabilityV1::PublicHttpsIngress,
                AdapterCapabilityV1::StructuredLogs,
                AdapterCapabilityV1::RevisionRollback,
                AdapterCapabilityV1::DeploymentDestroy,
            ],
        )
    }

    fn operator_plan(
        desired: &ManagedDeploymentDesiredStateV1,
        scope: ManagedDeploymentApprovalScopeV1,
        approval_byte: u8,
    ) -> ManagedDeploymentPlanV1 {
        ManagedDeploymentPlanV1::applicable(
            desired,
            capabilities(),
            ApplicableDeploymentPlanV1 {
                steps: vec![PlanStepV1 {
                    sequence: 1,
                    action: match &scope {
                        ManagedDeploymentApprovalScopeV1::Provision => PlanActionV1::AcquireCompute,
                        ManagedDeploymentApprovalScopeV1::Deploy => PlanActionV1::DeployRevision,
                        ManagedDeploymentApprovalScopeV1::Rollback { .. } => {
                            PlanActionV1::DeployRevision
                        }
                        ManagedDeploymentApprovalScopeV1::Destroy { .. } => {
                            PlanActionV1::DestroyDeployment
                        }
                    },
                    summary: "exact operation".to_string(),
                }],
                warnings: Vec::new(),
                recurring_cost: RecurringCostDisclosureV1::default(),
            },
            Some((scope, PlanApprovalHashV1::from_bytes([approval_byte; 32]))),
        )
    }

    fn valid_payload() -> ManagedPublicationPlanPayloadV1 {
        let desired = desired_state();
        let base_runner_image = OciImageV1 {
            repository: "ghcr.io/example/froglet-runner".to_string(),
            digest: digest('b'),
        };
        let operation_id =
            managed_publication_operation_id(&ManagedPublicationOperationIdentityV1 {
                service_id: "analytics",
                provider_id: &hex('1'),
                publish_request_digest: &hex('2'),
                source_package_digest: &hex('d'),
                base_runner_image: &base_runner_image,
                release_bundle_digest: &desired.revision.release_bundle_digest,
                output_repository: &desired.revision.image.repository,
                target: "regional-container",
                profile: "small-public",
            })
            .unwrap();
        ManagedPublicationPlanPayloadV1 {
            schema_version: MANAGED_PUBLICATION_PLAN_SCHEMA_V1.to_string(),
            operation_id,
            service_id: "analytics".to_string(),
            target: "regional-container".to_string(),
            profile: "small-public".to_string(),
            expected_provider_id: hex('1'),
            publish_request_digest: hex('2'),
            package: ManagedPublicationPackageV1 {
                schema_version: MANAGED_PUBLICATION_PACKAGE_SCHEMA_V1.to_string(),
                source_package_digest: hex('d'),
                build_evidence_digest: hex('e'),
                bundle_manifest_digest: desired.revision.revision_id.clone(),
                release_bundle_digest: desired.revision.release_bundle_digest.clone(),
                base_runner_image,
                image: desired.revision.image.clone(),
                content_visibility: ManagedPublicationContentVisibilityV1::PrivateRegistryRequired,
                runtime: "wasm".to_string(),
                package_kind: "inline_module".to_string(),
                builder_id: "froglet-managed-oci".to_string(),
                builder_fingerprint: hex('f'),
                runner_contract: MANAGED_PUBLICATION_RUNNER_CONTRACT_V1.to_string(),
            },
            provision_plan: None,
            deploy_plan: operator_plan(&desired, ManagedDeploymentApprovalScopeV1::Deploy, 3),
            compensation_plan: operator_plan(
                &desired,
                ManagedDeploymentApprovalScopeV1::Destroy {
                    confirmation: DestroyConfirmationV1 {
                        deployment_id: desired.deployment_id.clone(),
                        adapter_id: "ssh-oci".to_string(),
                    },
                },
                4,
            ),
            desired_state: desired,
            public_url: "https://analytics.example.test".to_string(),
        }
    }

    fn wasm_intent() -> PublicationIntent {
        PublicationIntent {
            schema_version: Some(PUBLICATION_INTENT_SCHEMA_V1.to_string()),
            service_id: "analytics".to_string(),
            wasm_module_hex: Some("0061736d01000000".to_string()),
            runtime: Some("wasm".to_string()),
            package_kind: Some("inline_module".to_string()),
            build_evidence: Some(PublicationBuildEvidence {
                schema_version: PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1.to_string(),
                builder: "wat".to_string(),
                builder_version: "1.245.1".to_string(),
                source_digest: hex('c'),
                artifact_digest: hex('d'),
                dependency_mode: "none".to_string(),
                components: Vec::new(),
                hermetic: true,
            }),
            verification: Some(VerificationFixture {
                input: serde_json::json!({"private": "fixture"}),
                expected_output: Some(serde_json::json!({"ok": true})),
            }),
            artifact_path: None,
            publication_state: Some("verified".to_string()),
            ..PublicationIntent::default()
        }
    }

    #[test]
    fn runtime_bundle_is_deterministic_private_and_strips_authoring_only_fields() {
        let intent = wasm_intent();
        let first = ManagedPublicationBundleManifestV1::from_intent(&intent).unwrap();
        let second = ManagedPublicationBundleManifestV1::from_intent(&intent).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            first.content_visibility,
            ManagedPublicationContentVisibilityV1::PrivateRegistryRequired
        );
        assert!(first.intent.verification.is_none());
        assert!(first.intent.artifact_path.is_none());
        assert!(first.intent.publication_state.is_none());
        assert_eq!(first.source_package_digest, hex('d'));
        assert_eq!(
            first.bundle_manifest_digest().unwrap(),
            second.bundle_manifest_digest().unwrap()
        );
        assert!(!first.canonical_bytes().unwrap().is_empty());
    }

    #[test]
    fn runtime_bundle_rejects_daemon_local_artifact_paths() {
        let mut intent = wasm_intent();
        intent.artifact_path = Some("/private/authoring/path.wasm".to_string());
        assert!(matches!(
            ManagedPublicationBundleManifestV1::from_intent(&intent),
            Err(ManagedPublicationContractError::InvalidField {
                field: "bundle.intent.artifact_path",
                ..
            })
        ));
    }

    #[test]
    fn runtime_bundle_rejects_package_digest_drift() {
        let mut bundle = ManagedPublicationBundleManifestV1::from_intent(&wasm_intent()).unwrap();
        bundle.source_package_digest = hex('e');
        assert!(matches!(
            bundle.validate(),
            Err(ManagedPublicationContractError::HashMismatch {
                field: "bundle.source_package_digest"
            })
        ));
    }

    #[test]
    fn package_request_exactly_validates_deterministic_packager_output() {
        let bundle = ManagedPublicationBundleManifestV1::from_intent(&wasm_intent()).unwrap();
        let base_runner_image = OciImageV1 {
            repository: "ghcr.io/example/froglet-runner".to_string(),
            digest: digest('7'),
        };
        let request = ManagedPublicationPackageRequestV1 {
            schema_version: MANAGED_PUBLICATION_PACKAGE_REQUEST_SCHEMA_V1.to_string(),
            operation_id: managed_publication_operation_id(
                &ManagedPublicationOperationIdentityV1 {
                    service_id: "analytics",
                    provider_id: &hex('1'),
                    publish_request_digest: &bundle.publish_request_digest,
                    source_package_digest: &hex('d'),
                    base_runner_image: &base_runner_image,
                    release_bundle_digest: &digest('8'),
                    output_repository: "registry.example.test/private/analytics",
                    target: "regional-container",
                    profile: "small-public",
                },
            )
            .unwrap(),
            bundle: bundle.clone(),
            output_repository: "registry.example.test/private/analytics".to_string(),
            base_runner_image: base_runner_image.clone(),
            release_bundle_digest: digest('8'),
        };
        let build_evidence_digest =
            canonical_hash(bundle.intent.build_evidence.as_ref().unwrap()).unwrap();
        let output = ManagedPublicationPackageV1 {
            schema_version: MANAGED_PUBLICATION_PACKAGE_SCHEMA_V1.to_string(),
            source_package_digest: bundle.source_package_digest.clone(),
            build_evidence_digest,
            bundle_manifest_digest: bundle.bundle_manifest_digest().unwrap(),
            release_bundle_digest: request.release_bundle_digest.clone(),
            base_runner_image,
            image: OciImageV1 {
                repository: request.output_repository.clone(),
                digest: digest('9'),
            },
            content_visibility: ManagedPublicationContentVisibilityV1::PrivateRegistryRequired,
            runtime: "wasm".to_string(),
            package_kind: "inline_module".to_string(),
            builder_id: "froglet-oci-layout".to_string(),
            builder_fingerprint: hex('a'),
            runner_contract: MANAGED_PUBLICATION_RUNNER_CONTRACT_V1.to_string(),
        };
        request.validate_output(&output).unwrap();

        let mut drifted = output;
        drifted.image.repository = "registry.example.test/private/other".to_string();
        assert!(request.validate_output(&drifted).is_err());
    }

    #[test]
    fn capsule_verifies_one_exact_provider_bound_artifact_chain() {
        let signing_key = crate::crypto::generate_signing_key();
        let provider_id = crate::crypto::public_key_hex(&signing_key);
        let sign = |message: &[u8]| crate::crypto::sign_message_hex(&signing_key, message);
        let descriptor = sign_artifact(
            &provider_id,
            sign,
            ARTIFACT_KIND_DESCRIPTOR,
            10,
            DescriptorPayload {
                provider_id: provider_id.clone(),
                descriptor_seq: 1,
                protocol_version: crate::protocol::FROGLET_SCHEMA_V1.to_string(),
                expires_at: None,
                linked_identities: Vec::new(),
                transport_endpoints: vec![crate::protocol::TransportEndpoint {
                    transport: "https".to_string(),
                    uri: "https://analytics.example.test".to_string(),
                    created_at: None,
                    expires_at: None,
                    priority: 10,
                    features: Vec::new(),
                }],
                capabilities: DescriptorCapabilities {
                    service_kinds: vec!["compute.execution.v1".to_string()],
                    execution_runtimes: vec!["wasm".to_string()],
                    max_concurrent_deals: Some(1),
                },
                accepted_payment_methods: vec!["none".to_string()],
            },
        )
        .unwrap();
        let offer = sign_artifact(
            &provider_id,
            sign,
            ARTIFACT_KIND_OFFER,
            11,
            OfferPayload {
                provider_id: provider_id.clone(),
                offer_id: "analytics-v1".to_string(),
                descriptor_hash: descriptor.hash.clone(),
                expires_at: None,
                offer_kind: "compute.execution.v1".to_string(),
                settlement_method: "none".to_string(),
                quote_ttl_secs: 60,
                execution_profile: OfferExecutionProfile {
                    runtime: crate::ExecutionRuntime::Wasm,
                    package_kind: "inline_module".to_string(),
                    contract_version: "froglet.wasm.run_json.v1".to_string(),
                    access_handles: Vec::new(),
                    abi_version: "froglet.wasm.v1".to_string(),
                    capabilities: Vec::new(),
                    max_input_bytes: 4096,
                    max_runtime_ms: 2500,
                    max_memory_bytes: 8 * 1024 * 1024,
                    max_output_bytes: 2048,
                    fuel_limit: 50_000,
                },
                price_schedule: OfferPriceSchedule {
                    base_fee_msat: 0,
                    success_fee_msat: 0,
                },
                terms_hash: None,
                confidential_profile_hash: None,
            },
        )
        .unwrap();
        let build_evidence = PublicationBuildEvidence {
            schema_version: PUBLICATION_BUILD_EVIDENCE_SCHEMA_V1.to_string(),
            builder: "wat".to_string(),
            builder_version: "1.245.1".to_string(),
            source_digest: hex('c'),
            artifact_digest: hex('d'),
            dependency_mode: "none".to_string(),
            components: Vec::new(),
            hermetic: true,
        };
        let verification_fixture = VerificationFixture {
            input: serde_json::json!({"query": "ok"}),
            expected_output: Some(serde_json::json!({"ok": true})),
        };
        let revision = sign_publication_revision(
            PublicationRevisionPayload {
                schema_version: PUBLICATION_REVISION_SCHEMA_V1.to_string(),
                provider_id: provider_id.clone(),
                service_id: "analytics".to_string(),
                offer_id: offer.payload.offer_id.clone(),
                offer_hash: offer.hash.clone(),
                binding_hash: hex('d'),
                package_digest: hex('d'),
                runtime: "wasm".to_string(),
                package_kind: "inline_module".to_string(),
                build_evidence: Some(build_evidence.clone()),
                service: PublicationRevisionService {
                    project_id: None,
                    summary: None,
                    starter: None,
                    source_kind: "artifact".to_string(),
                    entrypoint_kind: "handler".to_string(),
                    entrypoint: "run".to_string(),
                    contract_version: "froglet.wasm.run_json.v1".to_string(),
                    mode: "sync".to_string(),
                    mounts: Vec::new(),
                    capabilities: Vec::new(),
                    input_schema: Some(serde_json::json!({"type": "object"})),
                    output_schema: Some(serde_json::json!({"type": "object"})),
                },
                limits: ResolvedPublicationLimits {
                    max_input_bytes: 4096,
                    max_runtime_ms: 2500,
                    max_memory_bytes: 8 * 1024 * 1024,
                    max_output_bytes: 2048,
                    fuel_limit: 50_000,
                },
                price: PublicationRevisionPrice {
                    settlement_method: PublicationSettlement::None,
                    currency: PublicationCurrency::Sat,
                    base_amount_minor: 0,
                    success_amount_minor: 0,
                    offer_settlement_method: "none".to_string(),
                },
                local_verification: LocalVerificationEvidence {
                    input_hash: canonical_hash(&verification_fixture.input).unwrap(),
                    result_hash: hex('7'),
                    expected_output_matched: Some(true),
                },
            },
            sign,
        )
        .unwrap();

        let mut payload = valid_payload();
        payload.expected_provider_id = provider_id;
        payload.package.build_evidence_digest = canonical_hash(&build_evidence).unwrap();
        payload.operation_id =
            managed_publication_operation_id(&ManagedPublicationOperationIdentityV1 {
                service_id: &payload.service_id,
                provider_id: &payload.expected_provider_id,
                publish_request_digest: &payload.publish_request_digest,
                source_package_digest: &payload.package.source_package_digest,
                base_runner_image: &payload.package.base_runner_image,
                release_bundle_digest: &payload.package.release_bundle_digest,
                output_repository: &payload.package.image.repository,
                target: &payload.target,
                profile: &payload.profile,
            })
            .unwrap();
        let plan = ManagedPublicationPlanV1::new(payload).unwrap();
        let capsule = ManagedPublicationCapsuleV1 {
            schema_version: MANAGED_PUBLICATION_CAPSULE_SCHEMA_V1.to_string(),
            operation_id: plan.payload.operation_id.clone(),
            plan_hash: plan.plan_hash.clone(),
            consent_hash: hex('8'),
            plan: plan.clone(),
            source_revision_hash: revision.revision_hash.clone(),
            revision,
            verification_fixture,
            descriptor: serde_json::to_value(descriptor).unwrap(),
            offer: serde_json::to_value(offer).unwrap(),
        };
        capsule.validate().unwrap();

        let mut tampered = capsule;
        tampered.offer["payload"]["offer_id"] = Value::String("other".to_string());
        assert!(tampered.validate_against(&plan).is_err());
    }

    #[test]
    fn managed_publication_plan_hash_binds_every_nested_operator_plan() {
        let plan = ManagedPublicationPlanV1::new(valid_payload()).unwrap();
        plan.validate().unwrap();
        assert_eq!(
            plan.deploy_approval_hash().unwrap().to_hex(),
            "03".repeat(32)
        );

        let mut drifted = plan.clone();
        drifted.payload.deploy_plan = operator_plan(
            &drifted.payload.desired_state,
            ManagedDeploymentApprovalScopeV1::Deploy,
            9,
        );
        assert!(matches!(
            drifted.validate(),
            Err(ManagedPublicationContractError::HashMismatch { field: "plan_hash" })
        ));
    }

    #[test]
    fn managed_plan_requires_https_root_origin_and_explicit_registry() {
        let mut payload = valid_payload();
        payload.public_url = "https://analytics.example.test/api?token=no".to_string();
        assert!(matches!(
            payload.validate(),
            Err(ManagedPublicationContractError::InvalidField {
                field: "plan.public_url",
                ..
            })
        ));

        let mut payload = valid_payload();
        payload.package.image.repository = "local-name".to_string();
        assert!(matches!(
            payload.package.validate(),
            Err(ManagedPublicationContractError::InvalidField {
                field: "package.image.repository",
                ..
            })
        ));
    }

    #[test]
    fn compensation_must_be_exact_rollback_or_destroy() {
        let mut payload = valid_payload();
        payload.compensation_plan = operator_plan(
            &payload.desired_state,
            ManagedDeploymentApprovalScopeV1::Deploy,
            8,
        );
        assert!(matches!(
            payload.validate(),
            Err(ManagedPublicationContractError::InvalidField {
                field: "plan.compensation_plan",
                ..
            })
        ));
    }

    #[test]
    fn operation_id_is_stable_and_target_sensitive() {
        let base_runner = OciImageV1 {
            repository: "ghcr.io/example/froglet-runner".to_string(),
            digest: digest('b'),
        };
        let release_bundle = digest('8');
        let output_repository = "registry.example.test/private/analytics";
        let provider_id = hex('1');
        let publish_request_digest = hex('2');
        let source_package_digest = hex('d');
        let operation_id = |target: &str, release_bundle_digest: &str, request_digest: &str| {
            managed_publication_operation_id(&ManagedPublicationOperationIdentityV1 {
                service_id: "analytics",
                provider_id: &provider_id,
                publish_request_digest: request_digest,
                source_package_digest: &source_package_digest,
                base_runner_image: &base_runner,
                release_bundle_digest,
                output_repository,
                target,
                profile: "small-public",
            })
            .unwrap()
        };
        let first = operation_id(
            "regional-container",
            &release_bundle,
            &publish_request_digest,
        );
        let same = operation_id(
            "regional-container",
            &release_bundle,
            &publish_request_digest,
        );
        let changed = operation_id("generic-vm", &release_bundle, &publish_request_digest);
        let changed_release =
            operation_id("regional-container", &digest('9'), &publish_request_digest);
        let changed_request = operation_id("regional-container", &release_bundle, &hex('3'));
        assert_eq!(first, same);
        assert_ne!(first, changed);
        assert_ne!(first, changed_release);
        assert_ne!(first, changed_request);
    }

    #[test]
    fn operation_state_machine_rejects_skipping_external_evidence() {
        let plan = ManagedPublicationPlanV1::new(valid_payload()).unwrap();
        let mut operation = ManagedPublicationOperationV1 {
            schema_version: MANAGED_PUBLICATION_OPERATION_SCHEMA_V1.to_string(),
            operation_id: plan.payload.operation_id.clone(),
            plan_hash: plan.plan_hash.clone(),
            consent_hash: hex('5'),
            phase: ManagedPublicationPhaseV1::Planned,
            revision_hash: None,
            provision_result: None,
            deploy_result: None,
            compensation_result: None,
            remote_canary: None,
            registration: None,
            last_error: None,
            attempt_count: 0,
            updated_at_epoch_seconds: 10,
        };
        operation.validate_against(&plan).unwrap();
        assert!(matches!(
            operation.transition(ManagedPublicationPhaseV1::Active, 11),
            Err(ManagedPublicationContractError::InvalidTransition { .. })
        ));
        operation
            .transition(ManagedPublicationPhaseV1::LocalRevisionReady, 11)
            .unwrap();
        assert!(operation.validate_against(&plan).is_err());
        operation.revision_hash = Some(hex('6'));
        operation.validate_against(&plan).unwrap();
    }

    #[test]
    fn operation_requires_deploy_canary_and_registration_evidence_before_active() {
        let plan = ManagedPublicationPlanV1::new(valid_payload()).unwrap();
        let revision_hash = hex('6');
        let mut operation = ManagedPublicationOperationV1 {
            schema_version: MANAGED_PUBLICATION_OPERATION_SCHEMA_V1.to_string(),
            operation_id: plan.payload.operation_id.clone(),
            plan_hash: plan.plan_hash.clone(),
            consent_hash: hex('5'),
            phase: ManagedPublicationPhaseV1::Planned,
            revision_hash: None,
            provision_result: None,
            deploy_result: None,
            compensation_result: None,
            remote_canary: None,
            registration: None,
            last_error: None,
            attempt_count: 0,
            updated_at_epoch_seconds: 10,
        };
        operation
            .transition(ManagedPublicationPhaseV1::LocalRevisionReady, 11)
            .unwrap();
        operation.revision_hash = Some(revision_hash.clone());
        operation
            .transition(ManagedPublicationPhaseV1::Deploying, 12)
            .unwrap();
        operation
            .transition(ManagedPublicationPhaseV1::Deployed, 13)
            .unwrap();
        assert!(operation.validate_against(&plan).is_err());

        let mut deploy_result = ManagedDeploymentResultV1::new(
            ManagedDeploymentOperationV1::Deploy,
            plan.payload.desired_state.deployment_id.clone(),
            plan.payload.desired_state.revision.revision_id.clone(),
            plan.payload.deploy_plan.adapter.adapter_id.clone(),
            ManagedDeploymentOperationStatusV1::Succeeded,
            vec![plan.payload.desired_state.revision.image.clone()],
            DeploymentHealthV1 {
                status: DeploymentHealthStatusV1::Healthy,
                checked_url: Some(format!("{}/healthz", plan.payload.public_url)),
                detail: None,
            },
            13,
        );
        deploy_result.endpoints.push(DeploymentEndpointV1 {
            name: "public".to_string(),
            url: plan.payload.public_url.clone(),
            transport: EndpointTransportV1::Https,
        });
        operation.deploy_result = Some(deploy_result);
        operation.validate_against(&plan).unwrap();
        operation
            .transition(ManagedPublicationPhaseV1::CanaryVerified, 14)
            .unwrap();
        assert!(operation.validate_against(&plan).is_err());
        operation.remote_canary = Some(ManagedPublicationRemoteCanaryV1 {
            provider_id: plan.payload.expected_provider_id.clone(),
            revision_hash: revision_hash.clone(),
            public_url: plan.payload.public_url.clone(),
            observed_at_epoch_seconds: 14,
        });
        operation.validate_against(&plan).unwrap();
        operation
            .transition(ManagedPublicationPhaseV1::Registering, 15)
            .unwrap();
        operation
            .transition(ManagedPublicationPhaseV1::Active, 16)
            .unwrap();
        assert!(operation.validate_against(&plan).is_err());
        operation.registration = Some(ManagedPublicationRegistrationV1 {
            marketplace_url: "https://marketplace.example.test".to_string(),
            status: "active".to_string(),
            provider_id: plan.payload.expected_provider_id.clone(),
            validated_offer_hash: hex('7'),
            validated_revision_hash: revision_hash,
            registered_at_epoch_seconds: 16,
        });
        operation.validate_against(&plan).unwrap();
        operation
            .registration
            .as_mut()
            .unwrap()
            .validated_revision_hash = hex('8');
        assert!(operation.validate_against(&plan).is_err());
    }

    #[test]
    fn unknown_plan_fields_fail_closed() {
        let plan = ManagedPublicationPlanV1::new(valid_payload()).unwrap();
        let mut json = serde_json::to_value(plan).unwrap();
        json.as_object_mut().unwrap().insert(
            "provider_region".to_string(),
            Value::String("eu-west-1".to_string()),
        );
        assert!(serde_json::from_value::<ManagedPublicationPlanV1>(json).is_err());
    }
}
