//! Private authoring metadata and selected-data snapshots. Publication still
//! goes through the canonical manifest loader and publish engine.
use super::{CliError, pop_flag, pop_kv};
use crate::execution::BuiltinServiceHandler;
use froglet_protocol::publication::{PublicationCsvColumn, PublicationCsvColumnType};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ROWS: usize = 100_000;
const MAX_PROJECTS: usize = 128;
type Dataset = BTreeMap<String, Vec<Map<String, Value>>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareRequest {
    pub source: PathBuf,
    #[serde(default)]
    pub destination: Option<PathBuf>,
    #[serde(default)]
    pub service_id: Option<String>,
    #[serde(default)]
    pub selection: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub csv_columns: Vec<PublicationCsvColumn>,
    #[serde(default)]
    pub example_input: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Preparation {
    pub schema_version: String,
    pub request: PrepareRequest,
    pub source_sha256: String,
    pub snapshot_sha256: String,
    pub manifest_sha256: String,
    pub snapshot_file: String,
    pub prepared_at: i64,
}

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let _json = pop_flag(&mut args, "--json");
    let request_path = pop_kv(&mut args, "--request").ok_or_else(|| CliError::BadArgs("usage: froglet-node prepare-service --request FILE [--json]; FILE contains source, destination, service_id, selection and optional csv_columns/example_input".into()))?;
    if !args.is_empty() {
        return Err(CliError::BadArgs(
            "unexpected prepare-service arguments".into(),
        ));
    }
    let request = serde_json::from_slice(&bounded_read(Path::new(&request_path))?)
        .map_err(|e| CliError::BadArgs(format!("invalid preparation request: {e}")))?;
    println!("{}", prepare(request, &data_root()).await?);
    Ok(())
}

pub fn run_updates(mut args: Vec<String>) -> Result<(), CliError> {
    pop_flag(&mut args, "--json");
    if !args.is_empty() {
        return Err(CliError::BadArgs(
            "usage: froglet-node check-updates [--json]".into(),
        ));
    }
    println!("{}", check_updates(&data_root())?);
    Ok(())
}

pub(crate) fn data_root() -> PathBuf {
    std::env::var_os("FROGLET_DATA_ROOT")
        .or_else(|| std::env::var_os("FROGLET_DATA_DIR"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".froglet/data")))
        .unwrap_or_else(|| PathBuf::from("data"))
}

fn bad(message: impl Into<String>) -> CliError {
    CliError::BadArgs(message.into())
}

fn bounded_read(path: &Path) -> Result<Vec<u8>, CliError> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(bad(
            "source must be a regular file, not a directory or symlink",
        ));
    }
    if meta.len() > MAX_BYTES as u64 {
        return Err(bad(
            "source exceeds the 16 MiB preparation limit; select a smaller export",
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(bad("source grew past the 16 MiB preparation limit"));
    }
    Ok(bytes)
}

fn source_fingerprint(path: &Path) -> Result<String, CliError> {
    let mut material = bounded_read(path)?;
    if matches!(extension(path), "sqlite" | "sqlite3" | "db") {
        let wal = PathBuf::from(format!("{}-wal", path.display()));
        match bounded_read(&wal) {
            Ok(bytes) => {
                material.extend_from_slice(b"\0sqlite-wal\0");
                material.extend(bytes);
            }
            Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(crate::crypto::sha256_hex(material))
}

fn extension(path: &Path) -> &str {
    path.extension().and_then(|v| v.to_str()).unwrap_or("")
}

pub async fn prepare(mut request: PrepareRequest, state_root: &Path) -> Result<Value, CliError> {
    if !request.source.is_absolute() {
        return Err(bad("source must be an absolute path"));
    }
    // Reject a symlink before canonicalization, which would hide that fact.
    let source_hash = source_fingerprint(&request.source)?;
    request.source = request.source.canonicalize()?;
    let wasm = matches!(extension(&request.source), "wat" | "wasm");
    let (original, description) = if wasm {
        (BTreeMap::new(), json!({}))
    } else {
        read_dataset(&request)?
    };
    if request.destination.is_none()
        || request.service_id.is_none()
        || (!wasm && request.selection.is_empty())
    {
        return Ok(
            json!({"status":"decision_required", "stage":"select_data", "collections":description,
            "source_sha256":source_hash, "next_action":"Propose the useful tables and fields. Supply an explicit new destination, service_id, and selection. CSV requires explicit csv_columns types validated against every row. Wasm requires example_input."}),
        );
    }
    let service_id = request.service_id.as_deref().unwrap();
    if service_id.is_empty()
        || service_id.len() > 63
        || !service_id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || service_id.starts_with('-')
        || service_id.ends_with('-')
    {
        return Err(bad(
            "service_id must contain 1–63 lowercase letters, digits, or interior hyphens",
        ));
    }
    let requested_destination = request.destination.as_ref().unwrap();
    if !requested_destination.is_absolute() {
        return Err(bad("destination must be an explicit absolute directory"));
    }
    let parent = requested_destination
        .parent()
        .ok_or_else(|| bad("destination has no parent"))?
        .canonicalize()?;
    let destination = parent.join(
        requested_destination
            .file_name()
            .ok_or_else(|| bad("destination must name a project directory"))?,
    );
    request.destination = Some(destination.clone());
    if !destination.is_absolute() || destination == request.source {
        return Err(bad(
            "destination must be an explicit absolute project directory, different from the source",
        ));
    }
    let _lock = ProjectLock::acquire(&destination)?;
    if fs::symlink_metadata(&destination).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(bad("destination must not be a symlink"));
    }
    let previous = read_preparation(&destination)?;
    if destination.exists() && previous.is_none() {
        return Err(bad(
            "destination already exists without a Froglet preparation record; choose a new directory",
        ));
    }
    if let Some(previous) = &previous {
        if previous.request.service_id != request.service_id
            || previous.request.source != request.source
        {
            return Err(bad("destination belongs to another prepared service"));
        }
        verify_generated_files(&destination, previous)?;
    }
    let parent = destination
        .parent()
        .ok_or_else(|| bad("destination has no parent directory"))?;
    if !parent.is_dir() {
        return Err(bad(
            "destination parent must exist; choose a directory in the current project",
        ));
    }
    let staging = parent.join(format!(".froglet-prepare-{:016x}", rand::random::<u64>()));
    private_dir(&staging)?;
    let _cleanup = Staging(staging.clone());
    let (snapshot, example_input, example_result, included, omitted) = if wasm {
        let input = request
            .example_input
            .clone()
            .ok_or_else(|| bad("Wasm preparation requires a meaningful example_input"))?;
        let bytes = bounded_read(&request.source)?;
        let bytes = if extension(&request.source) == "wat" {
            wat::parse_bytes(&bytes)
                .map_err(|e| bad(format!("invalid Wasm source: {e}")))?
                .into_owned()
        } else {
            bytes
        };
        let sandbox = crate::sandbox::WasmSandbox::new(1).map_err(bad)?;
        let result = sandbox
            .execute_module(&bytes, &input, Duration::from_secs(5))
            .map_err(|e| bad(format!("local example failed: {e}")))?;
        (bytes, input, result, json!([]), json!([]))
    } else {
        let selected = select(&original, &request.selection, &description)?;
        let bytes = serde_json::to_vec(&selected).map_err(|e| bad(e.to_string()))?;
        if bytes.len() > MAX_BYTES {
            return Err(bad("selected snapshot exceeds 16 MiB"));
        }
        private_write(&staging.join("preview.json"), &bytes)?;
        let handler = crate::builtins::DataQueryHandler::open(
            &staging,
            "preview.json",
            crate::builtins::DataQuerySourceKind::Json,
        )
        .map_err(bad)?;
        let collection = request.selection.keys().next().unwrap();
        let input = request.example_input.clone().unwrap_or_else(|| json!({"op":"select", "collection":collection, "columns":request.selection[collection], "limit":3}));
        let result = handler
            .execute(input.clone())
            .await
            .map_err(|e| bad(format!("local example failed: {e}")))?;
        let omitted: BTreeMap<_, _> = description
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, info)| {
                let all = info["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let selected = request.selection.get(name).cloned().unwrap_or_default();
                (
                    name.clone(),
                    all.into_iter()
                        .filter(|column| !selected.contains(column))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        (
            bytes,
            input,
            result,
            json!(request.selection),
            json!(omitted),
        )
    };
    if source_fingerprint(&request.source)? != source_hash {
        return Err(bad(
            "source_changed: source changed while preparing; retry after saving it",
        ));
    }
    let snapshot_hash = crate::crypto::sha256_hex(&snapshot);
    let snapshot_file = format!(
        "snapshot-{snapshot_hash}.{}",
        if wasm { "wasm" } else { "json" }
    );
    let mut manifest = json!({"schema_version":"froglet-service/v4", "service_id":service_id, "summary":format!("Shared {}", service_id), "hosting":{"default":"relay"}, "settlement":{"method":"none"}, "price":{"sats":0}, "verification":{"input":example_input}});
    if wasm {
        manifest["runtime"] = json!("wasm");
        manifest["package_kind"] = json!("inline_module");
        manifest["entrypoint"] = json!(snapshot_file);
        manifest["entrypoint_kind"] = json!("module");
        manifest["contract_version"] = json!("froglet.wasm.run_json.v1");
    } else {
        manifest["runtime"] = json!("builtin");
        manifest["package_kind"] = json!("builtin");
        manifest["contract_version"] = json!("froglet.builtin.data_query.json.v1");
        manifest["data"] = json!({"path":snapshot_file, "format":"json"});
    }
    // `starter` is already part of the public, signed service metadata and
    // publication consent. Keep it distinct from the private verification input.
    manifest["starter"] =
        json!(serde_json::to_string(&example_input).map_err(|e| bad(e.to_string()))?);
    let manifest_text = toml::to_string_pretty(&manifest).map_err(|e| bad(e.to_string()))?;
    froglet_protocol::manifest::ServiceManifest::from_toml(&manifest_text)?;
    let record = Preparation {
        schema_version: "froglet.preparation.v1".into(),
        request: request.clone(),
        source_sha256: source_hash,
        snapshot_sha256: snapshot_hash,
        manifest_sha256: crate::crypto::sha256_hex(&manifest_text),
        snapshot_file: snapshot_file.clone(),
        prepared_at: crate::settlement::current_unix_timestamp(),
    };
    let registry = state_root.join("prepared-services");
    private_dir(&registry)?;
    let key = crate::crypto::sha256_hex(destination.to_string_lossy().as_bytes());
    let registry_path = registry.join(format!("{key}.json"));
    if !registry_path.exists() && fs::read_dir(&registry)?.count() >= MAX_PROJECTS {
        return Err(bad(
            "128 prepared projects are already registered; remove unused preparation records before adding more",
        ));
    }
    let record_bytes = serde_json::to_vec_pretty(&record).map_err(|e| bad(e.to_string()))?;
    // New projects appear all at once. Updates journal the next private record
    // before switching the manifest; readers select the record by manifest hash.
    let writing = if previous.is_none() {
        &staging
    } else {
        &destination
    };
    private_dir(&writing.join(".froglet"))?;
    let snapshot_path = writing.join(&snapshot_file);
    if snapshot_path.exists() {
        if bounded_read(&snapshot_path)? != snapshot {
            return Err(bad("prepared_files_changed: immutable snapshot was edited"));
        }
    } else {
        private_write(&snapshot_path, &snapshot)?;
    }
    atomic_private_write(
        &writing.join(".froglet/preparation-pending.json"),
        &record_bytes,
    )?;
    atomic_private_write(
        &writing.join("froglet-service.toml"),
        manifest_text.as_bytes(),
    )?;
    atomic_private_write(&writing.join(".froglet/preparation.json"), &record_bytes)?;
    fs::remove_file(writing.join(".froglet/preparation-pending.json"))?;
    if previous.is_none() {
        private_write(&staging.join(".gitignore"), b".froglet/\n")?;
        // The preview file is not part of the generated project.
        let _ = fs::remove_file(staging.join("preview.json"));
        fs::rename(&staging, &destination)?;
    }
    atomic_private_write(&registry_path, &record_bytes)?;
    Ok(
        json!({"status":"prepared", "stage":"local_example_verified", "project_dir":destination, "service_id":service_id,
        "included":included, "omitted":omitted, "example_input":example_input, "example_result":example_result,
        "snapshot_sha256":record.snapshot_sha256, "source":request.source, "source_sha256":record.source_sha256, "public":false, "next_action":"Review the selected data and example. Call marketplace_publish to prepare the exact public approval; nothing has been published."}),
    )
}

fn columns(rows: &[Map<String, Value>]) -> BTreeSet<String> {
    rows.iter().flat_map(|row| row.keys().cloned()).collect()
}
fn describe(data: &Dataset) -> Value {
    json!(
        data.iter()
            .map(|(name, rows)| (name, json!({"rows":rows.len(), "fields":columns(rows)})))
            .collect::<BTreeMap<_, _>>()
    )
}
fn select(
    data: &Dataset,
    selection: &BTreeMap<String, Vec<String>>,
    description: &Value,
) -> Result<Dataset, CliError> {
    let mut result = Dataset::new();
    for (name, selected) in selection {
        let rows = data
            .get(name)
            .ok_or_else(|| bad(format!("unknown collection {name:?}")))?;
        if rows.is_empty() {
            return Err(bad(format!(
                "collection {name:?} has no rows; choose a nonempty catalog so Froglet can verify a meaningful local example"
            )));
        }
        let all: BTreeSet<&str> = description[name]["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if selected.is_empty()
            || selected.len() > 64
            || selected.iter().collect::<BTreeSet<_>>().len() != selected.len()
            || selected.iter().any(|field| !all.contains(field.as_str()))
        {
            return Err(bad(format!(
                "select 1–64 unique existing fields from {name:?}"
            )));
        }
        result.insert(
            name.clone(),
            rows.iter()
                .map(|row| {
                    selected
                        .iter()
                        .map(|key| (key.clone(), row.get(key).cloned().unwrap_or(Value::Null)))
                        .collect()
                })
                .collect(),
        );
    }
    Ok(result)
}

fn read_dataset(request: &PrepareRequest) -> Result<(Dataset, Value), CliError> {
    let data = match extension(&request.source) {
        "json" => {
            let value: Value = serde_json::from_slice(&bounded_read(&request.source)?)
                .map_err(|e| bad(format!("invalid JSON: {e}")))?;
            let collections = match value {
                Value::Array(rows) => BTreeMap::from([("rows".into(), Value::Array(rows))]),
                Value::Object(map) => map.into_iter().collect(),
                _ => return Err(bad("JSON must be rows or an object of named row arrays")),
            };
            let mut data = Dataset::new();
            for (name, value) in collections {
                let rows = value
                    .as_array()
                    .ok_or_else(|| bad(format!("collection {name:?} must be an array")))?;
                data.insert(
                    name,
                    rows.iter()
                        .map(|row| {
                            row.as_object()
                                .cloned()
                                .ok_or_else(|| bad("every JSON row must be an object"))
                        })
                        .collect::<Result<_, _>>()?,
                );
            }
            data
        }
        "csv" => read_csv(request)?,
        "sqlite" | "sqlite3" | "db" => return read_sqlite(request),
        _ => {
            return Err(bad(
                "supported sources are JSON, CSV, SQLite, WAT, and Wasm; Python is an advanced Linux workflow",
            ));
        }
    };
    if data.is_empty()
        || data.len() > 128
        || data
            .values()
            .any(|rows| rows.len() > MAX_ROWS || columns(rows).len() > 256)
    {
        return Err(bad(
            "data exceeds preparation limits: 128 collections, 100,000 rows per collection, 256 fields",
        ));
    }
    let description = describe(&data);
    Ok((data, description))
}

fn read_csv(request: &PrepareRequest) -> Result<Dataset, CliError> {
    let bytes = bounded_read(&request.source)?;
    let mut reader = csv::Reader::from_reader(bytes.as_slice());
    let headers = reader.headers().map_err(|e| bad(e.to_string()))?.clone();
    if headers.is_empty()
        || headers.len() > 256
        || headers.iter().collect::<BTreeSet<_>>().len() != headers.len()
    {
        return Err(bad("CSV needs unique headers, at most 256"));
    }
    let typed = !request.csv_columns.is_empty();
    if typed
        && (request.csv_columns.len() != headers.len()
            || headers
                .iter()
                .zip(&request.csv_columns)
                .any(|(a, b)| a != b.name))
    {
        return Err(bad(
            "csv_columns must declare every header in its original order",
        ));
    }
    if !typed && !request.selection.is_empty() {
        return Err(bad(format!(
            "csv_types_required: supply csv_columns with an explicit type for every header: {headers:?}; use string when identifiers or mixed values must be preserved"
        )));
    }
    let mut rows = Vec::new();
    for (index, record) in reader.records().enumerate() {
        if index >= MAX_ROWS {
            return Err(bad(
                "CSV preparation supports at most 100,000 rows; export a smaller catalog",
            ));
        }
        let record = record.map_err(|e| bad(format!("CSV row {}: {e}", index + 2)))?;
        let mut row = Map::new();
        for (i, text) in record.iter().enumerate() {
            if text.len() > 64 * 1024 {
                return Err(bad("CSV cell exceeds 64 KiB"));
            }
            let value = if typed {
                csv_value(text, &request.csv_columns[i]).map_err(|e| {
                    bad(format!(
                        "CSV row {}, field {:?}: {e}",
                        index + 2,
                        &headers[i]
                    ))
                })?
            } else {
                json!(text)
            };
            row.insert(headers[i].to_string(), value);
        }
        rows.push(row);
    }
    Ok(BTreeMap::from([("rows".into(), rows)]))
}

fn csv_value(text: &str, column: &PublicationCsvColumn) -> Result<Value, String> {
    if text.is_empty() && column.nullable {
        return Ok(Value::Null);
    }
    match column.column_type {
        PublicationCsvColumnType::String => Ok(json!(text)),
        PublicationCsvColumnType::Integer => text
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| "expected an integer; choose string if formatting matters".into()),
        PublicationCsvColumnType::Number => text
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| "expected a finite number".into()),
        PublicationCsvColumnType::Boolean => match text {
            "true" => Ok(json!(true)),
            "false" => Ok(json!(false)),
            _ => Err("expected true or false".into()),
        },
    }
}

fn read_sqlite(request: &PrepareRequest) -> Result<(Dataset, Value), CliError> {
    let path = &request.source;
    let sql_error = |e: rusqlite::Error| bad(format!("SQLite source: {e}"));
    let mut conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sql_error)?;
    conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")
        .map_err(sql_error)?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(sql_error)?;
    let started = std::time::Instant::now();
    conn.progress_handler(
        10_000,
        Some(move || started.elapsed() > Duration::from_secs(5)),
    )
    .map_err(sql_error)?;
    let tx = conn.transaction().map_err(sql_error)?;
    let names = tx.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND sql NOT LIKE 'CREATE VIRTUAL%' ORDER BY name LIMIT 129").map_err(sql_error)?
        .query_map([], |row| row.get::<_,String>(0)).map_err(sql_error)?.collect::<Result<Vec<_>,_>>().map_err(sql_error)?;
    if names.len() > 128 {
        return Err(bad(
            "SQLite source has more than 128 tables; export a smaller database",
        ));
    }
    let mut data = Dataset::new();
    let mut total_bytes = 0usize;
    let mut description = Map::new();
    for name in names {
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
        let fields = tx
            .prepare(&format!("SELECT * FROM {quoted} LIMIT 0"))
            .map_err(sql_error)?
            .column_names()
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>();
        if fields.len() > 256 {
            return Err(bad("SQLite table has more than 256 fields"));
        }
        description.insert(name.clone(), json!({"rows":null, "fields":fields}));
        let Some(selected) = request.selection.get(&name) else {
            continue;
        };
        if selected.is_empty()
            || selected.len() > 64
            || selected.iter().collect::<BTreeSet<_>>().len() != selected.len()
            || selected.iter().any(|field| !fields.contains(field))
        {
            return Err(bad(format!(
                "select 1–64 unique existing fields from {name:?}"
            )));
        }
        let projection = selected
            .iter()
            .map(|field| format!("\"{}\"", field.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",");
        let mut statement = tx
            .prepare(&format!(
                "SELECT {projection} FROM {quoted} LIMIT {}",
                MAX_ROWS + 1
            ))
            .map_err(sql_error)?;
        let mut cursor = statement.query([]).map_err(sql_error)?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().map_err(sql_error)? {
            if rows.len() >= MAX_ROWS {
                return Err(bad(
                    "SQLite table exceeds 100,000 rows; export a smaller catalog",
                ));
            }
            let mut object = Map::new();
            for (index, field) in selected.iter().enumerate() {
                let value = match row.get_ref(index).map_err(sql_error)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                    rusqlite::types::ValueRef::Real(n) => serde_json::Number::from_f64(n)
                        .map(Value::Number)
                        .ok_or_else(|| bad("SQLite contains a non-finite number"))?,
                    rusqlite::types::ValueRef::Text(bytes) => json!(
                        std::str::from_utf8(bytes).map_err(|_| bad("SQLite text is not UTF-8"))?
                    ),
                    rusqlite::types::ValueRef::Blob(_) => {
                        return Err(bad(
                            "SQLite blobs need an explicit text/JSON export before preparation",
                        ));
                    }
                };
                total_bytes = total_bytes.saturating_add(value.to_string().len() + field.len());
                if total_bytes > MAX_BYTES {
                    return Err(bad(
                        "SQLite export exceeds 16 MiB; prepare a smaller database",
                    ));
                }
                object.insert(field.clone(), value);
            }
            rows.push(object);
        }
        description[&name]["rows"] = json!(rows.len());
        data.insert(name, rows);
    }
    for name in request.selection.keys() {
        if !data.contains_key(name) {
            return Err(bad(format!("unknown SQLite table {name:?}")));
        }
    }
    Ok((data, Value::Object(description)))
}

fn read_preparation(project: &Path) -> Result<Option<Preparation>, CliError> {
    let metadata_dir = project.join(".froglet");
    if fs::symlink_metadata(&metadata_dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(bad("private preparation metadata must not be a symlink"));
    }
    let pending = metadata_dir.join("preparation-pending.json");
    if pending.exists() {
        let record: Preparation = serde_json::from_slice(&bounded_read(&pending)?)
            .map_err(|e| bad(format!("invalid preparation journal: {e}")))?;
        if crate::crypto::sha256_hex(bounded_read(&project.join("froglet-service.toml"))?)
            == record.manifest_sha256
        {
            return Ok(Some(record));
        }
    }
    let path = metadata_dir.join("preparation.json");
    match bounded_read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| bad(format!("invalid preparation record: {e}"))),
        Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn verify_generated_files(project: &Path, record: &Preparation) -> Result<(), CliError> {
    if Path::new(&record.snapshot_file).components().count() != 1 {
        return Err(bad("invalid snapshot filename in preparation record"));
    }
    if crate::crypto::sha256_hex(bounded_read(&project.join(&record.snapshot_file))?)
        != record.snapshot_sha256
        || crate::crypto::sha256_hex(bounded_read(&project.join("froglet-service.toml"))?)
            != record.manifest_sha256
    {
        return Err(bad(
            "prepared_files_changed: generated files were edited; keep them and choose a new destination to prepare again",
        ));
    }
    Ok(())
}

pub(crate) fn check_prepared_source(project: &Path) -> Result<(), CliError> {
    if let Some(record) = read_preparation(project)? {
        verify_generated_files(project, &record)?;
        if source_fingerprint(&record.request.source)? != record.source_sha256 {
            return Err(bad(
                "source_changed: prepare the updated service and review its new publication plan; the published version is unchanged",
            ));
        }
    }
    Ok(())
}

pub fn check_updates(state_root: &Path) -> Result<Value, CliError> {
    let registry = state_root.join("prepared-services");
    let entries = match fs::read_dir(registry) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(
                json!({"status":"ok", "projects":[], "checked_at":crate::settlement::current_unix_timestamp()}),
            );
        }
        Err(e) => return Err(e.into()),
    };
    let mut projects = Vec::new();
    for entry in entries.take(MAX_PROJECTS) {
        let entry = entry?;
        if extension(&entry.path()) != "json" {
            continue;
        }
        let record: Preparation = match bounded_read(&entry.path())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string())))
        {
            Ok(record) => record,
            Err(_) => {
                projects.push(json!({"status":"record_unreadable", "next_action":"Inspect the local preparation record; no publication was changed"}));
                continue;
            }
        };
        let project_record = record.request.destination.as_deref().and_then(|project| {
            read_preparation(project).ok().flatten().filter(|latest| {
                latest.request.service_id == record.request.service_id
                    && latest.request.source == record.request.source
                    && verify_generated_files(project, latest).is_ok()
            })
        });
        let Some(record) = project_record else {
            projects.push(json!({"service_id":record.request.service_id, "project_dir":record.request.destination,
                "source":record.request.source, "status":"prepared_files_changed",
                "next_action":"The prepared project or its private record is missing, unreadable, or edited. Keep any edits and prepare into a new directory; the published revision is unchanged."}));
            continue;
        };
        let status = match source_fingerprint(&record.request.source) {
            Ok(hash) if hash == record.source_sha256 => "current",
            Ok(_) => "update_available",
            Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => "source_missing",
            Err(_) => "source_unreadable",
        };
        projects.push(json!({"service_id":record.request.service_id, "project_dir":record.request.destination, "source":record.request.source,
            "status":status, "prepared_at":record.prepared_at, "next_action":if status == "current" {"No source update detected"} else {"Review the source and call prepare_service again; publication requires a new approval"}}));
    }
    Ok(
        json!({"status":"ok", "projects":projects, "checked_at":crate::settlement::current_unix_timestamp(), "automatic_publication":false}),
    )
}

pub(crate) fn private_dir(path: &Path) -> Result<(), CliError> {
    fs::create_dir_all(path)?;
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(bad("private metadata directory must not be a symlink"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub(crate) fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let temporary = path.with_file_name(format!(".froglet-write-{:016x}", rand::random::<u64>()));
    private_write(&temporary, bytes)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(temporary);
        return Err(error.into());
    }
    Ok(())
}
struct ProjectLock;
impl ProjectLock {
    fn acquire(destination: &Path) -> Result<super::configure_agent::MetadataLock, CliError> {
        let path = destination.with_file_name(format!(
            ".{}.froglet-lock",
            destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
        super::configure_agent::lock_metadata(&path)
    }
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(root: &Path, source: &str) -> PrepareRequest {
        PrepareRequest {
            source: root.join(source),
            destination: Some(root.join("service")),
            service_id: Some("catalog".into()),
            selection: BTreeMap::from([("rows".into(), vec!["id".into(), "name".into()])]),
            csv_columns: vec![],
            example_input: None,
        }
    }
    #[tokio::test]
    async fn prepares_only_selected_data_and_detects_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("catalog.json"),
            br#"[{"id":1,"name":"Apple","secret":"never publish"}]"#,
        )
        .unwrap();
        let req = request(root, "catalog.json");
        let output = prepare(req.clone(), &root.join("state")).await.unwrap();
        assert_eq!(output["status"], "prepared");
        let record = read_preparation(&root.join("service")).unwrap().unwrap();
        let snapshot = fs::read_to_string(root.join("service").join(record.snapshot_file)).unwrap();
        assert!(!snapshot.contains("secret"));
        assert!(!snapshot.contains("never publish"));
        assert!(check_prepared_source(&root.join("service")).is_ok());
        fs::write(
            root.join("catalog.json"),
            br#"[{"id":2,"name":"Pear","secret":"hidden"}]"#,
        )
        .unwrap();
        assert_eq!(
            check_updates(&root.join("state")).unwrap()["projects"][0]["status"],
            "update_available"
        );
        assert!(check_prepared_source(&root.join("service")).is_err());
        prepare(req, &root.join("state")).await.unwrap();
        assert_eq!(
            check_updates(&root.join("state")).unwrap()["projects"][0]["status"],
            "current"
        );
        fs::write(root.join("service/.froglet/preparation.json"), b"broken").unwrap();
        assert_eq!(
            check_updates(&root.join("state")).unwrap()["projects"][0]["status"],
            "prepared_files_changed"
        );
    }
    #[tokio::test]
    async fn csv_validates_late_rows_and_preserves_original_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("catalog.csv"),
            "id,name\n1,Apple\nnot-an-integer,Pear\n",
        )
        .unwrap();
        let mut req = request(root, "catalog.csv");
        req.csv_columns = serde_json::from_value(
            json!([{"name":"id","type":"integer"},{"name":"name","type":"string"}]),
        )
        .unwrap();
        assert!(
            prepare(req.clone(), &root.join("state"))
                .await
                .unwrap_err()
                .to_string()
                .contains("CSV row 3")
        );
        assert!(!root.join("service").exists());
        fs::create_dir(root.join("service")).unwrap();
        fs::write(root.join("service/keep"), "user work").unwrap();
        req.csv_columns[0].column_type = PublicationCsvColumnType::String;
        assert!(prepare(req, &root.join("state")).await.is_err());
        assert_eq!(
            fs::read_to_string(root.join("service/keep")).unwrap(),
            "user work"
        );
    }
    #[tokio::test]
    async fn sqlite_snapshot_excludes_private_columns() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let conn = rusqlite::Connection::open(root.join("catalog.db")).unwrap();
        conn.execute_batch("CREATE TABLE rows(id INTEGER, name TEXT, secret BLOB); INSERT INTO rows VALUES (1,'Apple',X'010203'); CREATE TABLE private_rows(secret BLOB); INSERT INTO private_rows VALUES (X'040506');").unwrap();
        drop(conn);
        prepare(request(root, "catalog.db"), &root.join("state"))
            .await
            .unwrap();
        let record = read_preparation(&root.join("service")).unwrap().unwrap();
        let bytes = fs::read_to_string(root.join("service").join(record.snapshot_file)).unwrap();
        assert!(!bytes.contains("private"));
        assert!(!bytes.contains("secret"));
    }

    #[tokio::test]
    async fn interrupted_update_selects_record_by_atomic_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("catalog.json"), br#"[{"id":1,"name":"Apple"}]"#).unwrap();
        let req = request(root, "catalog.json");
        prepare(req.clone(), &root.join("state")).await.unwrap();
        let project = root.join("service");
        let old_record = fs::read(project.join(".froglet/preparation.json")).unwrap();
        fs::write(root.join("catalog.json"), br#"[{"id":2,"name":"Pear"}]"#).unwrap();
        prepare(req.clone(), &root.join("state")).await.unwrap();
        let next = fs::read(project.join(".froglet/preparation.json")).unwrap();
        fs::write(project.join(".froglet/preparation-pending.json"), next).unwrap();
        fs::write(project.join(".froglet/preparation.json"), old_record).unwrap();
        assert!(check_prepared_source(&project).is_ok());
        assert_eq!(
            check_updates(&root.join("state")).unwrap()["projects"][0]["status"],
            "current"
        );
        prepare(req, &root.join("state")).await.unwrap();
        assert!(!project.join(".froglet/preparation-pending.json").exists());
    }

    #[tokio::test]
    async fn wasm_example_executes_and_is_public_starter_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("echo.wat"),
            r#"(module (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 0)
            (func (export "run") (param i32 i32) (result i64) local.get 1 i64.extend_i32_u))"#,
        )
        .unwrap();
        let mut req = request(root, "echo.wat");
        req.selection.clear();
        req.example_input = Some(json!({"item":"apple"}));
        let report = prepare(req, &root.join("state")).await.unwrap();
        assert_eq!(report["example_result"], json!({"item":"apple"}));
        let manifest = fs::read_to_string(root.join("service/froglet-service.toml")).unwrap();
        let manifest: toml::Value = toml::from_str(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(manifest["starter"].as_str().unwrap()).unwrap(),
            report["example_input"]
        );
    }
}
