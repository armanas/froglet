//! Identity attestation artifacts.
//!
//! These are the on-the-wire types for DNS and OAuth/OIDC identity
//! attestations as specified in
//! [`docs/IDENTITY_ATTESTATION.md`](../../../../docs/IDENTITY_ATTESTATION.md).
//! The protocol crate owns the payload shape and the cryptographic
//! validator; the issuing service layer (see TODO.md Order 81) owns the
//! flows that populate these payloads.
//!
//! The signer of an `IdentityAttestation` is the **marketplace attestation
//! service**, not the subject. Consumers verifying an attestation verify the
//! outer signature against `signer`, then verify `signer == payload.issuer`
//! to bind the attestation to its claimed issuer, then check
//! `expires_at > now`.

use serde::{Deserialize, Serialize};

use super::kernel::SignedArtifact;

/// Canonical artifact type string for identity attestations.
pub const ARTIFACT_TYPE_IDENTITY_ATTESTATION: &str = "identity_attestation/v1";

/// The kind of real-world identifier an attestation binds a Froglet key to.
///
/// Only two kinds are supported in the v1 shape. Stronger identity forms
/// (W3C Verifiable Credentials, proof-of-personhood) are explicitly out of
/// scope per the Order 81 design.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdentityAttestationKind {
    /// The subject controls a DNS zone.
    Dns,
    /// The subject controls an account on a specific OAuth provider.
    Oauth,
}

/// Where the claim evidence lived at `issued_at`. The issuer re-verifies the
/// evidence on a schedule (see the spec), and a failed re-verification
/// invalidates the attestation regardless of `expires_at`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IdentityAttestationEvidenceRef {
    /// A DNS TXT record. `locator` is the full DNS name
    /// (e.g. `_froglet.example.com`).
    DnsTxt { locator: String },
    /// A public URL whose authorship the OAuth provider can attribute.
    Url { locator: String },
}

/// The claim being attested to. Tagged by `IdentityAttestationKind`. Only
/// the matching variant is populated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IdentityAttestationClaim {
    /// DNS zone controlled by the subject.
    Dns {
        /// Bare DNS zone, e.g. `example.com` (no leading `_froglet.` prefix,
        /// no trailing dot).
        dns_zone: String,
    },
    /// OAuth / OIDC provider account controlled by the subject.
    Oauth {
        /// Provider identifier, lower-case: `github`, `gitlab`, `google`,
        /// `gitea`, `microsoft`. Extending this enum is deliberate — new
        /// providers land as explicit additions, not free-form strings.
        oauth_provider: String,
        /// The provider's **stable** subject id. For GitHub this is the
        /// `login` field; for OIDC it is the `sub` claim. Display names are
        /// explicitly NOT used because they are mutable.
        oauth_subject: String,
    },
}

/// The payload of a signed `IdentityAttestation` artifact.
///
/// Design notes:
/// - `subject_pubkey` is the Froglet identity being attested.
/// - `issuer` is the pubkey of the marketplace attestation service that
///   signed this credential. On the signed artifact, `signer == issuer`.
/// - `issued_at` and `expires_at` are RFC 3339 UTC timestamps. Expiry is a
///   hard ceiling: verifiers MUST reject attestations past `expires_at`
///   regardless of cache state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityAttestationPayload {
    /// Schema version string. Always `froglet/v1` for this shape; bumped
    /// when a breaking schema change is introduced.
    pub schema_version: String,
    /// Canonical artifact type. Always
    /// [`ARTIFACT_TYPE_IDENTITY_ATTESTATION`].
    pub artifact_type: String,
    /// Hex-encoded secp256k1 public key of the Froglet identity being
    /// attested.
    pub subject_pubkey: String,
    /// Which kind of identity is being attested.
    pub attestation_kind: IdentityAttestationKind,
    /// The claim details, matching `attestation_kind`.
    pub attestation_claim: IdentityAttestationClaim,
    /// RFC 3339 UTC timestamp at which the issuer observed the evidence.
    pub issued_at: String,
    /// RFC 3339 UTC timestamp after which this attestation is invalid.
    /// Per the spec, `expires_at = issued_at + 180 days`.
    pub expires_at: String,
    /// Hex-encoded secp256k1 public key of the issuing marketplace
    /// attestation service. MUST equal the outer signed artifact's `signer`.
    pub issuer: String,
    /// Where the evidence lived at `issued_at`.
    pub evidence_ref: IdentityAttestationEvidenceRef,
}

/// Validate an `IdentityAttestation` artifact beyond its cryptographic
/// signature. Always call this **after** `verify_artifact`; a valid
/// signature alone does not bind the attestation to its claimed issuer.
///
/// Enforced invariants:
/// - `signer == payload.issuer` — binds the artifact to its claimed issuer.
/// - `payload.schema_version` is non-empty.
/// - `payload.artifact_type` equals the canonical constant.
/// - `payload.subject_pubkey` is non-empty.
/// - `payload.issuer` is non-empty (and by transitivity, matches `signer`).
/// - `payload.issued_at` and `payload.expires_at` are non-empty.
/// - The `attestation_kind` and `attestation_claim` variants agree (DNS kind
///   → Dns claim; OAuth kind → Oauth claim).
/// - The `evidence_ref` variant matches the claim kind (DNS kind →
///   `DnsTxt`; OAuth kind → `Url`).
///
/// Expiry enforcement (`expires_at > now`) is deliberately NOT done here:
/// the protocol crate has no clock dependency and stays storage-free.
/// Callers must check expiry against their own time source.
pub fn validate_identity_attestation_artifact(
    attestation: &SignedArtifact<IdentityAttestationPayload>,
) -> Result<(), String> {
    let p = &attestation.payload;

    if attestation.signer != p.issuer {
        return Err("identity attestation signer does not match payload.issuer".to_string());
    }
    if p.schema_version.trim().is_empty() {
        return Err("identity attestation schema_version must be non-empty".to_string());
    }
    if p.artifact_type != ARTIFACT_TYPE_IDENTITY_ATTESTATION {
        return Err(format!(
            "identity attestation artifact_type must be {ARTIFACT_TYPE_IDENTITY_ATTESTATION}, got {}",
            p.artifact_type
        ));
    }
    if p.subject_pubkey.trim().is_empty() {
        return Err("identity attestation subject_pubkey must be non-empty".to_string());
    }
    if p.issuer.trim().is_empty() {
        return Err("identity attestation issuer must be non-empty".to_string());
    }
    if p.issued_at.trim().is_empty() {
        return Err("identity attestation issued_at must be non-empty".to_string());
    }
    if p.expires_at.trim().is_empty() {
        return Err("identity attestation expires_at must be non-empty".to_string());
    }

    match (&p.attestation_kind, &p.attestation_claim, &p.evidence_ref) {
        (
            IdentityAttestationKind::Dns,
            IdentityAttestationClaim::Dns { dns_zone },
            IdentityAttestationEvidenceRef::DnsTxt { locator },
        ) => {
            if dns_zone.trim().is_empty() {
                return Err("identity attestation dns_zone must be non-empty".to_string());
            }
            if locator.trim().is_empty() {
                return Err(
                    "identity attestation evidence_ref.locator must be non-empty".to_string(),
                );
            }
        }
        (
            IdentityAttestationKind::Oauth,
            IdentityAttestationClaim::Oauth {
                oauth_provider,
                oauth_subject,
            },
            IdentityAttestationEvidenceRef::Url { locator },
        ) => {
            if oauth_provider.trim().is_empty() {
                return Err("identity attestation oauth_provider must be non-empty".to_string());
            }
            if oauth_subject.trim().is_empty() {
                return Err("identity attestation oauth_subject must be non-empty".to_string());
            }
            if locator.trim().is_empty() {
                return Err(
                    "identity attestation evidence_ref.locator must be non-empty".to_string(),
                );
            }
        }
        _ => {
            return Err(
                "identity attestation kind/claim/evidence variants do not agree".to_string(),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto;
    use crate::protocol::kernel::{FROGLET_SCHEMA_V1, sign_artifact, verify_artifact};

    fn dns_payload(issuer_hex: &str, subject_hex: &str) -> IdentityAttestationPayload {
        IdentityAttestationPayload {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            artifact_type: ARTIFACT_TYPE_IDENTITY_ATTESTATION.to_string(),
            subject_pubkey: subject_hex.to_string(),
            attestation_kind: IdentityAttestationKind::Dns,
            attestation_claim: IdentityAttestationClaim::Dns {
                dns_zone: "example.com".to_string(),
            },
            issued_at: "2026-04-19T00:00:00Z".to_string(),
            expires_at: "2026-10-16T00:00:00Z".to_string(),
            issuer: issuer_hex.to_string(),
            evidence_ref: IdentityAttestationEvidenceRef::DnsTxt {
                locator: "_froglet.example.com".to_string(),
            },
        }
    }

    fn oauth_payload(issuer_hex: &str, subject_hex: &str) -> IdentityAttestationPayload {
        IdentityAttestationPayload {
            schema_version: FROGLET_SCHEMA_V1.to_string(),
            artifact_type: ARTIFACT_TYPE_IDENTITY_ATTESTATION.to_string(),
            subject_pubkey: subject_hex.to_string(),
            attestation_kind: IdentityAttestationKind::Oauth,
            attestation_claim: IdentityAttestationClaim::Oauth {
                oauth_provider: "github".to_string(),
                oauth_subject: "armanas".to_string(),
            },
            issued_at: "2026-04-19T00:00:00Z".to_string(),
            expires_at: "2026-10-16T00:00:00Z".to_string(),
            issuer: issuer_hex.to_string(),
            evidence_ref: IdentityAttestationEvidenceRef::Url {
                locator: "https://gist.github.com/armanas/abc123".to_string(),
            },
        }
    }

    #[test]
    fn dns_attestation_roundtrips_sign_verify_validate() {
        let sk = crypto::generate_signing_key();
        let issuer_hex = crypto::public_key_hex(&sk);
        let subject_hex = hex::encode([0u8; 32]);
        let payload = dns_payload(&issuer_hex, &subject_hex);

        let artifact = sign_artifact(
            &issuer_hex,
            |msg| crypto::sign_message_hex(&sk, msg),
            ARTIFACT_TYPE_IDENTITY_ATTESTATION,
            1_713_484_800,
            payload,
        )
        .expect("sign ok");

        assert!(verify_artifact(&artifact), "signature must verify");
        validate_identity_attestation_artifact(&artifact).expect("dns validation ok");
    }

    #[test]
    fn oauth_attestation_roundtrips_sign_verify_validate() {
        let sk = crypto::generate_signing_key();
        let issuer_hex = crypto::public_key_hex(&sk);
        let subject_hex = hex::encode([1u8; 32]);
        let payload = oauth_payload(&issuer_hex, &subject_hex);

        let artifact = sign_artifact(
            &issuer_hex,
            |msg| crypto::sign_message_hex(&sk, msg),
            ARTIFACT_TYPE_IDENTITY_ATTESTATION,
            1_713_484_800,
            payload,
        )
        .expect("sign ok");

        assert!(verify_artifact(&artifact), "signature must verify");
        validate_identity_attestation_artifact(&artifact).expect("oauth validation ok");
    }

    #[test]
    fn rejects_signer_issuer_mismatch() {
        let sk = crypto::generate_signing_key();
        let other_sk = crypto::generate_signing_key();
        let other_issuer_hex = crypto::public_key_hex(&other_sk);
        let subject_hex = hex::encode([2u8; 32]);

        // Payload claims `other_issuer_hex` as issuer, but the artifact is
        // signed by `sk` (a different key). The signature verifies because
        // the signed bytes match `sk`, but validate must reject the
        // signer/issuer mismatch.
        let payload = dns_payload(&other_issuer_hex, &subject_hex);
        let signer_hex = crypto::public_key_hex(&sk);

        let artifact = sign_artifact(
            &signer_hex,
            |msg| crypto::sign_message_hex(&sk, msg),
            ARTIFACT_TYPE_IDENTITY_ATTESTATION,
            1_713_484_800,
            payload,
        )
        .expect("sign ok");

        assert!(verify_artifact(&artifact), "signature must still verify");
        let err = validate_identity_attestation_artifact(&artifact)
            .expect_err("validate must fail on signer/issuer mismatch");
        assert!(
            err.contains("signer does not match payload.issuer"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_kind_claim_mismatch() {
        let sk = crypto::generate_signing_key();
        let issuer_hex = crypto::public_key_hex(&sk);
        let subject_hex = hex::encode([3u8; 32]);

        // DNS kind but OAuth claim.
        let mut payload = dns_payload(&issuer_hex, &subject_hex);
        payload.attestation_claim = IdentityAttestationClaim::Oauth {
            oauth_provider: "github".to_string(),
            oauth_subject: "armanas".to_string(),
        };

        let artifact = sign_artifact(
            &issuer_hex,
            |msg| crypto::sign_message_hex(&sk, msg),
            ARTIFACT_TYPE_IDENTITY_ATTESTATION,
            1_713_484_800,
            payload,
        )
        .expect("sign ok");

        let err = validate_identity_attestation_artifact(&artifact)
            .expect_err("validate must fail on kind/claim mismatch");
        assert!(
            err.contains("variants do not agree"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_empty_subject_pubkey() {
        let sk = crypto::generate_signing_key();
        let issuer_hex = crypto::public_key_hex(&sk);
        let mut payload = dns_payload(&issuer_hex, "");
        payload.subject_pubkey = "   ".to_string();

        let artifact = sign_artifact(
            &issuer_hex,
            |msg| crypto::sign_message_hex(&sk, msg),
            ARTIFACT_TYPE_IDENTITY_ATTESTATION,
            1_713_484_800,
            payload,
        )
        .expect("sign ok");

        let err = validate_identity_attestation_artifact(&artifact)
            .expect_err("validate must fail on empty subject_pubkey");
        assert!(err.contains("subject_pubkey"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_wrong_artifact_type() {
        let sk = crypto::generate_signing_key();
        let issuer_hex = crypto::public_key_hex(&sk);
        let subject_hex = hex::encode([4u8; 32]);
        let mut payload = dns_payload(&issuer_hex, &subject_hex);
        payload.artifact_type = "other/v1".to_string();

        let artifact = sign_artifact(
            &issuer_hex,
            |msg| crypto::sign_message_hex(&sk, msg),
            ARTIFACT_TYPE_IDENTITY_ATTESTATION,
            1_713_484_800,
            payload,
        )
        .expect("sign ok");

        let err = validate_identity_attestation_artifact(&artifact)
            .expect_err("validate must fail on wrong artifact_type");
        assert!(err.contains("artifact_type"), "unexpected error: {err}");
    }

    #[test]
    fn serde_roundtrip_dns_payload() {
        let payload = dns_payload("ISSUER", "SUBJECT");
        let json = serde_json::to_string(&payload).expect("serialize");
        let back: IdentityAttestationPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(payload, back);
    }

    #[test]
    fn serde_roundtrip_oauth_payload() {
        let payload = oauth_payload("ISSUER", "SUBJECT");
        let json = serde_json::to_string(&payload).expect("serialize");
        let back: IdentityAttestationPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(payload, back);
    }
}
