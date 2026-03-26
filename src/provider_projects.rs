use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    canonical_json, crypto,
    execution::{
        CONTRACT_PYTHON_HANDLER_JSON_V1, ExecutionEntrypointKind, ExecutionMount,
        ExecutionPackageKind, ExecutionRuntime,
    },
    sandbox,
    state::AppState,
    wasm::{WASM_HOST_JSON_ABI_V1, WASM_RUN_JSON_ABI_V1},
};

pub const PROJECT_SCHEMA_VERSION: &str = "froglet-service/v2";
const MANIFEST_FILE_NAME: &str = "froglet-service.toml";
const DEFAULT_ENTRYPOINT: &str = "source/main.wat";
const DEFAULT_PYTHON_ENTRYPOINT: &str = "source/main.py";
const BUILD_DIR_NAME: &str = "build";
const BUILD_ARTIFACT_NAME: &str = "module.wasm";
const BUILD_PYTHON_ARTIFACT_NAME: &str = "main.py";

fn default_project_runtime() -> String {
    "python".to_string()
}

fn default_project_package_kind() -> String {
    "inline_source".to_string()
}

fn default_project_entrypoint_kind() -> String {
    "handler".to_string()
}

fn default_project_contract_version() -> String {
    CONTRACT_PYTHON_HANDLER_JSON_V1.to_string()
}

fn default_v1_execution_kind() -> String {
    "wasm_inline".to_string()
}

fn default_v1_abi_version() -> String {
    WASM_RUN_JSON_ABI_V1.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProjectManifest {
    pub schema_version: String,
    pub project_id: String,
    pub service_id: String,
    pub offer_id: String,
    pub summary: String,
    #[serde(default = "default_project_runtime")]
    pub runtime: String,
    #[serde(default = "default_project_package_kind")]
    pub package_kind: String,
    #[serde(default = "default_project_entrypoint_kind")]
    pub entrypoint_kind: String,
    #[serde(default = "default_project_contract_version")]
    pub contract_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<ExecutionMount>,
    pub mode: String,
    pub source_kind: String,
    pub entrypoint: String,
    pub price_sats: u64,
    pub publication_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderProjectRecord {
    pub project_id: String,
    pub service_id: String,
    pub offer_id: String,
    pub summary: String,
    pub runtime: String,
    pub package_kind: String,
    pub entrypoint_kind: String,
    pub contract_version: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<ExecutionMount>,
    pub mode: String,
    pub source_kind: String,
    pub entrypoint: String,
    pub price_sats: u64,
    pub publication_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_artifact_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderProjectBuildRecord {
    pub project: ProviderProjectRecord,
    pub build_artifact_path: String,
    pub module_hash: String,
    pub contract_version: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderProjectManifestV1 {
    pub project_id: String,
    pub service_id: String,
    pub offer_id: String,
    pub summary: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub package_kind: String,
    #[serde(default)]
    pub entrypoint_kind: String,
    #[serde(default)]
    pub contract_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<ExecutionMount>,
    #[serde(default = "default_v1_execution_kind")]
    pub execution_kind: String,
    #[serde(default = "default_v1_abi_version")]
    pub abi_version: String,
    pub mode: String,
    pub source_kind: String,
    pub entrypoint: String,
    pub price_sats: u64,
    pub publication_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ProjectManifestVersionProbe {
    pub schema_version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderProjectTestRecord {
    pub project: ProviderProjectRecord,
    pub build_artifact_path: String,
    pub module_hash: String,
    pub input: Value,
    pub output: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProviderProjectStarter {
    #[serde(rename = "blank")]
    BlankRunJson,
    #[serde(rename = "hello_world")]
    HelloWorld,
    #[serde(rename = "echo_json")]
    EchoJson,
    #[serde(rename = "http_fetch_passthrough")]
    HttpFetchPassthrough,
}

impl ProviderProjectStarter {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "blank" => Ok(Self::BlankRunJson),
            "blank_run_json" => Ok(Self::BlankRunJson),
            "hello_world" => Ok(Self::HelloWorld),
            "echo_json" => Ok(Self::EchoJson),
            "http_fetch_passthrough" => Ok(Self::HttpFetchPassthrough),
            _ => Err("starter must be one of blank, blank_run_json, hello_world, echo_json, http_fetch_passthrough".to_string()),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::BlankRunJson => "blank",
            Self::HelloWorld => "hello_world",
            Self::EchoJson => "echo_json",
            Self::HttpFetchPassthrough => "http_fetch_passthrough",
        }
    }

    pub fn abi_version(self) -> &'static str {
        match self {
            Self::HttpFetchPassthrough => WASM_HOST_JSON_ABI_V1,
            Self::BlankRunJson | Self::HelloWorld | Self::EchoJson => WASM_RUN_JSON_ABI_V1,
        }
    }

    pub fn initial_source(self) -> &'static str {
        match self {
            Self::BlankRunJson => BLANK_RUN_JSON_WAT,
            Self::HelloWorld => HELLO_WORLD_WAT,
            Self::EchoJson => ECHO_JSON_WAT,
            Self::HttpFetchPassthrough => HTTP_FETCH_PASSTHROUGH_WAT,
        }
    }
}

const BLANK_RUN_JSON_WAT: &str = r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 128))
  (func (export "alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap
    local.set $ptr
    global.get $heap
    local.get $len
    i32.add
    global.set $heap
    local.get $ptr)
  (func (export "dealloc") (param i32 i32))
  (func (export "run") (param i32 i32) (result i64)
    i64.const 4)
  (data (i32.const 0) "null"))"#;

const HELLO_WORLD_WAT: &str = r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 128))
  (func (export "alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap
    local.set $ptr
    global.get $heap
    local.get $len
    i32.add
    global.set $heap
    local.get $ptr)
  (func (export "dealloc") (param i32 i32))
  (func (export "run") (param i32 i32) (result i64)
    i64.const 25)
  (data (i32.const 0) "{\"message\":\"Hello World\"}"))"#;

const ECHO_JSON_WAT: &str = r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 4096))
  (func (export "alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap
    local.set $ptr
    global.get $heap
    local.get $len
    i32.add
    global.set $heap
    local.get $ptr)
  (func (export "dealloc") (param i32 i32))
  (func (export "run") (param $ptr i32) (param $len i32) (result i64)
    local.get $ptr
    i64.extend_i32_u
    i64.const 32
    i64.shl
    local.get $len
    i64.extend_i32_u
    i64.or))"#;

const HTTP_FETCH_PASSTHROUGH_WAT: &str = r#"(module
  (import "froglet_host" "call_json" (func $call_json (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 4096))
  (func (export "alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap
    local.set $ptr
    global.get $heap
    local.get $len
    i32.add
    global.set $heap
    local.get $ptr)
  (func (export "dealloc") (param i32 i32))
  (func (export "run") (param $ptr i32) (param $len i32) (result i64)
    local.get $ptr
    local.get $len
    call $call_json))"#;

const BLANK_PYTHON_SOURCE: &str = r#"def handler(event, context):
    return None
"#;

fn wat_data_string(bytes: &[u8]) -> String {
    let mut encoded = String::new();
    for byte in bytes {
        match byte {
            b'"' => encoded.push_str("\\22"),
            b'\\' => encoded.push_str("\\5c"),
            0x20..=0x7e => encoded.push(char::from(*byte)),
            _ => encoded.push_str(&format!("\\{:02x}", byte)),
        }
    }
    encoded
}

pub fn static_json_wat(value: &Value) -> Result<String, String> {
    let bytes = canonical_json::to_vec(value)
        .map_err(|error| format!("failed to encode static JSON: {error}"))?;
    let len = bytes.len();
    let data = wat_data_string(&bytes);
    Ok(format!(
        r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 128))
  (func (export "alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap
    local.set $ptr
    global.get $heap
    local.get $len
    i32.add
    global.set $heap
    local.get $ptr)
  (func (export "dealloc") (param i32 i32))
  (func (export "run") (param i32 i32) (result i64)
    i64.const {len})
  (data (i32.const 0) "{data}"))"#
    ))
}

pub fn static_json_python(value: &Value) -> Result<String, String> {
    let encoded = canonical_json::to_string(value)
        .map_err(|error| format!("failed to encode static JSON: {error}"))?;
    Ok(format!(
        "import json\n\n_STATIC = json.loads({encoded:?})\n\ndef handler(event, context):\n    return _STATIC\n"
    ))
}

pub fn projects_root_from_data_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("projects")
}

pub fn project_dir(root: &Path, project_id: &str) -> Result<PathBuf, String> {
    validate_project_dir(root, project_id)
}

pub fn default_project_input(manifest: &ProviderProjectManifest) -> Value {
    match manifest.starter.as_deref() {
        Some("http_fetch_passthrough") => json!({
            "op": "http.fetch",
            "request": {
                "method": "GET",
                "url": "https://example.com"
            }
        }),
        Some("echo_json") => json!({ "echo": true }),
        _ => Value::Null,
    }
}

pub fn list_projects(root: &Path) -> Result<Vec<ProviderProjectRecord>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut projects = Vec::new();
    let entries =
        fs::read_dir(root).map_err(|error| format!("failed to read projects root: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if !file_type.is_dir() {
            continue;
        }
        let manifest = load_manifest(&entry.path())?;
        projects.push(project_record(&entry.path(), manifest)?);
    }
    projects.sort_by(|left, right| left.project_id.cmp(&right.project_id));
    Ok(projects)
}

/// Optional overrides applied at creation time to avoid post-create load-mutate-save cycles.
#[derive(Default)]
pub struct CreateProjectOverrides {
    pub mode: Option<String>,
    pub clear_starter: bool,
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub runtime: Option<String>,
    pub package_kind: Option<String>,
    pub entrypoint_kind: Option<String>,
    pub entrypoint: Option<String>,
    pub contract_version: Option<String>,
    pub mounts: Option<Vec<ExecutionMount>>,
}

#[allow(clippy::too_many_arguments)]
pub fn create_project(
    root: &Path,
    project_id: &str,
    service_id: &str,
    offer_id: &str,
    starter: Option<ProviderProjectStarter>,
    summary: &str,
    price_sats: u64,
    publication_state: &str,
    overrides: CreateProjectOverrides,
) -> Result<ProviderProjectRecord, String> {
    let project_dir = validate_project_dir(root, project_id)?;
    if project_dir.exists() {
        return Err(format!("project already exists: {project_id}"));
    }
    fs::create_dir_all(project_dir.join("source"))
        .map_err(|error| format!("failed to create project directories: {error}"))?;
    let effective_starter = if overrides.clear_starter {
        None
    } else {
        starter.map(|value| value.id().to_string())
    };
    let (runtime, package_kind, entrypoint_kind, contract_version, source_kind, entrypoint, source) =
        if let Some(starter) = starter {
            (
                "wasm".to_string(),
                "inline_module".to_string(),
                "handler".to_string(),
                starter.abi_version().to_string(),
                "wat".to_string(),
                DEFAULT_ENTRYPOINT.to_string(),
                starter.initial_source().to_string(),
            )
        } else {
            (
                overrides
                    .runtime
                    .clone()
                    .unwrap_or_else(default_project_runtime),
                overrides
                    .package_kind
                    .clone()
                    .unwrap_or_else(default_project_package_kind),
                overrides
                    .entrypoint_kind
                    .clone()
                    .unwrap_or_else(default_project_entrypoint_kind),
                overrides
                    .contract_version
                    .clone()
                    .unwrap_or_else(default_project_contract_version),
                "python".to_string(),
                overrides
                    .entrypoint
                    .clone()
                    .unwrap_or_else(|| DEFAULT_PYTHON_ENTRYPOINT.to_string()),
                BLANK_PYTHON_SOURCE.to_string(),
            )
        };
    let manifest = ProviderProjectManifest {
        schema_version: PROJECT_SCHEMA_VERSION.to_string(),
        project_id: project_id.to_string(),
        service_id: service_id.to_string(),
        offer_id: offer_id.to_string(),
        summary: summary.trim().to_string(),
        runtime,
        package_kind,
        entrypoint_kind,
        contract_version: contract_version.clone(),
        mounts: overrides.mounts.unwrap_or_default(),
        mode: overrides.mode.unwrap_or_else(|| "sync".to_string()),
        source_kind,
        entrypoint: entrypoint.clone(),
        price_sats,
        publication_state: publication_state.to_string(),
        starter: effective_starter,
        input_schema: overrides.input_schema,
        output_schema: overrides.output_schema,
    };
    save_manifest(&project_dir, &manifest)?;
    write_project_file(root, project_id, &entrypoint, &source)?;
    project_record(&project_dir, manifest)
}

pub fn get_project(root: &Path, project_id: &str) -> Result<ProviderProjectRecord, String> {
    let project_dir = validate_project_dir(root, project_id)?;
    let manifest = load_manifest(&project_dir)?;
    project_record(&project_dir, manifest)
}

pub fn set_project_publication_state(
    root: &Path,
    project_id: &str,
    publication_state: &str,
) -> Result<ProviderProjectRecord, String> {
    if publication_state != "active" && publication_state != "hidden" {
        return Err(format!(
            "provider project publication_state must be active or hidden, got {publication_state}"
        ));
    }
    let project_dir = validate_project_dir(root, project_id)?;
    let mut manifest = load_manifest(&project_dir)?;
    manifest.publication_state = publication_state.to_string();
    save_manifest(&project_dir, &manifest)?;
    project_record(&project_dir, manifest)
}

pub fn read_project_file(
    root: &Path,
    project_id: &str,
    relative_path: &str,
) -> Result<String, String> {
    let project_dir = validate_project_dir(root, project_id)?;
    let full_path = resolve_relative_path(&project_dir, relative_path)?;
    let contents = fs::read_to_string(&full_path).map_err(|error| {
        format!(
            "failed to read project file {}: {error}",
            full_path.display()
        )
    })?;
    Ok(contents)
}

pub fn write_project_file(
    root: &Path,
    project_id: &str,
    relative_path: &str,
    contents: &str,
) -> Result<(), String> {
    let project_dir = validate_project_dir(root, project_id)?;
    let full_path = resolve_relative_path(&project_dir, relative_path)?;
    if let Some(parent) = full_path.parent() {
        ensure_non_symlink_tree(&project_dir, parent)?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create project directories: {error}"))?;
    }
    fs::write(&full_path, contents).map_err(|error| {
        format!(
            "failed to write project file {}: {error}",
            full_path.display()
        )
    })
}

pub fn write_static_result_project(
    root: &Path,
    project_id: &str,
    result: &Value,
) -> Result<(), String> {
    let project_dir = validate_project_dir(root, project_id)?;
    let manifest = load_manifest(&project_dir)?;
    let source = if manifest.source_kind == "python" {
        static_json_python(result)?
    } else {
        static_json_wat(result)?
    };
    write_project_file(root, project_id, &manifest.entrypoint, &source)
}

pub fn build_project(root: &Path, project_id: &str) -> Result<ProviderProjectBuildRecord, String> {
    let project_dir = validate_project_dir(root, project_id)?;
    let manifest = load_manifest(&project_dir)?;
    let entrypoint = resolve_relative_path(&project_dir, &manifest.entrypoint)?;
    let build_dir = project_dir.join(BUILD_DIR_NAME);
    fs::create_dir_all(&build_dir)
        .map_err(|error| format!("failed to create build directory: {error}"))?;
    let (build_artifact_path, module_hash) = if manifest.source_kind == "wat" {
        let wat_source = fs::read_to_string(&entrypoint).map_err(|error| {
            format!(
                "failed to read WAT source {}: {error}",
                entrypoint.display()
            )
        })?;
        let module_bytes = wat::parse_str(&wat_source)
            .map_err(|error| format!("failed to compile WAT source: {error}"))?;
        sandbox::validate_module_bytes_for_abi(&module_bytes, &manifest.contract_version)
            .map_err(|error| format!("build validation failed: {error}"))?;
        let build_artifact_path = build_dir.join(BUILD_ARTIFACT_NAME);
        fs::write(&build_artifact_path, &module_bytes).map_err(|error| {
            format!(
                "failed to write build artifact {}: {error}",
                build_artifact_path.display()
            )
        })?;
        (build_artifact_path, crypto::sha256_hex(&module_bytes))
    } else if manifest.source_kind == "python" {
        let source = fs::read_to_string(&entrypoint).map_err(|error| {
            format!(
                "failed to read Python source {}: {error}",
                entrypoint.display()
            )
        })?;
        let build_artifact_path = build_dir.join(BUILD_PYTHON_ARTIFACT_NAME);
        fs::write(&build_artifact_path, &source).map_err(|error| {
            format!(
                "failed to write build artifact {}: {error}",
                build_artifact_path.display()
            )
        })?;
        (build_artifact_path, crypto::sha256_hex(source.as_bytes()))
    } else {
        return Err(format!(
            "unsupported provider project source_kind: {}",
            manifest.source_kind
        ));
    };
    Ok(ProviderProjectBuildRecord {
        project: project_record(&project_dir, manifest.clone())?,
        build_artifact_path: build_artifact_path.display().to_string(),
        module_hash,
        contract_version: manifest.contract_version,
    })
}

pub fn reject_blank_scaffold_publication(root: &Path, project_id: &str) -> Result<(), String> {
    let project_dir = validate_project_dir(root, project_id)?;
    let manifest = load_manifest(&project_dir)?;
    if manifest.starter.is_some() {
        return Ok(());
    }
    let entrypoint = resolve_relative_path(&project_dir, &manifest.entrypoint)?;
    let source = fs::read_to_string(&entrypoint).map_err(|error| {
        format!(
            "failed to read project source {}: {error}",
            entrypoint.display()
        )
    })?;
    if (manifest.source_kind == "wat" && source.trim() == BLANK_RUN_JSON_WAT.trim())
        || (manifest.source_kind == "python" && source.trim() == BLANK_PYTHON_SOURCE.trim())
    {
        return Err(
            "blank projects are scaffolds only; write source or provide result_json before publishing"
                .to_string(),
        );
    }
    Ok(())
}

pub fn test_project(
    root: &Path,
    state: &AppState,
    project_id: &str,
    input: Option<Value>,
) -> Result<ProviderProjectTestRecord, String> {
    let build = build_project(root, project_id)?;
    let module_bytes = fs::read(&build.build_artifact_path).map_err(|error| {
        format!(
            "failed to read build artifact {}: {error}",
            build.build_artifact_path
        )
    })?;
    let project_dir = validate_project_dir(root, project_id)?;
    let manifest = load_manifest(&project_dir)?;
    let input = input.unwrap_or_else(|| default_project_input(&manifest));
    let output = if manifest.source_kind == "python" {
        let source = fs::read_to_string(&build.build_artifact_path).map_err(|error| {
            format!(
                "failed to read build artifact {}: {error}",
                build.build_artifact_path
            )
        })?;
        let tempdir = std::env::temp_dir().join(format!(
            "froglet-project-python-{}",
            crate::discovery::random_hex(16)
        ));
        fs::create_dir_all(&tempdir)
            .map_err(|error| format!("failed to create python tempdir: {error}"))?;
        let source_path = tempdir.join("main.py");
        fs::write(&source_path, &source)
            .map_err(|error| format!("failed to write python source: {error}"))?;
        let input_bytes = canonical_json::to_vec(&input).map_err(|error| error.to_string())?;
        let runner = r#"
import json, os, sys
source_path = sys.argv[1]
namespace = {"__name__": "__froglet__", "__file__": source_path}
event = json.load(sys.stdin)
with open(source_path, "r", encoding="utf-8") as handle:
    source = handle.read()
exec(compile(source, source_path, "exec"), namespace)
handler = namespace.get("handler")
if callable(handler):
    result = handler(event, {"mounts": {}})
elif "result" in namespace:
    result = namespace["result"]
elif callable(namespace.get("main")):
    result = namespace["main"](event, {"mounts": {}})
else:
    raise RuntimeError("python project must define handler, main, or result")
json.dump(result, sys.stdout, separators=(",", ":"))
"#;
        let mut child = std::process::Command::new("python3")
            .arg("-I")
            .arg("-c")
            .arg(runner)
            .arg(source_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to spawn python3: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write as _;
            stdin
                .write_all(&input_bytes)
                .map_err(|error| format!("failed to write python input: {error}"))?;
        }
        let deadline = std::time::Duration::from_secs(state.config.execution_timeout_secs);
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if start.elapsed() > deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "python test execution timed out after {}s",
                        deadline.as_secs()
                    ));
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(error) => return Err(format!("failed waiting for python execution: {error}")),
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("failed waiting for python execution: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "python execution failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        serde_json::from_slice::<Value>(&output.stdout)
            .map_err(|error| format!("python execution returned invalid JSON: {error}"))?
    } else if manifest.contract_version == WASM_HOST_JSON_ABI_V1 {
        let Some(host_environment) = state.wasm_host.clone() else {
            return Err("unsupported_capability: froglet.wasm.host_json.v1 requires a configured Wasm host environment".to_string());
        };
        state
            .wasm_sandbox
            .execute_module_with_options(
                &module_bytes,
                &input,
                sandbox::WasmExecutionOptions {
                    abi_version: manifest.contract_version.clone(),
                    capabilities_granted: host_environment.advertised_capabilities(),
                    host_environment: Some(host_environment),
                },
                Duration::from_secs(5),
            )
            .map_err(|error| error.to_string())?
    } else {
        state
            .wasm_sandbox
            .execute_module(&module_bytes, &input, Duration::from_secs(5))
            .map_err(|error| error.to_string())?
    };
    Ok(ProviderProjectTestRecord {
        project: build.project.clone(),
        build_artifact_path: build.build_artifact_path,
        module_hash: build.module_hash,
        input,
        output,
    })
}

pub fn load_manifest(project_dir: &Path) -> Result<ProviderProjectManifest, String> {
    let manifest_path = project_dir.join(MANIFEST_FILE_NAME);
    let manifest_text = fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
            "failed to read project manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let probe: ProjectManifestVersionProbe = toml::from_str(&manifest_text)
        .map_err(|error| format!("invalid project manifest: {error}"))?;
    let is_legacy_v1 = probe.schema_version == "froglet-service/v1";
    let manifest = if is_legacy_v1 {
        let legacy: ProviderProjectManifestV1 = toml::from_str(&manifest_text)
            .map_err(|error| format!("invalid project manifest: {error}"))?;
        migrate_manifest_v1(legacy)
    } else {
        toml::from_str::<ProviderProjectManifest>(&manifest_text)
            .map_err(|error| format!("invalid project manifest: {error}"))?
    };
    if is_legacy_v1 {
        let _ = save_manifest(project_dir, &manifest);
    }
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn save_manifest(project_dir: &Path, manifest: &ProviderProjectManifest) -> Result<(), String> {
    validate_manifest(manifest)?;
    let manifest_text = toml::to_string_pretty(manifest)
        .map_err(|error| format!("failed to encode project manifest: {error}"))?;
    fs::write(project_dir.join(MANIFEST_FILE_NAME), manifest_text)
        .map_err(|error| format!("failed to write project manifest: {error}"))
}

fn validate_manifest(manifest: &ProviderProjectManifest) -> Result<(), String> {
    if manifest.schema_version != PROJECT_SCHEMA_VERSION {
        return Err(format!(
            "unsupported project schema_version: {}",
            manifest.schema_version
        ));
    }
    let runtime = ExecutionRuntime::parse(&manifest.runtime)?;
    let package_kind = ExecutionPackageKind::parse(&manifest.package_kind)?;
    let entrypoint_kind = ExecutionEntrypointKind::parse(&manifest.entrypoint_kind)?;
    if manifest.source_kind != "wat" && manifest.source_kind != "python" {
        return Err("provider project source_kind must be wat or python".to_string());
    }
    if manifest.source_kind == "wat"
        && manifest.contract_version != WASM_RUN_JSON_ABI_V1
        && manifest.contract_version != WASM_HOST_JSON_ABI_V1
    {
        return Err(format!(
            "unsupported provider project contract_version: {}",
            manifest.contract_version
        ));
    }
    if manifest.source_kind == "wat"
        && (runtime != ExecutionRuntime::Wasm || package_kind != ExecutionPackageKind::InlineModule)
    {
        return Err(
            "WAT provider projects must use runtime=wasm and package_kind=inline_module"
                .to_string(),
        );
    }
    if manifest.source_kind == "python"
        && (runtime != ExecutionRuntime::Python
            || package_kind != ExecutionPackageKind::InlineSource)
    {
        return Err(
            "Python provider projects must use runtime=python and package_kind=inline_source"
                .to_string(),
        );
    }
    if runtime == ExecutionRuntime::Builtin || package_kind == ExecutionPackageKind::Builtin {
        return Err("provider projects cannot use builtin execution".to_string());
    }
    if entrypoint_kind == ExecutionEntrypointKind::Builtin {
        return Err("provider projects cannot use entrypoint_kind=builtin".to_string());
    }
    if manifest.publication_state != "active" && manifest.publication_state != "hidden" {
        return Err("provider project publication_state must be active or hidden".to_string());
    }
    if manifest.mode != "sync" && manifest.mode != "async" {
        return Err("provider project mode must be sync or async".to_string());
    }
    validate_slug(&manifest.project_id, "project_id")?;
    validate_slug(&manifest.service_id, "service_id")?;
    validate_slug(&manifest.offer_id, "offer_id")?;
    if manifest.summary.trim().is_empty() {
        return Err("provider project summary must not be empty".to_string());
    }
    validate_relative_file_path(&manifest.entrypoint, "entrypoint")?;
    Ok(())
}

fn migrate_manifest_v1(manifest: ProviderProjectManifestV1) -> ProviderProjectManifest {
    let runtime = if manifest.runtime.trim().is_empty() {
        if manifest.source_kind == "python" {
            "python".to_string()
        } else if manifest.execution_kind == "builtin" {
            "builtin".to_string()
        } else {
            "wasm".to_string()
        }
    } else {
        manifest.runtime
    };
    let package_kind = if manifest.package_kind.trim().is_empty() {
        match manifest.execution_kind.as_str() {
            "wasm_oci" => "oci_image".to_string(),
            "builtin" => "builtin".to_string(),
            _ => {
                if manifest.source_kind == "python" {
                    "inline_source".to_string()
                } else {
                    "inline_module".to_string()
                }
            }
        }
    } else {
        manifest.package_kind
    };
    let entrypoint_kind = if manifest.entrypoint_kind.trim().is_empty() {
        if runtime == "builtin" {
            "builtin".to_string()
        } else {
            "handler".to_string()
        }
    } else {
        manifest.entrypoint_kind
    };
    let contract_version = if manifest.contract_version.trim().is_empty() {
        if manifest.source_kind == "python" {
            CONTRACT_PYTHON_HANDLER_JSON_V1.to_string()
        } else {
            manifest.abi_version.clone()
        }
    } else {
        manifest.contract_version
    };
    ProviderProjectManifest {
        schema_version: PROJECT_SCHEMA_VERSION.to_string(),
        project_id: manifest.project_id,
        service_id: manifest.service_id,
        offer_id: manifest.offer_id,
        summary: manifest.summary,
        runtime,
        package_kind,
        entrypoint_kind,
        contract_version,
        mounts: manifest.mounts,
        mode: manifest.mode,
        source_kind: manifest.source_kind,
        entrypoint: manifest.entrypoint,
        price_sats: manifest.price_sats,
        publication_state: manifest.publication_state,
        starter: manifest.starter,
        input_schema: manifest.input_schema,
        output_schema: manifest.output_schema,
    }
}

fn validate_slug(value: &str, field_name: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field_name} must not be empty"));
    }
    if value.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '-' | '_' | '.')
    }) {
        Ok(())
    } else {
        Err(format!(
            "{field_name} must contain only lowercase ASCII letters, digits, '.', '-', or '_'"
        ))
    }
}

fn project_record(
    project_dir: &Path,
    manifest: ProviderProjectManifest,
) -> Result<ProviderProjectRecord, String> {
    let build_name = if manifest.source_kind == "python" {
        BUILD_PYTHON_ARTIFACT_NAME
    } else {
        BUILD_ARTIFACT_NAME
    };
    let build_path = project_dir.join(BUILD_DIR_NAME).join(build_name);
    let (build_artifact_path, module_hash) = if build_path.is_file() {
        let bytes = fs::read(&build_path).map_err(|error| {
            format!(
                "failed to read build artifact {}: {error}",
                build_path.display()
            )
        })?;
        (
            Some(build_path.display().to_string()),
            Some(crypto::sha256_hex(bytes)),
        )
    } else {
        (None, None)
    };
    Ok(ProviderProjectRecord {
        project_id: manifest.project_id,
        service_id: manifest.service_id,
        offer_id: manifest.offer_id,
        summary: manifest.summary,
        runtime: manifest.runtime,
        package_kind: manifest.package_kind,
        entrypoint_kind: manifest.entrypoint_kind,
        contract_version: manifest.contract_version,
        mounts: manifest.mounts,
        mode: manifest.mode,
        source_kind: manifest.source_kind,
        entrypoint: manifest.entrypoint,
        price_sats: manifest.price_sats,
        publication_state: manifest.publication_state,
        starter: manifest.starter,
        input_schema: manifest.input_schema,
        output_schema: manifest.output_schema,
        build_artifact_path,
        module_hash,
    })
}

fn validate_project_dir(root: &Path, project_id: &str) -> Result<PathBuf, String> {
    validate_slug(project_id, "project_id")?;
    Ok(root.join(project_id))
}

fn validate_relative_file_path(value: &str, field_name: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field_name} must not be empty"));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(format!("{field_name} must be a relative path"));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("{field_name} must not traverse parent directories"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{field_name} must be a relative path"));
            }
        }
    }
    Ok(())
}

fn resolve_relative_path(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    validate_relative_file_path(relative_path, "path")?;
    let joined = root.join(relative_path);
    if let Some(parent) = joined.parent() {
        ensure_non_symlink_tree(root, parent)?;
    }
    // Reject symlink leaves to prevent escaping the project root.
    if let Ok(metadata) = fs::symlink_metadata(&joined)
        && metadata.file_type().is_symlink()
    {
        return Err(format!(
            "symlink paths are not allowed in provider projects: {}",
            joined.display()
        ));
    }
    Ok(joined)
}

fn ensure_non_symlink_tree(root: &Path, target: &Path) -> Result<(), String> {
    if !target.starts_with(root) {
        return Err("path escapes project root".to_string());
    }
    let relative = target
        .strip_prefix(root)
        .map_err(|_| "path escapes project root".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if let Ok(metadata) = fs::symlink_metadata(&current)
            && metadata.file_type().is_symlink()
        {
            return Err(format!(
                "symlink paths are not allowed in provider projects: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::json;

    use crate::{
        confidential::ConfidentialConfig,
        config::{
            DiscoveryMode, IdentityConfig, LightningConfig, LightningMode, NetworkMode, NodeConfig,
            PaymentBackend, PricingConfig, StorageConfig, TorSidecarConfig, WasmConfig,
        },
        state,
    };

    static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "froglet-provider-projects-{label}-{}-{unique}-{counter}",
            std::process::id()
        ))
    }

    fn test_app_state() -> std::sync::Arc<AppState> {
        let temp_dir = unique_temp_dir("state");
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        let node_config = NodeConfig {
            network_mode: NetworkMode::Clearnet,
            listen_addr: "127.0.0.1:0".to_string(),
            public_base_url: None,
            runtime_listen_addr: "127.0.0.1:0".to_string(),
            runtime_allow_non_loopback: false,
            provider_control_listen_addr: "127.0.0.1:0".to_string(),
            provider_control_allow_non_loopback: false,
            http_ca_cert_path: None,
            tor: TorSidecarConfig {
                binary_path: "tor".to_string(),
                backend_listen_addr: "127.0.0.1:0".to_string(),
                startup_timeout_secs: 90,
            },
            discovery_mode: DiscoveryMode::None,
            identity: IdentityConfig {
                auto_generate: true,
            },
            reference_discovery: None,
            pricing: PricingConfig {
                events_query: 0,
                execute_wasm: 0,
            },
            payment_backend: PaymentBackend::None,
            execution_timeout_secs: 5,
            lightning: LightningConfig {
                mode: LightningMode::Mock,
                destination_identity: None,
                base_invoice_expiry_secs: 300,
                success_hold_expiry_secs: 300,
                min_final_cltv_expiry: 18,
                sync_interval_ms: 100,
                lnd_rest: None,
            },
            storage: StorageConfig {
                data_dir: temp_dir.clone(),
                db_path: temp_dir.join("node.db"),
                identity_dir: temp_dir.join("identity"),
                identity_seed_path: temp_dir.join("identity/secp256k1.seed"),
                nostr_publication_seed_path: temp_dir
                    .join("identity/nostr-publication.secp256k1.seed"),
                runtime_dir: temp_dir.join("runtime"),
                runtime_auth_token_path: temp_dir.join("runtime/auth.token"),
                consumer_control_auth_token_path: temp_dir.join("runtime/consumerctl.token"),
                provider_control_auth_token_path: temp_dir.join("runtime/froglet-control.token"),
                tor_dir: temp_dir.join("tor"),
                host_readable_control_token: false,
            },
            wasm: WasmConfig {
                policy_path: None,
                policy: None,
            },
            confidential: ConfidentialConfig {
                policy_path: None,
                policy: None,
                session_ttl_secs: 300,
            },
        };
        state::build_app_state(node_config).expect("build app state")
    }

    #[test]
    fn hello_world_project_builds_and_executes() {
        let state = test_app_state();
        let root = projects_root_from_data_dir(&state.config.storage.data_dir);
        create_project(
            &root,
            "hello-world",
            "hello-world",
            "hello-world",
            Some(ProviderProjectStarter::HelloWorld),
            "Hello World service",
            0,
            "active",
            CreateProjectOverrides::default(),
        )
        .expect("create project");

        let build = build_project(&root, "hello-world").expect("build project");
        assert!(Path::new(&build.build_artifact_path).is_file());

        let result =
            test_project(&root, state.as_ref(), "hello-world", None).expect("test project");
        assert_eq!(result.output, json!({ "message": "Hello World" }));
    }

    #[test]
    fn echo_json_project_round_trips_input() {
        let state = test_app_state();
        let root = projects_root_from_data_dir(&state.config.storage.data_dir);
        create_project(
            &root,
            "echo-json",
            "echo-json",
            "echo-json",
            Some(ProviderProjectStarter::EchoJson),
            "Echo JSON service",
            0,
            "active",
            CreateProjectOverrides::default(),
        )
        .expect("create project");

        let input = json!({ "hello": "world", "count": 2 });
        let result = test_project(&root, state.as_ref(), "echo-json", Some(input.clone()))
            .expect("test project");
        assert_eq!(result.output, input);
    }

    #[test]
    fn static_result_project_returns_configured_json() {
        let state = test_app_state();
        let root = projects_root_from_data_dir(&state.config.storage.data_dir);
        create_project(
            &root,
            "lol",
            "lol",
            "lol",
            Some(ProviderProjectStarter::BlankRunJson),
            "Returns lol",
            0,
            "active",
            CreateProjectOverrides::default(),
        )
        .expect("create project");
        write_static_result_project(&root, "lol", &json!("lol")).expect("write static result");

        let result = test_project(
            &root,
            state.as_ref(),
            "lol",
            Some(json!({"input": "ignored"})),
        )
        .expect("test project");
        assert_eq!(result.output, json!("lol"));
    }

    #[test]
    fn host_template_requires_configured_wasm_host() {
        let state = test_app_state();
        let root = projects_root_from_data_dir(&state.config.storage.data_dir);
        create_project(
            &root,
            "http-fetch",
            "http-fetch",
            "http-fetch",
            Some(ProviderProjectStarter::HttpFetchPassthrough),
            "HTTP fetch passthrough",
            0,
            "active",
            CreateProjectOverrides::default(),
        )
        .expect("create project");

        let error = test_project(&root, state.as_ref(), "http-fetch", None)
            .expect_err("expected host capability error");
        assert!(error.contains("unsupported_capability"));
    }

    #[test]
    fn project_file_writes_reject_parent_traversal() {
        let state = test_app_state();
        let root = projects_root_from_data_dir(&state.config.storage.data_dir);
        create_project(
            &root,
            "blank-project",
            "blank-project",
            "blank-project",
            Some(ProviderProjectStarter::BlankRunJson),
            "Blank project",
            0,
            "active",
            CreateProjectOverrides::default(),
        )
        .expect("create project");

        let error = write_project_file(&root, "blank-project", "../escape.wat", "(module)")
            .expect_err("expected traversal error");
        assert!(error.contains("must not traverse parent directories"));
    }
}
