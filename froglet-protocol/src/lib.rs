pub mod canonical_json;
pub mod crypto;
pub mod protocol;

use serde::{Deserialize, Serialize};

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
