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

/// Domain separator for the DNS bind statement (Flow 1 step 1 in the spec).
pub const DNS_BIND_DOMAIN: &str = "froglet-identity-bind/v1";
/// Version tag carried in the `_froglet.<zone>` TXT record.
pub const DNS_TXT_RECORD_VERSION: &str = "froglet1";

/// The canonical JSON object the subject signs to bind a Froglet key to a
/// DNS zone. Both sides of the flow — the subject producing the TXT record
/// and the attestation service verifying it — MUST build the signed bytes
/// through [`dns_bind_statement_bytes`] so the JCS encoding is identical.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DnsBindStatement {
    /// Always [`DNS_BIND_DOMAIN`].
    pub domain: String,
    /// The claim, `dns:<zone>` (bare zone, no `_froglet.` prefix, no
    /// trailing dot).
    pub claim: String,
    /// Hex-encoded secp256k1 public key of the Froglet identity.
    pub subject_pubkey: String,
    /// RFC 3339 UTC timestamp; the issuer enforces a 10-minute freshness
    /// window at verification time (replay protection).
    pub ts: String,
}

/// Canonical signed bytes for the DNS bind statement: the JCS encoding of
/// [`DnsBindStatement`].
pub fn dns_bind_statement_bytes(
    dns_zone: &str,
    subject_pubkey_hex: &str,
    timestamp_rfc3339: &str,
) -> Result<Vec<u8>, String> {
    let zone = normalize_dns_zone(dns_zone)?;
    if subject_pubkey_hex.trim().is_empty() {
        return Err("subject pubkey must not be empty".to_string());
    }
    if timestamp_rfc3339.trim().is_empty() {
        return Err("timestamp must not be empty".to_string());
    }
    let statement = DnsBindStatement {
        domain: DNS_BIND_DOMAIN.to_string(),
        claim: format!("dns:{zone}"),
        subject_pubkey: subject_pubkey_hex.trim().to_string(),
        ts: timestamp_rfc3339.trim().to_string(),
    };
    crate::canonical_json::to_vec(&statement)
        .map_err(|error| format!("bind statement is not canonical-JSON encodable: {error}"))
}

/// Render the `_froglet.<zone>` TXT record value:
/// `v=froglet1; pubkey=<hex>; sig=<hex>; ts=<rfc3339>`.
pub fn format_dns_txt_record(
    subject_pubkey_hex: &str,
    signature_hex: &str,
    timestamp_rfc3339: &str,
) -> String {
    format!(
        "v={DNS_TXT_RECORD_VERSION}; pubkey={}; sig={}; ts={}",
        subject_pubkey_hex.trim(),
        signature_hex.trim(),
        timestamp_rfc3339.trim()
    )
}

/// Parsed fields of a `_froglet.<zone>` TXT record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsTxtRecord {
    pub pubkey: String,
    pub sig: String,
    pub ts: String,
}

/// Parse a `_froglet.<zone>` TXT record value produced by
/// [`format_dns_txt_record`]. Field order is not significant; unknown
/// fields are ignored so the record format can grow additively.
pub fn parse_dns_txt_record(record: &str) -> Result<DnsTxtRecord, String> {
    let mut version = None;
    let mut pubkey = None;
    let mut sig = None;
    let mut ts = None;
    for field in record.split(';') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "v" => version = Some(value.to_string()),
            "pubkey" => pubkey = Some(value.to_string()),
            "sig" => sig = Some(value.to_string()),
            "ts" => ts = Some(value.to_string()),
            _ => {}
        }
    }
    match version.as_deref() {
        Some(DNS_TXT_RECORD_VERSION) => {}
        Some(other) => return Err(format!("unsupported TXT record version {other:?}")),
        None => return Err("TXT record is missing the v= field".to_string()),
    }
    Ok(DnsTxtRecord {
        pubkey: pubkey
            .filter(|v| !v.is_empty())
            .ok_or("TXT record is missing pubkey=")?,
        sig: sig
            .filter(|v| !v.is_empty())
            .ok_or("TXT record is missing sig=")?,
        ts: ts
            .filter(|v| !v.is_empty())
            .ok_or("TXT record is missing ts=")?,
    })
}

/// Verify a parsed TXT record against a zone: rebuilds the canonical bind
/// statement from the record's own fields and checks the signature against
/// the record's pubkey. Freshness (the 10-minute `ts` window) is the
/// caller's job — this crate has no clock dependency.
pub fn verify_dns_txt_record(record: &DnsTxtRecord, dns_zone: &str) -> Result<(), String> {
    let message = dns_bind_statement_bytes(dns_zone, &record.pubkey, &record.ts)?;
    if crate::crypto::verify_message(&record.pubkey, &record.sig, &message) {
        Ok(())
    } else {
        Err("TXT record signature does not verify against its pubkey".to_string())
    }
}

/// Bare-zone normalization: lowercase, no scheme, no `_froglet.` prefix, no
/// trailing dot, at least one interior dot.
fn normalize_dns_zone(zone: &str) -> Result<String, String> {
    let zone = zone.trim().trim_end_matches('.').to_ascii_lowercase();
    if zone.is_empty() {
        return Err("dns zone must not be empty".to_string());
    }
    if zone.contains("://") || zone.contains('/') {
        return Err(format!("dns zone {zone:?} must be a bare zone, not a URL"));
    }
    if let Some(stripped) = zone.strip_prefix("_froglet.") {
        return Err(format!(
            "pass the bare zone {stripped:?}; the _froglet. prefix is added by the TXT record name"
        ));
    }
    if !zone.contains('.') {
        return Err(format!("dns zone {zone:?} does not look like a zone"));
    }
    if !zone
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
    {
        return Err(format!("dns zone {zone:?} contains unsupported characters"));
    }
    Ok(zone)
}

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

    #[test]
    fn dns_bind_round_trip_signs_formats_parses_and_verifies() {
        let sk = crypto::generate_signing_key();
        let pubkey = crypto::public_key_hex(&sk);
        let ts = "2026-07-04T12:00:00Z";

        let message = dns_bind_statement_bytes("Example.COM.", &pubkey, ts).expect("bind bytes");
        let sig = crypto::sign_message_hex(&sk, &message);
        let record_value = format_dns_txt_record(&pubkey, &sig, ts);
        assert!(record_value.starts_with("v=froglet1; pubkey="));

        let parsed = parse_dns_txt_record(&record_value).expect("parses");
        assert_eq!(parsed.pubkey, pubkey);
        assert_eq!(parsed.ts, ts);
        // Zone normalization: the verifier's casing/dots don't matter.
        verify_dns_txt_record(&parsed, "example.com").expect("verifies");
        verify_dns_txt_record(&parsed, "EXAMPLE.com.").expect("verifies normalized");
        // A different zone breaks the bind.
        assert!(verify_dns_txt_record(&parsed, "evil.com").is_err());
    }

    #[test]
    fn dns_bind_statement_is_deterministic_jcs() {
        let a = dns_bind_statement_bytes("example.com", "ab", "2026-07-04T12:00:00Z").unwrap();
        let b = dns_bind_statement_bytes("EXAMPLE.COM.", "ab", "2026-07-04T12:00:00Z").unwrap();
        assert_eq!(a, b, "normalized zones must produce identical signed bytes");
        let text = String::from_utf8(a).unwrap();
        assert_eq!(
            text,
            r#"{"claim":"dns:example.com","domain":"froglet-identity-bind/v1","subject_pubkey":"ab","ts":"2026-07-04T12:00:00Z"}"#,
            "the JCS shape is a cross-service contract; changing it breaks issued records"
        );
    }

    #[test]
    fn dns_txt_record_parsing_rejects_bad_shapes() {
        assert!(parse_dns_txt_record("v=froglet2; pubkey=a; sig=b; ts=c").is_err());
        assert!(parse_dns_txt_record("pubkey=a; sig=b; ts=c").is_err());
        assert!(parse_dns_txt_record("v=froglet1; sig=b; ts=c").is_err());
        // Unknown fields are ignored (additive growth).
        assert!(parse_dns_txt_record("v=froglet1; pubkey=a; sig=b; ts=c; future=x").is_ok());
    }

    #[test]
    fn dns_zone_normalization_rejects_urls_prefix_and_non_zones() {
        assert!(dns_bind_statement_bytes("https://example.com", "ab", "t").is_err());
        assert!(dns_bind_statement_bytes("_froglet.example.com", "ab", "t").is_err());
        assert!(dns_bind_statement_bytes("localhost", "ab", "t").is_err());
        assert!(dns_bind_statement_bytes("", "ab", "t").is_err());
    }

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
