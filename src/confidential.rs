use aes_gcm_siv::{
    Aes256GcmSiv, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use k256::{
    PublicKey, SecretKey,
    ecdh::diffie_hellman,
    elliptic_curve::{rand_core::OsRng, sec1::ToEncodedPoint},
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::{canonical_json, crypto, protocol};

pub const ARTIFACT_TYPE_CONFIDENTIAL_PROFILE: &str = "confidential_profile";
pub const ARTIFACT_TYPE_CONFIDENTIAL_SESSION: &str = "confidential_session";
pub const ARTIFACT_KIND_CONFIDENTIAL_PROFILE: &str = ARTIFACT_TYPE_CONFIDENTIAL_PROFILE;
pub const ARTIFACT_KIND_CONFIDENTIAL_SESSION: &str = ARTIFACT_TYPE_CONFIDENTIAL_SESSION;

pub const WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1: &str = "confidential.service.v1";
pub const WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1: &str = "compute.wasm.attested.v1";

pub const ENCRYPTED_ENVELOPE_TYPE_V1: &str = "encrypted_envelope";
pub const ENCRYPTED_ENVELOPE_FORMAT_V1: &str = "application/froglet.encrypted-envelope+json";
pub const ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1: &str = "secp256k1_ecdh_aes_256_gcm_v1";

pub const EXECUTION_MODE_TEE: &str = "tee";
pub const ATTESTATION_PLATFORM_NVIDIA: &str = "nvidia";
pub const ATTESTATION_BACKEND_NVIDIA_MOCK_V1: &str = "nvidia.mock.v1";
pub const KEY_RELEASE_PROVIDER_MOCK_EXTERNAL_V1: &str = "mock_external_kms.v1";

const ENVELOPE_DIRECTION_REQUEST: &str = "request";
const ENVELOPE_DIRECTION_RESULT: &str = "result";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedEnvelope {
    pub schema_version: String,
    pub envelope_type: String,
    pub algorithm: String,
    pub confidential_session_hash: String,
    pub direction: String,
    pub payload_format: String,
    pub nonce_b64: String,
    pub ciphertext_b64: String,
}

impl EncryptedEnvelope {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != protocol::FROGLET_SCHEMA_V1 {
            return Err(format!(
                "unsupported encrypted envelope schema_version: {}",
                self.schema_version
            ));
        }
        if self.envelope_type != ENCRYPTED_ENVELOPE_TYPE_V1 {
            return Err(format!(
                "unsupported encrypted envelope type: {}",
                self.envelope_type
            ));
        }
        if self.algorithm != ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1 {
            return Err(format!(
                "unsupported encrypted envelope algorithm: {}",
                self.algorithm
            ));
        }
        validate_hex_field(
            "confidential_session_hash",
            &self.confidential_session_hash,
            32,
        )?;
        if self.direction != ENVELOPE_DIRECTION_REQUEST
            && self.direction != ENVELOPE_DIRECTION_RESULT
        {
            return Err(format!(
                "unsupported encrypted envelope direction: {}",
                self.direction
            ));
        }
        let nonce = BASE64
            .decode(&self.nonce_b64)
            .map_err(|_| "encrypted envelope nonce_b64 must be valid base64".to_string())?;
        if nonce.len() != 12 {
            return Err("encrypted envelope nonce must decode to 12 bytes".to_string());
        }
        let ciphertext = BASE64
            .decode(&self.ciphertext_b64)
            .map_err(|_| "encrypted envelope ciphertext_b64 must be valid base64".to_string())?;
        if ciphertext.len() < 16 {
            return Err("encrypted envelope ciphertext is too short".to_string());
        }
        Ok(())
    }

    pub fn envelope_hash(&self) -> Result<String, String> {
        let encoded = canonical_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(crypto::sha256_hex(encoded))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfidentialProfilePayload {
    pub provider_id: String,
    pub profile_id: String,
    pub service_class: String,
    pub allowed_workload_kind: String,
    pub execution_mode: String,
    pub attestation_platform: String,
    pub measurement: String,
    pub key_release_policy_hash: String,
    pub input_encryption_algorithm: String,
    pub output_encryption_algorithm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_connectors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    pub max_input_bytes: usize,
    pub max_runtime_ms: u64,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfidentialSessionPayload {
    pub provider_id: String,
    pub requester_id: String,
    pub session_id: String,
    pub confidential_profile_hash: String,
    pub allowed_workload_kind: String,
    pub execution_mode: String,
    pub attestation_platform: String,
    pub measurement: String,
    pub attestation_evidence_hash: String,
    pub key_release_policy_hash: String,
    pub session_public_key: String,
    pub requester_public_key: String,
    pub encryption_algorithm: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttestationBundle {
    pub schema_version: String,
    pub platform: String,
    pub backend: String,
    pub measurement: String,
    pub session_public_key: String,
    pub key_release_policy_hash: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub claims: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyReleaseEvidence {
    pub schema_version: String,
    pub provider: String,
    pub confidential_session_hash: String,
    pub policy_hash: String,
    pub released_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionPrivateMaterial {
    pub confidential_session_hash: String,
    pub confidential_profile_hash: String,
    pub session_id: String,
    pub session_private_key: String,
    pub session_public_key: String,
    pub requester_public_key: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidentialSessionOpenRequest {
    pub requester_id: String,
    pub confidential_profile_hash: String,
    pub allowed_workload_kind: String,
    pub requester_public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidentialSessionOpenResponse {
    pub profile: crate::protocol::SignedArtifact<ConfidentialProfilePayload>,
    pub session: crate::protocol::SignedArtifact<ConfidentialSessionPayload>,
    pub attestation: AttestationBundle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidentialConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_path: Option<PathBuf>,
    #[serde(skip_serializing, skip_deserializing)]
    pub policy: Option<ConfidentialPolicy>,
    pub session_ttl_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidentialPolicy {
    pub backend: ConfidentialBackendConfig,
    pub verifier: AttestationVerifierConfig,
    pub key_release: KeyReleaseConfig,
    #[serde(default)]
    pub connectors: BTreeMap<String, ConfidentialConnectorConfig>,
    #[serde(default)]
    pub services: BTreeMap<String, ConfidentialServiceConfig>,
    #[serde(default)]
    pub profiles: BTreeMap<String, ConfidentialProfileConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidentialBackendConfig {
    pub kind: String,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationVerifierConfig {
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyReleaseConfig {
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidentialProfileConfig {
    pub offer_id: String,
    pub allowed_workload_kind: String,
    pub measurement: String,
    pub key_release_policy_hash: String,
    #[serde(default = "default_execution_mode")]
    pub execution_mode: String,
    #[serde(default = "default_attestation_platform")]
    pub attestation_platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(default)]
    pub allowed_connectors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default = "default_confidential_max_input_bytes")]
    pub max_input_bytes: usize,
    #[serde(default = "default_confidential_max_runtime_ms")]
    pub max_runtime_ms: u64,
    #[serde(default = "default_confidential_max_output_bytes")]
    pub max_output_bytes: usize,
    #[serde(default)]
    pub price_sats: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terms_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidentialServiceConfig {
    pub profile: String,
    pub handler: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector: Option<String>,
    #[serde(default = "default_confidential_max_search_results")]
    pub max_results: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfidentialConnectorConfig {
    InlineJson {
        #[serde(default)]
        documents: Vec<Value>,
    },
    SqliteSearch {
        path: PathBuf,
        table: String,
        columns: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct ConfidentialExecutionContext<'a> {
    pub confidential_session_hash: &'a str,
    pub now: i64,
}

pub trait AttestationProvider {
    fn issue_attestation(
        &self,
        profile: &ConfidentialProfilePayload,
        session_public_key: &str,
        issued_at: i64,
        expires_at: i64,
    ) -> Result<AttestationBundle, String>;
}

pub trait KeyReleaseProvider {
    fn release_key(
        &self,
        confidential_session_hash: &str,
        session: &ConfidentialSessionPayload,
        attestation: &AttestationBundle,
        released_at: i64,
    ) -> Result<KeyReleaseEvidence, String>;
}

pub trait ConfidentialExecutor {
    fn execute_service(
        &self,
        service_id: &str,
        input: Value,
        context: &ConfidentialExecutionContext<'_>,
    ) -> Result<Value, String>;
}

pub trait ServiceConnector {
    fn execute_search(&self, input: &Value, max_results: usize, now: i64) -> Result<Value, String>;
}

#[derive(Debug, Clone)]
pub struct NvidiaMockAttestationProvider;

impl AttestationProvider for NvidiaMockAttestationProvider {
    fn issue_attestation(
        &self,
        profile: &ConfidentialProfilePayload,
        session_public_key: &str,
        issued_at: i64,
        expires_at: i64,
    ) -> Result<AttestationBundle, String> {
        Ok(AttestationBundle {
            schema_version: protocol::FROGLET_SCHEMA_V1.to_string(),
            platform: profile.attestation_platform.clone(),
            backend: ATTESTATION_BACKEND_NVIDIA_MOCK_V1.to_string(),
            measurement: profile.measurement.clone(),
            session_public_key: session_public_key.to_string(),
            key_release_policy_hash: profile.key_release_policy_hash.clone(),
            issued_at,
            expires_at,
            claims: json!({
                "vendor": "nvidia",
                "execution_mode": profile.execution_mode,
                "session_public_key": session_public_key,
            }),
        })
    }
}

#[derive(Debug, Clone)]
pub struct MockExternalKeyReleaseProvider;

impl KeyReleaseProvider for MockExternalKeyReleaseProvider {
    fn release_key(
        &self,
        confidential_session_hash: &str,
        session: &ConfidentialSessionPayload,
        attestation: &AttestationBundle,
        released_at: i64,
    ) -> Result<KeyReleaseEvidence, String> {
        if attestation.platform != session.attestation_platform {
            return Err("attestation platform does not match confidential session".to_string());
        }
        if attestation.measurement != session.measurement {
            return Err("attestation measurement does not match confidential session".to_string());
        }
        if released_at > session.expires_at {
            return Err("confidential session already expired".to_string());
        }
        Ok(KeyReleaseEvidence {
            schema_version: protocol::FROGLET_SCHEMA_V1.to_string(),
            provider: KEY_RELEASE_PROVIDER_MOCK_EXTERNAL_V1.to_string(),
            confidential_session_hash: confidential_session_hash.to_string(),
            policy_hash: session.key_release_policy_hash.clone(),
            released_at,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PolicyConfidentialExecutor {
    pub policy: ConfidentialPolicy,
}

impl ConfidentialExecutor for PolicyConfidentialExecutor {
    fn execute_service(
        &self,
        service_id: &str,
        input: Value,
        context: &ConfidentialExecutionContext<'_>,
    ) -> Result<Value, String> {
        let Some(service) = self.policy.services.get(service_id) else {
            return Err(format!("unknown confidential service: {service_id}"));
        };
        match service.handler.as_str() {
            "echo" => Ok(json!({
                "service_id": service_id,
                "confidential_session_hash": context.confidential_session_hash,
                "executed_at": context.now,
                "input": input,
            })),
            "json_search" => {
                let connector_name = service.connector.as_deref().ok_or_else(|| {
                    format!("confidential service {service_id} is missing connector")
                })?;
                let connector = connector_from_policy(&self.policy, connector_name)?;
                connector.execute_search(&input, service.max_results, context.now)
            }
            other => Err(format!("unsupported confidential service handler: {other}")),
        }
    }
}

pub fn default_execution_mode() -> String {
    EXECUTION_MODE_TEE.to_string()
}

pub fn default_attestation_platform() -> String {
    ATTESTATION_PLATFORM_NVIDIA.to_string()
}

fn default_confidential_max_input_bytes() -> usize {
    256 * 1024
}

fn default_confidential_max_runtime_ms() -> u64 {
    10_000
}

fn default_confidential_max_output_bytes() -> usize {
    256 * 1024
}

fn default_confidential_max_search_results() -> usize {
    20
}

pub fn load_policy(path: &Path, internal_db_path: &Path) -> Result<ConfidentialPolicy, String> {
    let document = fs::read_to_string(path).map_err(|error| {
        format!(
            "Failed to read FROGLET_CONFIDENTIAL_POLICY_PATH {}: {error}",
            path.display()
        )
    })?;
    let policy: ConfidentialPolicy = toml::from_str(&document).map_err(|error| {
        format!(
            "Failed to parse FROGLET_CONFIDENTIAL_POLICY_PATH {}: {error}",
            path.display()
        )
    })?;
    validate_policy(&policy, internal_db_path)?;
    Ok(policy)
}

pub fn validate_policy(policy: &ConfidentialPolicy, internal_db_path: &Path) -> Result<(), String> {
    if policy.backend.kind.trim().is_empty() {
        return Err("confidential backend kind must not be empty".to_string());
    }
    if policy.backend.platform.trim().is_empty() {
        return Err("confidential backend platform must not be empty".to_string());
    }
    if policy.verifier.mode.trim().is_empty() {
        return Err("confidential verifier mode must not be empty".to_string());
    }
    if policy.key_release.provider.trim().is_empty() {
        return Err("confidential key_release provider must not be empty".to_string());
    }
    if policy.profiles.is_empty() {
        return Err("confidential policy must define at least one profile".to_string());
    }

    for (profile_id, profile) in &policy.profiles {
        validate_policy_name("confidential profile", profile_id)?;
        validate_policy_name("confidential offer_id", &profile.offer_id)?;
        validate_measurement(&profile.measurement)?;
        validate_hex_field(
            "key_release_policy_hash",
            &profile.key_release_policy_hash,
            32,
        )?;
        match profile.allowed_workload_kind.as_str() {
            WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1 | WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1 => {}
            other => {
                return Err(format!(
                    "confidential profile '{profile_id}' has unsupported allowed_workload_kind '{other}'"
                ));
            }
        }
        if profile.max_input_bytes == 0 {
            return Err(format!(
                "confidential profile '{profile_id}' max_input_bytes must be greater than zero"
            ));
        }
        if profile.max_runtime_ms == 0 {
            return Err(format!(
                "confidential profile '{profile_id}' max_runtime_ms must be greater than zero"
            ));
        }
        if profile.max_output_bytes == 0 {
            return Err(format!(
                "confidential profile '{profile_id}' max_output_bytes must be greater than zero"
            ));
        }
        for connector in &profile.allowed_connectors {
            validate_policy_name("confidential connector reference", connector)?;
            if !policy.connectors.contains_key(connector) {
                return Err(format!(
                    "confidential profile '{profile_id}' references unknown connector '{connector}'"
                ));
            }
        }
        if profile.allowed_workload_kind == WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1
            && profile.service_id.is_none()
        {
            return Err(format!(
                "confidential profile '{profile_id}' requires service_id for confidential.service.v1"
            ));
        }
    }

    for (service_id, service) in &policy.services {
        validate_policy_name("confidential service", service_id)?;
        if !policy.profiles.contains_key(&service.profile) {
            return Err(format!(
                "confidential service '{service_id}' references unknown profile '{}'",
                service.profile
            ));
        }
        if service.max_results == 0 {
            return Err(format!(
                "confidential service '{service_id}' max_results must be greater than zero"
            ));
        }
        if let Some(connector) = service.connector.as_deref() {
            validate_policy_name("confidential connector", connector)?;
            if !policy.connectors.contains_key(connector) {
                return Err(format!(
                    "confidential service '{service_id}' references unknown connector '{connector}'"
                ));
            }
        }
    }

    let normalized_internal_db_path = normalize_path_for_comparison(internal_db_path)?;
    for (connector_name, connector) in &policy.connectors {
        validate_policy_name("confidential connector", connector_name)?;
        match connector {
            ConfidentialConnectorConfig::InlineJson { .. } => {}
            ConfidentialConnectorConfig::SqliteSearch {
                path,
                table,
                columns,
            } => {
                if table.trim().is_empty() {
                    return Err(format!(
                        "confidential sqlite connector '{connector_name}' table must not be empty"
                    ));
                }
                if columns.is_empty() {
                    return Err(format!(
                        "confidential sqlite connector '{connector_name}' must list at least one column"
                    ));
                }
                if !path.exists() {
                    return Err(format!(
                        "confidential sqlite connector '{connector_name}' path does not exist: {}",
                        path.display()
                    ));
                }
                let normalized_path = normalize_path_for_comparison(path)?;
                if normalized_path == normalized_internal_db_path {
                    return Err(format!(
                        "confidential sqlite connector '{connector_name}' must not reference Froglet's internal database"
                    ));
                }
            }
        }
    }

    Ok(())
}

pub fn profile_payload_from_config(
    provider_id: &str,
    profile_id: &str,
    profile: &ConfidentialProfileConfig,
) -> ConfidentialProfilePayload {
    ConfidentialProfilePayload {
        provider_id: provider_id.to_string(),
        profile_id: profile_id.to_string(),
        service_class: profile.allowed_workload_kind.clone(),
        allowed_workload_kind: profile.allowed_workload_kind.clone(),
        execution_mode: profile.execution_mode.clone(),
        attestation_platform: profile.attestation_platform.clone(),
        measurement: profile.measurement.clone(),
        key_release_policy_hash: profile.key_release_policy_hash.clone(),
        input_encryption_algorithm: ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1.to_string(),
        output_encryption_algorithm: ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1.to_string(),
        service_id: profile.service_id.clone(),
        allowed_connectors: profile.allowed_connectors.clone(),
        service_schema: profile.service_schema.clone(),
        output_schema: profile.output_schema.clone(),
        max_input_bytes: profile.max_input_bytes,
        max_runtime_ms: profile.max_runtime_ms,
        max_output_bytes: profile.max_output_bytes,
    }
}

pub fn generate_keypair() -> (String, String) {
    let secret_key = SecretKey::random(&mut OsRng);
    let private_hex = hex::encode(secret_key.to_bytes());
    let public_hex = hex::encode(secret_key.public_key().to_encoded_point(true).as_bytes());
    (private_hex, public_hex)
}

pub fn encrypt_request_envelope<T: Serialize>(
    confidential_session_hash: &str,
    requester_private_key: &str,
    session_public_key: &str,
    payload: &T,
    payload_format: &str,
) -> Result<EncryptedEnvelope, String> {
    encrypt_envelope(
        confidential_session_hash,
        requester_private_key,
        session_public_key,
        ENVELOPE_DIRECTION_REQUEST,
        payload_format,
        payload,
    )
}

pub fn encrypt_result_envelope<T: Serialize>(
    confidential_session_hash: &str,
    session_private_key: &str,
    requester_public_key: &str,
    payload: &T,
    payload_format: &str,
) -> Result<EncryptedEnvelope, String> {
    encrypt_envelope(
        confidential_session_hash,
        session_private_key,
        requester_public_key,
        ENVELOPE_DIRECTION_RESULT,
        payload_format,
        payload,
    )
}

pub fn decrypt_request_envelope<T: DeserializeOwned>(
    confidential_session_hash: &str,
    session_private_key: &str,
    requester_public_key: &str,
    envelope: &EncryptedEnvelope,
) -> Result<T, String> {
    decrypt_envelope(
        confidential_session_hash,
        session_private_key,
        requester_public_key,
        ENVELOPE_DIRECTION_REQUEST,
        envelope,
    )
}

pub fn decrypt_result_envelope<T: DeserializeOwned>(
    confidential_session_hash: &str,
    requester_private_key: &str,
    session_public_key: &str,
    envelope: &EncryptedEnvelope,
) -> Result<T, String> {
    decrypt_envelope(
        confidential_session_hash,
        requester_private_key,
        session_public_key,
        ENVELOPE_DIRECTION_RESULT,
        envelope,
    )
}

pub fn verify_attestation_bundle(
    profile: &ConfidentialProfilePayload,
    session: &crate::protocol::SignedArtifact<ConfidentialSessionPayload>,
    attestation: &AttestationBundle,
    now: i64,
) -> Result<(), String> {
    if session.payload.confidential_profile_hash.is_empty() {
        return Err("confidential session missing confidential_profile_hash".to_string());
    }
    if session.payload.allowed_workload_kind != profile.allowed_workload_kind {
        return Err("confidential session workload kind does not match profile".to_string());
    }
    if session.payload.attestation_platform != profile.attestation_platform {
        return Err("confidential session platform does not match profile".to_string());
    }
    if session.payload.measurement != profile.measurement {
        return Err("confidential session measurement does not match profile".to_string());
    }
    if session.payload.key_release_policy_hash != profile.key_release_policy_hash {
        return Err(
            "confidential session key_release_policy_hash does not match profile".to_string(),
        );
    }
    if now > session.payload.expires_at || now > attestation.expires_at {
        return Err("confidential session attestation is expired".to_string());
    }
    if attestation.platform != profile.attestation_platform {
        return Err("attestation platform does not match confidential profile".to_string());
    }
    if attestation.measurement != profile.measurement {
        return Err("attestation measurement does not match confidential profile".to_string());
    }
    if attestation.session_public_key != session.payload.session_public_key {
        return Err(
            "attestation session_public_key does not match confidential session".to_string(),
        );
    }
    if attestation.key_release_policy_hash != profile.key_release_policy_hash {
        return Err(
            "attestation key_release_policy_hash does not match confidential profile".to_string(),
        );
    }
    let attestation_hash = attestation_hash(attestation)?;
    if attestation_hash != session.payload.attestation_evidence_hash {
        return Err("attestation evidence hash does not match confidential session".to_string());
    }
    Ok(())
}

pub fn attestation_hash(attestation: &AttestationBundle) -> Result<String, String> {
    let bytes = canonical_json::to_vec(attestation).map_err(|error| error.to_string())?;
    Ok(crypto::sha256_hex(bytes))
}

fn encrypt_envelope<T: Serialize>(
    confidential_session_hash: &str,
    sender_private_key: &str,
    recipient_public_key: &str,
    direction: &str,
    payload_format: &str,
    payload: &T,
) -> Result<EncryptedEnvelope, String> {
    validate_hex_field("confidential_session_hash", confidential_session_hash, 32)?;
    let plaintext = canonical_json::to_vec(payload).map_err(|error| error.to_string())?;
    let key = derive_shared_aead_key(
        sender_private_key,
        recipient_public_key,
        confidential_session_hash,
        direction,
    )?;
    let mut nonce_bytes = [0u8; 12];
    use rand::RngCore as _;
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let cipher = Aes256GcmSiv::new_from_slice(&key).map_err(|error| error.to_string())?;
    let aad = envelope_aad(confidential_session_hash, direction, payload_format)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| "failed to encrypt confidential envelope".to_string())?;
    Ok(EncryptedEnvelope {
        schema_version: protocol::FROGLET_SCHEMA_V1.to_string(),
        envelope_type: ENCRYPTED_ENVELOPE_TYPE_V1.to_string(),
        algorithm: ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1.to_string(),
        confidential_session_hash: confidential_session_hash.to_string(),
        direction: direction.to_string(),
        payload_format: payload_format.to_string(),
        nonce_b64: BASE64.encode(nonce_bytes),
        ciphertext_b64: BASE64.encode(ciphertext),
    })
}

fn decrypt_envelope<T: DeserializeOwned>(
    confidential_session_hash: &str,
    recipient_private_key: &str,
    sender_public_key: &str,
    direction: &str,
    envelope: &EncryptedEnvelope,
) -> Result<T, String> {
    envelope.validate()?;
    if envelope.confidential_session_hash != confidential_session_hash {
        return Err("encrypted envelope confidential_session_hash does not match".to_string());
    }
    if envelope.direction != direction {
        return Err("encrypted envelope direction does not match expected direction".to_string());
    }
    let key = derive_shared_aead_key(
        recipient_private_key,
        sender_public_key,
        confidential_session_hash,
        direction,
    )?;
    let nonce = BASE64
        .decode(&envelope.nonce_b64)
        .map_err(|_| "failed to decode encrypted envelope nonce".to_string())?;
    let ciphertext = BASE64
        .decode(&envelope.ciphertext_b64)
        .map_err(|_| "failed to decode encrypted envelope ciphertext".to_string())?;
    let aad = envelope_aad(
        &envelope.confidential_session_hash,
        &envelope.direction,
        &envelope.payload_format,
    )?;
    let cipher = Aes256GcmSiv::new_from_slice(&key).map_err(|error| error.to_string())?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| "failed to decrypt confidential envelope".to_string())?;
    serde_json::from_slice(&plaintext).map_err(|error| error.to_string())
}

fn derive_shared_aead_key(
    private_key_hex: &str,
    peer_public_key_hex: &str,
    confidential_session_hash: &str,
    direction: &str,
) -> Result<[u8; 32], String> {
    let private_key = parse_private_key(private_key_hex)?;
    let public_key = parse_public_key(peer_public_key_hex)?;
    let secret = diffie_hellman(private_key.to_nonzero_scalar(), public_key.as_affine());
    let mut digest = Sha256::new();
    digest.update(b"froglet.confidential.v1");
    digest.update(secret.raw_secret_bytes());
    digest.update(confidential_session_hash.as_bytes());
    digest.update(direction.as_bytes());
    let output = digest.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&output);
    Ok(key)
}

fn envelope_aad(
    confidential_session_hash: &str,
    direction: &str,
    payload_format: &str,
) -> Result<Vec<u8>, String> {
    canonical_json::to_vec(&json!([
        protocol::FROGLET_SCHEMA_V1,
        ENCRYPTED_ENVELOPE_TYPE_V1,
        ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1,
        confidential_session_hash,
        direction,
        payload_format,
    ]))
    .map_err(|error| error.to_string())
}

fn parse_private_key(private_key_hex: &str) -> Result<SecretKey, String> {
    let normalized = validate_hex_field("private_key", private_key_hex, 32)?;
    SecretKey::from_slice(&hex::decode(normalized).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn parse_public_key(public_key_hex: &str) -> Result<PublicKey, String> {
    let normalized = public_key_hex.trim().to_lowercase();
    if normalized.is_empty() {
        return Err("public_key must be a non-empty hex string".to_string());
    }
    let bytes = hex::decode(&normalized).map_err(|_| "public_key must be valid hex".to_string())?;
    PublicKey::from_sec1_bytes(&bytes).map_err(|error| error.to_string())
}

fn validate_measurement(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("measurement must not be empty".to_string());
    }
    let normalized = value.trim().to_lowercase();
    if !normalized.chars().all(|ch| ch.is_ascii_hexdigit()) || normalized.len() < 32 {
        return Err(format!(
            "measurement must be lowercase hex with at least 32 characters, got '{value}'"
        ));
    }
    Ok(())
}

pub fn validate_hex_field<'a>(
    field_name: &str,
    value: &'a str,
    expected_bytes: usize,
) -> Result<&'a str, String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(format!("{field_name} must not be empty"));
    }
    if normalized.len() != expected_bytes * 2
        || !normalized.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return Err(format!(
            "{field_name} must be {expected_bytes} bytes of lowercase hex"
        ));
    }
    Ok(normalized)
}

pub fn validate_public_key_hex(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_lowercase();
    parse_public_key(&normalized)?;
    Ok(normalized)
}

fn connector_from_policy(
    policy: &ConfidentialPolicy,
    connector_name: &str,
) -> Result<Box<dyn ServiceConnector>, String> {
    let Some(connector) = policy.connectors.get(connector_name).cloned() else {
        return Err(format!("unknown confidential connector: {connector_name}"));
    };
    match connector {
        ConfidentialConnectorConfig::InlineJson { documents } => {
            Ok(Box::new(InlineJsonSearchConnector { documents }))
        }
        ConfidentialConnectorConfig::SqliteSearch {
            path,
            table,
            columns,
        } => Ok(Box::new(SqliteSearchConnector {
            path,
            table,
            columns,
        })),
    }
}

#[derive(Debug, Clone)]
struct InlineJsonSearchConnector {
    documents: Vec<Value>,
}

impl ServiceConnector for InlineJsonSearchConnector {
    fn execute_search(&self, input: &Value, max_results: usize, now: i64) -> Result<Value, String> {
        let (query, limit) = parse_search_input(input, max_results)?;
        let query_lower = query.to_lowercase();
        let matches = self
            .documents
            .iter()
            .filter(|document| document_matches_query(document, &query_lower))
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        Ok(json!({
            "query": query,
            "returned": matches.len(),
            "executed_at": now,
            "matches": matches,
        }))
    }
}

#[derive(Debug, Clone)]
struct SqliteSearchConnector {
    path: PathBuf,
    table: String,
    columns: Vec<String>,
}

impl ServiceConnector for SqliteSearchConnector {
    fn execute_search(&self, input: &Value, max_results: usize, now: i64) -> Result<Value, String> {
        let (query, limit) = parse_search_input(input, max_results)?;
        let like_pattern = format!("%{}%", query.to_lowercase());
        let select_columns = self
            .columns
            .iter()
            .map(|column| format!("CAST({column} AS TEXT) AS {column}"))
            .collect::<Vec<_>>()
            .join(", ");
        let where_clause = self
            .columns
            .iter()
            .map(|column| format!("LOWER(CAST({column} AS TEXT)) LIKE ?1"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT {select_columns} FROM {} WHERE {where_clause} LIMIT {}",
            self.table, limit
        );
        let conn = Connection::open(&self.path).map_err(|error| error.to_string())?;
        let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
        let column_count = self.columns.len();
        let rows = statement
            .query_map([like_pattern], |row| {
                let mut object = serde_json::Map::new();
                for index in 0..column_count {
                    let value: Option<String> = row.get(index)?;
                    object.insert(
                        self.columns[index].clone(),
                        value.map(Value::String).unwrap_or(Value::Null),
                    );
                }
                Ok(Value::Object(object))
            })
            .map_err(|error| error.to_string())?;
        let mut matches = Vec::new();
        for row in rows {
            matches.push(row.map_err(|error| error.to_string())?);
        }
        Ok(json!({
            "query": query,
            "returned": matches.len(),
            "executed_at": now,
            "matches": matches,
        }))
    }
}

fn parse_search_input(input: &Value, max_results: usize) -> Result<(String, usize), String> {
    let Some(object) = input.as_object() else {
        return Err("confidential json_search input must be an object".to_string());
    };
    let Some(query) = object.get("query").and_then(Value::as_str) else {
        return Err("confidential json_search input requires string field 'query'".to_string());
    };
    let query = query.trim().to_string();
    if query.is_empty() {
        return Err("confidential json_search query must not be empty".to_string());
    }
    let limit = object
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(max_results)
        .clamp(1, max_results.max(1));
    Ok((query, limit))
}

fn document_matches_query(document: &Value, query_lower: &str) -> bool {
    match document {
        Value::Null => false,
        Value::Bool(value) => value.to_string().contains(query_lower),
        Value::Number(value) => value.to_string().to_lowercase().contains(query_lower),
        Value::String(value) => value.to_lowercase().contains(query_lower),
        Value::Array(values) => values
            .iter()
            .any(|value| document_matches_query(value, query_lower)),
        Value::Object(map) => map
            .values()
            .any(|value| document_matches_query(value, query_lower)),
    }
}

fn validate_policy_name(kind: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_' | '.')
        })
    {
        return Err(format!(
            "Invalid {kind} name '{value}'. Allowed characters: lowercase ascii letters, digits, '-', '_' and '.'"
        ));
    }
    Ok(())
}

fn normalize_path_for_comparison(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("failed to resolve current working directory: {error}"))?
            .join(path)
    };

    let mut existing_prefix = absolute.as_path();
    let mut missing_components = Vec::new();
    while !existing_prefix.exists() {
        let component = existing_prefix.file_name().ok_or_else(|| {
            format!(
                "failed to normalize path '{}': no existing parent directory found",
                absolute.display()
            )
        })?;
        missing_components.push(component.to_os_string());
        existing_prefix = existing_prefix.parent().ok_or_else(|| {
            format!(
                "failed to normalize path '{}': no existing parent directory found",
                absolute.display()
            )
        })?;
    }

    let mut normalized = fs::canonicalize(existing_prefix).map_err(|error| {
        format!(
            "failed to canonicalize path prefix '{}': {error}",
            existing_prefix.display()
        )
    })?;
    while let Some(component) = missing_components.pop() {
        normalized.push(component);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip_uses_shared_confidential_session_hash() {
        let (session_private_key, session_public_key) = generate_keypair();
        let (requester_private_key, requester_public_key) = generate_keypair();
        let confidential_session_hash = &"ab".repeat(32);
        let payload = json!({
            "query": "froglet",
            "limit": 2
        });

        let envelope = encrypt_request_envelope(
            confidential_session_hash,
            &requester_private_key,
            &session_public_key,
            &payload,
            crate::wasm::JCS_JSON_FORMAT,
        )
        .expect("request envelope");

        let decrypted: Value = decrypt_request_envelope(
            confidential_session_hash,
            &session_private_key,
            &requester_public_key,
            &envelope,
        )
        .expect("decrypt request envelope");

        assert_eq!(decrypted, payload);
        assert_eq!(envelope.direction, ENVELOPE_DIRECTION_REQUEST);
    }

    #[test]
    fn inline_json_search_connector_filters_matching_documents() {
        let connector = InlineJsonSearchConnector {
            documents: vec![
                json!({"name": "alpha", "note": "froglet"}),
                json!({"name": "beta", "note": "other"}),
            ],
        };

        let result = connector
            .execute_search(&json!({"query": "frog", "limit": 10}), 10, 1_700_000_000)
            .expect("search result");

        assert_eq!(result["returned"], 1);
        assert_eq!(result["matches"][0]["name"], "alpha");
    }

    #[test]
    fn verify_attestation_rejects_wrong_measurement() {
        let profile = ConfidentialProfilePayload {
            provider_id: "provider".to_string(),
            profile_id: "main".to_string(),
            service_class: WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1.to_string(),
            allowed_workload_kind: WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1.to_string(),
            execution_mode: EXECUTION_MODE_TEE.to_string(),
            attestation_platform: ATTESTATION_PLATFORM_NVIDIA.to_string(),
            measurement: "aa".repeat(16),
            key_release_policy_hash: "bb".repeat(32),
            input_encryption_algorithm: ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1.to_string(),
            output_encryption_algorithm: ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1.to_string(),
            service_id: Some("search".to_string()),
            allowed_connectors: Vec::new(),
            service_schema: None,
            output_schema: None,
            max_input_bytes: 1024,
            max_runtime_ms: 1_000,
            max_output_bytes: 1024,
        };

        let session_payload = ConfidentialSessionPayload {
            provider_id: "provider".to_string(),
            requester_id: "requester".to_string(),
            session_id: "session".to_string(),
            confidential_profile_hash: "cc".repeat(32),
            allowed_workload_kind: WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1.to_string(),
            execution_mode: EXECUTION_MODE_TEE.to_string(),
            attestation_platform: ATTESTATION_PLATFORM_NVIDIA.to_string(),
            measurement: "ff".repeat(16),
            attestation_evidence_hash: "00".repeat(32),
            key_release_policy_hash: "bb".repeat(32),
            session_public_key: generate_keypair().1,
            requester_public_key: generate_keypair().1,
            encryption_algorithm: ENCRYPTION_ALGORITHM_SECP256K1_AES_256_GCM_V1.to_string(),
            expires_at: 1_800_000_000,
        };
        let session = crate::protocol::SignedArtifact {
            artifact_type: ARTIFACT_TYPE_CONFIDENTIAL_SESSION.to_string(),
            schema_version: protocol::FROGLET_SCHEMA_V1.to_string(),
            signer: "provider".to_string(),
            created_at: 1_700_000_000,
            payload_hash: "11".repeat(32),
            hash: "22".repeat(32),
            payload: session_payload,
            signature: "33".repeat(64),
        };
        let attestation = AttestationBundle {
            schema_version: protocol::FROGLET_SCHEMA_V1.to_string(),
            platform: ATTESTATION_PLATFORM_NVIDIA.to_string(),
            backend: ATTESTATION_BACKEND_NVIDIA_MOCK_V1.to_string(),
            measurement: "ff".repeat(16),
            session_public_key: session.payload.session_public_key.clone(),
            key_release_policy_hash: "bb".repeat(32),
            issued_at: 1_700_000_000,
            expires_at: 1_800_000_000,
            claims: json!({}),
        };

        let error = verify_attestation_bundle(&profile, &session, &attestation, 1_700_000_001)
            .expect_err("expected measurement mismatch");
        assert!(error.contains("measurement"));
    }
}
