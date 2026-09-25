//! Plan → build → verify → expose → register pipeline for publishing
//! Froglet services. Public publication is deliberately a two-call protocol:
//! the first call builds and discloses an immutable plan without mutation, and
//! the second rebuilds that plan once before consuming its approval hash.
//!
//! See `docs/MANIFEST.md` for the contract this engine consumes.
//!
//! ## Surfaces
//!
//! - **CLI**: `froglet-node publish` calls [`plan_publication`] and [`publish`]
//!   with manifests read from disk.
//! - **MCP**: the native `marketplace_publish` action delegates to those same
//!   entry points and the CLI's canonical manifest loader.
//!
//! Both surfaces wrap the same entry point so there is one source of
//! truth for the publish flow.
//!
//! Supported workload builders are locked Python, inline Wasm/WAT, and native
//! read-only JSON/CSV/SQLite data. OCI remains an explicit adapter seam.

pub mod backends;
pub mod builder;
pub mod daemon_client;
pub mod error;
pub mod managed_oci;
pub mod python_bundle;
pub mod registration;

pub use daemon_client::{ControlAuth, DaemonClient};
pub use error::{PublishError, PublishWarning};

use froglet_protocol::publication::{
    LocalVerificationEvidence, PublicationCurrency, PublicationDataFormat,
    PublicationIdentityBackup, PublicationIntent, PublicationLimits, PublicationMount,
    PublicationPrecondition, PublicationSettlement, SignedPublicationRevision,
};
use froglet_protocol::{
    canonical_json, crypto,
    managed_publication::{
        ManagedPublicationBundleManifestV1, ManagedPublicationPhaseV1, ManagedPublicationPlanV1,
    },
    manifest::{ProjectManifest, ServiceManifest},
};
use std::{path::PathBuf, sync::Once};
use url::Url;

static INSTALL_RUSTLS_PROVIDER: Once = Once::new();

/// Build every publish-engine HTTP client against the workspace's single
/// Ring-backed Rustls provider. Reqwest deliberately uses its
/// `rustls-no-provider` feature so it does not pull a second AWS-LC provider
/// into the released Froglet binary.
pub(crate) fn http_client_builder() -> reqwest::ClientBuilder {
    INSTALL_RUSTLS_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    reqwest::Client::builder()
}

/// Input to [`publish()`].
#[derive(Debug, Clone)]
pub struct PublishInput {
    /// Project-level manifest (`froglet.toml`). Optional — when absent,
    /// engine defaults apply.
    pub project: Option<ProjectManifest>,
    /// Per-service manifest (`froglet-service.toml` v2-v4).
    pub service: ServiceManifest,
    /// Where the service source code lives.
    pub source: SourceLocator,
    /// Override the manifest's hosting choice. Used by CLI/MCP flags
    /// like `--host tor` that ignore the manifest's `[hosting] default`.
    pub hosting_override: Option<HostingChoice>,
    /// Marketplace URL to register against. Overrides any value in the
    /// manifests.
    pub marketplace_url: Url,
    /// Hash returned by [`plan_publication`]. Required for any public hosting
    /// choice so no tunnel or marketplace mutation occurs before the caller
    /// approves the exact disclosure and terms.
    pub approved_consent_hash: Option<String>,
}

pub const PUBLICATION_CONSENT_SCHEMA_V2: &str = "froglet.publication-consent.v2";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RelayConsentDisclosure {
    pub outbound_wss: bool,
    pub relay_control_url: String,
    pub tls_terminated_by_relay: bool,
    pub relay_can_observe_plaintext: bool,
    pub quota_policy: String,
    pub health_lease_required: bool,
}

/// Commercial responsibility shown before a paid publication is approved.
///
/// The headline publication path uses direct provider-owned rails. It does not
/// create a marketplace payout relationship or claim that a live payment has
/// been exercised. Keeping this as a tagged enum prevents a free publication
/// from carrying a partially applicable paid-commerce statement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "model", rename_all = "snake_case")]
pub enum PublicationCommerceDisclosure {
    NotApplicable,
    DirectProviderRail {
        seller_and_payee: String,
        froglet_marketplace_payout_involved: bool,
        stripe_connect_involved: bool,
        froglet_platform_fee_amount_minor: u64,
        external_rail_fees_estimated: bool,
        external_rail_fee_terms_owner: String,
        refund_responsibility: String,
        automated_froglet_marketplace_refund: bool,
        live_payment_verified: bool,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct PublicationConsentSummary {
    pub schema_version: String,
    pub service_id: String,
    pub hosting: String,
    pub opens_public_tunnel: bool,
    /// Truthful timing of the selected transport's public reachability. Tor
    /// is deliberately non-default because its sidecar is already live while
    /// planning; Relay is the only current adapter activated by this consent.
    pub transport_activation: String,
    pub marketplace_url: String,
    /// Identity and endpoint observed through a read-only daemon preflight.
    /// Every public plan binds the daemon identity; Relay additionally requires
    /// an exact ready endpoint before it is approvable.
    pub provider_id: Option<String>,
    pub public_url: Option<String>,
    pub runtime: String,
    pub package_kind: String,
    pub source_kind: Option<String>,
    pub source_binding: String,
    /// Exact package digest that the provider will bind into the revision.
    pub package_digest: String,
    /// Canonical digest of the exact build/dependency evidence.
    pub build_evidence_digest: String,
    /// Canonical digest of the complete provider-control publish request.
    pub publish_request_digest: String,
    /// Exact provider/configuration resolution that the daemon must validate
    /// again before staging data or mutating publication state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_precondition: Option<PublicationPrecondition>,
    pub mounts: Vec<PublicationMount>,
    pub capabilities: Vec<String>,
    pub requested_limits: Option<PublicationLimits>,
    pub settlement_method: PublicationSettlement,
    pub currency: PublicationCurrency,
    pub base_amount_minor: u64,
    pub success_amount_minor: u64,
    pub commerce: PublicationCommerceDisclosure,
    pub verification_input_hash: Option<String>,
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
    /// Data-format/schema disclosure for native data publications.
    pub data_schema: Option<serde_json::Value>,
    pub relay: Option<RelayConsentDisclosure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed: Option<ManagedPublicationPlanV1>,
    pub identity_backup: PublicationIdentityBackup,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct PublicationConsent {
    pub status: String,
    pub consent_hash: String,
    pub summary: PublicationConsentSummary,
}

/// Where the service source code can be loaded from.
#[derive(Debug, Clone)]
pub enum SourceLocator {
    /// Inline source text, e.g., the Python script body.
    Inline(String),
    /// Filesystem path to the source file. Engine reads + hashes it.
    File(PathBuf),
    /// Immutable bytes captured from an authorized project-relative file.
    /// The path is retained only for the authored filename and adjacent
    /// dependency-lock resolution; builders must consume `bytes` rather than
    /// reopening the source path.
    FileSnapshot { path: PathBuf, bytes: Vec<u8> },
    /// OCI image reference + digest. Engine does not pull; it trusts
    /// the digest. Phase 1B + OCI runtimes only.
    OciImage { reference: String, digest: String },
}

/// Hosting backend selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostingChoice {
    /// Private development; binds 127.0.0.1, does NOT register with
    /// marketplace.
    Local,
    /// Default public path: the daemon's outbound WSS Reachability Lease.
    Relay,
    /// Tor v3 hidden service spawned in-process by the engine. Reuses
    /// `froglet::tor::start_hidden_service()`.
    Tor,
    /// User-supplied public URL. Engine validates reachability + asks
    /// marketplace to register.
    SelfHosted { url: Url },
    /// Provider-neutral managed deployment selector. Concrete cloud accounts,
    /// regions, and resource identifiers belong to operator adapter state.
    Managed {
        slug: Option<String>,
        target: String,
        profile: String,
    },
}

/// Successful publish result. The caller uses these fields to tell the
/// user where the service is live and how to invoke it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PublishOutput {
    /// Product-facing progress; evidence remains in the existing fields below.
    pub status: String,
    pub progress: PublicationProgress,
    pub share_url: Option<String>,
    /// The provider's signing-key pubkey (64-hex BIP340).
    pub provider_id: String,
    /// The public URL the service is reachable at. For `Local` this is
    /// `http://127.0.0.1:<port>`; for `Tor` it's the `.onion` URL; for
    /// `SelfHosted` it's the user-supplied URL.
    pub public_url: String,
    /// SHA256 hash of the published offer artifact.
    pub offer_hash: String,
    /// Marketplace URL where the offer is queryable. `None` for
    /// `Local` (no marketplace registration).
    pub marketplace_offer_url: Option<String>,
    /// How to invoke the published service. Leads with the CLI
    /// `froglet-node invoke` subcommand (local-node invocation) and keeps
    /// the MCP `invoke_service` action as the remote-capable alternative.
    pub invoke_command: String,
    /// Status URL the caller can poll for indexer eventual-consistency
    /// updates. `None` for `Local`.
    pub status_url: Option<String>,
    /// Local sandboxed invocation evidence. When absent, marketplace
    /// registration stays on the legacy operator-review path.
    pub local_verification: Option<LocalVerificationEvidence>,
    /// Immutable provider-signed non-Kernel revision binding this offer to
    /// package bytes, currency-aware terms, limits, and local verification.
    pub publication_revision: Option<SignedPublicationRevision>,
    /// Independent publishing-agent invocation run after exact relay
    /// activation but before marketplace submission. This is intentionally
    /// distinct from the marketplace-owned admission canary.
    pub requester_canary: Option<registration::RequesterCanaryEvidence>,
    /// Exact disclosure and terms that were approved before opening a public
    /// transport. Local-only publications include the same non-public plan.
    pub consent: PublicationConsent,
    /// Soft warnings from manifest validation or backend choice.
    pub warnings: Vec<PublishWarning>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PublicationProgress {
    pub local_verified: bool,
    pub public_reachable: Option<bool>,
    pub marketplace_active: bool,
    pub requester_execution_verified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExactRegistrationDisposition {
    Active,
    PendingReview,
}

fn classify_exact_registration(
    registration: &registration::RegistrationResponse,
    marketplace_url: &Url,
    expected_offer_hash: &str,
    expected_revision_hash: &str,
) -> Result<ExactRegistrationDisposition, PublishError> {
    if registration.validated_offer_hash.as_deref() != Some(expected_offer_hash)
        || registration.validated_revision_hash.as_deref() != Some(expected_revision_hash)
    {
        return Err(PublishError::Registration {
            url: marketplace_url.to_string(),
            status: 200,
            body: format!(
                "marketplace did not validate the exact locally verified revision: status={:?}, validated_offer_hash={:?}, validated_revision_hash={:?}",
                registration.status,
                registration.validated_offer_hash,
                registration.validated_revision_hash,
            ),
        });
    }

    match registration.status.as_str() {
        "active" => Ok(ExactRegistrationDisposition::Active),
        "pending_review" => Ok(ExactRegistrationDisposition::PendingReview),
        status => Err(PublishError::Registration {
            url: marketplace_url.to_string(),
            status: 200,
            body: format!("marketplace returned unexpected exact-revision status {status:?}"),
        }),
    }
}

/// Run the full build → host → sign → register pipeline against a
/// running `froglet-node` daemon.
///
/// Phase 1A.7 wiring: composes the 5 sub-phase pieces (LocalBackend,
/// TorBackend, SelfHostedBackend, Python builder, DaemonClient,
/// registration helpers) into one coherent flow.
///
/// Caller is responsible for ensuring the daemon is reachable (the
/// CLI's `init` subcommand handles that for the operator path).
pub async fn publish(
    input: PublishInput,
    daemon: &DaemonClient,
) -> Result<PublishOutput, PublishError> {
    // 1. Build the exact immutable request and perform read-only transport
    // preflight before asking for (or checking) approval.
    let BuiltPublicationPlan {
        hosting,
        publish_request,
        mut consent,
        managed,
    } = build_publication_plan(&input, daemon).await?;

    if !matches!(hosting, HostingChoice::Local)
        && input.approved_consent_hash.as_deref() != Some(consent.consent_hash.as_str())
    {
        return Err(PublishError::InvalidInput {
            field: "approved_consent_hash",
            reason: format!(
                "public publication requires approval of consent {}; run `froglet-node publish --plan --json`, present the summary to the user, then retry with `--approve-consent {}`",
                consent.consent_hash, consent.consent_hash
            ),
        });
    }
    if !matches!(hosting, HostingChoice::Local) {
        consent.status = "approved".to_string();
    }
    let managed_operation_id = managed
        .as_ref()
        .map(|managed| managed.plan.payload.operation_id.clone());

    tracing::info!(
        service_id = %input.service.service_id,
        runtime = %input.service.runtime,
        hosting = ?hosting,
        marketplace = %input.marketplace_url,
        "froglet-publish-engine: pipeline start",
    );

    // 2. Publish and run the provider-private fixture through the daemon. No
    // hosting backend is prepared until this local gate succeeds.
    let response = daemon
        .publish_artifact(
            &publish_request,
            consent
                .summary
                .publication_precondition
                .as_ref()
                .map(|precondition| precondition.precondition_token.as_str()),
        )
        .await?;
    let evidence = response.evidence;
    let requires_compensation = !matches!(hosting, HostingChoice::Local);
    let compensation_target = evidence
        .service_id
        .clone()
        .zip(
            evidence
                .publication_revision
                .as_ref()
                .map(|revision| revision.revision_hash.clone()),
        )
        .map(|(service_id, revision_hash)| {
            (
                service_id,
                revision_hash,
                evidence.activation_token.clone(),
                evidence.previous_publication.clone(),
                evidence.previous_transport_grants.clone(),
            )
        });
    let completion = async {

    if !daemon_client::is_activation_token(&evidence.activation_token) {
        return Err(PublishError::Hosting {
            backend: "daemon",
            reason: "daemon publish evidence omitted a valid 64-hex activation token"
                .to_string(),
        });
    }

    if let Some(revision) = evidence.publication_revision.as_ref() {
        revision.verify().map_err(|error| PublishError::Hosting {
            backend: "daemon",
            reason: format!("daemon returned an invalid publication revision: {error}"),
        })?;
        if revision.payload.offer_hash != evidence.offer_hash
            || revision.payload.provider_id != evidence.provider_id
            || revision.payload.offer_id != evidence.offer_id
            || revision.payload.local_verification != evidence.local_verification.clone().ok_or_else(
                || PublishError::Hosting {
                    backend: "daemon",
                    reason: "daemon returned a publication revision without local verification evidence"
                        .to_string(),
                },
            )?
        {
            return Err(PublishError::Hosting {
                backend: "daemon",
                reason: "daemon publication revision does not match publish evidence".to_string(),
            });
        }
        validate_publish_evidence_against_plan(&publish_request, &consent, &evidence)?;
    } else if evidence.local_verification.is_some() {
        return Err(PublishError::Hosting {
            backend: "daemon",
            reason: "daemon verified the fixture but omitted the immutable publication revision"
                .to_string(),
        });
    }
    if !matches!(hosting, HostingChoice::Local) && evidence.local_verification.is_none() {
        return Err(PublishError::Hosting {
            backend: "daemon",
            reason: "public publication did not return successful local verification evidence; hosting remains untouched"
                .to_string(),
        });
    }

    // 3. Only a locally verified immutable request may prepare a public
    // transport. The prepared endpoint must still match any endpoint the user
    // approved during read-only preflight.
    let prepared = pipeline::prepare_hosting(
        &hosting,
        daemon,
        &evidence,
        &consent,
        managed.as_ref(),
    )
    .await?;
    if let Some(approved_url) = consent.summary.public_url.as_deref()
        && prepared.public_url.trim_end_matches('/') != approved_url.trim_end_matches('/')
    {
        return Err(PublishError::Hosting {
            backend: "publication-consent",
            reason: format!(
                "prepared public URL {:?} does not match approved URL {:?}; re-plan before registration",
                prepared.public_url, approved_url
            ),
        });
    }

    // 4. Exercise the exact endpoint from a requester perspective before any
    // marketplace submission can create a visible listing. A canary failure
    // therefore compensates local lifecycle + relay grant with no listing
    // lease residual.
    let exact_revision = prepared
        .publication_revision
        .as_ref()
        .or(evidence.publication_revision.as_ref());
    let exact_offer_hash = exact_revision
        .map(|revision| revision.payload.offer_hash.as_str())
        .unwrap_or(evidence.offer_hash.as_str())
        .to_string();
    let requester_canary = if prepared.register_with_marketplace {
        let requester_input = input
            .service
            .verification
            .as_ref()
            .map(|fixture| &fixture.input)
            .ok_or_else(|| daemon_plan_mismatch("verification"))?;
        Some(
            registration::run_requester_canary(
                &prepared.public_url,
                exact_revision.ok_or_else(|| daemon_plan_mismatch("publication_revision"))?,
                requester_input,
            )
            .await?,
        )
    } else {
        None
    };

    // 5. Register only after independent public reachability succeeds.
    let (marketplace_offer_url, status_url, marketplace_warning, requester_canary) =
        if prepared.register_with_marketplace {
            let transport_hint = hosting_transport_hint(&hosting);
            if let Some(managed) = managed.as_ref() {
                let operation = daemon
                    .begin_managed_publication_registration(
                        &managed.plan.payload.operation_id,
                    )
                    .await?;
                if !matches!(
                    operation.phase,
                    ManagedPublicationPhaseV1::Registering | ManagedPublicationPhaseV1::Active
                ) {
                    return Err(PublishError::Hosting {
                        backend: "managed-publication-registration",
                        reason: format!(
                            "managed operation returned phase {:?} before marketplace registration resume",
                            operation.phase
                        ),
                    });
                }
            }
            let registration = registration::register_with_marketplace(
                &input.marketplace_url,
                &prepared.public_url,
                transport_hint,
                exact_revision.map(|_| exact_offer_hash.as_str()),
                exact_revision,
                exact_revision.and_then(|_| {
                    input
                        .service
                        .verification
                        .as_ref()
                        .map(|fixture| &fixture.input)
                }),
            )
            .await?;
            if let Some(managed) = managed.as_ref() {
                let revision = exact_revision
                    .ok_or_else(|| daemon_plan_mismatch("publication_revision"))?;
                let registration_evidence = froglet_protocol::managed_publication::ManagedPublicationRegistrationV1 {
                    marketplace_url: input.marketplace_url.to_string(),
                    status: registration.status.clone(),
                    provider_id: registration.provider_id.clone(),
                    validated_offer_hash: registration.validated_offer_hash.clone().ok_or_else(|| {
                        PublishError::Hosting {
                            backend: "managed-publication-registration",
                            reason: "marketplace omitted validated_offer_hash".to_string(),
                        }
                    })?,
                    validated_revision_hash: registration.validated_revision_hash.clone().ok_or_else(|| {
                        PublishError::Hosting {
                            backend: "managed-publication-registration",
                            reason: "marketplace omitted validated_revision_hash".to_string(),
                        }
                    })?,
                    registered_at_epoch_seconds: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_| PublishError::Hosting {
                            backend: "managed-publication-registration",
                            reason: "system clock is before Unix epoch".to_string(),
                        })?
                        .as_secs()
                        .try_into()
                        .map_err(|_| PublishError::Hosting {
                            backend: "managed-publication-registration",
                            reason: "system clock is outside supported range".to_string(),
                        })?,
                };
                registration_evidence
                    .validate_against(&managed.plan, &revision.revision_hash)
                    .map_err(|error| PublishError::Hosting {
                        backend: "managed-publication-registration",
                        reason: error.to_string(),
                    })?;
                let operation = daemon
                    .complete_managed_publication(
                        &managed.plan.payload.operation_id,
                        &registration_evidence,
                    )
                    .await?;
                if operation.phase != ManagedPublicationPhaseV1::Active {
                    return Err(PublishError::Hosting {
                        backend: "managed-publication-registration",
                        reason: format!(
                            "managed operation returned phase {:?} after exact registration",
                            operation.phase
                        ),
                    });
                }
            }
            if let Some(revision) = exact_revision {
                let revision_hash = revision.revision_hash.as_str();
                match classify_exact_registration(
                    &registration,
                    &input.marketplace_url,
                    &exact_offer_hash,
                    revision_hash,
                )? {
                    ExactRegistrationDisposition::Active => {
                        let offer_url = registration::marketplace_offer_url(
                            &input.marketplace_url,
                            &exact_offer_hash,
                        );
                        let offer_status_url = offer_url.clone();

                        // 6. Wait for the exact offer projection (eventually consistent).
                        let warning = match registration::wait_for_offer(
                            &input.marketplace_url,
                            &exact_offer_hash,
                        )
                        .await
                        {
                            Ok(_) => None,
                            Err(error) => Some(PublishWarning::MarketplaceProjectionUnconfirmed {
                                status_url: offer_status_url.clone(),
                                reason: error.to_string(),
                            }),
                        };
                        (
                            Some(offer_url),
                            Some(offer_status_url),
                            warning,
                            requester_canary.clone(),
                        )
                    }
                    // Paid, Tor, and self-hosted exact candidates are deliberately
                    // retained for operator review. Exact validation succeeded,
                    // but there is no active listing to poll or requester-canary.
                    ExactRegistrationDisposition::PendingReview => (
                        None,
                        None,
                        Some(PublishWarning::PendingMarketplaceReview {
                            status: registration.status,
                        }),
                        requester_canary.clone(),
                    ),
                }
            } else {
                (
                    None,
                    None,
                    Some(PublishWarning::PendingMarketplaceReview {
                        status: registration.status,
                    }),
                    requester_canary.clone(),
                )
            }
        } else {
            (None, None, None, None)
        };

    let mut warnings: Vec<PublishWarning> = Vec::new();
    if !prepared.register_with_marketplace {
        warnings.push(PublishWarning::NotRegistered {
            reason: format!("hosting backend {:?} is private", hosting),
        });
    }
    if let Some(w) = marketplace_warning {
        warnings.push(w);
    }

    let invoke_service_id = evidence
        .service_id
        .clone()
        .unwrap_or_else(|| evidence.offer_id.clone());
    let progress = PublicationProgress {
        local_verified: evidence.local_verification.is_some(),
        public_reachable: requester_canary.as_ref().map(|_| true),
        marketplace_active: marketplace_offer_url.is_some(),
        requester_execution_verified: requester_canary.is_some(),
    };
    let status = if progress.marketplace_active && progress.requester_execution_verified { "healthy" }
        else if progress.marketplace_active { "marketplace_active" }
        else if !prepared.register_with_marketplace && progress.local_verified { "local_verified" }
        else if !prepared.register_with_marketplace { "local_published" }
        else { "pending_review" }.to_string();
    let share_url = if matches!(hosting, HostingChoice::Relay) {
        let mut url = configured_share_site_origin();
        url.set_path("/service/");
        url.query_pairs_mut().append_pair("provider", &evidence.provider_id).append_pair("service", &invoke_service_id);
        Some(url.to_string())
    } else { None };
    let invoke_command = if prepared.register_with_marketplace {
        format!("froglet-node invoke {invoke_service_id} '<json_input>' --provider-id {} --provider-url {}", evidence.provider_id, prepared.public_url)
    } else { format!("froglet-node invoke {invoke_service_id} '<json_input>'") };
    Ok(PublishOutput {
        status, progress, share_url, invoke_command,
        provider_id: evidence.provider_id,
        public_url: prepared.public_url,
        offer_hash: exact_offer_hash,
        marketplace_offer_url,
        status_url,
        local_verification: evidence.local_verification,
        publication_revision: prepared
            .publication_revision
            .or(evidence.publication_revision),
        requester_canary,
        consent,
        warnings,
    })
    }
    .await;

    complete_publication_or_compensate(
        daemon,
        requires_compensation,
        compensation_target,
        managed_operation_id,
        input.service.service_id,
        completion,
    )
    .await
}

fn configured_share_site_origin() -> Url {
    std::env::var("FROGLET_SHARE_SITE_ORIGIN")
        .ok()
        .and_then(|value| first_party_share_site_origin(&value))
        .unwrap_or_else(|| Url::parse("https://froglet.dev/").expect("static share origin"))
}

fn first_party_share_site_origin(value: &str) -> Option<Url> {
    Url::parse(value).ok().filter(|url| {
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|host| host == "froglet.dev" || host.ends_with(".froglet.dev"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

type PublicationCompensationTarget = (
    String,
    String,
    String,
    Option<daemon_client::PublicationLifecycleSnapshot>,
    Vec<daemon_client::PublicationTransportGrantSnapshot>,
);

async fn complete_publication_or_compensate<T>(
    daemon: &DaemonClient,
    requires_compensation: bool,
    compensation_target: Option<PublicationCompensationTarget>,
    managed_operation_id: Option<String>,
    fallback_service_id: String,
    completion: Result<T, PublishError>,
) -> Result<T, PublishError> {
    if !requires_compensation {
        return completion;
    }
    let completion_error = match completion {
        Ok(output) => return Ok(output),
        Err(error) => error,
    };
    let Some((
        service_id,
        revision_hash,
        activation_token,
        previous_publication,
        previous_transport_grants,
    )) = compensation_target
    else {
        return Err(PublishError::PartialState {
            service_id: fallback_service_id,
            revision_hash: "unknown".to_string(),
            completion_error: completion_error.to_string(),
            compensation_error:
                "daemon response omitted the exact service/revision/activation compensation target"
                    .to_string(),
        });
    };
    let managed_compensation_error = if let Some(operation_id) = managed_operation_id {
        match daemon.compensate_managed_publication(&operation_id).await {
            Ok(operation)
                if matches!(
                    operation.phase,
                    ManagedPublicationPhaseV1::Compensated | ManagedPublicationPhaseV1::Planned
                ) =>
            {
                None
            }
            Ok(operation) => Some(format!(
                "managed compensation returned phase {:?}",
                operation.phase
            )),
            Err(error) => Some(error.to_string()),
        }
    } else {
        None
    };
    match daemon
        .pause_publication_revision(
            &service_id,
            &revision_hash,
            &activation_token,
            previous_publication.as_ref(),
            &previous_transport_grants,
        )
        .await
    {
        Ok(outcome) if managed_compensation_error.is_none() => match completion_error {
            PublishError::MarketplaceStateUnknown(mut context) => {
                context.local_pause_result = "succeeded".to_string();
                context.relay_withdrawal_result = format!(
                    "{} ({} other grant(s) remain)",
                    outcome.relay_withdrawal_status, outcome.remaining_relay_grants
                );
                Err(PublishError::MarketplaceStateUnknown(context))
            }
            other => Err(other),
        },
        local_result => {
            let local_error = local_result.err().map(|error| error.to_string());
            let compensation_error = [
                managed_compensation_error
                    .map(|error| format!("managed compensation failed: {error}")),
                local_error.map(|error| format!("local compensation failed: {error}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; ");
            match completion_error {
                PublishError::MarketplaceStateUnknown(mut context) => {
                    context.reason = format!(
                        "{}; exact local compensation also failed: {compensation_error}",
                        context.reason
                    );
                    context.local_pause_result = format!("failed: {compensation_error}");
                    context.relay_withdrawal_result =
                        "unknown because exact pause failed".to_string();
                    Err(PublishError::MarketplaceStateUnknown(context))
                }
                other => Err(PublishError::PartialState {
                    service_id,
                    revision_hash,
                    completion_error: other.to_string(),
                    compensation_error: compensation_error.to_string(),
                }),
            }
        }
    }
}

fn daemon_plan_mismatch(field: &str) -> PublishError {
    PublishError::Hosting {
        backend: "daemon",
        reason: format!(
            "daemon publication revision differs from the exact approved request at {field}"
        ),
    }
}

fn effective_intent_capabilities(intent: &PublicationIntent) -> Vec<String> {
    let mut capabilities = intent.capabilities.clone().unwrap_or_default();
    capabilities.extend(
        intent
            .mounts
            .iter()
            .flatten()
            .map(PublicationMount::access_capability),
    );
    capabilities.sort();
    capabilities.dedup();
    capabilities
}

/// Verify that a correctly signed daemon response is also the exact service
/// projection the caller approved. Signature validity alone is insufficient:
/// a daemon can legitimately sign a different package or set of terms.
fn validate_publish_evidence_against_plan(
    intent: &PublicationIntent,
    consent: &PublicationConsent,
    evidence: &daemon_client::PublishEvidence,
) -> Result<(), PublishError> {
    let revision = evidence
        .publication_revision
        .as_ref()
        .ok_or_else(|| daemon_plan_mismatch("publication_revision"))?;
    let payload = &revision.payload;
    let expected_request_digest = crypto::sha256_hex(
        canonical_json::to_vec(intent).map_err(|error| PublishError::Signing(error.to_string()))?,
    );
    if consent.summary.publish_request_digest != expected_request_digest {
        return Err(daemon_plan_mismatch("publish_request_digest"));
    }
    if let Some(expected_provider_id) = consent.summary.provider_id.as_deref()
        && (evidence.provider_id != expected_provider_id
            || payload.provider_id != expected_provider_id)
    {
        return Err(daemon_plan_mismatch("provider_id"));
    }
    let expected_offer_id = intent.offer_id.as_deref().unwrap_or(&intent.service_id);
    if evidence.service_id.as_deref() != Some(intent.service_id.as_str())
        || payload.service_id != intent.service_id
    {
        return Err(daemon_plan_mismatch("service_id"));
    }
    if evidence.offer_id != expected_offer_id || payload.offer_id != expected_offer_id {
        return Err(daemon_plan_mismatch("offer_id"));
    }
    if payload.binding_hash != consent.summary.package_digest
        || payload.package_digest != consent.summary.package_digest
    {
        return Err(daemon_plan_mismatch("package_digest"));
    }
    let expected_build_evidence = intent
        .build_evidence
        .as_ref()
        .ok_or_else(|| daemon_plan_mismatch("build_evidence"))?;
    if payload.build_evidence.as_ref() != Some(expected_build_evidence) {
        return Err(daemon_plan_mismatch("build_evidence"));
    }
    let returned_build_evidence_digest = crypto::sha256_hex(
        canonical_json::to_vec(expected_build_evidence)
            .map_err(|error| PublishError::Signing(error.to_string()))?,
    );
    if returned_build_evidence_digest != consent.summary.build_evidence_digest {
        return Err(daemon_plan_mismatch("build_evidence_digest"));
    }

    let runtime = intent
        .runtime
        .as_deref()
        .ok_or_else(|| daemon_plan_mismatch("runtime"))?;
    let package_kind = intent
        .package_kind
        .as_deref()
        .ok_or_else(|| daemon_plan_mismatch("package_kind"))?;
    if payload.runtime != runtime || payload.package_kind != package_kind {
        return Err(daemon_plan_mismatch("runtime/package_kind"));
    }

    let entrypoint_kind = intent.entrypoint_kind.as_deref().unwrap_or(match runtime {
        "builtin" => "builtin",
        "wasm" => "module",
        _ => "handler",
    });
    let default_entrypoint = match (runtime, entrypoint_kind) {
        ("builtin", _) => "events.query",
        (_, "script") => "__main__",
        (_, "module") => "run",
        ("python", _) => "handler",
        _ => "run",
    };
    let expected_entrypoint = intent.entrypoint.as_deref().unwrap_or(default_entrypoint);
    let default_contract = match (runtime, package_kind, entrypoint_kind) {
        ("python", "inline_source", "script") => "froglet.python.script_json.v1",
        ("python", "inline_source", _) => "froglet.python.handler_json.v1",
        ("container" | "python", "oci_image", _) => "froglet.container.stdin_json.v1",
        ("builtin", "builtin", _) => "froglet.builtin.events_query.v1",
        _ => "froglet.wasm.run_json.v1",
    };
    let expected_contract = intent
        .contract_version
        .as_deref()
        .unwrap_or(default_contract);
    let expected_source_kind = intent.source_kind.clone().unwrap_or_else(|| {
        intent.data_source.as_ref().map_or_else(
            || match (runtime, package_kind) {
                (_, "oci_image") => "oci".to_string(),
                ("wasm", "inline_module") => "artifact".to_string(),
                _ => runtime.to_string(),
            },
            |data| format!("data_query.{}", data.format),
        )
    });
    let expected_summary = intent
        .summary
        .clone()
        .or_else(|| Some(format!("Froglet service {expected_offer_id}")));
    let expected_starter = intent.starter.clone().or_else(|| {
        intent
            .data_source
            .as_ref()
            .map(|_| r#"{"op":"describe"}"#.to_string())
    });
    let expected_mounts = intent.mounts.clone().unwrap_or_default();
    let expected_capabilities = effective_intent_capabilities(intent);
    let service = &payload.service;
    if service.project_id != intent.project_id
        || service.summary != expected_summary
        || service.starter != expected_starter
        || service.source_kind != expected_source_kind
        || service.entrypoint_kind != entrypoint_kind
        || service.entrypoint != expected_entrypoint
        || service.contract_version != expected_contract
        || service.mode != intent.mode.as_deref().unwrap_or("sync")
        || service.mounts != expected_mounts
        || service.capabilities != expected_capabilities
    {
        return Err(daemon_plan_mismatch("service_projection"));
    }
    // Native data schemas are deterministically derived by the daemon from the
    // exact package-bound bytes. Explicit author schemas must always match;
    // non-data services have no derived schema and therefore compare exactly.
    if (intent.data_source.is_none() || intent.input_schema.is_some())
        && service.input_schema != intent.input_schema
    {
        return Err(daemon_plan_mismatch("input_schema"));
    }
    if (intent.data_source.is_none() || intent.output_schema.is_some())
        && service.output_schema != intent.output_schema
    {
        return Err(daemon_plan_mismatch("output_schema"));
    }
    if let Some(precondition) = consent.summary.publication_precondition.as_ref() {
        if payload.limits != precondition.resolved_limits {
            return Err(daemon_plan_mismatch("resolved_limits"));
        }
    } else if let Some(limits) = intent.limits.as_ref() {
        let resolved = &payload.limits;
        if limits
            .max_input_bytes
            .is_some_and(|expected| resolved.max_input_bytes != expected)
            || limits
                .max_runtime_ms
                .is_some_and(|expected| resolved.max_runtime_ms != expected)
            || limits
                .max_memory_bytes
                .is_some_and(|expected| resolved.max_memory_bytes != expected)
            || limits
                .max_output_bytes
                .is_some_and(|expected| resolved.max_output_bytes != expected)
            || limits
                .fuel_limit
                .is_some_and(|expected| resolved.fuel_limit != expected)
        {
            return Err(daemon_plan_mismatch("limits"));
        }
    }
    if payload.price.settlement_method != consent.summary.settlement_method
        || payload.price.currency != consent.summary.currency
        || payload.price.base_amount_minor != consent.summary.base_amount_minor
        || payload.price.success_amount_minor != consent.summary.success_amount_minor
    {
        return Err(daemon_plan_mismatch("price"));
    }
    if consent.summary.verification_input_hash.as_deref()
        != Some(payload.local_verification.input_hash.as_str())
        || payload.local_verification.expected_output_matched
            != intent
                .verification
                .as_ref()
                .and_then(|fixture| fixture.expected_output.as_ref().map(|_| true))
    {
        return Err(daemon_plan_mismatch("local_verification"));
    }
    Ok(())
}

#[derive(Debug)]
struct BuiltPublicationPlan {
    hosting: HostingChoice,
    publish_request: daemon_client::PublishArtifactRequest,
    consent: PublicationConsent,
    managed: Option<BuiltManagedPublication>,
}

#[derive(Debug)]
struct BuiltManagedPublication {
    bundle: ManagedPublicationBundleManifestV1,
    plan: ManagedPublicationPlanV1,
    oci_layout: managed_oci::PlannedManagedOciLayoutV1,
}

#[derive(Debug, Clone, Default)]
struct ConsentTransportPreflight {
    provider_id: Option<String>,
    public_url: Option<String>,
    relay_control_url: Option<String>,
}

/// Produce the non-mutating public publication disclosure. The exact package
/// is built first, but neither the daemon nor any hosting backend is mutated.
/// Its canonical hash is the approval token consumed by [`publish`]. Source,
/// wheel, data, and private fixture bytes remain undisclosed; their canonical
/// package/request hashes are approval-bound.
pub async fn plan_publication(
    input: &PublishInput,
    daemon: &DaemonClient,
) -> Result<PublicationConsent, PublishError> {
    Ok(build_publication_plan(input, daemon).await?.consent)
}

async fn build_publication_plan(
    input: &PublishInput,
    daemon: &DaemonClient,
) -> Result<BuiltPublicationPlan, PublishError> {
    let hosting = resolve_hosting_choice(input)?;
    require_public_verification(&hosting, &input.service)?;
    let publish_request = pipeline::build_publish_request(
        input,
        input.service.runtime.as_str(),
        input.service.package_kind.as_str(),
    )
    .await?;
    let publication_precondition = if matches!(hosting, HostingChoice::Local) {
        None
    } else {
        let precondition = daemon.publication_precondition(&publish_request).await?;
        precondition
            .validate()
            .map_err(|reason| PublishError::Hosting {
                backend: "daemon-preflight",
                reason,
            })?;
        let expected_intent_digest = crypto::sha256_hex(
            canonical_json::to_vec(&publish_request)
                .map_err(|error| PublishError::Signing(error.to_string()))?,
        );
        if precondition.intent_digest != expected_intent_digest {
            return Err(PublishError::Hosting {
                backend: "daemon-preflight",
                reason: "daemon preflight resolved a different publication intent".to_string(),
            });
        }
        Some(precondition)
    };
    let managed = if let HostingChoice::Managed {
        target, profile, ..
    } = &hosting
    {
        let bundle = ManagedPublicationBundleManifestV1::from_intent(&publish_request)
            .map_err(|error| PublishError::Build(error.to_string()))?;
        let (package_request, base_manifest, base_config) = daemon
            .managed_publication_package_profile(&bundle, target, profile)
            .await?;
        let oci_layout =
            managed_oci::plan_managed_oci_layout(&package_request, &base_manifest, &base_config)?;
        let plan = daemon
            .managed_publication_plan(&bundle, &oci_layout.package, target, profile)
            .await?;
        Some(BuiltManagedPublication {
            bundle,
            plan,
            oci_layout,
        })
    } else {
        None
    };
    let transport = if let Some(managed) = &managed {
        ConsentTransportPreflight {
            provider_id: Some(managed.plan.payload.expected_provider_id.clone()),
            public_url: Some(managed.plan.payload.public_url.clone()),
            relay_control_url: None,
        }
    } else {
        consent_transport_preflight(&hosting, daemon).await?
    };
    if let (Some(transport_provider_id), Some(precondition)) = (
        transport.provider_id.as_deref(),
        publication_precondition.as_ref(),
    ) && transport_provider_id != precondition.provider_id
    {
        return Err(PublishError::Hosting {
            backend: "daemon-preflight",
            reason: "daemon publication preflight identity differs from transport identity"
                .to_string(),
        });
    }
    let consent = publication_consent_for_request(
        input,
        &hosting,
        &publish_request,
        transport,
        publication_precondition,
        managed.as_ref().map(|managed| managed.plan.clone()),
    )?;
    Ok(BuiltPublicationPlan {
        hosting,
        publish_request,
        consent,
        managed,
    })
}

async fn consent_transport_preflight(
    hosting: &HostingChoice,
    daemon: &DaemonClient,
) -> Result<ConsentTransportPreflight, PublishError> {
    if matches!(hosting, HostingChoice::Local) {
        return Ok(ConsentTransportPreflight {
            provider_id: None,
            public_url: Some(daemon.daemon_url.as_str().trim_end_matches('/').to_string()),
            relay_control_url: None,
        });
    }
    if let HostingChoice::Managed {
        target, profile, ..
    } = hosting
    {
        return Err(PublishError::NotImplemented {
            what: format!("managed deployment adapter for target={target:?} profile={profile:?}"),
        });
    }

    let capabilities = daemon.capabilities().await?;
    let provider_id = capabilities.identity.node_id.trim();
    if provider_id.len() != 64 || !provider_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PublishError::InvalidInput {
            field: "daemon.identity.node_id",
            reason: "public publication requires a 64-hex provider identity from the daemon"
                .to_string(),
        });
    }
    let provider_id = provider_id.to_string();

    match hosting {
        HostingChoice::Relay => {
            let relay = capabilities.transports.relay.ok_or_else(|| {
                PublishError::InvalidInput {
                    field: "hosting.relay",
                    reason: "relay prerequisite is unavailable: daemon returned no relay capability; configure FROGLET_RELAY_URL and restart froglet-node"
                        .to_string(),
                }
            })?;
            if !relay.enabled {
                return Err(PublishError::InvalidInput {
                    field: "hosting.relay",
                    reason: "relay prerequisite is disabled; configure FROGLET_RELAY_URL and restart froglet-node before requesting approval"
                        .to_string(),
                });
            }
            let status = relay.status.as_deref().unwrap_or("unknown");
            if !matches!(status, "reserved" | "starting" | "up" | "down") {
                return Err(PublishError::InvalidInput {
                    field: "hosting.relay",
                    reason: format!(
                        "relay prerequisite cannot plan an endpoint (status={status:?})"
                    ),
                });
            }
            let relay_url = relay.url.as_deref().ok_or_else(|| PublishError::InvalidInput {
                field: "hosting.relay",
                reason: "relay has no identity-derived planned public URL; cannot produce an approvable plan"
                    .to_string(),
            })?;
            let public_url = crate::backends::relay::validated_public_relay_url(relay_url)?;
            let relay_control_url =
                relay
                    .control_url
                    .as_deref()
                    .ok_or_else(|| {
                        PublishError::InvalidInput {
                    field: "hosting.relay",
                    reason:
                        "relay has no exact configured WSS control endpoint to bind into consent"
                            .to_string(),
                }
                    })?;
            let relay_control_url =
                crate::backends::relay::validated_relay_control_url(relay_control_url)?;
            Ok(ConsentTransportPreflight {
                provider_id: Some(provider_id),
                public_url: Some(public_url),
                relay_control_url: Some(relay_control_url),
            })
        }
        HostingChoice::SelfHosted { url } => Ok(ConsentTransportPreflight {
            provider_id: Some(provider_id),
            public_url: Some(
                crate::backends::self_hosted::validated_public_self_hosted_url(url.as_str())?,
            ),
            relay_control_url: None,
        }),
        HostingChoice::Tor => {
            let tor = capabilities.transports.tor.ok_or_else(|| {
                PublishError::InvalidInput {
                    field: "hosting.tor",
                    reason: "Tor prerequisite is unavailable: daemon returned no Tor capability; set FROGLET_NETWORK_MODE=tor (or dual) and restart froglet-node"
                        .to_string(),
                }
            })?;
            if !tor.enabled {
                return Err(PublishError::InvalidInput {
                    field: "hosting.tor",
                    reason: "Tor prerequisite is disabled; set FROGLET_NETWORK_MODE=tor (or dual) and restart froglet-node before requesting approval"
                        .to_string(),
                });
            }
            let onion_url = tor
                .url
                .as_deref()
                .or(tor.onion_url.as_deref())
                .ok_or_else(|| PublishError::InvalidInput {
                    field: "hosting.tor",
                    reason: "Tor is enabled but has no exact v3 onion endpoint; wait for hidden-service bootstrap before requesting approval"
                        .to_string(),
                })?;
            Ok(ConsentTransportPreflight {
                provider_id: Some(provider_id),
                public_url: Some(crate::backends::tor::validated_public_tor_url(onion_url)?),
                relay_control_url: None,
            })
        }
        HostingChoice::Local | HostingChoice::Managed { .. } => Err(PublishError::Hosting {
            backend: "publication-consent",
            reason: "private or managed hosting reached public transport preflight unexpectedly"
                .to_string(),
        }),
    }
}

fn publication_consent_for_request(
    input: &PublishInput,
    hosting: &HostingChoice,
    intent: &daemon_client::PublishArtifactRequest,
    transport: ConsentTransportPreflight,
    publication_precondition: Option<PublicationPrecondition>,
    managed: Option<ManagedPublicationPlanV1>,
) -> Result<PublicationConsent, PublishError> {
    let (base_fee_msat, success_fee_msat) =
        intent
            .requested_price_schedule()
            .map_err(|error| PublishError::InvalidInput {
                field: "price",
                reason: error.to_string(),
            })?;
    let total_msat =
        base_fee_msat
            .checked_add(success_fee_msat)
            .ok_or_else(|| PublishError::InvalidInput {
                field: "price",
                reason: "price schedule overflow".to_string(),
            })?;
    let settlement_method = match (intent.settlement_method, total_msat) {
        (Some(method), _) => method,
        (None, 0) => PublicationSettlement::None,
        (None, _) => {
            return Err(PublishError::InvalidInput {
                field: "settlement",
                reason: "paid publication requires an explicit settlement method".to_string(),
            });
        }
    };
    let currency = intent.price_currency.unwrap_or(PublicationCurrency::Sat);
    let commerce = if total_msat == 0 {
        PublicationCommerceDisclosure::NotApplicable
    } else {
        PublicationCommerceDisclosure::DirectProviderRail {
            seller_and_payee: "provider".to_string(),
            froglet_marketplace_payout_involved: false,
            stripe_connect_involved: false,
            froglet_platform_fee_amount_minor: 0,
            external_rail_fees_estimated: false,
            external_rail_fee_terms_owner: "provider_account".to_string(),
            refund_responsibility: "provider_or_payment_rail".to_string(),
            automated_froglet_marketplace_refund: false,
            live_payment_verified: false,
        }
    };
    let verification_input_hash = intent
        .verification
        .as_ref()
        .map(|fixture| {
            canonical_json::to_vec(&fixture.input)
                .map(crypto::sha256_hex)
                .map_err(|error| PublishError::InvalidInput {
                    field: "verification.input",
                    reason: error.to_string(),
                })
        })
        .transpose()?;
    let build_evidence = intent.build_evidence.as_ref().ok_or_else(|| {
        PublishError::Build("canonical publish request has no build evidence".to_string())
    })?;
    build_evidence.validate().map_err(PublishError::Build)?;
    let build_evidence_digest = crypto::sha256_hex(
        canonical_json::to_vec(build_evidence)
            .map_err(|error| PublishError::Signing(error.to_string()))?,
    );
    let publish_request_digest = crypto::sha256_hex(
        canonical_json::to_vec(intent).map_err(|error| PublishError::Signing(error.to_string()))?,
    );
    let data_schema = intent.data_source.as_ref().map(|source| {
        serde_json::json!({
            "format": source.format,
            "csv_schema": source.csv_schema.clone(),
        })
    });
    let hosting_name = match hosting {
        HostingChoice::Local => "local",
        HostingChoice::Relay => "relay",
        HostingChoice::Tor => "tor",
        HostingChoice::SelfHosted { .. } => "self",
        HostingChoice::Managed { .. } => "managed",
    };
    let identity_backup = if matches!(hosting, HostingChoice::Local) {
        PublicationIdentityBackup::not_required_for_private_local_proof()
    } else {
        publication_precondition
            .as_ref()
            .map(|precondition| precondition.identity_backup.clone())
            .ok_or_else(|| daemon_plan_mismatch("identity_backup"))?
    };
    let relay = if matches!(hosting, HostingChoice::Relay) {
        Some(RelayConsentDisclosure {
            outbound_wss: true,
            relay_control_url: transport.relay_control_url.clone().ok_or_else(|| {
                PublishError::InvalidInput {
                    field: "hosting.relay",
                    reason: "relay transport preflight omitted its exact control URL".to_string(),
                }
            })?,
            tls_terminated_by_relay: true,
            relay_can_observe_plaintext: true,
            quota_policy:
                "relay and marketplace operator quotas apply; over-quota candidates fail closed"
                    .to_string(),
            health_lease_required: true,
        })
    } else {
        None
    };
    let summary = PublicationConsentSummary {
        schema_version: PUBLICATION_CONSENT_SCHEMA_V2.to_string(),
        service_id: intent.service_id.clone(),
        hosting: hosting_name.to_string(),
        opens_public_tunnel: matches!(hosting, HostingChoice::Relay),
        transport_activation: match hosting {
            HostingChoice::Local => "private_no_public_transport",
            HostingChoice::Relay => "opens_after_exact_approval",
            HostingChoice::Tor => "already_live_before_approval",
            HostingChoice::SelfHosted { .. } => "operator_managed_existing_endpoint",
            HostingChoice::Managed { .. } => "operator_adapter_managed",
        }
        .to_string(),
        marketplace_url: input.marketplace_url.to_string(),
        provider_id: publication_precondition
            .as_ref()
            .map(|precondition| precondition.provider_id.clone())
            .or(transport.provider_id),
        public_url: transport.public_url,
        runtime: intent.runtime.clone().ok_or_else(|| {
            PublishError::Build("canonical publish request has no runtime".to_string())
        })?,
        package_kind: intent.package_kind.clone().ok_or_else(|| {
            PublishError::Build("canonical publish request has no package_kind".to_string())
        })?,
        source_kind: intent.source_kind.clone().or_else(|| {
            intent
                .data_source
                .as_ref()
                .map(|data| format!("data_query.{}", data.format))
        }),
        source_binding: build_evidence.source_digest.clone(),
        package_digest: build_evidence.artifact_digest.clone(),
        build_evidence_digest,
        publish_request_digest,
        publication_precondition,
        mounts: intent.mounts.clone().unwrap_or_default(),
        capabilities: effective_intent_capabilities(intent),
        requested_limits: intent.limits.clone(),
        settlement_method,
        currency,
        base_amount_minor: base_fee_msat / 1_000,
        success_amount_minor: success_fee_msat / 1_000,
        commerce,
        verification_input_hash,
        input_schema: intent.input_schema.clone(),
        output_schema: intent.output_schema.clone(),
        data_schema,
        relay,
        managed,
        identity_backup,
    };
    let consent_hash = crypto::sha256_hex(
        canonical_json::to_vec(&summary)
            .map_err(|error| PublishError::Signing(error.to_string()))?,
    );
    Ok(PublicationConsent {
        status: if matches!(hosting, HostingChoice::Local) {
            "ready".to_string()
        } else {
            "approval_required".to_string()
        },
        consent_hash,
        summary,
    })
}

fn hosting_transport_hint(hosting: &HostingChoice) -> Option<&'static str> {
    match hosting {
        HostingChoice::Tor => Some("tor"),
        HostingChoice::Relay => Some("relay"),
        _ => Some("clearnet"),
    }
}

fn require_public_verification(
    hosting: &HostingChoice,
    service: &ServiceManifest,
) -> Result<(), PublishError> {
    if !matches!(hosting, HostingChoice::Local) && service.verification.is_none() {
        return Err(PublishError::InvalidInput {
            field: "verification",
            reason: "public marketplace publication requires a local invocation fixture; no candidate is submitted without successful immutable revision evidence".to_string(),
        });
    }
    Ok(())
}

/// Pipeline helpers — kept private to encourage callers to use [`publish`].
mod pipeline {
    use super::*;
    use crate::backends::{HostingBackend, PreparedHosting};
    use crate::backends::{local::LocalBackend, self_hosted::SelfHostedBackend, tor::TorBackend};

    pub(super) async fn prepare_hosting(
        choice: &HostingChoice,
        daemon: &DaemonClient,
        evidence: &daemon_client::PublishEvidence,
        consent: &PublicationConsent,
        managed: Option<&BuiltManagedPublication>,
    ) -> Result<PreparedHosting, PublishError> {
        match choice {
            HostingChoice::Local => {
                LocalBackend::with_url(daemon.daemon_url.as_str())
                    .prepare()
                    .await
            }
            HostingChoice::Relay => {
                let revision = evidence
                    .publication_revision
                    .as_ref()
                    .ok_or_else(|| daemon_plan_mismatch("publication_revision"))?;
                let service_id = evidence
                    .service_id
                    .as_deref()
                    .ok_or_else(|| daemon_plan_mismatch("service_id"))?;
                let public_url = consent
                    .summary
                    .public_url
                    .as_deref()
                    .ok_or_else(|| daemon_plan_mismatch("public_url"))?;
                let relay_control_url = consent
                    .summary
                    .relay
                    .as_ref()
                    .map(|relay| relay.relay_control_url.as_str())
                    .ok_or_else(|| daemon_plan_mismatch("relay_control_url"))?;
                daemon
                    .activate_relay_transport(
                        service_id,
                        &revision.revision_hash,
                        &evidence.activation_token,
                        public_url,
                        relay_control_url,
                    )
                    .await?;
                Ok(PreparedHosting {
                    public_url: crate::backends::relay::validated_public_relay_url(public_url)?,
                    register_with_marketplace: true,
                    publication_revision: None,
                })
            }
            HostingChoice::Tor => TorBackend::new(daemon.daemon_url.clone()).prepare().await,
            HostingChoice::SelfHosted { url } => {
                SelfHostedBackend::new(url.clone()).prepare().await
            }
            HostingChoice::Managed { .. } => {
                let managed = managed.ok_or_else(|| daemon_plan_mismatch("managed_plan"))?;
                if consent.summary.managed.as_ref() != Some(&managed.plan) {
                    return Err(daemon_plan_mismatch("managed_plan"));
                }
                let revision = evidence
                    .publication_revision
                    .as_ref()
                    .ok_or_else(|| daemon_plan_mismatch("publication_revision"))?;
                let capsule = daemon
                    .managed_publication_capsule(
                        &managed.plan,
                        &consent.consent_hash,
                        &revision.revision_hash,
                    )
                    .await?;
                let mut durable = daemon
                    .prepare_managed_publication(&managed.plan, &consent.consent_hash)
                    .await?;
                if durable.phase == ManagedPublicationPhaseV1::ReconciliationRequired {
                    durable = daemon
                        .reconcile_managed_publication(&managed.plan.payload.operation_id)
                        .await?;
                }
                if durable.phase == ManagedPublicationPhaseV1::Planned {
                    daemon
                        .upload_managed_publication_image(&managed.plan, &managed.oci_layout)
                        .await?;
                }
                let operation = daemon
                    .activate_managed_publication(&managed.bundle, &capsule)
                    .await?;
                if !matches!(
                    operation.phase,
                    ManagedPublicationPhaseV1::CanaryVerified
                        | ManagedPublicationPhaseV1::Registering
                        | ManagedPublicationPhaseV1::Active
                ) {
                    return Err(PublishError::Hosting {
                        backend: "managed-publication-activate",
                        reason: format!(
                            "managed activation returned phase {:?}; expected requester-canary verification, resumable registration, or exact active state",
                            operation.phase
                        ),
                    });
                }
                Ok(PreparedHosting {
                    public_url: managed.plan.payload.public_url.clone(),
                    register_with_marketplace: true,
                    publication_revision: Some(capsule.revision),
                })
            }
        }
    }

    pub(super) async fn build_publish_request(
        input: &PublishInput,
        runtime: &str,
        package_kind: &str,
    ) -> Result<daemon_client::PublishArtifactRequest, PublishError> {
        let mut request = daemon_client::PublishArtifactRequest::from_service_manifest(
            &input.service,
        )
        .map_err(|error| PublishError::InvalidInput {
            field: "service_manifest",
            reason: error.to_string(),
        })?;

        match (runtime, package_kind) {
            ("python", "inline_source") => {
                let artifact = crate::builder::build_python_locked(
                    &input.source,
                    input.service.entrypoint.as_deref(),
                    input
                        .service
                        .python
                        .as_ref()
                        .and_then(|python| python.lock.as_deref()),
                )
                .await?;
                request.build_evidence = Some(artifact.build_evidence);
                request.python_bundle = artifact.python_bundle;
            }
            ("wasm", "inline_module") => {
                let artifact = crate::builder::build_wasm_inline(&input.source).await?;
                request.wasm_module_hex = Some(hex::encode(artifact.source_bytes));
                request.build_evidence = Some(artifact.build_evidence);
                // The manifest entrypoint locates authoring input (.wat/.wasm).
                // The immutable runtime interface always exports `run`.
                request.entrypoint_kind = Some("module".to_string());
                request.entrypoint = Some("run".to_string());
                request.contract_version = Some("froglet.wasm.run_json.v1".to_string());
                request.source_kind = Some("wasm".to_string());
            }
            ("builtin", "builtin") => {
                let data =
                    input
                        .service
                        .data
                        .as_ref()
                        .ok_or_else(|| PublishError::InvalidInput {
                            field: "data",
                            reason: "builtin publication requires the native read-only data source"
                                .to_string(),
                        })?;
                let format = match data.format.as_str() {
                    "json" => PublicationDataFormat::Json,
                    "csv" => PublicationDataFormat::Csv,
                    "sqlite" => PublicationDataFormat::Sqlite,
                    other => {
                        return Err(PublishError::InvalidInput {
                            field: "data.format",
                            reason: format!("unsupported data format {other:?}"),
                        });
                    }
                };
                let built =
                    crate::builder::build_data_source(&input.source, format, data.csv_schema())
                        .await?;
                request.data_source = Some(built.data_source);
                request.build_evidence = Some(built.build_evidence);
                request.entrypoint_kind = Some("builtin".to_string());
                request.entrypoint = Some(input.service.service_id.clone());
                request.contract_version = Some(
                    match format {
                        PublicationDataFormat::Json => "froglet.builtin.data_query.json.v1",
                        PublicationDataFormat::Csv => "froglet.builtin.data_query.csv.v1",
                        PublicationDataFormat::Sqlite => "froglet.builtin.data_query.sqlite.v1",
                    }
                    .to_string(),
                );
                request.source_kind = Some(format!("data_query.{format}"));
            }
            (rt, pk) => {
                return Err(PublishError::NotImplemented {
                    what: format!("builder for runtime={rt} package_kind={pk} (Phase 1B)"),
                });
            }
        }

        Ok(request)
    }
}

fn resolve_hosting_choice(input: &PublishInput) -> Result<HostingChoice, PublishError> {
    // Override wins.
    if let Some(choice) = &input.hosting_override {
        return Ok(choice.clone());
    }
    // Then the service manifest's [hosting] default.
    if let Some(hosting) = &input.service.hosting {
        return parse_hosting_default(&hosting.default, input);
    }
    // Legacy v2 service manifests can omit [hosting]; only that compatibility
    // path reaches the project-level hosting default. V3/v4 declare hosting
    // explicitly and return above.
    if let Some(project) = &input.project
        && let Some(defaults) = &project.project.defaults
        && let Some(default) = &defaults.hosting
    {
        return parse_hosting_default(default, input);
    }
    // Final fallback: local (private).
    Ok(HostingChoice::Local)
}

fn parse_hosting_default(name: &str, input: &PublishInput) -> Result<HostingChoice, PublishError> {
    match name {
        "local" => Ok(HostingChoice::Local),
        "relay" => Ok(HostingChoice::Relay),
        "tor" => Ok(HostingChoice::Tor),
        "self" => {
            // Pull the URL from the service manifest's [hosting.self] section.
            let url_str = input
                .service
                .hosting
                .as_ref()
                .and_then(|h| h.self_hosted.as_ref())
                .map(|s| s.url.as_str())
                .ok_or_else(|| PublishError::InvalidInput {
                    field: "hosting.self.url",
                    reason: "self-hosted backend requires hosting.self.url".to_string(),
                })?;
            let url = Url::parse(url_str).map_err(|e| PublishError::InvalidInput {
                field: "hosting.self.url",
                reason: format!("not a valid URL: {e}"),
            })?;
            Ok(HostingChoice::SelfHosted { url })
        }
        "managed" => {
            let managed = input
                .service
                .hosting
                .as_ref()
                .and_then(|hosting| hosting.managed.as_ref())
                .ok_or_else(|| PublishError::InvalidInput {
                    field: "hosting.managed",
                    reason: "managed hosting requires a provider-neutral target and profile"
                        .to_string(),
                })?;
            let target = managed
                .target
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| PublishError::InvalidInput {
                    field: "hosting.managed.target",
                    reason: "managed hosting target must not be empty".to_string(),
                })?;
            let profile = managed
                .profile
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| PublishError::InvalidInput {
                    field: "hosting.managed.profile",
                    reason: "managed hosting profile must not be empty".to_string(),
                })?;
            Ok(HostingChoice::Managed {
                slug: managed
                    .slug
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                target: target.to_string(),
                profile: profile.to_string(),
            })
        }
        "fly" => Err(PublishError::InvalidInput {
            field: "hosting.default",
            reason: "legacy v3 Fly hosting is readable for migration only; import its app/region through a deployment adapter, then author provider-neutral hosting.managed target/profile"
                .to_string(),
        }),
        other => Err(PublishError::InvalidInput {
            field: "hosting.default",
            reason: format!("unknown hosting backend {other:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use froglet_protocol::manifest::{
        HostingSection, ManifestWarning, SelfHostingConfig, ServiceManifest, SettlementSection,
    };
    use froglet_protocol::publication::PublicationIdentityBackupState;
    use serde_json::json;

    #[test]
    fn share_site_override_accepts_only_first_party_https_origins() {
        assert_eq!(
            first_party_share_site_origin("https://candidate.froglet.dev/")
                .expect("candidate site")
                .as_str(),
            "https://candidate.froglet.dev/"
        );
        for value in [
            "http://candidate.froglet.dev/",
            "https://froglet.dev.evil.example/",
            "https://candidate.froglet.dev/service/",
            "https://candidate.froglet.dev/?token=x",
            "https://user@candidate.froglet.dev/",
        ] {
            assert!(first_party_share_site_origin(value).is_none(), "{value}");
        }
    }

    fn minimal_service_manifest(hosting_default: &str) -> ServiceManifest {
        let toml = format!(
            r#"
            schema_version = "froglet-service/v3"
            service_id = "test-service"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            verification = {{ input = {{}} }}
            [hosting]
            default = "{}"
            [settlement]
            method = "none"
            "#,
            hosting_default
        );
        ServiceManifest::from_toml(&toml).unwrap().0
    }

    async fn read_test_http_request(
        stream: &mut tokio::net::TcpStream,
    ) -> (String, String, Vec<u8>) {
        use tokio::io::AsyncReadExt;

        let mut bytes = Vec::new();
        let mut scratch = [0_u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut scratch).await.unwrap();
            assert!(read != 0, "request ended before headers");
            bytes.extend_from_slice(&scratch[..read]);
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while bytes.len() - header_end < content_length {
            let read = stream.read(&mut scratch).await.unwrap();
            assert!(read != 0, "request ended before body");
            bytes.extend_from_slice(&scratch[..read]);
        }
        (
            headers.lines().next().unwrap().to_string(),
            headers,
            bytes[header_end..header_end + content_length].to_vec(),
        )
    }

    fn test_precondition_response(provider_id: &str, body: &[u8]) -> String {
        use froglet_protocol::publication::ResolvedPublicationLimits;

        let intent: PublicationIntent =
            serde_json::from_slice(body).expect("test preflight intent");
        let available = if intent.runtime.as_deref() == Some("builtin") {
            ResolvedPublicationLimits {
                max_input_bytes: 1024 * 1024,
                max_runtime_ms: 5_000,
                max_memory_bytes: 0,
                max_output_bytes: 1024 * 1024,
                fuel_limit: 0,
            }
        } else {
            ResolvedPublicationLimits {
                max_input_bytes: 128 * 1024,
                max_runtime_ms: 5_000,
                max_memory_bytes: 64 * 1024 * 1024,
                max_output_bytes: 1024 * 1024,
                fuel_limit: 50_000_000,
            }
        };
        let resolved_limits = intent
            .limits
            .clone()
            .unwrap_or_default()
            .resolve_within(available)
            .expect("test resolved limits");
        let intent_digest =
            crypto::sha256_hex(canonical_json::to_vec(&intent).expect("canonical test intent"));
        serde_json::to_string(
            &PublicationPrecondition::new(
                provider_id.to_string(),
                intent_digest,
                resolved_limits,
                "aa".repeat(32),
                PublicationIdentityBackup::new(PublicationIdentityBackupState::Missing, None)
                    .expect("test missing-backup observation"),
            )
            .expect("test precondition"),
        )
        .expect("serialize test precondition")
    }

    async fn capabilities_daemon(body: String, plans: usize) -> DaemonClient {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider_id =
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["identity"]["node_id"]
                .as_str()
                .unwrap()
                .to_string();
        tokio::spawn(async move {
            for _ in 0..plans * 2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (request_line, _, request_body) = read_test_http_request(&mut stream).await;
                let response_body =
                    if request_line.starts_with("POST /v1/provider/artifacts/preflight ") {
                        test_precondition_response(&provider_id, &request_body)
                    } else {
                        assert!(request_line.starts_with("GET /v1/node/capabilities "));
                        body.clone()
                    };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    response_body.len(),
                    body = response_body,
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        DaemonClient::new(
            Url::parse(&format!("http://{address}")).unwrap(),
            ControlAuth::Value("test-token".to_string()),
        )
        .unwrap()
    }

    async fn relay_preflight_daemon(requests: usize) -> DaemonClient {
        capabilities_daemon(
            format!(
                r#"{{"identity":{{"node_id":"{}"}},"transports":{{"relay":{{"enabled":true,"status":"reserved","url":"https://provider.relay.example","control_url":"wss://control.relay.example/v1/tunnel"}}}}}}"#,
                "11".repeat(32)
            ),
            requests,
        )
        .await
    }

    // Note: an end-to-end `publish()` test requires a running daemon +
    // marketplace, and lives in Phase 1A.8 as an `#[ignore]`d integration
    // test. The pure-function tests below cover the hosting-resolution
    // logic without a daemon.

    fn registration_response(
        status: &str,
        offer_hash: Option<&str>,
        revision_hash: Option<&str>,
    ) -> registration::RegistrationResponse {
        registration::RegistrationResponse {
            status: status.to_string(),
            provider_id: "11".repeat(32),
            provider_url: "https://provider.example".to_string(),
            transport: "relay".to_string(),
            descriptor_hash: "22".repeat(32),
            offers_seen: 1,
            already_registered: false,
            validation_mode: Some("exact_revision".to_string()),
            validated_offer_hash: offer_hash.map(str::to_string),
            validated_revision_hash: revision_hash.map(str::to_string),
            validation_candidate_id: Some(1),
            deprecation_warning: None,
        }
    }

    #[test]
    fn exact_paid_candidate_can_remain_pending_review_after_validation() {
        let marketplace = Url::parse("https://marketplace.froglet.dev").unwrap();
        let offer_hash = "33".repeat(32);
        let revision_hash = "44".repeat(32);
        let registration =
            registration_response("pending_review", Some(&offer_hash), Some(&revision_hash));

        assert_eq!(
            classify_exact_registration(&registration, &marketplace, &offer_hash, &revision_hash,)
                .unwrap(),
            ExactRegistrationDisposition::PendingReview
        );
    }

    #[test]
    fn exact_registration_rejects_a_different_validated_revision() {
        let marketplace = Url::parse("https://marketplace.froglet.dev").unwrap();
        let offer_hash = "33".repeat(32);
        let revision_hash = "44".repeat(32);
        let registration =
            registration_response("active", Some(&offer_hash), Some(&"55".repeat(32)));

        let error =
            classify_exact_registration(&registration, &marketplace, &offer_hash, &revision_hash)
                .expect_err("a different exact revision must fail closed");
        assert!(error.to_string().contains("did not validate the exact"));
    }

    #[tokio::test]
    async fn downstream_public_failures_use_exact_activation_compensation() {
        use std::sync::{Arc, Mutex};
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = Arc::clone(&requests);
        let revision_hash = "ab".repeat(32);
        let activation_token = "cd".repeat(32);
        let server_revision_hash = revision_hash.clone();
        let server_activation_token = activation_token.clone();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (request_line, _, request_body) = read_test_http_request(&mut stream).await;
                server_requests.lock().unwrap().push(request_line);
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&request_body).unwrap(),
                    json!({
                        "activation_token": server_activation_token,
                        "previous_transport_grants": [],
                    }),
                );
                let body = format!(
                    r#"{{"operation":"pause","publication":{{"status":"paused","selected_revision_hash":"{server_revision_hash}","activation_token":"{server_activation_token}"}},"relay_withdrawal":{{"status":"reserved","remaining_grants":0}}}}"#
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let daemon = DaemonClient::new(
            Url::parse(&format!("http://{address}")).unwrap(),
            ControlAuth::Value("test-token".to_string()),
        )
        .unwrap();

        let failures: Vec<PublishError> = vec![
            PublishError::Hosting {
                backend: "relay",
                reason: "prepared endpoint drifted".to_string(),
            },
            PublishError::Registration {
                url: "https://marketplace.example".to_string(),
                status: 503,
                body: "unavailable".to_string(),
            },
            PublishError::Verification {
                tries: 1,
                url: "https://marketplace.example/offers/test".to_string(),
                reason: "requester canary failed".to_string(),
            },
        ];
        for failure in failures {
            let expected = failure.to_string();
            let error = complete_publication_or_compensate(
                &daemon,
                true,
                Some((
                    "service-1".to_string(),
                    revision_hash.clone(),
                    activation_token.clone(),
                    None,
                    Vec::new(),
                )),
                None,
                "service-1".to_string(),
                Err::<(), _>(failure),
            )
            .await
            .expect_err("downstream failure remains visible after compensation");
            assert_eq!(error.to_string(), expected);
        }
        server.await.unwrap();

        let expected_path = format!(
            "POST /v1/provider/publications/service-1/revisions/{revision_hash}/pause HTTP/1.1"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(
            requests.as_slice(),
            [expected_path.clone(), expected_path.clone(), expected_path,]
        );
    }

    #[tokio::test]
    async fn delayed_compensation_after_same_revision_resume_is_explicit_partial_state() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (_, _, request_body) = read_test_http_request(&mut stream).await;
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&request_body).unwrap(),
                json!({
                    "activation_token": "ef".repeat(32),
                    "previous_transport_grants": [],
                }),
            );
            let body =
                r#"{"error":"activation token precondition mismatch after same-revision resume"}"#;
            let response = format!(
                "HTTP/1.1 409 Conflict\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let daemon = DaemonClient::new(
            Url::parse(&format!("http://{address}")).unwrap(),
            ControlAuth::Value("test-token".to_string()),
        )
        .unwrap();
        let revision_hash = "cd".repeat(32);
        let activation_token = "ef".repeat(32);
        let error = complete_publication_or_compensate(
            &daemon,
            true,
            Some((
                "service-1".to_string(),
                revision_hash.clone(),
                activation_token,
                None,
                Vec::new(),
            )),
            None,
            "service-1".to_string(),
            Err::<(), _>(PublishError::Hosting {
                backend: "relay",
                reason: "endpoint mismatch".to_string(),
            }),
        )
        .await
        .expect_err("failed compensation must be explicit");
        server.await.unwrap();

        match error {
            PublishError::PartialState {
                service_id,
                revision_hash: actual_revision_hash,
                completion_error,
                compensation_error,
            } => {
                assert_eq!(service_id, "service-1");
                assert_eq!(actual_revision_hash, revision_hash);
                assert!(completion_error.contains("endpoint mismatch"));
                assert!(compensation_error.contains("409 Conflict"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn resolves_hosting_from_manifest_default() {
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("tor"),
            source: SourceLocator::Inline("x".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        assert_eq!(resolve_hosting_choice(&input).unwrap(), HostingChoice::Tor);
    }

    #[test]
    fn resolves_relay_hosting_from_manifest_default() {
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("relay"),
            source: SourceLocator::Inline("x".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        assert_eq!(
            resolve_hosting_choice(&input).unwrap(),
            HostingChoice::Relay
        );
    }

    #[test]
    fn resolves_provider_neutral_managed_selector() {
        let service = ServiceManifest::from_toml(
            r#"
            schema_version = "froglet-service/v4"
            service_id = "managed-service"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            verification = { input = {} }
            [hosting]
            default = "managed"
            [hosting.managed]
            slug = "managed-service"
            target = "regional-container"
            profile = "small-public"
            [settlement]
            method = "none"
            "#,
        )
        .unwrap()
        .0;
        let input = PublishInput {
            project: None,
            service,
            source: SourceLocator::Inline("x = 1\n".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };

        assert_eq!(
            resolve_hosting_choice(&input).unwrap(),
            HostingChoice::Managed {
                slug: Some("managed-service".to_string()),
                target: "regional-container".to_string(),
                profile: "small-public".to_string(),
            }
        );
    }

    #[test]
    fn legacy_v3_fly_is_readable_but_requires_adapter_import() {
        let (service, warnings) = ServiceManifest::from_toml(
            r#"
            schema_version = "froglet-service/v3"
            service_id = "legacy-fly"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            verification = { input = {} }
            [hosting]
            default = "fly"
            [hosting.fly]
            app = "legacy-app"
            region = "zrh"
            [settlement]
            method = "none"
            "#,
        )
        .expect("legacy v3 Fly remains a readable migration input");
        assert!(warnings.contains(&ManifestWarning::DeprecatedFlyHosting));
        assert_eq!(
            service
                .hosting
                .as_ref()
                .and_then(|hosting| hosting.fly.as_ref())
                .map(|fly| (fly.app.as_str(), fly.region.as_str())),
            Some(("legacy-app", "zrh"))
        );
        let input = PublishInput {
            project: None,
            service,
            source: SourceLocator::Inline("x = 1\n".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };

        let error = resolve_hosting_choice(&input)
            .expect_err("legacy Fly must not become a publication hosting semantic");
        match error {
            PublishError::InvalidInput {
                field: "hosting.default",
                reason,
            } => {
                assert!(reason.contains("deployment adapter"));
                assert!(reason.contains("hosting.managed target/profile"));
            }
            other => panic!("unexpected legacy Fly resolution error: {other}"),
        }
    }

    #[tokio::test]
    async fn relay_consent_is_deterministic_and_binds_source_without_exposing_it() {
        let daemon = relay_preflight_daemon(3).await;
        let mut input = PublishInput {
            project: None,
            service: minimal_service_manifest("relay"),
            source: SourceLocator::Inline("secret source v1".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let first = plan_publication(&input, &daemon)
            .await
            .expect("relay consent");
        let again = plan_publication(&input, &daemon)
            .await
            .expect("stable relay consent");
        assert_eq!(first, again);
        assert_eq!(first.status, "approval_required");
        let expected_provider_id = "11".repeat(32);
        assert_eq!(
            first.summary.provider_id.as_deref(),
            Some(expected_provider_id.as_str())
        );
        assert_eq!(
            first.summary.public_url.as_deref(),
            Some("https://provider.relay.example")
        );
        let precondition = first
            .summary
            .publication_precondition
            .as_ref()
            .expect("public consent binds provider precondition");
        precondition.validate().expect("valid precondition token");
        assert_eq!(precondition.provider_id, expected_provider_id);
        assert_eq!(
            precondition.intent_digest,
            first.summary.publish_request_digest
        );
        assert_eq!(precondition.resolved_limits.max_input_bytes, 128 * 1024);
        assert_eq!(precondition.resolved_limits.max_runtime_ms, 5_000);
        assert_eq!(
            precondition.resolved_limits.max_memory_bytes,
            64 * 1024 * 1024
        );
        assert_eq!(precondition.resolved_limits.max_output_bytes, 1024 * 1024);
        assert_eq!(precondition.resolved_limits.fuel_limit, 50_000_000);
        assert_eq!(
            precondition.identity_backup.state,
            PublicationIdentityBackupState::Missing
        );
        assert_eq!(first.summary.identity_backup, precondition.identity_backup);
        for digest in [
            &first.summary.source_binding,
            &first.summary.package_digest,
            &first.summary.build_evidence_digest,
            &first.summary.publish_request_digest,
        ] {
            assert_eq!(digest.len(), 64);
            assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
        let relay = first.summary.relay.as_ref().expect("relay disclosure");
        assert!(relay.relay_can_observe_plaintext);
        assert!(relay.tls_terminated_by_relay);
        assert_eq!(
            relay.relay_control_url,
            "wss://control.relay.example/v1/tunnel"
        );
        let serialized = serde_json::to_string(&first).unwrap();
        assert!(!serialized.contains("secret source v1"));

        input.source = SourceLocator::Inline("secret source v2".to_string());
        let changed = plan_publication(&input, &daemon)
            .await
            .expect("changed consent");
        assert_ne!(first.consent_hash, changed.consent_hash);
    }

    #[tokio::test]
    async fn relay_consent_hash_binds_the_exact_control_endpoint() {
        let provider_id = "11".repeat(32);
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("relay"),
            source: SourceLocator::Inline(
                "def handler(event, context): return event\n".to_string(),
            ),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let daemon_a = capabilities_daemon(
            format!(
                r#"{{"identity":{{"node_id":"{provider_id}"}},"transports":{{"relay":{{"enabled":true,"status":"reserved","url":"https://provider.relay.example","control_url":"wss://control-a.relay.example/v1/tunnel"}}}}}}"#
            ),
            1,
        )
        .await;
        let first = plan_publication(&input, &daemon_a)
            .await
            .expect("first relay consent");
        let daemon_b = capabilities_daemon(
            format!(
                r#"{{"identity":{{"node_id":"{provider_id}"}},"transports":{{"relay":{{"enabled":true,"status":"reserved","url":"https://provider.relay.example","control_url":"wss://control-b.relay.example/v1/tunnel"}}}}}}"#
            ),
            1,
        )
        .await;
        let changed = plan_publication(&input, &daemon_b)
            .await
            .expect("changed relay consent");

        assert_ne!(first.consent_hash, changed.consent_hash);
        assert_eq!(
            first
                .summary
                .relay
                .as_ref()
                .expect("first relay disclosure")
                .relay_control_url,
            "wss://control-a.relay.example/v1/tunnel"
        );
        assert_eq!(
            changed
                .summary
                .relay
                .as_ref()
                .expect("changed relay disclosure")
                .relay_control_url,
            "wss://control-b.relay.example/v1/tunnel"
        );
    }

    #[tokio::test]
    async fn relay_plan_is_blocked_without_complete_endpoint_metadata() {
        for relay in [
            r#"{"enabled":false,"status":"disabled"}"#,
            r#"{"enabled":true,"status":"starting"}"#,
            r#"{"enabled":true,"status":"up"}"#,
        ] {
            let daemon = capabilities_daemon(
                format!(
                    r#"{{"identity":{{"node_id":"{}"}},"transports":{{"relay":{relay}}}}}"#,
                    "11".repeat(32)
                ),
                1,
            )
            .await;
            let input = PublishInput {
                project: None,
                service: minimal_service_manifest("relay"),
                source: SourceLocator::Inline(
                    "def handler(event, context): return event\n".to_string(),
                ),
                hosting_override: None,
                marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
                approved_consent_hash: None,
            };
            let error = plan_publication(&input, &daemon)
                .await
                .expect_err("incomplete relay metadata must not yield an approval token");
            assert!(matches!(
                error,
                PublishError::InvalidInput {
                    field: "hosting.relay",
                    ..
                }
            ));
        }
    }

    #[tokio::test]
    async fn tor_plan_rejects_malformed_v2_path_and_query_endpoints() {
        let v3_origin = format!("http://{}.onion", "a".repeat(56));
        for onion_url in [
            "not-a-url".to_string(),
            "http://abcdefghijklmnop.onion".to_string(),
            format!("{v3_origin}/service"),
            format!("{v3_origin}?token=x"),
        ] {
            let daemon = capabilities_daemon(
                format!(
                    r#"{{"identity":{{"node_id":"{}"}},"transports":{{"tor":{{"enabled":true,"status":"up","onion_url":"{onion_url}"}}}}}}"#,
                    "11".repeat(32)
                ),
                1,
            )
            .await;
            let input = PublishInput {
                project: None,
                service: minimal_service_manifest("tor"),
                source: SourceLocator::Inline(
                    "def handler(event, context): return event\n".to_string(),
                ),
                hosting_override: None,
                marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
                approved_consent_hash: None,
            };

            let error = plan_publication(&input, &daemon)
                .await
                .expect_err("invalid Tor endpoint must not yield an approval token");
            assert!(
                error.to_string().contains("Tor URL"),
                "unexpected error for {onion_url}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn relay_and_self_hosted_plans_reject_path_bearing_endpoints() {
        let provider_id = "11".repeat(32);
        let relay_daemon = capabilities_daemon(
            format!(
                r#"{{"identity":{{"node_id":"{provider_id}"}},"transports":{{"relay":{{"enabled":true,"status":"up","url":"https://provider.relay.example/api","control_url":"wss://control.relay.example/v1/tunnel"}}}}}}"#
            ),
            1,
        )
        .await;
        let relay_input = PublishInput {
            project: None,
            service: minimal_service_manifest("relay"),
            source: SourceLocator::Inline(
                "def handler(event, context): return event\n".to_string(),
            ),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let error = plan_publication(&relay_input, &relay_daemon)
            .await
            .expect_err("a relay path must not enter consent");
        assert!(error.to_string().contains("HTTPS origin"));

        let self_daemon = capabilities_daemon(
            format!(r#"{{"identity":{{"node_id":"{provider_id}"}},"transports":{{}}}}"#),
            1,
        )
        .await;
        let self_input = PublishInput {
            project: None,
            service: minimal_service_manifest("local"),
            source: SourceLocator::Inline(
                "def handler(event, context): return event\n".to_string(),
            ),
            hosting_override: Some(HostingChoice::SelfHosted {
                url: Url::parse("https://provider.example/api").unwrap(),
            }),
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let error = plan_publication(&self_input, &self_daemon)
            .await
            .expect_err("a self-hosted path must not enter consent");
        assert!(error.to_string().contains("root origin"));
    }

    #[tokio::test]
    async fn changed_package_rejects_stale_approval_before_daemon_publish() {
        use std::sync::{Arc, Mutex};
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests_for_server = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            let provider_id = "11".repeat(32);
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (request_line, _, request_body) = read_test_http_request(&mut stream).await;
                requests_for_server
                    .lock()
                    .unwrap()
                    .push(request_line.clone());
                let body = if request_line.starts_with("POST /v1/provider/artifacts/preflight ") {
                    test_precondition_response(&provider_id, &request_body)
                } else {
                    assert!(request_line.starts_with("GET /v1/node/capabilities "));
                    format!(
                        r#"{{"identity":{{"node_id":"{provider_id}"}},"transports":{{"relay":{{"enabled":true,"status":"reserved","url":"https://provider.relay.example","control_url":"wss://control.relay.example/v1/tunnel"}}}}}}"#
                    )
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let daemon = DaemonClient::new(
            Url::parse(&format!("http://{address}")).unwrap(),
            ControlAuth::Value("test-token".to_string()),
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("handler.py");
        std::fs::write(
            &source_path,
            "def handler(event, context):\n    return {'version': 1}\n",
        )
        .unwrap();
        let mut input = PublishInput {
            project: None,
            service: minimal_service_manifest("relay"),
            source: SourceLocator::File(source_path.clone()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let plan = plan_publication(&input, &daemon).await.unwrap();

        std::fs::write(
            source_path,
            "def handler(event, context):\n    return {'version': 2}\n",
        )
        .unwrap();
        input.approved_consent_hash = Some(plan.consent_hash);
        let error = publish(input, &daemon)
            .await
            .expect_err("changed package must invalidate its prior approval");
        assert!(matches!(
            error,
            PublishError::InvalidInput {
                field: "approved_consent_hash",
                ..
            }
        ));
        server.await.unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(
            requests
                .iter()
                .all(|request| request.starts_with("GET /v1/node/capabilities ")
                    || request.starts_with("POST /v1/provider/artifacts/preflight ")),
            "a stale approval must be rejected before POST /v1/provider/artifacts/publish"
        );
    }

    async fn assert_identity_change_rejects_stale_public_approval(hosting: HostingChoice) {
        use std::sync::{Arc, Mutex};
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests_for_server = Arc::clone(&requests);
        let tor_url = format!("http://{}.onion", "a".repeat(56));
        let is_tor = matches!(&hosting, HostingChoice::Tor);
        let transports = if is_tor {
            format!(r#"{{"tor":{{"enabled":true,"status":"up","onion_url":"{tor_url}"}}}}"#)
        } else {
            "{}".to_string()
        };
        let server = tokio::spawn(async move {
            for node_id in ["11".repeat(32), "22".repeat(32)] {
                for _ in 0..2 {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let (request_line, _, request_body) = read_test_http_request(&mut stream).await;
                    requests_for_server
                        .lock()
                        .unwrap()
                        .push(request_line.clone());
                    let body = if request_line.starts_with("POST /v1/provider/artifacts/preflight ")
                    {
                        test_precondition_response(&node_id, &request_body)
                    } else {
                        assert!(request_line.starts_with("GET /v1/node/capabilities "));
                        format!(
                            r#"{{"identity":{{"node_id":"{node_id}"}},"transports":{transports}}}"#
                        )
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
            }
        });
        let daemon = DaemonClient::new(
            Url::parse(&format!("http://{address}")).unwrap(),
            ControlAuth::Value("test-token".to_string()),
        )
        .unwrap();
        let mut input = PublishInput {
            project: None,
            service: minimal_service_manifest("local"),
            source: SourceLocator::Inline(
                "def handler(event, context): return event\n".to_string(),
            ),
            hosting_override: Some(hosting),
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let plan = plan_publication(&input, &daemon)
            .await
            .expect("public identity-bound plan");
        let first_provider_id = "11".repeat(32);
        assert_eq!(
            plan.summary.provider_id.as_deref(),
            Some(first_provider_id.as_str())
        );
        if is_tor {
            assert_eq!(plan.summary.public_url.as_deref(), Some(tor_url.as_str()));
        }

        input.approved_consent_hash = Some(plan.consent_hash);
        let error = publish(input, &daemon)
            .await
            .expect_err("a changed daemon identity must invalidate public approval");
        assert!(matches!(
            error,
            PublishError::InvalidInput {
                field: "approved_consent_hash",
                ..
            }
        ));
        server.await.unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(
            requests
                .iter()
                .all(|request| request.starts_with("GET /v1/node/capabilities ")
                    || request.starts_with("POST /v1/provider/artifacts/preflight "))
        );
    }

    #[tokio::test]
    async fn self_hosted_identity_change_rejects_stale_approval_before_publish() {
        assert_identity_change_rejects_stale_public_approval(HostingChoice::SelfHosted {
            url: Url::parse("https://provider.example").unwrap(),
        })
        .await;
    }

    #[tokio::test]
    async fn tor_identity_change_rejects_stale_approval_before_publish() {
        assert_identity_change_rejects_stale_public_approval(HostingChoice::Tor).await;
    }

    #[tokio::test]
    async fn local_plan_does_not_require_daemon_identity_preflight() {
        let daemon = DaemonClient::new(
            Url::parse("http://127.0.0.1:1").unwrap(),
            ControlAuth::Value("test-token".to_string()),
        )
        .unwrap();
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("local"),
            source: SourceLocator::Inline("x = 1\n".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };

        let plan = plan_publication(&input, &daemon)
            .await
            .expect("private local plan");
        assert_eq!(plan.status, "ready");
        assert!(plan.summary.provider_id.is_none());
        assert_eq!(
            plan.summary.public_url.as_deref(),
            Some("http://127.0.0.1:1")
        );
        assert_eq!(
            plan.summary.identity_backup.state,
            PublicationIdentityBackupState::NotRequiredForPrivateLocalProof
        );
    }

    fn consent_for_built_request(
        input: &PublishInput,
        request: &daemon_client::PublishArtifactRequest,
    ) -> PublicationConsent {
        let intent_digest =
            crypto::sha256_hex(canonical_json::to_vec(request).expect("canonical test intent"));
        let precondition = PublicationPrecondition::new(
            "11".repeat(32),
            intent_digest,
            froglet_protocol::publication::ResolvedPublicationLimits {
                max_input_bytes: 128 * 1024,
                max_runtime_ms: 5_000,
                max_memory_bytes: 64 * 1024 * 1024,
                max_output_bytes: 1024 * 1024,
                fuel_limit: 50_000_000,
            },
            "22".repeat(32),
            PublicationIdentityBackup::new(PublicationIdentityBackupState::Missing, None)
                .expect("test missing-backup observation"),
        )
        .expect("test publication precondition");
        publication_consent_for_request(
            input,
            &HostingChoice::Local,
            request,
            ConsentTransportPreflight::default(),
            Some(precondition),
            None,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn commerce_disclosure_is_not_applicable_for_free_publication() {
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("local"),
            source: SourceLocator::Inline("x = 1\n".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let request = pipeline::build_publish_request(&input, "python", "inline_source")
            .await
            .unwrap();
        let consent = consent_for_built_request(&input, &request);
        assert_eq!(
            consent.summary.commerce,
            PublicationCommerceDisclosure::NotApplicable
        );
    }

    #[tokio::test]
    async fn paid_direct_rail_commerce_terms_are_explicit_and_hash_bound() {
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("local"),
            source: SourceLocator::Inline("x = 1\n".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let built = pipeline::build_publish_request(&input, "python", "inline_source")
            .await
            .unwrap();

        for (settlement, currency) in [
            (PublicationSettlement::Lightning, PublicationCurrency::Sat),
            (PublicationSettlement::Stripe, PublicationCurrency::Usd),
        ] {
            let mut request = built.clone();
            request.price_sats = 1;
            request.settlement_method = Some(settlement);
            request.price_currency = Some(currency);
            let consent = consent_for_built_request(&input, &request);
            match &consent.summary.commerce {
                PublicationCommerceDisclosure::DirectProviderRail {
                    seller_and_payee,
                    froglet_marketplace_payout_involved,
                    stripe_connect_involved,
                    froglet_platform_fee_amount_minor,
                    external_rail_fees_estimated,
                    external_rail_fee_terms_owner,
                    refund_responsibility,
                    automated_froglet_marketplace_refund,
                    live_payment_verified,
                } => {
                    assert_eq!(seller_and_payee, "provider");
                    assert!(!froglet_marketplace_payout_involved);
                    assert!(!stripe_connect_involved);
                    assert_eq!(*froglet_platform_fee_amount_minor, 0);
                    assert!(!external_rail_fees_estimated);
                    assert_eq!(external_rail_fee_terms_owner, "provider_account");
                    assert_eq!(refund_responsibility, "provider_or_payment_rail");
                    assert!(!automated_froglet_marketplace_refund);
                    assert!(!live_payment_verified);
                }
                PublicationCommerceDisclosure::NotApplicable => {
                    panic!("paid publication must disclose direct-commerce responsibility")
                }
            }

            let mut tampered_summary = consent.summary.clone();
            tampered_summary.commerce = PublicationCommerceDisclosure::NotApplicable;
            let tampered_hash = crypto::sha256_hex(
                canonical_json::to_vec(&tampered_summary).expect("canonical consent summary"),
            );
            assert_ne!(consent.consent_hash, tampered_hash);
        }
    }

    fn pure_python_test_wheel(value: u8) -> Vec<u8> {
        use std::io::{Cursor, Write};
        use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut output);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            for (name, bytes) in [
                (
                    "demo/__init__.py",
                    format!("VALUE = {value}\n").into_bytes(),
                ),
                (
                    "demo-1.0.dist-info/METADATA",
                    b"Metadata-Version: 2.1\nName: demo\nVersion: 1.0\n".to_vec(),
                ),
                (
                    "demo-1.0.dist-info/WHEEL",
                    b"Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n".to_vec(),
                ),
            ] {
                writer.start_file(name, options).unwrap();
                writer.write_all(&bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    #[tokio::test]
    async fn changed_locked_wheel_changes_package_and_consent() {
        use froglet_protocol::publication::{
            PYTHON_LOCK_SCHEMA_V1, PythonLockManifest, PythonLockedArtifact,
        };

        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("handler.py");
        let lock_path = directory.path().join("froglet-python.lock.json");
        let artifacts_dir = directory.path().join("artifacts");
        std::fs::create_dir(&artifacts_dir).unwrap();
        let source = b"from demo import VALUE\ndef handler(event, context): return VALUE\n";
        std::fs::write(&source_path, source).unwrap();
        let auto = crate::builder::build_python_inline(
            &SourceLocator::File(source_path.clone()),
            Some("handler.py"),
        )
        .await
        .unwrap();
        let runtime = auto.python_bundle.unwrap().lock.runtime;
        let service = ServiceManifest::from_toml(
            r#"
            schema_version = "froglet-service/v4"
            service_id = "locked-python"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"

            [python]
            lock = "froglet-python.lock.json"

            [hosting]
            default = "local"

            [settlement]
            method = "none"
            "#,
        )
        .unwrap()
        .0;
        let input = PublishInput {
            project: None,
            service,
            source: SourceLocator::File(source_path),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let mut plans = Vec::new();
        for value in [1_u8, 2_u8] {
            let wheel = pure_python_test_wheel(value);
            let filename = "demo-1.0-py3-none-any.whl";
            std::fs::write(artifacts_dir.join(filename), &wheel).unwrap();
            let lock = PythonLockManifest {
                schema_version: PYTHON_LOCK_SCHEMA_V1.to_string(),
                source_sha256: crypto::sha256_hex(source),
                runtime: runtime.clone(),
                artifacts: vec![PythonLockedArtifact {
                    name: "demo".to_string(),
                    version: "1.0".to_string(),
                    filename: filename.to_string(),
                    sha256: crypto::sha256_hex(&wheel),
                }],
            };
            std::fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
            let request = pipeline::build_publish_request(&input, "python", "inline_source")
                .await
                .unwrap();
            plans.push(consent_for_built_request(&input, &request));
        }
        assert_ne!(
            plans[0].summary.package_digest,
            plans[1].summary.package_digest
        );
        assert_ne!(plans[0].consent_hash, plans[1].consent_hash);
    }

    #[tokio::test]
    async fn changed_csv_schema_changes_package_and_consent() {
        let manifest = |collection: &str| {
            ServiceManifest::from_toml(&format!(
                r#"
                schema_version = "froglet-service/v4"
                service_id = "people"
                runtime = "builtin"
                package_kind = "builtin"

                [data]
                path = "people.csv"
                format = "csv"
                collection = "{collection}"

                [[data.columns]]
                name = "id"
                type = "integer"
                indexed = true

                [[data.columns]]
                name = "name"
                type = "string"

                [hosting]
                default = "local"

                [settlement]
                method = "none"
                "#
            ))
            .unwrap()
            .0
        };
        let mut consents = Vec::new();
        for collection in ["people", "persons"] {
            let input = PublishInput {
                project: None,
                service: manifest(collection),
                source: SourceLocator::Inline("id,name\n1,Ada\n".to_string()),
                hosting_override: None,
                marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
                approved_consent_hash: None,
            };
            let request = pipeline::build_publish_request(&input, "builtin", "builtin")
                .await
                .unwrap();
            consents.push(consent_for_built_request(&input, &request));
        }
        assert_ne!(
            consents[0].summary.package_digest,
            consents[1].summary.package_digest
        );
        assert_ne!(
            consents[0].summary.data_schema,
            consents[1].summary.data_schema
        );
        assert_ne!(consents[0].consent_hash, consents[1].consent_hash);
    }

    #[tokio::test]
    async fn approved_publish_reuses_exact_build_and_does_not_prepare_hosting_on_local_failure() {
        use std::sync::{Arc, Mutex};
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::<(String, String, Vec<u8>)>::new()));
        let requests_for_server = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            let provider_id = "11".repeat(32);
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (request_line, headers, body) = read_test_http_request(&mut stream).await;
                requests_for_server.lock().unwrap().push((
                    request_line.clone(),
                    headers,
                    body.clone(),
                ));
                let (status, body) = if request_line
                    .starts_with("POST /v1/provider/artifacts/preflight ")
                {
                    ("200 OK", test_precondition_response(&provider_id, &body))
                } else if request_line.starts_with("GET /v1/node/capabilities ") {
                    (
                        "200 OK",
                        format!(
                            r#"{{"identity":{{"node_id":"{}"}},"transports":{{"relay":{{"enabled":true,"status":"reserved","url":"https://provider.relay.example","control_url":"wss://control.relay.example/v1/tunnel"}}}}}}"#,
                            provider_id
                        ),
                    )
                } else {
                    (
                        "422 Unprocessable Entity",
                        r#"{"error":"local fixture failed"}"#.to_string(),
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let daemon = DaemonClient::new(
            Url::parse(&format!("http://{address}")).unwrap(),
            ControlAuth::Value("test-token".to_string()),
        )
        .unwrap();
        let mut input = PublishInput {
            project: None,
            service: minimal_service_manifest("relay"),
            source: SourceLocator::Inline(
                "def handler(event, context): return event\n".to_string(),
            ),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        let plan = plan_publication(&input, &daemon).await.unwrap();
        input.approved_consent_hash = Some(plan.consent_hash.clone());
        publish(input, &daemon)
            .await
            .expect_err("daemon local verification failure must stop publication");
        server.await.unwrap();

        let captured = requests.lock().unwrap();
        assert_eq!(
            captured
                .iter()
                .filter(|(line, _, _)| line.starts_with("GET /v1/node/capabilities "))
                .count(),
            2,
            "only the two read-only transport preflights may run; hosting prepare must remain untouched"
        );
        assert_eq!(
            captured
                .iter()
                .filter(|(line, _, _)| {
                    line.starts_with("POST /v1/provider/artifacts/preflight ")
                })
                .count(),
            2,
            "the plan and approved rebuild must each resolve the exact daemon precondition"
        );
        let (_, headers, body) = captured
            .iter()
            .find(|(line, _, _)| line.starts_with("POST /v1/provider/artifacts/publish "))
            .expect("exact daemon publish request");
        let approved_precondition = plan
            .summary
            .publication_precondition
            .as_ref()
            .expect("public plan precondition");
        assert!(headers.lines().any(|line| {
            line.eq_ignore_ascii_case(&format!(
                "{}: {}",
                froglet_protocol::publication::PUBLICATION_PRECONDITION_HEADER,
                approved_precondition.precondition_token
            ))
        }));
        let request: daemon_client::PublishArtifactRequest = serde_json::from_slice(body).unwrap();
        assert_eq!(
            crypto::sha256_hex(canonical_json::to_vec(&request).unwrap()),
            plan.summary.publish_request_digest,
            "approved call must send the exact request whose digest was approved"
        );
    }

    #[test]
    fn resolves_hosting_override_wins() {
        let input = PublishInput {
            project: None,
            service: minimal_service_manifest("tor"),
            source: SourceLocator::Inline("x".to_string()),
            hosting_override: Some(HostingChoice::Local),
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        assert_eq!(
            resolve_hosting_choice(&input).unwrap(),
            HostingChoice::Local
        );
    }

    #[test]
    fn public_hosting_requires_a_verification_fixture_before_mutation() {
        let mut service = minimal_service_manifest("tor");
        service.verification = None;
        let error = require_public_verification(&HostingChoice::Tor, &service)
            .expect_err("public publication without a fixture must fail closed");
        assert!(matches!(
            error,
            PublishError::InvalidInput {
                field: "verification",
                ..
            }
        ));
        require_public_verification(&HostingChoice::Local, &service)
            .expect("private local publication remains backward compatible");
    }

    #[test]
    fn resolves_self_hosting_pulls_url_from_manifest() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            service_id = "test-service"
            runtime = "python"
            package_kind = "inline_source"
            entrypoint = "handler.py"
            [hosting]
            default = "self"
            [hosting.self]
            url = "https://my-host.example.com"
            [settlement]
            method = "none"
        "#;
        let service = ServiceManifest::from_toml(toml).unwrap().0;
        let input = PublishInput {
            project: None,
            service,
            source: SourceLocator::Inline("x".to_string()),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };
        match resolve_hosting_choice(&input).unwrap() {
            HostingChoice::SelfHosted { url } => {
                assert_eq!(url.as_str(), "https://my-host.example.com/")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn publish_request_preserves_the_complete_manifest_intent() {
        let toml = r#"
            schema_version = "froglet-service/v3"
            project_id = "analytics"
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
            source_kind = "python"
            publication_state = "hidden"
            mounts = [{ handle = "warehouse", kind = "postgres", read_only = true }]
            capabilities = ["network.http.fetch", "network.http.fetch"]
            limits = { max_input_bytes = 4096, max_runtime_ms = 2500, max_memory_bytes = 8388608, max_output_bytes = 2048, fuel_limit = 50000 }
            input_schema = { type = "object", required = ["query"] }
            output_schema = { type = "object", properties = { rows = { type = "array" } } }
            verification = { input = { query = "select 1" }, expected_output = { rows = [] } }
            [hosting]
            default = "local"
            [settlement]
            method = "lightning"
            [price]
            sats = 3
            currency = "sat"
        "#;
        let service = ServiceManifest::from_toml(toml).unwrap().0;
        let input = PublishInput {
            project: None,
            service,
            source: SourceLocator::Inline(
                "def handler(event, context):\n    return event\n".to_string(),
            ),
            hosting_override: None,
            marketplace_url: Url::parse("https://marketplace.froglet.dev").unwrap(),
            approved_consent_hash: None,
        };

        let request = pipeline::build_publish_request(&input, "python", "inline_source")
            .await
            .unwrap();

        assert_eq!(request.project_id.as_deref(), Some("analytics"));
        assert_eq!(request.offer_id.as_deref(), Some("analytics-read-v2"));
        assert_eq!(request.runtime.as_deref(), Some("python"));
        assert_eq!(request.mounts.as_ref().unwrap()[0].handle, "warehouse");
        assert_eq!(
            request.capabilities.as_deref(),
            Some(&["network.http.fetch".to_string()][..])
        );
        assert_eq!(request.limits.as_ref().unwrap().max_runtime_ms, Some(2500));
        assert_eq!(
            request.settlement_method,
            Some(froglet_protocol::publication::PublicationSettlement::Lightning)
        );
        assert_eq!(
            request.price_currency,
            Some(froglet_protocol::publication::PublicationCurrency::Sat)
        );
        assert_eq!(request.price_sats, 3);
        assert_eq!(request.starter.as_deref(), Some(r#"{"query":"select 1"}"#));
        assert_eq!(
            request.input_schema.as_ref().unwrap()["required"][0],
            "query"
        );
        assert_eq!(
            request.output_schema.as_ref().unwrap()["properties"]["rows"]["type"],
            "array"
        );
        assert_eq!(
            request.verification.as_ref().unwrap().input["query"],
            "select 1"
        );
    }

    #[test]
    fn _silence_unused_warning() {
        // Keep these types reachable for Phase 1B without unused-import noise.
        let _ = HostingSection {
            default: "self".to_string(),
            local: None,
            relay: None,
            tor: None,
            self_hosted: Some(SelfHostingConfig {
                url: "https://x".to_string(),
            }),
            managed: None,
            fly: None,
        };
        let _ = SettlementSection {
            method: "none".to_string(),
        };
    }
}
