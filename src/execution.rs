use crate::{
    canonical_json,
    confidential::{
        EncryptedEnvelope, WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1,
        WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1,
    },
    crypto,
    wasm::{self, ComputeWasmWorkload, OciWasmSubmission, OciWasmWorkload, WasmSubmission},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const WORKLOAD_KIND_EXECUTION_V1: &str = "compute.execution.v1";
pub const CONTRACT_BUILTIN_EVENTS_QUERY_V1: &str = "froglet.builtin.events_query.v1";
pub const CONTRACT_CONTAINER_JSON_V1: &str = "froglet.container.stdin_json.v1";
pub const CONTRACT_PYTHON_HANDLER_JSON_V1: &str = "froglet.python.handler_json.v1";
pub const CONTRACT_PYTHON_SCRIPT_JSON_V1: &str = "froglet.python.script_json.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRuntime {
    Any,
    Wasm,
    Python,
    Container,
    Builtin,
    TeeService,
    TeeWasm,
    TeePython,
}

impl ExecutionRuntime {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "any" => Ok(Self::Any),
            "wasm" => Ok(Self::Wasm),
            "python" => Ok(Self::Python),
            "container" => Ok(Self::Container),
            "builtin" => Ok(Self::Builtin),
            "tee.service" | "tee_service" => Ok(Self::TeeService),
            "tee.wasm" | "tee_wasm" => Ok(Self::TeeWasm),
            "tee.python" | "tee_python" => Ok(Self::TeePython),
            other => Err(format!("unsupported runtime: {other}")),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Wasm => "wasm",
            Self::Python => "python",
            Self::Container => "container",
            Self::Builtin => "builtin",
            Self::TeeService => "tee.service",
            Self::TeeWasm => "tee.wasm",
            Self::TeePython => "tee.python",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPackageKind {
    InlineModule,
    InlineSource,
    OciImage,
    Builtin,
}

impl ExecutionPackageKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "inline_module" => Ok(Self::InlineModule),
            "inline_source" => Ok(Self::InlineSource),
            "oci_image" => Ok(Self::OciImage),
            "builtin" => Ok(Self::Builtin),
            other => Err(format!("unsupported package_kind: {other}")),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InlineModule => "inline_module",
            Self::InlineSource => "inline_source",
            Self::OciImage => "oci_image",
            Self::Builtin => "builtin",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionEntrypointKind {
    Handler,
    Script,
    Builtin,
}

impl ExecutionEntrypointKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "handler" => Ok(Self::Handler),
            "script" => Ok(Self::Script),
            "builtin" => Ok(Self::Builtin),
            other => Err(format!("unsupported entrypoint_kind: {other}")),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Handler => "handler",
            Self::Script => "script",
            Self::Builtin => "builtin",
        }
    }
}

pub fn default_entrypoint_kind_for(runtime: &ExecutionRuntime) -> ExecutionEntrypointKind {
    match runtime {
        ExecutionRuntime::Builtin => ExecutionEntrypointKind::Builtin,
        _ => ExecutionEntrypointKind::Handler,
    }
}

pub fn default_entrypoint_for(
    runtime: &ExecutionRuntime,
    entrypoint_kind: &ExecutionEntrypointKind,
) -> &'static str {
    match (runtime, entrypoint_kind) {
        (ExecutionRuntime::Builtin, _) => "events.query",
        (ExecutionRuntime::Any, _) => "",
        (_, ExecutionEntrypointKind::Script) => "__main__",
        (ExecutionRuntime::Python, _) | (ExecutionRuntime::TeePython, _) => "handler",
        _ => "run",
    }
}

pub fn default_contract_version_for(
    runtime: &ExecutionRuntime,
    package_kind: &ExecutionPackageKind,
    entrypoint_kind: &ExecutionEntrypointKind,
) -> &'static str {
    match (runtime, package_kind, entrypoint_kind) {
        (ExecutionRuntime::Any, _, _) => "",
        (
            ExecutionRuntime::Python,
            ExecutionPackageKind::InlineSource,
            ExecutionEntrypointKind::Script,
        )
        | (
            ExecutionRuntime::TeePython,
            ExecutionPackageKind::InlineSource,
            ExecutionEntrypointKind::Script,
        ) => CONTRACT_PYTHON_SCRIPT_JSON_V1,
        (ExecutionRuntime::Python, ExecutionPackageKind::InlineSource, _)
        | (ExecutionRuntime::TeePython, ExecutionPackageKind::InlineSource, _) => {
            CONTRACT_PYTHON_HANDLER_JSON_V1
        }
        (ExecutionRuntime::Container, ExecutionPackageKind::OciImage, _)
        | (ExecutionRuntime::Python, ExecutionPackageKind::OciImage, _) => {
            CONTRACT_CONTAINER_JSON_V1
        }
        (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin, _) => {
            CONTRACT_BUILTIN_EVENTS_QUERY_V1
        }
        _ => wasm::WASM_RUN_JSON_ABI_V1,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionEntrypoint {
    pub kind: ExecutionEntrypointKind,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSecurityMode {
    Standard,
    Tee,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionSecurity {
    pub mode: ExecutionSecurityMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidential_session_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_envelope: Option<EncryptedEnvelope>,
}

impl Default for ExecutionSecurity {
    fn default() -> Self {
        Self {
            mode: ExecutionSecurityMode::Standard,
            confidential_session_hash: None,
            service_id: None,
            request_envelope: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionMount {
    pub handle: String,
    pub kind: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionWorkload {
    pub schema_version: String,
    pub workload_kind: String,
    pub runtime: ExecutionRuntime,
    pub package_kind: ExecutionPackageKind,
    pub entrypoint: ExecutionEntrypoint,
    pub contract_version: String,
    pub input_format: String,
    pub input_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested_access: Vec<String>,
    #[serde(default)]
    pub security: ExecutionSecurity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<ExecutionMount>,
    #[serde(default = "default_input_value")]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_bytes_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin_name: Option<String>,
}

fn default_input_value() -> Value {
    Value::Null
}

pub fn digest_pinned_oci_image_reference(
    oci_reference: &str,
    oci_digest: &str,
) -> Result<String, String> {
    let digest = oci_digest.trim().to_ascii_lowercase();
    if digest.len() != 64
        || !digest
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err("oci_digest must be a 64-character sha256 hex string".to_string());
    }

    let trimmed = oci_reference.trim();
    if trimmed.is_empty() {
        return Err("oci_reference must not be empty".to_string());
    }
    if trimmed.contains(['?', '#']) {
        return Err("oci_reference must not include query or fragment components".to_string());
    }

    let reference = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let last_slash = reference.rfind('/').ok_or_else(|| {
        "oci_reference must include a registry host and repository path".to_string()
    })?;
    if last_slash == 0 || last_slash == reference.len() - 1 {
        return Err("oci_reference must include a registry host and repository path".to_string());
    }

    let repository = if let Some(at_pos) = reference.rfind('@') {
        &reference[..at_pos]
    } else if let Some(colon_pos) = reference.rfind(':') {
        if colon_pos > last_slash {
            &reference[..colon_pos]
        } else {
            reference
        }
    } else {
        reference
    };

    if repository.is_empty() || repository.ends_with('/') {
        return Err("oci_reference must include a registry host and repository path".to_string());
    }

    Ok(format!("{repository}@sha256:{digest}"))
}

impl ExecutionWorkload {
    pub fn request_hash(&self) -> Result<String, String> {
        let encoded = canonical_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(crypto::sha256_hex(encoded))
    }

    pub fn is_service_addressed(&self) -> bool {
        self.security.mode == ExecutionSecurityMode::Standard
            && self
                .security
                .service_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }

    pub fn service_id(&self) -> Option<&str> {
        self.security.service_id.as_deref()
    }

    pub fn binding_hash(&self) -> Option<&str> {
        match self.package_kind {
            ExecutionPackageKind::InlineSource => self.source_hash.as_deref(),
            ExecutionPackageKind::InlineModule | ExecutionPackageKind::OciImage => {
                self.module_hash.as_deref()
            }
            ExecutionPackageKind::Builtin => None,
        }
    }

    fn validate_service_addressed_shape(&self) -> Result<(), String> {
        let Some(service_id) = self.service_id() else {
            return Err("service-addressed execution requires security.service_id".to_string());
        };
        if service_id.trim().is_empty() {
            return Err("service-addressed execution requires security.service_id".to_string());
        }
        if self.contract_version.trim().is_empty() {
            return Err("service-addressed execution requires contract_version".to_string());
        }
        if self.entrypoint.value.trim().is_empty() {
            return Err("service-addressed execution requires entrypoint".to_string());
        }
        let expected_access = self
            .mounts
            .iter()
            .map(|mount| {
                format!(
                    "mount.{}.{}.{}",
                    mount.kind,
                    if mount.read_only { "read" } else { "write" },
                    mount.handle
                )
            })
            .collect::<Vec<_>>();
        if self.requested_access != expected_access {
            return Err(
                "service-addressed execution requested_access must match the declared mounts"
                    .to_string(),
            );
        }
        match (&self.runtime, &self.package_kind) {
            (ExecutionRuntime::Wasm, ExecutionPackageKind::InlineModule)
            | (ExecutionRuntime::TeeWasm, ExecutionPackageKind::InlineModule) => {
                if self.module_hash.as_deref().unwrap_or("").trim().is_empty() {
                    return Err("service-addressed Wasm execution requires module_hash".to_string());
                }
                if self.module_bytes_hex.is_some()
                    || self.inline_source.is_some()
                    || self.oci_reference.is_some()
                    || self.oci_digest.is_some()
                {
                    return Err(
                        "service-addressed Wasm execution must not embed binding payloads"
                            .to_string(),
                    );
                }
            }
            (ExecutionRuntime::Python, ExecutionPackageKind::InlineSource)
            | (ExecutionRuntime::TeePython, ExecutionPackageKind::InlineSource) => {
                if self.source_hash.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(
                        "service-addressed inline source execution requires source_hash"
                            .to_string(),
                    );
                }
                if self.module_bytes_hex.is_some()
                    || self.inline_source.is_some()
                    || self.oci_reference.is_some()
                    || self.oci_digest.is_some()
                {
                    return Err(
                        "service-addressed inline source execution must not embed binding payloads"
                            .to_string(),
                    );
                }
            }
            (_, ExecutionPackageKind::OciImage) => {
                if self.module_hash.as_deref().unwrap_or("").trim().is_empty() {
                    return Err("service-addressed OCI execution requires module_hash".to_string());
                }
                if self.module_bytes_hex.is_some()
                    || self.inline_source.is_some()
                    || self.oci_reference.is_some()
                    || self.oci_digest.is_some()
                {
                    return Err(
                        "service-addressed OCI execution must not embed binding payloads"
                            .to_string(),
                    );
                }
            }
            (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin) => {
                if self.builtin_name.as_deref().unwrap_or("").trim().is_empty() {
                    return Err("builtin execution requires builtin_name".to_string());
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn validate_basic(&self) -> Result<(), String> {
        let input_bytes = canonical_json::to_vec(&self.input).map_err(|error| error.to_string())?;
        let input_hash = crypto::sha256_hex(input_bytes);
        if input_hash != self.input_hash {
            return Err("input hash does not match canonical input".to_string());
        }
        if self.is_service_addressed() {
            return self.validate_service_addressed_shape();
        }
        match (&self.runtime, &self.package_kind) {
            (ExecutionRuntime::Any, _) => {
                return Err("wildcard runtime is only valid in offer metadata".to_string());
            }
            (ExecutionRuntime::Wasm, ExecutionPackageKind::InlineModule)
            | (ExecutionRuntime::TeeWasm, ExecutionPackageKind::InlineModule) => {
                let Some(module_bytes_hex) = self.module_bytes_hex.as_ref() else {
                    return Err("inline Wasm execution requires module_bytes_hex".to_string());
                };
                let module_bytes = hex::decode(module_bytes_hex)
                    .map_err(|error| format!("invalid module hex: {error}"))?;
                let computed_hash = crypto::sha256_hex(&module_bytes);
                if self.module_hash.as_deref() != Some(computed_hash.as_str()) {
                    return Err("module hash does not match module bytes".to_string());
                }
            }
            (ExecutionRuntime::Python, ExecutionPackageKind::InlineSource)
            | (ExecutionRuntime::TeePython, ExecutionPackageKind::InlineSource) => {
                let Some(inline_source) = self.inline_source.as_ref() else {
                    return Err("inline source execution requires inline_source".to_string());
                };
                let computed_hash = crypto::sha256_hex(inline_source.as_bytes());
                if self.source_hash.as_deref() != Some(computed_hash.as_str()) {
                    return Err("source hash does not match inline_source".to_string());
                }
            }
            (_, ExecutionPackageKind::OciImage) => {
                let oci_reference = self
                    .oci_reference
                    .as_deref()
                    .ok_or_else(|| "oci_image execution requires oci_reference".to_string())?;
                let oci_digest = self
                    .oci_digest
                    .as_deref()
                    .ok_or_else(|| "oci_image execution requires oci_digest".to_string())?;
                digest_pinned_oci_image_reference(oci_reference, oci_digest)?;
                if self.module_hash.as_deref() != Some(oci_digest) {
                    return Err("module hash does not match oci_digest".to_string());
                }
            }
            (ExecutionRuntime::Builtin, ExecutionPackageKind::Builtin)
            | (ExecutionRuntime::TeeService, ExecutionPackageKind::Builtin) => {
                if self.builtin_name.as_deref().unwrap_or("").trim().is_empty() {
                    return Err("builtin execution requires builtin_name".to_string());
                }
            }
            _ => {}
        }
        if self.security.mode == ExecutionSecurityMode::Tee {
            let Some(confidential_session_hash) = self.security.confidential_session_hash.as_ref()
            else {
                return Err("tee execution requires confidential_session_hash".to_string());
            };
            if confidential_session_hash.trim().is_empty() {
                return Err("tee execution requires confidential_session_hash".to_string());
            }
            if matches!(
                self.runtime,
                ExecutionRuntime::TeeService | ExecutionRuntime::TeeWasm
            ) && self.security.request_envelope.is_none()
            {
                return Err("tee execution requires request_envelope".to_string());
            }
        }
        Ok(())
    }

    pub fn resource_kind(&self) -> &'static str {
        match self.runtime {
            ExecutionRuntime::Builtin if self.builtin_name.as_deref() == Some("events.query") => {
                "data"
            }
            ExecutionRuntime::TeeService
            | ExecutionRuntime::TeeWasm
            | ExecutionRuntime::TeePython => "confidential",
            _ => "compute",
        }
    }

    pub fn runtime_name(&self) -> &'static str {
        self.runtime.as_str()
    }

    pub fn contract_version(&self) -> &str {
        self.contract_version.as_str()
    }

    pub fn confidential_session_hash(&self) -> Option<&str> {
        self.security.confidential_session_hash.as_deref()
    }

    pub fn requested_access(&self) -> &[String] {
        &self.requested_access
    }

    pub fn requires_wasm_permit(&self) -> bool {
        matches!(
            self.runtime,
            ExecutionRuntime::Wasm | ExecutionRuntime::TeeWasm
        )
    }

    pub fn from_wasm_submission(submission: WasmSubmission) -> Result<Self, String> {
        let workload = &submission.workload;
        Ok(Self {
            schema_version: workload.schema_version.clone(),
            workload_kind: workload.workload_kind.clone(),
            runtime: ExecutionRuntime::Wasm,
            package_kind: ExecutionPackageKind::InlineModule,
            entrypoint: ExecutionEntrypoint {
                kind: ExecutionEntrypointKind::Handler,
                value: "run".to_string(),
            },
            contract_version: workload.abi_version.clone(),
            input_format: workload.input_format.clone(),
            input_hash: workload.input_hash.clone(),
            requested_access: workload.requested_capabilities.clone(),
            security: ExecutionSecurity::default(),
            mounts: Vec::new(),
            input: submission.input,
            module_hash: Some(workload.module_hash.clone()),
            module_bytes_hex: Some(submission.module_bytes_hex),
            source_hash: None,
            inline_source: None,
            oci_reference: None,
            oci_digest: None,
            builtin_name: None,
        })
    }

    pub fn to_wasm_submission(&self) -> Result<WasmSubmission, String> {
        if self.runtime != ExecutionRuntime::Wasm
            || self.package_kind != ExecutionPackageKind::InlineModule
        {
            return Err("execution workload is not an inline Wasm submission".to_string());
        }
        let module_bytes_hex = self
            .module_bytes_hex
            .clone()
            .ok_or_else(|| "inline Wasm submission requires module_bytes_hex".to_string())?;
        let module_hash = self
            .module_hash
            .clone()
            .ok_or_else(|| "inline Wasm submission requires module_hash".to_string())?;
        Ok(WasmSubmission {
            schema_version: self.schema_version.clone(),
            submission_type: wasm::WASM_SUBMISSION_TYPE_V1.to_string(),
            workload: ComputeWasmWorkload {
                schema_version: self.schema_version.clone(),
                workload_kind: self.workload_kind.clone(),
                abi_version: self.contract_version.clone(),
                module_format: wasm::WASM_MODULE_FORMAT.to_string(),
                module_hash,
                input_format: self.input_format.clone(),
                input_hash: self.input_hash.clone(),
                requested_capabilities: self.requested_access.clone(),
            },
            module_bytes_hex,
            input: self.input.clone(),
        })
    }

    pub fn from_oci_wasm_submission(submission: OciWasmSubmission) -> Result<Self, String> {
        let workload = &submission.workload;
        Ok(Self {
            schema_version: workload.schema_version.clone(),
            workload_kind: workload.workload_kind.clone(),
            runtime: ExecutionRuntime::Wasm,
            package_kind: ExecutionPackageKind::OciImage,
            entrypoint: ExecutionEntrypoint {
                kind: ExecutionEntrypointKind::Handler,
                value: "run".to_string(),
            },
            contract_version: workload.abi_version.clone(),
            input_format: workload.input_format.clone(),
            input_hash: workload.input_hash.clone(),
            requested_access: workload.requested_capabilities.clone(),
            security: ExecutionSecurity::default(),
            mounts: Vec::new(),
            input: submission.input,
            module_hash: Some(workload.oci_digest.clone()),
            module_bytes_hex: None,
            source_hash: None,
            inline_source: None,
            oci_reference: Some(workload.oci_reference.clone()),
            oci_digest: Some(workload.oci_digest.clone()),
            builtin_name: None,
        })
    }

    pub fn to_oci_wasm_submission(&self) -> Result<OciWasmSubmission, String> {
        if self.runtime != ExecutionRuntime::Wasm
            || self.package_kind != ExecutionPackageKind::OciImage
        {
            return Err("execution workload is not an OCI Wasm submission".to_string());
        }
        let oci_reference = self
            .oci_reference
            .clone()
            .ok_or_else(|| "OCI Wasm submission requires oci_reference".to_string())?;
        let oci_digest = self
            .oci_digest
            .clone()
            .ok_or_else(|| "OCI Wasm submission requires oci_digest".to_string())?;
        Ok(OciWasmSubmission {
            schema_version: self.schema_version.clone(),
            submission_type: wasm::WASM_OCI_SUBMISSION_TYPE_V1.to_string(),
            workload: OciWasmWorkload {
                schema_version: self.schema_version.clone(),
                workload_kind: self.workload_kind.clone(),
                abi_version: self.contract_version.clone(),
                module_format: wasm::WASM_MODULE_OCI_FORMAT.to_string(),
                oci_reference,
                oci_digest: oci_digest.clone(),
                input_format: self.input_format.clone(),
                input_hash: self.input_hash.clone(),
                requested_capabilities: self.requested_access.clone(),
            },
            input: self.input.clone(),
        })
    }

    pub fn python_inline_handler(
        source: String,
        entrypoint: String,
        input: Value,
    ) -> Result<Self, String> {
        let source_hash = crypto::sha256_hex(source.as_bytes());
        let input_hash =
            crypto::sha256_hex(canonical_json::to_vec(&input).map_err(|error| error.to_string())?);
        Ok(Self {
            schema_version: wasm::FROGLET_SCHEMA_V1.to_string(),
            workload_kind: WORKLOAD_KIND_EXECUTION_V1.to_string(),
            runtime: ExecutionRuntime::Python,
            package_kind: ExecutionPackageKind::InlineSource,
            entrypoint: ExecutionEntrypoint {
                kind: ExecutionEntrypointKind::Handler,
                value: entrypoint,
            },
            contract_version: CONTRACT_PYTHON_HANDLER_JSON_V1.to_string(),
            input_format: wasm::JCS_JSON_FORMAT.to_string(),
            input_hash,
            requested_access: Vec::new(),
            security: ExecutionSecurity::default(),
            mounts: Vec::new(),
            input,
            module_hash: None,
            module_bytes_hex: None,
            source_hash: Some(source_hash),
            inline_source: Some(source),
            oci_reference: None,
            oci_digest: None,
            builtin_name: None,
        })
    }

    pub fn python_inline_script(source: String, input: Value) -> Result<Self, String> {
        let source_hash = crypto::sha256_hex(source.as_bytes());
        let input_hash =
            crypto::sha256_hex(canonical_json::to_vec(&input).map_err(|error| error.to_string())?);
        Ok(Self {
            schema_version: wasm::FROGLET_SCHEMA_V1.to_string(),
            workload_kind: WORKLOAD_KIND_EXECUTION_V1.to_string(),
            runtime: ExecutionRuntime::Python,
            package_kind: ExecutionPackageKind::InlineSource,
            entrypoint: ExecutionEntrypoint {
                kind: ExecutionEntrypointKind::Script,
                value: "__main__".to_string(),
            },
            contract_version: CONTRACT_PYTHON_SCRIPT_JSON_V1.to_string(),
            input_format: wasm::JCS_JSON_FORMAT.to_string(),
            input_hash,
            requested_access: Vec::new(),
            security: ExecutionSecurity::default(),
            mounts: Vec::new(),
            input,
            module_hash: None,
            module_bytes_hex: None,
            source_hash: Some(source_hash),
            inline_source: Some(source),
            oci_reference: None,
            oci_digest: None,
            builtin_name: None,
        })
    }

    pub fn container_oci(
        runtime: ExecutionRuntime,
        oci_reference: String,
        oci_digest: String,
        entrypoint_kind: ExecutionEntrypointKind,
        entrypoint: String,
        input: Value,
    ) -> Result<Self, String> {
        let input_hash =
            crypto::sha256_hex(canonical_json::to_vec(&input).map_err(|error| error.to_string())?);
        Ok(Self {
            schema_version: wasm::FROGLET_SCHEMA_V1.to_string(),
            workload_kind: WORKLOAD_KIND_EXECUTION_V1.to_string(),
            runtime,
            package_kind: ExecutionPackageKind::OciImage,
            entrypoint: ExecutionEntrypoint {
                kind: entrypoint_kind,
                value: entrypoint,
            },
            contract_version: CONTRACT_CONTAINER_JSON_V1.to_string(),
            input_format: wasm::JCS_JSON_FORMAT.to_string(),
            input_hash,
            requested_access: Vec::new(),
            security: ExecutionSecurity::default(),
            mounts: Vec::new(),
            input,
            module_hash: Some(oci_digest.clone()),
            module_bytes_hex: None,
            source_hash: None,
            inline_source: None,
            oci_reference: Some(oci_reference),
            oci_digest: Some(oci_digest),
            builtin_name: None,
        })
    }

    pub fn builtin_events_query(kinds: Vec<String>, limit: Option<usize>) -> Result<Self, String> {
        let input = json!({
            "kinds": kinds,
            "limit": limit,
        });
        let input_hash =
            crypto::sha256_hex(canonical_json::to_vec(&input).map_err(|error| error.to_string())?);
        Ok(Self {
            schema_version: wasm::FROGLET_SCHEMA_V1.to_string(),
            workload_kind: "events.query".to_string(),
            runtime: ExecutionRuntime::Builtin,
            package_kind: ExecutionPackageKind::Builtin,
            entrypoint: ExecutionEntrypoint {
                kind: ExecutionEntrypointKind::Builtin,
                value: "events.query".to_string(),
            },
            contract_version: CONTRACT_BUILTIN_EVENTS_QUERY_V1.to_string(),
            input_format: wasm::JCS_JSON_FORMAT.to_string(),
            input_hash,
            requested_access: Vec::new(),
            security: ExecutionSecurity::default(),
            mounts: Vec::new(),
            input,
            module_hash: None,
            module_bytes_hex: None,
            source_hash: None,
            inline_source: None,
            oci_reference: None,
            oci_digest: None,
            builtin_name: Some("events.query".to_string()),
        })
    }

    pub fn tee_confidential_service(
        confidential_session_hash: String,
        service_id: String,
        request_envelope: EncryptedEnvelope,
    ) -> Self {
        Self {
            schema_version: wasm::FROGLET_SCHEMA_V1.to_string(),
            workload_kind: WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1.to_string(),
            runtime: ExecutionRuntime::TeeService,
            package_kind: ExecutionPackageKind::Builtin,
            entrypoint: ExecutionEntrypoint {
                kind: ExecutionEntrypointKind::Builtin,
                value: service_id.clone(),
            },
            contract_version: "froglet.confidential.service.v1".to_string(),
            input_format: wasm::JCS_JSON_FORMAT.to_string(),
            input_hash: String::new(),
            requested_access: Vec::new(),
            security: ExecutionSecurity {
                mode: ExecutionSecurityMode::Tee,
                confidential_session_hash: Some(confidential_session_hash),
                service_id: Some(service_id),
                request_envelope: Some(request_envelope),
            },
            mounts: Vec::new(),
            input: Value::Null,
            module_hash: None,
            module_bytes_hex: None,
            source_hash: None,
            inline_source: None,
            oci_reference: None,
            oci_digest: None,
            builtin_name: Some("confidential.service".to_string()),
        }
    }

    pub fn tee_attested_wasm(
        confidential_session_hash: String,
        request_envelope: EncryptedEnvelope,
    ) -> Self {
        Self {
            schema_version: wasm::FROGLET_SCHEMA_V1.to_string(),
            workload_kind: WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1.to_string(),
            runtime: ExecutionRuntime::TeeWasm,
            package_kind: ExecutionPackageKind::InlineModule,
            entrypoint: ExecutionEntrypoint {
                kind: ExecutionEntrypointKind::Handler,
                value: "run".to_string(),
            },
            contract_version: "froglet.confidential.attested_wasm.v1".to_string(),
            input_format: wasm::JCS_JSON_FORMAT.to_string(),
            input_hash: String::new(),
            requested_access: Vec::new(),
            security: ExecutionSecurity {
                mode: ExecutionSecurityMode::Tee,
                confidential_session_hash: Some(confidential_session_hash),
                service_id: None,
                request_envelope: Some(request_envelope),
            },
            mounts: Vec::new(),
            input: Value::Null,
            module_hash: None,
            module_bytes_hex: None,
            source_hash: None,
            inline_source: None,
            oci_reference: None,
            oci_digest: None,
            builtin_name: Some("attested.wasm".to_string()),
        }
    }

    pub fn events_query_params(&self) -> Option<(Vec<String>, Option<usize>)> {
        if self.runtime != ExecutionRuntime::Builtin
            || self.builtin_name.as_deref() != Some("events.query")
        {
            return None;
        }
        let kinds = self
            .input
            .get("kinds")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let limit = self
            .input
            .get("limit")
            .and_then(Value::as_u64)
            .map(|value| value as usize);
        Some((kinds, limit))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExecutionEntrypointKind, ExecutionRuntime, ExecutionWorkload,
        digest_pinned_oci_image_reference,
    };
    use serde_json::Value;

    #[test]
    fn digest_pinned_oci_image_reference_strips_tags_and_schemes() {
        let pinned = digest_pinned_oci_image_reference(
            "https://ghcr.io/froglet/image:latest",
            &"ab".repeat(32),
        )
        .expect("digest-pinned reference");

        assert_eq!(
            pinned,
            format!("ghcr.io/froglet/image@sha256:{}", "ab".repeat(32))
        );
    }

    #[test]
    fn digest_pinned_oci_image_reference_rejects_invalid_digest() {
        let error = digest_pinned_oci_image_reference("ghcr.io/froglet/image:latest", "xyz")
            .expect_err("invalid digest should fail");

        assert!(error.contains("oci_digest"), "unexpected error: {error}");
    }

    #[test]
    fn oci_workload_validation_rejects_mismatched_module_hash() {
        let mut workload = ExecutionWorkload::container_oci(
            ExecutionRuntime::Container,
            "ghcr.io/froglet/image:latest".to_string(),
            "ab".repeat(32),
            ExecutionEntrypointKind::Handler,
            "run".to_string(),
            Value::Null,
        )
        .expect("container workload");
        workload.module_hash = Some("cd".repeat(32));

        let error = workload
            .validate_basic()
            .expect_err("mismatched hash should fail");
        assert!(error.contains("module hash"), "unexpected error: {error}");
    }
}
