use crate::{
    canonical_json,
    confidential::{
        EncryptedEnvelope, WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1,
        WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1,
    },
    crypto,
    execution::{ExecutionRuntime, ExecutionWorkload},
    jobs::JobSpec,
    pricing::ServiceId,
    wasm::WasmSubmission,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkloadSpec {
    Execution {
        execution: Box<ExecutionWorkload>,
    },
    Wasm {
        submission: Box<WasmSubmission>,
    },
    OciWasm {
        submission: Box<crate::wasm::OciWasmSubmission>,
    },
    ConfidentialService {
        confidential_session_hash: String,
        service_id: String,
        request_envelope: Box<EncryptedEnvelope>,
    },
    AttestedWasm {
        confidential_session_hash: String,
        request_envelope: Box<EncryptedEnvelope>,
    },
    EventsQuery {
        kinds: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
}

impl WorkloadSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            WorkloadSpec::Execution { .. } => "execution",
            WorkloadSpec::Wasm { .. } => "wasm",
            WorkloadSpec::OciWasm { .. } => "oci_wasm",
            WorkloadSpec::ConfidentialService { .. } => "confidential_service",
            WorkloadSpec::AttestedWasm { .. } => "attested_wasm",
            WorkloadSpec::EventsQuery { .. } => "events_query",
        }
    }

    pub fn workload_kind(&self) -> &str {
        match self {
            WorkloadSpec::Execution { execution } => execution.workload_kind.as_str(),
            WorkloadSpec::Wasm { .. } => crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_V1,
            WorkloadSpec::OciWasm { .. } => crate::wasm::WORKLOAD_KIND_COMPUTE_WASM_OCI_V1,
            WorkloadSpec::ConfidentialService { .. } => WORKLOAD_KIND_CONFIDENTIAL_SERVICE_V1,
            WorkloadSpec::AttestedWasm { .. } => WORKLOAD_KIND_COMPUTE_WASM_ATTESTED_V1,
            WorkloadSpec::EventsQuery { .. } => "events.query",
        }
    }

    pub fn service_id(&self) -> ServiceId {
        match self {
            WorkloadSpec::Execution { execution } => {
                if execution.runtime == ExecutionRuntime::Builtin
                    && execution.builtin_name.as_deref() == Some("events.query")
                {
                    ServiceId::EventsQuery
                } else {
                    ServiceId::ExecuteWasm
                }
            }
            WorkloadSpec::Wasm { .. } => ServiceId::ExecuteWasm,
            WorkloadSpec::OciWasm { .. } => ServiceId::ExecuteWasm,
            WorkloadSpec::ConfidentialService { .. } => ServiceId::ExecuteWasm,
            WorkloadSpec::AttestedWasm { .. } => ServiceId::ExecuteWasm,
            WorkloadSpec::EventsQuery { .. } => ServiceId::EventsQuery,
        }
    }

    pub fn resource_kind(&self) -> &'static str {
        match self {
            WorkloadSpec::Execution { execution } => execution.resource_kind(),
            WorkloadSpec::EventsQuery { .. } => "data",
            WorkloadSpec::Wasm { .. } => "compute",
            WorkloadSpec::OciWasm { .. } => "compute",
            WorkloadSpec::ConfidentialService { .. } => "confidential",
            WorkloadSpec::AttestedWasm { .. } => "confidential",
        }
    }

    pub fn runtime(&self) -> Option<&str> {
        match self {
            WorkloadSpec::Execution { execution } => Some(execution.runtime_name()),
            WorkloadSpec::Wasm { .. } => Some("wasm"),
            WorkloadSpec::OciWasm { .. } => Some("wasm"),
            WorkloadSpec::ConfidentialService { .. } => None,
            WorkloadSpec::AttestedWasm { .. } => Some("tee.wasm"),
            WorkloadSpec::EventsQuery { .. } => Some("builtin"),
        }
    }

    pub fn contract_version(&self) -> Option<&str> {
        match self {
            WorkloadSpec::Execution { execution } => Some(execution.contract_version()),
            WorkloadSpec::Wasm { submission } => Some(submission.workload.abi_version.as_str()),
            WorkloadSpec::OciWasm { submission } => Some(submission.workload.abi_version.as_str()),
            WorkloadSpec::ConfidentialService { .. } => Some("froglet.confidential.service.v1"),
            WorkloadSpec::AttestedWasm { .. } => Some("froglet.confidential.attested_wasm.v1"),
            WorkloadSpec::EventsQuery { .. } => Some("froglet.builtin.events_query.v1"),
        }
    }

    pub fn abi_version(&self) -> Option<&str> {
        self.contract_version()
    }

    pub fn request_hash(&self) -> Result<String, String> {
        match self {
            WorkloadSpec::Execution { execution } => execution.request_hash(),
            WorkloadSpec::Wasm { submission } => submission.workload_hash(),
            WorkloadSpec::OciWasm { submission } => submission.workload_hash(),
            WorkloadSpec::ConfidentialService {
                request_envelope, ..
            } => request_envelope.envelope_hash(),
            WorkloadSpec::AttestedWasm {
                request_envelope, ..
            } => request_envelope.envelope_hash(),
            WorkloadSpec::EventsQuery { .. } => {
                let encoded = canonical_json::to_vec(self).map_err(|e| e.to_string())?;
                Ok(crypto::sha256_hex(encoded))
            }
        }
    }

    pub fn requested_capabilities(&self) -> &[String] {
        match self {
            WorkloadSpec::Execution { execution } => execution.requested_access(),
            WorkloadSpec::Wasm { submission } => &submission.workload.requested_capabilities,
            WorkloadSpec::OciWasm { submission } => &submission.workload.requested_capabilities,
            WorkloadSpec::ConfidentialService { .. } => &[],
            WorkloadSpec::AttestedWasm { .. } => &[],
            WorkloadSpec::EventsQuery { .. } => &[],
        }
    }

    pub fn confidential_session_hash(&self) -> Option<&str> {
        match self {
            WorkloadSpec::Execution { execution } => execution.confidential_session_hash(),
            WorkloadSpec::ConfidentialService {
                confidential_session_hash,
                ..
            } => Some(confidential_session_hash.as_str()),
            WorkloadSpec::AttestedWasm {
                confidential_session_hash,
                ..
            } => Some(confidential_session_hash.as_str()),
            WorkloadSpec::Wasm { .. }
            | WorkloadSpec::OciWasm { .. }
            | WorkloadSpec::EventsQuery { .. } => None,
        }
    }
}

impl From<JobSpec> for WorkloadSpec {
    fn from(value: JobSpec) -> Self {
        match value {
            JobSpec::Execution { execution } => WorkloadSpec::Execution { execution },
            JobSpec::Wasm { submission } => WorkloadSpec::Wasm {
                submission: Box::new(submission),
            },
            JobSpec::OciWasm { submission } => WorkloadSpec::OciWasm {
                submission: Box::new(submission),
            },
        }
    }
}
