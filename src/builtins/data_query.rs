//! Dependency-free read-only data service for JSON, explicitly-typed CSV, and
//! SQLite snapshots.
//!
//! A [`DataQueryHandler`] is bound to one provider-owned file at construction
//! time. Invocation input never accepts a path or raw SQL. The v1 contract has
//! two operations:
//!
//! - `{ "op": "describe" }` lists collections and columns.
//! - `{ "op": "select", "collection": "samples", "columns": ["id"],
//!   "equals": {"status": "ready"}, "limit": 50, "offset": 0 }` performs
//!   projection plus scalar equality filtering.
//!
//! JSON data is an array of objects (exposed as collection `rows`) or an object
//! whose values are arrays of objects. SQLite input exposes ordinary tables,
//! not views or virtual tables. BLOB values are returned as
//! `{ "$blob_hex": "..." }`.

use crate::execution::BuiltinServiceHandler;
use froglet_protocol::publication::{PublicationCsvColumnType, PublicationCsvSchema};
use rusqlite::{
    Connection, ErrorCode, MAIN_DB, OpenFlags,
    config::DbConfig,
    hooks::{AuthAction, Authorization},
    types::{Value as SqlValue, ValueRef},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    fs::{self, File, OpenOptions},
    future::Future,
    io::Read,
    path::{Component, Path, PathBuf},
    pin::Pin,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

pub const DATA_QUERY_CONTRACT_V1: &str = "froglet.builtin.data_query.v1";
pub const DATA_QUERY_JSON_CONTRACT_V1: &str = "froglet.builtin.data_query.json.v1";
pub const DATA_QUERY_CSV_CONTRACT_V1: &str = "froglet.builtin.data_query.csv.v1";
pub const DATA_QUERY_SQLITE_CONTRACT_V1: &str = "froglet.builtin.data_query.sqlite.v1";
pub const DATA_QUERY_STARTER_V1: &str = r#"{"op":"describe"}"#;
const JSON_ARRAY_COLLECTION: &str = "rows";
const DEFAULT_LIMIT: usize = 50;
const DEFAULT_HANDLER_CACHE_CAPACITY: usize = 128;

type CollectionSchema = BTreeMap<String, Vec<String>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DataQueryCacheKey {
    contract_version: String,
    package_digest: String,
}

/// Per-node cache of native data-query handlers keyed by the complete signed
/// package identity. A cache entry is initialized once even when concurrent
/// invocations arrive before the first validation has completed.
///
/// Initialization errors are deliberately not cached: publication recovery
/// may install a previously missing immutable snapshot, after which a later
/// invocation must be able to validate it successfully.
pub struct DataQueryHandlerCache {
    state: AsyncMutex<DataQueryHandlerCacheState>,
    capacity: usize,
}

#[derive(Default)]
struct DataQueryHandlerCacheState {
    entries: HashMap<DataQueryCacheKey, DataQueryHandlerCacheEntry>,
    access_clock: u64,
}

struct DataQueryHandlerCacheEntry {
    cell: Arc<OnceCell<Arc<DataQueryHandler>>>,
    last_access: u64,
}

impl Default for DataQueryHandlerCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_HANDLER_CACHE_CAPACITY)
    }
}

impl DataQueryHandlerCache {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            state: AsyncMutex::new(DataQueryHandlerCacheState::default()),
            capacity: capacity.max(1),
        }
    }

    pub(crate) async fn get_or_try_init<F, Fut>(
        &self,
        contract_version: &str,
        package_digest: &str,
        initialize: F,
    ) -> Result<Arc<DataQueryHandler>, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Arc<DataQueryHandler>, String>>,
    {
        let key = DataQueryCacheKey {
            contract_version: contract_version.to_string(),
            package_digest: package_digest.to_string(),
        };
        let cell = {
            let mut state = self.state.lock().await;
            state.access_clock = state.access_clock.wrapping_add(1);
            let access = state.access_clock;
            if let Some(entry) = state.entries.get_mut(&key) {
                entry.last_access = access;
                Arc::clone(&entry.cell)
            } else {
                if state.entries.len() >= self.capacity
                    && let Some(eviction_key) = state
                        .entries
                        .iter()
                        .min_by_key(|(_, entry)| entry.last_access)
                        .map(|(key, _)| key.clone())
                {
                    // Removing an in-use entry is safe: active callers own an
                    // Arc to its OnceCell/handler. It only permits a later
                    // request for that evicted digest to revalidate it, while
                    // keeping idle cache memory strictly bounded.
                    state.entries.remove(&eviction_key);
                }
                let cell = Arc::new(OnceCell::new());
                state.entries.insert(
                    key,
                    DataQueryHandlerCacheEntry {
                        cell: Arc::clone(&cell),
                        last_access: access,
                    },
                );
                cell
            }
        };
        cell.get_or_try_init(initialize).await.map(Arc::clone)
    }
}

/// JSON Schema advertised for every v1 native data-query service.
pub fn data_query_input_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "oneOf": [
            {
                "type": "object",
                "properties": { "op": { "const": "describe" } },
                "required": ["op"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "op": { "const": "select" },
                    "collection": { "type": "string", "minLength": 1 },
                    "columns": { "type": "array", "items": { "type": "string" }, "maxItems": 64 },
                    "equals": { "type": "object", "maxProperties": 16 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                    "offset": { "type": "integer", "minimum": 0, "maximum": 100000 }
                },
                "required": ["op", "collection"],
                "additionalProperties": false
            }
        ]
    })
}

/// Output schema plus the exact collections/columns exposed by the staged
/// snapshot. `x-froglet-collections` is descriptive metadata; callers should
/// still use `{op:"describe"}` when they need a live response.
pub fn data_query_output_schema(collections: &BTreeMap<String, Vec<String>>) -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "additionalProperties": true,
        "x-froglet-collections": collections,
    })
}

pub fn data_query_csv_output_schema(
    collections: &BTreeMap<String, Vec<String>>,
    schema: &PublicationCsvSchema,
    row_count: usize,
) -> serde_json::Result<Value> {
    let mut output = data_query_output_schema(collections);
    output["x-froglet-csv-schema"] = serde_json::to_value(schema)?;
    output["x-froglet-row-count"] = json!(row_count);
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataQuerySourceKind {
    Json,
    Csv,
    Sqlite,
}

impl FromStr for DataQuerySourceKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            "sqlite" | "sqlite3" | "db" => Ok(Self::Sqlite),
            other => Err(format!(
                "unsupported data query source kind {other:?}; allowed: json, csv, sqlite"
            )),
        }
    }
}

impl fmt::Display for DataQuerySourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Sqlite => "sqlite",
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DataQueryLimits {
    pub max_json_bytes: usize,
    pub max_json_rows: usize,
    pub max_csv_bytes: usize,
    pub max_csv_rows: usize,
    pub max_collections: usize,
    pub max_columns_per_collection: usize,
    pub max_query_columns: usize,
    pub max_equality_filters: usize,
    pub max_rows_per_query: usize,
    pub max_offset: usize,
    pub max_cell_bytes: usize,
    pub max_result_bytes: usize,
    pub max_query_ms: u64,
}

impl Default for DataQueryLimits {
    fn default() -> Self {
        Self {
            max_json_bytes: 16 * 1024 * 1024,
            max_json_rows: 100_000,
            max_csv_bytes: 16 * 1024 * 1024,
            max_csv_rows: 1_000_000,
            max_collections: 128,
            max_columns_per_collection: 256,
            max_query_columns: 64,
            max_equality_filters: 16,
            max_rows_per_query: 100,
            max_offset: 100_000,
            max_cell_bytes: 64 * 1024,
            max_result_bytes: 1024 * 1024,
            max_query_ms: 500,
        }
    }
}

impl DataQueryLimits {
    fn validate(self) -> Result<Self, String> {
        for (name, value) in [
            ("max_json_bytes", self.max_json_bytes),
            ("max_json_rows", self.max_json_rows),
            ("max_csv_bytes", self.max_csv_bytes),
            ("max_csv_rows", self.max_csv_rows),
            ("max_collections", self.max_collections),
            (
                "max_columns_per_collection",
                self.max_columns_per_collection,
            ),
            ("max_query_columns", self.max_query_columns),
            ("max_equality_filters", self.max_equality_filters),
            ("max_rows_per_query", self.max_rows_per_query),
            ("max_cell_bytes", self.max_cell_bytes),
            ("max_result_bytes", self.max_result_bytes),
        ] {
            if value == 0 {
                return Err(format!("data query limit {name} must be greater than zero"));
            }
        }
        if self.max_query_ms == 0 {
            return Err("data query limit max_query_ms must be greater than zero".to_string());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum DataQueryRequest {
    Describe,
    Select {
        collection: String,
        #[serde(default)]
        columns: Vec<String>,
        #[serde(default)]
        equals: BTreeMap<String, Value>,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

#[derive(Clone)]
pub struct DataQueryHandler {
    source: DataSource,
    limits: DataQueryLimits,
}

impl DataQueryHandler {
    /// Bind a handler to `relative_path` beneath `root` with conservative
    /// default limits. Absolute paths, parent traversal, and symlinked path
    /// components are rejected.
    pub fn open(
        root: impl AsRef<Path>,
        relative_path: impl AsRef<Path>,
        source_kind: DataQuerySourceKind,
    ) -> Result<Self, String> {
        Self::open_with_limits(root, relative_path, source_kind, DataQueryLimits::default())
    }

    pub fn open_with_limits(
        root: impl AsRef<Path>,
        relative_path: impl AsRef<Path>,
        source_kind: DataQuerySourceKind,
        limits: DataQueryLimits,
    ) -> Result<Self, String> {
        let limits = limits.validate()?;
        let source_path = resolve_source_path(root.as_ref(), relative_path.as_ref())?;
        let source = match source_kind {
            DataQuerySourceKind::Json => {
                DataSource::Json(Arc::new(JsonDataset::load(&source_path, limits)?))
            }
            DataQuerySourceKind::Csv => {
                return Err(
                    "CSV sources require open_csv_indexed with an explicit schema".to_string(),
                );
            }
            DataQuerySourceKind::Sqlite => {
                DataSource::Sqlite(Arc::new(SqliteDataset::open(&source_path, limits)?))
            }
        };
        Ok(Self { source, limits })
    }

    /// Validate and import an explicitly-typed CSV snapshot into a private
    /// content-addressed SQLite cache. The cache is built once and all later
    /// selections use bounded SQL over declared indexes.
    pub fn open_csv_indexed(
        root: impl AsRef<Path>,
        relative_path: impl AsRef<Path>,
        schema: PublicationCsvSchema,
        package_digest: &str,
    ) -> Result<Self, String> {
        Self::open_csv_indexed_with_limits(
            root,
            relative_path,
            schema,
            package_digest,
            DataQueryLimits::default(),
        )
    }

    pub fn open_csv_indexed_with_limits(
        root: impl AsRef<Path>,
        relative_path: impl AsRef<Path>,
        schema: PublicationCsvSchema,
        package_digest: &str,
        limits: DataQueryLimits,
    ) -> Result<Self, String> {
        let limits = limits.validate()?;
        schema.validate()?;
        if !valid_lower_hex_digest(package_digest) {
            return Err("CSV package digest must be 64 lowercase hex characters".to_string());
        }
        let source_path = resolve_source_path(root.as_ref(), relative_path.as_ref())?;
        let source_bytes = read_bounded_file(&source_path, limits.max_csv_bytes, "CSV")?;
        let package = froglet_protocol::publication::PublicationDataSource {
            format: froglet_protocol::publication::PublicationDataFormat::Csv,
            content_base64: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &source_bytes,
            ),
            csv_schema: Some(schema.clone()),
        };
        if package.csv_package_digest()? != package_digest {
            return Err(
                "CSV bytes or explicit schema no longer match the publication binding".to_string(),
            );
        }
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| format!("failed to resolve data source root: {error}"))?;
        let cache_name = format!("{package_digest}.csv.sqlite");
        let cache_path = root.join(cache_name);
        let expected_metadata = inspect_csv_source(&source_bytes, &schema, package_digest, limits)?;
        let dataset = open_or_rebuild_csv_cache(
            &cache_path,
            &source_bytes,
            &schema,
            &expected_metadata,
            limits,
        )?;
        Ok(Self {
            source: DataSource::Csv {
                dataset: Arc::new(dataset),
                schema,
                row_count: expected_metadata.row_count,
            },
            limits,
        })
    }

    pub fn source_kind(&self) -> DataQuerySourceKind {
        self.source.kind()
    }

    /// Snapshot of the collections and columns that can be advertised in a
    /// Service Manifest without exposing the provider filesystem path.
    pub fn collection_schema(&self) -> BTreeMap<String, Vec<String>> {
        self.source.schema().clone()
    }

    pub fn csv_summary(&self) -> Option<(PublicationCsvSchema, usize)> {
        self.source
            .csv_metadata()
            .map(|(schema, row_count)| (schema.clone(), row_count))
    }
}

impl BuiltinServiceHandler for DataQueryHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        let source = self.source.clone();
        let limits = self.limits;
        Box::pin(async move {
            let request: DataQueryRequest = serde_json::from_value(input)
                .map_err(|error| format!("invalid {DATA_QUERY_CONTRACT_V1} input: {error}"))?;
            tokio::task::spawn_blocking(move || execute_request(&source, request, limits))
                .await
                .map_err(|error| format!("data query worker failed: {error}"))?
        })
    }
}

#[derive(Clone)]
enum DataSource {
    Json(Arc<JsonDataset>),
    Csv {
        dataset: Arc<SqliteDataset>,
        schema: PublicationCsvSchema,
        row_count: usize,
    },
    Sqlite(Arc<SqliteDataset>),
}

impl DataSource {
    fn kind(&self) -> DataQuerySourceKind {
        match self {
            Self::Json(_) => DataQuerySourceKind::Json,
            Self::Csv { .. } => DataQuerySourceKind::Csv,
            Self::Sqlite(_) => DataQuerySourceKind::Sqlite,
        }
    }

    fn schema(&self) -> &CollectionSchema {
        match self {
            Self::Json(dataset) => &dataset.schema,
            Self::Csv { dataset, .. } => &dataset.schema,
            Self::Sqlite(dataset) => &dataset.schema,
        }
    }

    fn csv_metadata(&self) -> Option<(&PublicationCsvSchema, usize)> {
        match self {
            Self::Csv {
                schema, row_count, ..
            } => Some((schema, *row_count)),
            Self::Json(_) | Self::Sqlite(_) => None,
        }
    }
}

struct JsonDataset {
    collections: BTreeMap<String, Vec<Map<String, Value>>>,
    schema: CollectionSchema,
}

impl JsonDataset {
    fn load(path: &Path, limits: DataQueryLimits) -> Result<Self, String> {
        let mut file = File::open(path)
            .map_err(|error| format!("failed to open JSON data source: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("failed to inspect JSON data source: {error}"))?;
        if metadata.len() > limits.max_json_bytes as u64 {
            return Err(format!(
                "JSON data source exceeds {} byte limit",
                limits.max_json_bytes
            ));
        }
        let mut encoded = Vec::new();
        file.by_ref()
            .take(limits.max_json_bytes.saturating_add(1) as u64)
            .read_to_end(&mut encoded)
            .map_err(|error| format!("failed to read JSON data source: {error}"))?;
        if encoded.len() > limits.max_json_bytes {
            return Err(format!(
                "JSON data source exceeds {} byte limit",
                limits.max_json_bytes
            ));
        }
        let value: Value = serde_json::from_slice(&encoded)
            .map_err(|error| format!("invalid JSON data source: {error}"))?;
        let collections = json_collections(value)?;
        if collections.len() > limits.max_collections {
            return Err(format!(
                "JSON data source has {} collections; limit is {}",
                collections.len(),
                limits.max_collections
            ));
        }

        let mut total_rows = 0usize;
        let mut schema = BTreeMap::new();
        for (collection, rows) in &collections {
            validate_name("collection", collection)?;
            total_rows = total_rows
                .checked_add(rows.len())
                .ok_or_else(|| "JSON row count overflow".to_string())?;
            if total_rows > limits.max_json_rows {
                return Err(format!(
                    "JSON data source exceeds {} row limit",
                    limits.max_json_rows
                ));
            }
            let mut columns = BTreeSet::new();
            for row in rows {
                for column in row.keys() {
                    validate_name("column", column)?;
                    columns.insert(column.clone());
                }
            }
            if columns.len() > limits.max_columns_per_collection {
                return Err(format!(
                    "JSON collection {collection:?} has {} columns; limit is {}",
                    columns.len(),
                    limits.max_columns_per_collection
                ));
            }
            schema.insert(collection.clone(), columns.into_iter().collect());
        }

        Ok(Self {
            collections,
            schema,
        })
    }
}

fn json_collections(value: Value) -> Result<BTreeMap<String, Vec<Map<String, Value>>>, String> {
    match value {
        Value::Array(rows) => Ok(BTreeMap::from([(
            JSON_ARRAY_COLLECTION.to_string(),
            json_rows(JSON_ARRAY_COLLECTION, rows)?,
        )])),
        Value::Object(collections) if !collections.is_empty() => collections
            .into_iter()
            .map(|(name, value)| match value {
                Value::Array(rows) => Ok((name.clone(), json_rows(&name, rows)?)),
                _ => Err(format!(
                    "JSON collection {name:?} must be an array of objects"
                )),
            })
            .collect(),
        Value::Object(_) => {
            Err("JSON data source must contain at least one collection".to_string())
        }
        _ => Err(
            "JSON data source must be an array of objects or an object of named arrays".to_string(),
        ),
    }
}

fn json_rows(collection: &str, rows: Vec<Value>) -> Result<Vec<Map<String, Value>>, String> {
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| match row {
            Value::Object(row) => Ok(row),
            _ => Err(format!(
                "JSON collection {collection:?} row {index} must be an object"
            )),
        })
        .collect()
}

const CSV_CACHE_SCHEMA_V2: &str = "froglet.csv-query-cache.v2";
const CSV_CACHE_METADATA_TABLE: &str = "_froglet_csv_metadata";
const CSV_CACHE_CONTENT_DIGEST_DOMAIN: &[u8] = b"froglet.csv-query-cache.content.v1\0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CsvCacheMetadata {
    schema_version: String,
    package_digest: String,
    row_count: usize,
    content_digest: String,
    schema: PublicationCsvSchema,
}

impl CsvCacheMetadata {
    fn load(connection: &Connection, expected: &Self) -> Result<Self, String> {
        let encoded: String = connection
            .query_row(
                &format!(
                    "SELECT metadata_json FROM {} WHERE id = 1",
                    quote_identifier(CSV_CACHE_METADATA_TABLE)
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to read CSV query cache metadata: {error}"))?;
        let metadata: Self = serde_json::from_str(&encoded)
            .map_err(|error| format!("invalid CSV query cache metadata: {error}"))?;
        if metadata != *expected {
            return Err("CSV query cache metadata does not match the publication".to_string());
        }
        Ok(metadata)
    }
}

struct CsvContentDigest {
    hasher: Sha256,
}

impl CsvContentDigest {
    fn new(package_digest: &str, schema: &PublicationCsvSchema) -> Result<Self, String> {
        let schema_json = serde_json::to_vec(schema)
            .map_err(|error| format!("failed to encode CSV schema for cache binding: {error}"))?;
        let mut hasher = Sha256::new();
        hasher.update(CSV_CACHE_CONTENT_DIGEST_DOMAIN);
        update_digest_bytes(&mut hasher, package_digest.as_bytes());
        update_digest_bytes(&mut hasher, &schema_json);
        Ok(Self { hasher })
    }

    fn update_owned_row(&mut self, values: &[SqlValue]) -> Result<(), String> {
        self.hasher.update([0x52]);
        self.hasher.update(
            u64::try_from(values.len())
                .map_err(|_| "CSV column count exceeds u64".to_string())?
                .to_be_bytes(),
        );
        for value in values {
            match value {
                SqlValue::Null => self.hasher.update([0x00]),
                SqlValue::Integer(value) => {
                    self.hasher.update([0x01]);
                    self.hasher.update(value.to_be_bytes());
                }
                SqlValue::Real(value) => {
                    self.hasher.update([0x02]);
                    self.hasher
                        .update(canonical_csv_f64_bits(*value).to_be_bytes());
                }
                SqlValue::Text(value) => {
                    self.hasher.update([0x03]);
                    update_digest_bytes(&mut self.hasher, value.as_bytes());
                }
                SqlValue::Blob(value) => {
                    self.hasher.update([0x04]);
                    update_digest_bytes(&mut self.hasher, value);
                }
            }
        }
        Ok(())
    }

    fn update_borrowed_row(&mut self, values: &[ValueRef<'_>]) -> Result<(), String> {
        self.hasher.update([0x52]);
        self.hasher.update(
            u64::try_from(values.len())
                .map_err(|_| "CSV cache column count exceeds u64".to_string())?
                .to_be_bytes(),
        );
        for value in values {
            match value {
                ValueRef::Null => self.hasher.update([0x00]),
                ValueRef::Integer(value) => {
                    self.hasher.update([0x01]);
                    self.hasher.update(value.to_be_bytes());
                }
                ValueRef::Real(value) => {
                    self.hasher.update([0x02]);
                    self.hasher
                        .update(canonical_csv_f64_bits(*value).to_be_bytes());
                }
                ValueRef::Text(value) => {
                    self.hasher.update([0x03]);
                    update_digest_bytes(&mut self.hasher, value);
                }
                ValueRef::Blob(value) => {
                    self.hasher.update([0x04]);
                    update_digest_bytes(&mut self.hasher, value);
                }
            }
        }
        Ok(())
    }

    fn finish(mut self, row_count: usize) -> Result<String, String> {
        self.hasher.update([0x45]);
        self.hasher.update(
            u64::try_from(row_count)
                .map_err(|_| "CSV row count exceeds u64".to_string())?
                .to_be_bytes(),
        );
        Ok(hex::encode(self.hasher.finalize()))
    }
}

fn update_digest_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn canonical_csv_f64_bits(value: f64) -> u64 {
    if value == 0.0 {
        0.0f64.to_bits()
    } else {
        value.to_bits()
    }
}

struct CsvRecords<'a> {
    bytes: &'a [u8],
    cursor: usize,
    max_cell_bytes: usize,
}

impl<'a> CsvRecords<'a> {
    fn new(bytes: &'a [u8], max_cell_bytes: usize) -> Self {
        Self {
            bytes,
            cursor: 0,
            max_cell_bytes,
        }
    }

    fn next_record(&mut self) -> Result<Option<Vec<String>>, String> {
        if self.cursor >= self.bytes.len() {
            return Ok(None);
        }
        let mut record = Vec::new();
        let mut field = Vec::new();
        let mut in_quotes = false;
        let mut after_quote = false;
        let mut field_started = false;

        loop {
            let Some(&byte) = self.bytes.get(self.cursor) else {
                if in_quotes {
                    return Err("CSV ended inside a quoted field".to_string());
                }
                record.push(decode_csv_field(field)?);
                return Ok(Some(record));
            };
            self.cursor += 1;
            if in_quotes {
                if byte == b'"' {
                    if self.bytes.get(self.cursor) == Some(&b'"') {
                        self.cursor += 1;
                        field.push(b'"');
                    } else {
                        in_quotes = false;
                        after_quote = true;
                    }
                } else {
                    field.push(byte);
                }
            } else if after_quote {
                match byte {
                    b',' => {
                        record.push(decode_csv_field(std::mem::take(&mut field))?);
                        after_quote = false;
                        field_started = false;
                    }
                    b'\n' => {
                        record.push(decode_csv_field(field)?);
                        return Ok(Some(record));
                    }
                    b'\r' if self.bytes.get(self.cursor) == Some(&b'\n') => {
                        self.cursor += 1;
                        record.push(decode_csv_field(field)?);
                        return Ok(Some(record));
                    }
                    _ => {
                        return Err(
                            "CSV quoted field must be followed by a comma or line ending"
                                .to_string(),
                        );
                    }
                }
            } else {
                match byte {
                    b'"' if !field_started && field.is_empty() => {
                        in_quotes = true;
                        field_started = true;
                    }
                    b'"' => {
                        return Err("CSV quote appeared inside an unquoted field".to_string());
                    }
                    b',' => {
                        record.push(decode_csv_field(std::mem::take(&mut field))?);
                        field_started = false;
                    }
                    b'\n' => {
                        record.push(decode_csv_field(field)?);
                        return Ok(Some(record));
                    }
                    b'\r' if self.bytes.get(self.cursor) == Some(&b'\n') => {
                        self.cursor += 1;
                        record.push(decode_csv_field(field)?);
                        return Ok(Some(record));
                    }
                    b'\r' => return Err("CSV uses a bare carriage return".to_string()),
                    _ => {
                        field.push(byte);
                        field_started = true;
                    }
                }
            }
            if field.len() > self.max_cell_bytes {
                return Err(format!(
                    "CSV cell exceeds {} byte limit",
                    self.max_cell_bytes
                ));
            }
        }
    }
}

fn decode_csv_field(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| "CSV fields must be valid UTF-8".to_string())
}

fn csv_sql_type(column_type: PublicationCsvColumnType) -> &'static str {
    match column_type {
        PublicationCsvColumnType::String => "TEXT",
        PublicationCsvColumnType::Integer | PublicationCsvColumnType::Boolean => "INTEGER",
        PublicationCsvColumnType::Number => "REAL",
    }
}

fn csv_cell_to_sql(
    value: &str,
    column: &froglet_protocol::publication::PublicationCsvColumn,
) -> Result<SqlValue, String> {
    if value.is_empty() && column.nullable {
        return Ok(SqlValue::Null);
    }
    match column.column_type {
        PublicationCsvColumnType::String => Ok(SqlValue::Text(value.to_string())),
        PublicationCsvColumnType::Integer => value
            .parse::<i64>()
            .map(SqlValue::Integer)
            .map_err(|_| format!("CSV column {:?} requires an i64 integer", column.name)),
        PublicationCsvColumnType::Number => {
            let value = value
                .parse::<f64>()
                .map_err(|_| format!("CSV column {:?} requires a finite number", column.name))?;
            if !value.is_finite() {
                return Err(format!(
                    "CSV column {:?} requires a finite number",
                    column.name
                ));
            }
            Ok(SqlValue::Real(value))
        }
        PublicationCsvColumnType::Boolean => match value {
            "true" => Ok(SqlValue::Integer(1)),
            "false" => Ok(SqlValue::Integer(0)),
            _ => Err(format!(
                "CSV column {:?} requires true or false",
                column.name
            )),
        },
    }
}

fn visit_csv_rows(
    source_bytes: &[u8],
    schema: &PublicationCsvSchema,
    limits: DataQueryLimits,
    mut visit: impl FnMut(usize, &[SqlValue]) -> Result<(), String>,
) -> Result<usize, String> {
    let mut records = CsvRecords::new(source_bytes, limits.max_cell_bytes);
    let header = records
        .next_record()?
        .ok_or_else(|| "CSV data source is empty".to_string())?;
    let expected_header = schema
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    if header != expected_header {
        return Err(format!(
            "CSV header does not match explicit schema; expected {expected_header:?}"
        ));
    }

    let mut row_count = 0usize;
    while let Some(record) = records.next_record()? {
        if record.len() != schema.columns.len() {
            return Err(format!(
                "CSV row {} has {} cells; expected {}",
                row_count + 2,
                record.len(),
                schema.columns.len()
            ));
        }
        row_count = row_count
            .checked_add(1)
            .ok_or_else(|| "CSV row count overflow".to_string())?;
        if row_count > limits.max_csv_rows {
            return Err(format!(
                "CSV data source exceeds {} row limit",
                limits.max_csv_rows
            ));
        }
        let values = record
            .iter()
            .zip(&schema.columns)
            .map(|(value, column)| csv_cell_to_sql(value, column))
            .collect::<Result<Vec<_>, _>>()?;
        visit(row_count, &values)?;
    }
    Ok(row_count)
}

fn inspect_csv_source(
    source_bytes: &[u8],
    schema: &PublicationCsvSchema,
    package_digest: &str,
    limits: DataQueryLimits,
) -> Result<CsvCacheMetadata, String> {
    let mut digest = CsvContentDigest::new(package_digest, schema)?;
    let row_count = visit_csv_rows(source_bytes, schema, limits, |_, values| {
        digest.update_owned_row(values)
    })?;
    Ok(CsvCacheMetadata {
        schema_version: CSV_CACHE_SCHEMA_V2.to_string(),
        package_digest: package_digest.to_string(),
        row_count,
        content_digest: digest.finish(row_count)?,
        schema: schema.clone(),
    })
}

fn build_csv_cache(
    cache_path: &Path,
    source_bytes: &[u8],
    schema: &PublicationCsvSchema,
    expected_metadata: &CsvCacheMetadata,
    limits: DataQueryLimits,
) -> Result<(), String> {
    let nonce = rand::random::<u64>();
    let temp_path = cache_path.with_extension(format!("csv.sqlite.{nonce:016x}.tmp"));
    let result = (|| -> Result<(), String> {
        let mut connection = Connection::open_with_flags(
            &temp_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("failed to create CSV query cache: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=OFF; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY;",
            )
            .map_err(|error| format!("failed to configure CSV query cache: {error}"))?;
        let column_definitions = schema
            .columns
            .iter()
            .map(|column| {
                format!(
                    "{} {}{}",
                    quote_identifier(&column.name),
                    csv_sql_type(column.column_type),
                    if column.nullable { "" } else { " NOT NULL" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        connection
            .execute_batch(&format!(
                "CREATE TABLE {} ({column_definitions});\
                 CREATE TABLE {} (id INTEGER PRIMARY KEY CHECK (id = 1), metadata_json TEXT NOT NULL);",
                quote_identifier(&schema.collection),
                quote_identifier(CSV_CACHE_METADATA_TABLE),
            ))
            .map_err(|error| format!("failed to create CSV query schema: {error}"))?;

        let placeholders = (1..=schema.columns.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let insert_sql = format!(
            "INSERT INTO {} VALUES ({placeholders})",
            quote_identifier(&schema.collection)
        );
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start CSV import: {error}"))?;
        let row_count = {
            let mut insert = transaction
                .prepare(&insert_sql)
                .map_err(|error| format!("failed to prepare CSV import: {error}"))?;
            visit_csv_rows(source_bytes, schema, limits, |row_count, values| {
                insert
                    .execute(rusqlite::params_from_iter(values.iter()))
                    .map(|_| ())
                    .map_err(|error| format!("failed to import CSV row {row_count}: {error}"))
            })?
        };
        if row_count != expected_metadata.row_count {
            return Err("CSV source changed while its query cache was being built".to_string());
        }

        for column in schema.columns.iter().filter(|column| column.indexed) {
            transaction
                .execute_batch(&format!(
                    "CREATE INDEX {} ON {} ({});",
                    quote_identifier(&format!("idx_{}_{}", schema.collection, column.name)),
                    quote_identifier(&schema.collection),
                    quote_identifier(&column.name),
                ))
                .map_err(|error| format!("failed to create CSV index: {error}"))?;
        }
        let metadata_json = serde_json::to_string(expected_metadata)
            .map_err(|error| format!("failed to encode CSV cache metadata: {error}"))?;
        transaction
            .execute(
                &format!(
                    "INSERT INTO {} (id, metadata_json) VALUES (1, ?1)",
                    quote_identifier(CSV_CACHE_METADATA_TABLE)
                ),
                [metadata_json],
            )
            .map_err(|error| format!("failed to persist CSV cache metadata: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit CSV query cache: {error}"))?;
        drop(connection);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("failed to secure CSV query cache: {error}"))?;
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temp_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("failed to durably flush CSV query cache: {error}"))?;
        sync_cache_directory(cache_path, "before installing CSV query cache")?;
        fs::rename(&temp_path, cache_path)
            .map_err(|error| format!("failed to install CSV query cache: {error}"))?;
        sync_cache_directory(cache_path, "after installing CSV query cache")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn sync_cache_directory(cache_path: &Path, action: &str) -> Result<(), String> {
    let parent = cache_path
        .parent()
        .ok_or_else(|| "CSV query cache path has no parent directory".to_string())?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("failed to {action}: {error}"))
}

fn discard_csv_cache(cache_path: &Path) -> Result<(), String> {
    match fs::remove_file(cache_path) {
        Ok(()) => sync_cache_directory(cache_path, "record removal of invalid CSV query cache"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to discard invalid CSV query cache: {error}"
        )),
    }
}

fn open_or_rebuild_csv_cache(
    cache_path: &Path,
    source_bytes: &[u8],
    schema: &PublicationCsvSchema,
    expected_metadata: &CsvCacheMetadata,
    limits: DataQueryLimits,
) -> Result<SqliteDataset, String> {
    match fs::symlink_metadata(cache_path) {
        Ok(_) => match SqliteDataset::open_csv_cache(cache_path, limits, schema, expected_metadata)
        {
            Ok(dataset) => return Ok(dataset),
            Err(_) => discard_csv_cache(cache_path)?,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("failed to inspect CSV query cache: {error}"));
        }
    }

    build_csv_cache(cache_path, source_bytes, schema, expected_metadata, limits)?;
    match SqliteDataset::open_csv_cache(cache_path, limits, schema, expected_metadata) {
        Ok(dataset) => Ok(dataset),
        Err(error) => {
            discard_csv_cache(cache_path)?;
            Err(format!(
                "rebuilt CSV query cache failed integrity validation: {error}"
            ))
        }
    }
}

struct SqliteDataset {
    connection: Mutex<Connection>,
    schema: Arc<CollectionSchema>,
}

impl SqliteDataset {
    fn open(path: &Path, limits: DataQueryLimits) -> Result<Self, String> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("failed to open SQLite data source read-only: {error}"))?;
        if !connection
            .is_readonly(MAIN_DB)
            .map_err(|error| format!("failed to inspect SQLite read-only state: {error}"))?
        {
            return Err("SQLite data source did not open read-only".to_string());
        }
        let schema = Arc::new(load_sqlite_schema(&connection, limits)?);
        harden_sqlite_connection(&connection, &schema, limits)?;

        Ok(Self {
            connection: Mutex::new(connection),
            schema,
        })
    }

    fn open_csv_cache(
        path: &Path,
        limits: DataQueryLimits,
        csv_schema: &PublicationCsvSchema,
        expected_metadata: &CsvCacheMetadata,
    ) -> Result<Self, String> {
        let file_type = fs::symlink_metadata(path)
            .map_err(|error| format!("failed to inspect CSV query cache path: {error}"))?
            .file_type();
        if file_type.is_symlink() || !file_type.is_file() {
            return Err("CSV query cache path must be a regular non-symlink file".to_string());
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("failed to open CSV query cache read-only: {error}"))?;
        if !connection
            .is_readonly(MAIN_DB)
            .map_err(|error| format!("failed to inspect CSV cache read-only state: {error}"))?
        {
            return Err("CSV query cache did not open read-only".to_string());
        }
        let integrity: String = connection
            .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
            .map_err(|error| format!("failed to verify CSV query cache integrity: {error}"))?;
        if integrity != "ok" {
            return Err(format!(
                "CSV query cache failed SQLite integrity validation: {integrity}"
            ));
        }
        CsvCacheMetadata::load(&connection, expected_metadata)?;

        let declared_columns = csv_schema
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        validate_csv_cache_table_schema(&connection, csv_schema)?;
        validate_csv_cache_indexes(&connection, csv_schema)?;
        validate_csv_cache_content(&connection, csv_schema, expected_metadata)?;

        let schema = Arc::new(BTreeMap::from([(
            csv_schema.collection.clone(),
            declared_columns,
        )]));
        harden_sqlite_connection(&connection, &schema, limits)?;

        Ok(Self {
            connection: Mutex::new(connection),
            schema,
        })
    }
}

fn validate_csv_cache_table_schema(
    connection: &Connection,
    schema: &PublicationCsvSchema,
) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!(
            "PRAGMA table_info({})",
            quote_identifier(&schema.collection)
        ))
        .map_err(|error| format!("failed to inspect CSV query cache table schema: {error}"))?;
    let actual = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|error| format!("failed to read CSV query cache table schema: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to decode CSV query cache table schema: {error}"))?;
    let expected = schema
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            Ok((
                i64::try_from(index).map_err(|_| "CSV column index exceeds i64".to_string())?,
                column.name.clone(),
                csv_sql_type(column.column_type).to_string(),
                if column.nullable { 0 } else { 1 },
                None,
                0,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    if actual != expected {
        return Err("CSV query cache table schema does not match the publication".to_string());
    }
    Ok(())
}

fn validate_csv_cache_indexes(
    connection: &Connection,
    schema: &PublicationCsvSchema,
) -> Result<(), String> {
    let expected = schema
        .columns
        .iter()
        .filter(|column| column.indexed)
        .map(|column| {
            (
                format!("idx_{}_{}", schema.collection, column.name),
                column.name.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut statement = connection
        .prepare(&format!(
            "PRAGMA index_list({})",
            quote_identifier(&schema.collection)
        ))
        .map_err(|error| format!("failed to inspect CSV query cache indexes: {error}"))?;
    let indexes = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|error| format!("failed to read CSV query cache indexes: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to decode CSV query cache indexes: {error}"))?;
    drop(statement);

    let mut actual = BTreeMap::new();
    for (name, unique, origin, partial) in indexes {
        if unique != 0 || origin != "c" || partial != 0 {
            return Err("CSV query cache contains an unexpected index".to_string());
        }
        let mut statement = connection
            .prepare(&format!("PRAGMA index_info({})", quote_identifier(&name)))
            .map_err(|error| {
                format!("failed to inspect CSV query cache index {name:?}: {error}")
            })?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(2))
            .map_err(|error| format!("failed to read CSV query cache index {name:?}: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("failed to decode CSV query cache index {name:?}: {error}"))?;
        if columns.len() != 1 || actual.insert(name, columns[0].clone()).is_some() {
            return Err("CSV query cache contains an invalid index definition".to_string());
        }
    }
    if actual != expected {
        return Err("CSV query cache indexes do not match the publication schema".to_string());
    }
    Ok(())
}

fn validate_csv_cache_content(
    connection: &Connection,
    schema: &PublicationCsvSchema,
    expected_metadata: &CsvCacheMetadata,
) -> Result<(), String> {
    let columns = schema
        .columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = connection
        .prepare(&format!(
            "SELECT {columns} FROM {} ORDER BY rowid",
            quote_identifier(&schema.collection)
        ))
        .map_err(|error| format!("failed to prepare CSV query cache validation: {error}"))?;
    let mut rows = statement
        .query([])
        .map_err(|error| format!("failed to read CSV query cache for validation: {error}"))?;
    let mut digest = CsvContentDigest::new(&expected_metadata.package_digest, schema)?;
    let mut row_count = 0usize;
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("failed to validate CSV query cache row: {error}"))?
    {
        row_count = row_count
            .checked_add(1)
            .ok_or_else(|| "CSV query cache row count overflow".to_string())?;
        if row_count > expected_metadata.row_count {
            return Err("CSV query cache contains unexpected rows".to_string());
        }
        let values = (0..schema.columns.len())
            .map(|index| {
                row.get_ref(index)
                    .map_err(|error| format!("failed to read CSV query cache value: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        digest.update_borrowed_row(&values)?;
    }
    if row_count != expected_metadata.row_count
        || digest.finish(row_count)? != expected_metadata.content_digest
    {
        return Err("CSV query cache content does not match the published source".to_string());
    }
    Ok(())
}

fn harden_sqlite_connection(
    connection: &Connection,
    schema: &Arc<CollectionSchema>,
    limits: DataQueryLimits,
) -> Result<(), String> {
    connection
        .busy_timeout(Duration::from_millis(limits.max_query_ms))
        .map_err(|error| format!("failed to bound SQLite lock wait: {error}"))?;
    connection
        .execute_batch("PRAGMA query_only = ON;")
        .map_err(|error| format!("failed to enable SQLite query_only: {error}"))?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(|error| format!("failed to enable SQLite defensive mode: {error}"))?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
        .map_err(|error| format!("failed to disable SQLite trusted schema: {error}"))?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)
        .map_err(|error| format!("failed to disable SQLite triggers: {error}"))?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false)
        .map_err(|error| format!("failed to disable SQLite views: {error}"))?;

    let authorized_schema = Arc::clone(schema);
    connection
        .authorizer(Some(
            move |context: rusqlite::hooks::AuthContext<'_>| match context.action {
                AuthAction::Select => Authorization::Allow,
                AuthAction::Read {
                    table_name,
                    column_name,
                } if context.database_name == Some("main")
                    && context.accessor.is_none()
                    && authorized_schema.get(table_name).is_some_and(|columns| {
                        columns.iter().any(|column| column == column_name)
                    }) =>
                {
                    Authorization::Allow
                }
                _ => Authorization::Deny,
            },
        ))
        .map_err(|error| format!("failed to install SQLite read authorizer: {error}"))?;
    Ok(())
}

fn load_sqlite_schema(
    connection: &Connection,
    limits: DataQueryLimits,
) -> Result<CollectionSchema, String> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table'
               AND name NOT LIKE 'sqlite_%'
               AND upper(trim(sql)) NOT LIKE 'CREATE VIRTUAL TABLE%'
             ORDER BY name",
        )
        .map_err(|error| format!("failed to inspect SQLite tables: {error}"))?;
    let table_names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("failed to inspect SQLite tables: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to inspect SQLite tables: {error}"))?;
    drop(statement);
    if table_names.len() > limits.max_collections {
        return Err(format!(
            "SQLite data source has {} tables; limit is {}",
            table_names.len(),
            limits.max_collections
        ));
    }

    let mut schema = BTreeMap::new();
    for table in table_names {
        validate_name("table", &table)?;
        let sql = format!("SELECT * FROM {} LIMIT 0", quote_identifier(&table));
        let statement = connection
            .prepare(&sql)
            .map_err(|error| format!("failed to inspect SQLite table {table:?}: {error}"))?;
        let columns = statement
            .column_names()
            .iter()
            .map(|column| column.to_string())
            .collect::<Vec<_>>();
        if columns.len() > limits.max_columns_per_collection {
            return Err(format!(
                "SQLite table {table:?} has {} columns; limit is {}",
                columns.len(),
                limits.max_columns_per_collection
            ));
        }
        for column in &columns {
            validate_name("column", column)?;
        }
        schema.insert(table, columns);
    }
    if schema.is_empty() {
        return Err("SQLite data source contains no ordinary tables".to_string());
    }
    Ok(schema)
}

#[derive(Debug)]
struct ValidatedSelect {
    collection: String,
    columns: Vec<String>,
    equals: BTreeMap<String, Value>,
    limit: usize,
    offset: usize,
}

fn execute_request(
    source: &DataSource,
    request: DataQueryRequest,
    limits: DataQueryLimits,
) -> Result<Value, String> {
    let response = match request {
        DataQueryRequest::Describe => {
            let mut response = json!({
                "contract_version": DATA_QUERY_CONTRACT_V1,
                "source_kind": source.kind(),
                "collections": source.schema(),
            });
            if let Some((schema, row_count)) = source.csv_metadata() {
                response["csv_schema"] = serde_json::to_value(schema)
                    .map_err(|error| format!("failed to encode CSV schema: {error}"))?;
                response["row_counts"] = json!({ schema.collection.clone(): row_count });
            }
            response
        }
        DataQueryRequest::Select {
            collection,
            columns,
            equals,
            limit,
            offset,
        } => {
            let query = validate_select(
                source.schema(),
                collection,
                columns,
                equals,
                limit,
                offset,
                limits,
            )?;
            match source {
                DataSource::Json(dataset) => query_json(dataset, query, limits)?,
                DataSource::Csv {
                    dataset, schema, ..
                } => {
                    validate_csv_indexed_selection(schema, &query)?;
                    query_sqlite(
                        dataset,
                        query,
                        limits,
                        DataQuerySourceKind::Csv,
                        Some(schema),
                    )?
                }
                DataSource::Sqlite(dataset) => {
                    query_sqlite(dataset, query, limits, DataQuerySourceKind::Sqlite, None)?
                }
            }
        }
    };
    ensure_result_bound(&response, limits.max_result_bytes)?;
    Ok(response)
}

fn validate_select(
    schema: &CollectionSchema,
    collection: String,
    columns: Vec<String>,
    equals: BTreeMap<String, Value>,
    limit: usize,
    offset: usize,
    limits: DataQueryLimits,
) -> Result<ValidatedSelect, String> {
    validate_name("collection", &collection)?;
    let available = schema
        .get(&collection)
        .ok_or_else(|| format!("unknown data collection {collection:?}"))?;
    if limit == 0 || limit > limits.max_rows_per_query {
        return Err(format!(
            "select limit must be between 1 and {}",
            limits.max_rows_per_query
        ));
    }
    if offset > limits.max_offset {
        return Err(format!("select offset exceeds {}", limits.max_offset));
    }
    if columns.len() > limits.max_query_columns {
        return Err(format!(
            "select requests at most {} columns",
            limits.max_query_columns
        ));
    }
    if equals.len() > limits.max_equality_filters {
        return Err(format!(
            "select requests at most {} equality filters",
            limits.max_equality_filters
        ));
    }

    let columns = if columns.is_empty() {
        available.clone()
    } else {
        let unique = columns.iter().collect::<BTreeSet<_>>();
        if unique.len() != columns.len() {
            return Err("select columns must not contain duplicates".to_string());
        }
        for column in &columns {
            require_column(available, column)?;
        }
        columns
    };
    for (column, value) in &equals {
        require_column(available, column)?;
        if !matches!(
            value,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
        ) {
            return Err(format!("equality filter {column:?} must be a JSON scalar"));
        }
    }

    Ok(ValidatedSelect {
        collection,
        columns,
        equals,
        limit,
        offset,
    })
}

fn require_column(available: &[String], column: &str) -> Result<(), String> {
    validate_name("column", column)?;
    if available.iter().any(|candidate| candidate == column) {
        Ok(())
    } else {
        Err(format!("unknown data column {column:?}"))
    }
}

fn query_json(
    dataset: &JsonDataset,
    query: ValidatedSelect,
    limits: DataQueryLimits,
) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_millis(limits.max_query_ms);
    let rows = dataset
        .collections
        .get(&query.collection)
        .ok_or_else(|| format!("unknown data collection {:?}", query.collection))?;
    let mut matched = 0usize;
    let mut selected = Vec::new();
    for row in rows {
        if Instant::now() >= deadline {
            return Err("JSON data query deadline exceeded".to_string());
        }
        if !query
            .equals
            .iter()
            .all(|(column, expected)| row.get(column) == Some(expected))
        {
            continue;
        }
        if matched < query.offset {
            matched += 1;
            continue;
        }
        if selected.len() > query.limit {
            break;
        }
        let projected = query
            .columns
            .iter()
            .map(|column| {
                (
                    column.clone(),
                    row.get(column).cloned().unwrap_or(Value::Null),
                )
            })
            .collect::<Map<_, _>>();
        selected.push(Value::Object(projected));
    }
    select_response(
        DataQuerySourceKind::Json,
        query,
        selected,
        limits.max_result_bytes,
    )
}

fn query_sqlite(
    dataset: &SqliteDataset,
    query: ValidatedSelect,
    limits: DataQueryLimits,
    source_kind: DataQuerySourceKind,
    csv_schema: Option<&PublicationCsvSchema>,
) -> Result<Value, String> {
    let connection = dataset
        .connection
        .lock()
        .map_err(|_| "SQLite data query lock is poisoned".to_string())?;
    let deadline = Instant::now() + Duration::from_millis(limits.max_query_ms);
    connection
        .progress_handler(1_000, Some(move || Instant::now() >= deadline))
        .map_err(|error| format!("failed to install SQLite query deadline: {error}"))?;
    let result = query_sqlite_inner(&connection, query, limits, source_kind, csv_schema);
    let clear_result = connection.progress_handler(0, None::<fn() -> bool>);
    match (result, clear_result) {
        (result, Ok(())) => result,
        (_, Err(error)) => Err(format!("failed to clear SQLite query deadline: {error}")),
    }
}

fn query_sqlite_inner(
    connection: &Connection,
    query: ValidatedSelect,
    limits: DataQueryLimits,
    source_kind: DataQuerySourceKind,
    csv_schema: Option<&PublicationCsvSchema>,
) -> Result<Value, String> {
    let projection = query
        .columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let predicates = query
        .equals
        .keys()
        .map(|column| format!("{} IS ?", quote_identifier(column)))
        .collect::<Vec<_>>();
    let where_clause = if predicates.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", predicates.join(" AND "))
    };
    let sql = format!(
        "SELECT {projection} FROM {}{where_clause} LIMIT ? OFFSET ?",
        quote_identifier(&query.collection)
    );
    let mut params = query
        .equals
        .iter()
        .map(|(column, value)| match csv_schema {
            Some(schema) => csv_filter_to_sql(schema, column, value),
            None => json_scalar_to_sql(value),
        })
        .collect::<Result<Vec<_>, _>>()?;
    params.push(SqlValue::Integer(
        i64::try_from(query.limit.saturating_add(1))
            .map_err(|_| "select limit is too large".to_string())?,
    ));
    params.push(SqlValue::Integer(
        i64::try_from(query.offset).map_err(|_| "select offset is too large".to_string())?,
    ));

    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| map_sqlite_error(error, "SQLite select prepare failed"))?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|error| map_sqlite_error(error, "SQLite select failed"))?;
    let mut selected = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| map_sqlite_error(error, "SQLite row iteration failed"))?
    {
        let mut projected = Map::new();
        for (index, column) in query.columns.iter().enumerate() {
            let value = row
                .get_ref(index)
                .map_err(|error| format!("SQLite column decode failed: {error}"))?;
            let value = match csv_schema {
                Some(schema) => {
                    csv_sqlite_value_to_json(schema, column, value, limits.max_cell_bytes)?
                }
                None => sqlite_value_to_json(value, limits.max_cell_bytes)?,
            };
            projected.insert(column.clone(), value);
        }
        selected.push(Value::Object(projected));
        if selected.len() > query.limit {
            break;
        }
        ensure_partial_rows_bound(&selected, limits.max_result_bytes)?;
    }

    select_response(source_kind, query, selected, limits.max_result_bytes)
}

fn select_response(
    source_kind: DataQuerySourceKind,
    query: ValidatedSelect,
    mut rows: Vec<Value>,
    max_result_bytes: usize,
) -> Result<Value, String> {
    let has_more = rows.len() > query.limit;
    rows.truncate(query.limit);
    let response = json!({
        "contract_version": DATA_QUERY_CONTRACT_V1,
        "source_kind": source_kind,
        "collection": query.collection,
        "columns": query.columns,
        "rows": rows,
        "returned": rows.len(),
        "limit": query.limit,
        "offset": query.offset,
        "has_more": has_more,
    });
    ensure_result_bound(&response, max_result_bytes)?;
    Ok(response)
}

fn ensure_partial_rows_bound(rows: &[Value], max_result_bytes: usize) -> Result<(), String> {
    let encoded = serde_json::to_vec(rows)
        .map_err(|error| format!("failed to encode data query rows: {error}"))?;
    if encoded.len() > max_result_bytes {
        Err(format!(
            "data query result exceeds {max_result_bytes} byte limit"
        ))
    } else {
        Ok(())
    }
}

fn ensure_result_bound(value: &Value, max_result_bytes: usize) -> Result<(), String> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| format!("failed to encode data query result: {error}"))?;
    if encoded.len() > max_result_bytes {
        Err(format!(
            "data query result exceeds {max_result_bytes} byte limit"
        ))
    } else {
        Ok(())
    }
}

fn json_scalar_to_sql(value: &Value) -> Result<SqlValue, String> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::Bool(value) => Ok(SqlValue::Integer(i64::from(*value))),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(SqlValue::Integer(value))
            } else if let Some(value) = number.as_u64() {
                i64::try_from(value)
                    .map(SqlValue::Integer)
                    .map_err(|_| "SQLite equality integer exceeds i64 range".to_string())
            } else if let Some(value) = number.as_f64() {
                Ok(SqlValue::Real(value))
            } else {
                Err("unsupported JSON number for SQLite equality".to_string())
            }
        }
        Value::String(value) => Ok(SqlValue::Text(value.clone())),
        Value::Array(_) | Value::Object(_) => {
            Err("SQLite equality values must be JSON scalars".to_string())
        }
    }
}

fn csv_column<'a>(
    schema: &'a PublicationCsvSchema,
    name: &str,
) -> Result<&'a froglet_protocol::publication::PublicationCsvColumn, String> {
    schema
        .columns
        .iter()
        .find(|column| column.name == name)
        .ok_or_else(|| format!("unknown CSV column {name:?}"))
}

fn validate_csv_indexed_selection(
    schema: &PublicationCsvSchema,
    query: &ValidatedSelect,
) -> Result<(), String> {
    if !query.equals.is_empty()
        && !query.equals.keys().any(|name| {
            schema
                .columns
                .iter()
                .any(|column| column.name.as_str() == name.as_str() && column.indexed)
        })
    {
        return Err("CSV equality selection must include at least one indexed column".to_string());
    }
    Ok(())
}

fn csv_filter_to_sql(
    schema: &PublicationCsvSchema,
    name: &str,
    value: &Value,
) -> Result<SqlValue, String> {
    let column = csv_column(schema, name)?;
    if value.is_null() {
        return if column.nullable {
            Ok(SqlValue::Null)
        } else {
            Err(format!("CSV column {name:?} is not nullable"))
        };
    }
    match (column.column_type, value) {
        (PublicationCsvColumnType::String, Value::String(value)) => {
            Ok(SqlValue::Text(value.clone()))
        }
        (PublicationCsvColumnType::Integer, Value::Number(value)) => value
            .as_i64()
            .map(SqlValue::Integer)
            .ok_or_else(|| format!("CSV filter {name:?} requires an i64 integer")),
        (PublicationCsvColumnType::Number, Value::Number(value)) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(SqlValue::Real)
            .ok_or_else(|| format!("CSV filter {name:?} requires a finite number")),
        (PublicationCsvColumnType::Boolean, Value::Bool(value)) => {
            Ok(SqlValue::Integer(i64::from(*value)))
        }
        (expected, _) => Err(format!("CSV filter {name:?} requires a {expected} value")),
    }
}

fn csv_sqlite_value_to_json(
    schema: &PublicationCsvSchema,
    name: &str,
    value: ValueRef<'_>,
    max_cell_bytes: usize,
) -> Result<Value, String> {
    let column = csv_column(schema, name)?;
    if matches!(value, ValueRef::Null) {
        return if column.nullable {
            Ok(Value::Null)
        } else {
            Err(format!(
                "CSV cache returned null for non-nullable column {name:?}"
            ))
        };
    }
    match (column.column_type, value) {
        (PublicationCsvColumnType::String, ValueRef::Text(value)) => {
            if value.len() > max_cell_bytes {
                return Err(format!("CSV text cell exceeds {max_cell_bytes} byte limit"));
            }
            let value = std::str::from_utf8(value)
                .map_err(|_| "CSV cache returned non-UTF-8 text".to_string())?;
            Ok(Value::String(value.to_string()))
        }
        (PublicationCsvColumnType::Integer, ValueRef::Integer(value)) => Ok(json!(value)),
        (PublicationCsvColumnType::Number, ValueRef::Real(value)) => {
            serde_json::Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| "CSV cache returned a non-finite number".to_string())
        }
        (PublicationCsvColumnType::Boolean, ValueRef::Integer(0)) => Ok(Value::Bool(false)),
        (PublicationCsvColumnType::Boolean, ValueRef::Integer(1)) => Ok(Value::Bool(true)),
        (expected, _) => Err(format!(
            "CSV cache value for {name:?} does not match declared {expected} type"
        )),
    }
}

fn sqlite_value_to_json(value: ValueRef<'_>, max_cell_bytes: usize) -> Result<Value, String> {
    match value {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(value) => Ok(json!(value)),
        ValueRef::Real(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| "SQLite returned a non-finite real".to_string()),
        ValueRef::Text(value) => {
            if value.len() > max_cell_bytes {
                return Err(format!(
                    "SQLite text cell exceeds {max_cell_bytes} byte limit"
                ));
            }
            let value = std::str::from_utf8(value)
                .map_err(|_| "SQLite returned non-UTF-8 text".to_string())?;
            Ok(Value::String(value.to_string()))
        }
        ValueRef::Blob(value) => {
            if value.len() > max_cell_bytes {
                return Err(format!(
                    "SQLite blob cell exceeds {max_cell_bytes} byte limit"
                ));
            }
            Ok(json!({ "$blob_hex": hex::encode(value) }))
        }
    }
}

fn map_sqlite_error(error: rusqlite::Error, prefix: &str) -> String {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(ref inner, _)
            if inner.code == ErrorCode::OperationInterrupted
    ) {
        "SQLite data query deadline exceeded".to_string()
    } else {
        format!("{prefix}: {error}")
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn validate_name(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 128 {
        return Err(format!("data {label} name must be 1-128 bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!(
            "data {label} name must not contain control characters"
        ));
    }
    Ok(())
}

fn valid_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn read_bounded_file(path: &Path, max_bytes: usize, label: &str) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|error| format!("failed to open {label}: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if metadata.len() > max_bytes as u64 {
        return Err(format!("{label} exceeds {max_bytes} byte limit"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {label}: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!("{label} exceeds {max_bytes} byte limit"));
    }
    Ok(bytes)
}

fn resolve_source_path(root: &Path, relative_path: &Path) -> Result<PathBuf, String> {
    if relative_path.as_os_str().is_empty() {
        return Err("data source path must not be empty".to_string());
    }
    if relative_path.is_absolute() {
        return Err("data source path must be relative to its publication root".to_string());
    }
    if relative_path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(
            "data source path must not contain parent, current-directory, root, or prefix components"
                .to_string(),
        );
    }

    let root = root
        .canonicalize()
        .map_err(|error| format!("failed to resolve data source root: {error}"))?;
    if !root.is_dir() {
        return Err("data source root must be a directory".to_string());
    }
    let mut candidate = root.clone();
    for component in relative_path.components() {
        let Component::Normal(segment) = component else {
            return Err(
                "data source path must not contain parent, current-directory, root, or prefix components"
                    .to_string(),
            );
        };
        candidate.push(segment);
        let metadata = std::fs::symlink_metadata(&candidate)
            .map_err(|error| format!("failed to inspect data source path: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("data source path must not contain symlinks".to_string());
        }
    }
    let candidate = candidate
        .canonicalize()
        .map_err(|error| format!("failed to resolve data source path: {error}"))?;
    if !candidate.starts_with(&root) {
        return Err("data source path escapes its publication root".to_string());
    }
    if !candidate.is_file() {
        return Err("data source path must identify a regular file".to_string());
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use froglet_protocol::publication::{
        PublicationCsvColumn, PublicationDataFormat, PublicationDataSource,
    };
    use rusqlite::params;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn handler_cache_evicts_the_least_recently_used_package() {
        let root = tempfile::tempdir().expect("temp root");
        fs::write(root.path().join("rows.json"), b"[{\"id\":1}]").expect("write JSON fixture");
        let handler = Arc::new(
            DataQueryHandler::open(root.path(), "rows.json", DataQuerySourceKind::Json)
                .expect("open JSON handler"),
        );
        let cache = DataQueryHandlerCache::with_capacity(2);
        let initializations = Arc::new(AtomicUsize::new(0));

        for digest in ["11".repeat(32), "22".repeat(32)] {
            let handler = Arc::clone(&handler);
            let initializations = Arc::clone(&initializations);
            cache
                .get_or_try_init("froglet.test.data.v1", &digest, move || async move {
                    initializations.fetch_add(1, Ordering::SeqCst);
                    Ok(handler)
                })
                .await
                .expect("initialize cache entry");
        }

        let handler_for_hit = Arc::clone(&handler);
        let initializations_for_hit = Arc::clone(&initializations);
        cache
            .get_or_try_init(
                "froglet.test.data.v1",
                &"11".repeat(32),
                move || async move {
                    initializations_for_hit.fetch_add(1, Ordering::SeqCst);
                    Ok(handler_for_hit)
                },
            )
            .await
            .expect("refresh first entry");
        assert_eq!(initializations.load(Ordering::SeqCst), 2);

        for digest in ["33".repeat(32), "22".repeat(32)] {
            let handler = Arc::clone(&handler);
            let initializations = Arc::clone(&initializations);
            cache
                .get_or_try_init("froglet.test.data.v1", &digest, move || async move {
                    initializations.fetch_add(1, Ordering::SeqCst);
                    Ok(handler)
                })
                .await
                .expect("initialize replacement cache entry");
        }
        assert_eq!(
            initializations.load(Ordering::SeqCst),
            4,
            "adding a third package evicts the least-recently-used second package"
        );
    }

    fn csv_schema() -> PublicationCsvSchema {
        PublicationCsvSchema {
            collection: "people".to_string(),
            columns: vec![
                PublicationCsvColumn {
                    name: "id".to_string(),
                    column_type: PublicationCsvColumnType::Integer,
                    nullable: false,
                    indexed: true,
                },
                PublicationCsvColumn {
                    name: "name".to_string(),
                    column_type: PublicationCsvColumnType::String,
                    nullable: false,
                    indexed: false,
                },
                PublicationCsvColumn {
                    name: "active".to_string(),
                    column_type: PublicationCsvColumnType::Boolean,
                    nullable: false,
                    indexed: true,
                },
                PublicationCsvColumn {
                    name: "score".to_string(),
                    column_type: PublicationCsvColumnType::Number,
                    nullable: true,
                    indexed: false,
                },
            ],
        }
    }

    fn csv_package_digest(bytes: &[u8], schema: &PublicationCsvSchema) -> String {
        let package = PublicationDataSource {
            format: PublicationDataFormat::Csv,
            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            csv_schema: Some(schema.clone()),
        };
        package.csv_package_digest().unwrap()
    }

    #[tokio::test]
    async fn json_fixture_supports_describe_and_structured_select() {
        let root = tempfile::tempdir().expect("temp root");
        fs::write(
            root.path().join("samples.json"),
            serde_json::to_vec(&json!({
                "samples": [
                    {"id": 1, "status": "ready", "private_note": "a"},
                    {"id": 2, "status": "pending", "private_note": "b"},
                    {"id": 3, "status": "ready", "private_note": "c"}
                ]
            }))
            .unwrap(),
        )
        .expect("write fixture");
        let handler =
            DataQueryHandler::open(root.path(), "samples.json", DataQuerySourceKind::Json)
                .expect("open JSON fixture");

        let description = handler.execute(json!({"op": "describe"})).await.unwrap();
        assert_eq!(description["source_kind"], "json");
        assert_eq!(
            description["collections"]["samples"],
            json!(["id", "private_note", "status"])
        );

        let selected = handler
            .execute(json!({
                "op": "select",
                "collection": "samples",
                "columns": ["id", "status"],
                "equals": {"status": "ready"},
                "limit": 1
            }))
            .await
            .unwrap();
        assert_eq!(selected["rows"], json!([{"id": 1, "status": "ready"}]));
        assert_eq!(selected["returned"], 1);
        assert_eq!(selected["has_more"], true);
    }

    #[tokio::test]
    async fn invocation_rejects_raw_sql_paths_and_non_scalar_filters() {
        let root = tempfile::tempdir().expect("temp root");
        fs::write(root.path().join("rows.json"), b"[{\"id\":1}]").unwrap();
        let handler =
            DataQueryHandler::open(root.path(), "rows.json", DataQuerySourceKind::Json).unwrap();

        for input in [
            json!({"op": "select", "collection": "rows", "sql": "DELETE FROM rows"}),
            json!({"op": "select", "collection": "rows", "path": "../secret"}),
            json!({"op": "select", "collection": "rows", "equals": {"id": [1]}}),
        ] {
            assert!(handler.execute(input).await.is_err());
        }
    }

    #[test]
    fn source_binding_rejects_parent_and_symlink_escape() {
        let parent = tempfile::tempdir().expect("temp parent");
        let root = parent.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(parent.path().join("outside.json"), b"[]").unwrap();

        let error = DataQueryHandler::open(
            &root,
            Path::new("../outside.json"),
            DataQuerySourceKind::Json,
        )
        .err()
        .expect("parent traversal must fail");
        assert!(error.contains("parent"));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(parent.path().join("outside.json"), root.join("link.json"))
                .unwrap();
            let error =
                DataQueryHandler::open(&root, Path::new("link.json"), DataQuerySourceKind::Json)
                    .err()
                    .expect("symlink escape must fail");
            assert!(error.contains("symlink"));
        }
    }

    #[tokio::test]
    async fn sqlite_is_read_only_and_uses_structured_query_allowlist() {
        let root = tempfile::tempdir().expect("temp root");
        let db_path = root.path().join("samples.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE samples (id INTEGER PRIMARY KEY, status TEXT, payload BLOB);
                 INSERT INTO samples (id, status, payload) VALUES (1, 'ready', x'cafe');
                 INSERT INTO samples (id, status, payload) VALUES (2, 'pending', x'beef');",
            )
            .unwrap();
        drop(connection);

        let handler =
            DataQueryHandler::open(root.path(), "samples.sqlite", DataQuerySourceKind::Sqlite)
                .expect("open SQLite fixture");
        let selected = handler
            .execute(json!({
                "op": "select",
                "collection": "samples",
                "columns": ["id", "payload"],
                "equals": {"status": "ready"}
            }))
            .await
            .unwrap();
        assert_eq!(
            selected["rows"],
            json!([{"id": 1, "payload": {"$blob_hex": "cafe"}}])
        );

        let injection = handler
            .execute(json!({
                "op": "select",
                "collection": "samples\"; DELETE FROM samples; --"
            }))
            .await
            .expect_err("identifier injection must fail before SQL construction");
        assert!(injection.contains("unknown data collection"));

        let DataSource::Sqlite(dataset) = &handler.source else {
            unreachable!();
        };
        let connection = dataset.connection.lock().unwrap();
        assert!(connection.execute("DELETE FROM samples", []).is_err());
        drop(connection);
        drop(handler);

        let connection = Connection::open(&db_path).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM samples", params![], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2, "read-only handler must not mutate source data");
    }

    #[tokio::test]
    async fn csv_builds_indexed_cache_and_preserves_explicit_types() {
        let root = tempfile::tempdir().expect("temp root");
        let bytes = b"id,name,active,score\n1,\"Ada, Lovelace\",true,9.5\n2,Grace,false,\n";
        fs::write(root.path().join("people.csv"), bytes).unwrap();
        let schema = csv_schema();
        let package_digest = csv_package_digest(bytes, &schema);
        let handler = DataQueryHandler::open_csv_indexed(
            root.path(),
            "people.csv",
            schema.clone(),
            &package_digest,
        )
        .expect("build CSV cache");

        let description = handler.execute(json!({"op": "describe"})).await.unwrap();
        assert_eq!(description["source_kind"], "csv");
        assert_eq!(description["row_counts"]["people"], 2);
        assert_eq!(description["csv_schema"]["columns"][0]["type"], "integer");

        let selected = handler
            .execute(json!({
                "op": "select",
                "collection": "people",
                "columns": ["id", "name", "active", "score"],
                "equals": {"active": false}
            }))
            .await
            .unwrap();
        assert_eq!(
            selected["rows"],
            json!([{"id": 2, "name": "Grace", "active": false, "score": null}])
        );
        assert!(
            root.path()
                .join(format!("{package_digest}.csv.sqlite"))
                .is_file()
        );

        let reopened =
            DataQueryHandler::open_csv_indexed(root.path(), "people.csv", schema, &package_digest)
                .expect("reuse CSV cache");
        assert_eq!(reopened.csv_summary().unwrap().1, 2);
    }

    #[tokio::test]
    async fn csv_rebuilds_disposable_cache_after_content_metadata_or_file_corruption() {
        let root = tempfile::tempdir().expect("temp root");
        let bytes = b"id,name,active,score\n1,Ada,true,9.5\n2,Grace,false,\n";
        fs::write(root.path().join("people.csv"), bytes).unwrap();
        let schema = csv_schema();
        let package_digest = csv_package_digest(bytes, &schema);
        let cache_path = root.path().join(format!("{package_digest}.csv.sqlite"));

        let open = || {
            DataQueryHandler::open_csv_indexed(
                root.path(),
                "people.csv",
                schema.clone(),
                &package_digest,
            )
            .expect("open or rebuild CSV cache")
        };

        drop(open());
        let connection = Connection::open(&cache_path).unwrap();
        connection
            .execute("UPDATE people SET name = 'Mallory' WHERE id = 1", [])
            .unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA integrity_check(1)", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "ok",
            "semantic tampering fixture must remain a structurally valid SQLite file"
        );
        drop(connection);
        let handler = open();
        let selected = handler
            .execute(json!({
                "op": "select",
                "collection": "people",
                "columns": ["name"],
                "equals": {"id": 1}
            }))
            .await
            .unwrap();
        assert_eq!(selected["rows"], json!([{"name": "Ada"}]));
        drop(handler);

        let connection = Connection::open(&cache_path).unwrap();
        let encoded: String = connection
            .query_row(
                &format!(
                    "SELECT metadata_json FROM {} WHERE id = 1",
                    quote_identifier(CSV_CACHE_METADATA_TABLE)
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut metadata: Value = serde_json::from_str(&encoded).unwrap();
        metadata["content_digest"] = json!("00".repeat(32));
        connection
            .execute(
                &format!(
                    "UPDATE {} SET metadata_json = ?1 WHERE id = 1",
                    quote_identifier(CSV_CACHE_METADATA_TABLE)
                ),
                [serde_json::to_string(&metadata).unwrap()],
            )
            .unwrap();
        drop(connection);
        drop(open());

        fs::write(&cache_path, b"not a SQLite database").unwrap();
        let handler = open();
        assert_eq!(handler.csv_summary().unwrap().1, 2);
        drop(handler);

        let connection = Connection::open_with_flags(
            &cache_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA integrity_check(1)", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    }

    #[tokio::test]
    async fn csv_rejects_unindexed_filter_and_enforces_row_limit() {
        let root = tempfile::tempdir().expect("temp root");
        let bytes = b"id,name,active,score\n1,Ada,true,1\n2,Grace,false,2\n";
        fs::write(root.path().join("people.csv"), bytes).unwrap();
        let schema = csv_schema();
        let package_digest = csv_package_digest(bytes, &schema);
        let handler = DataQueryHandler::open_csv_indexed(
            root.path(),
            "people.csv",
            schema.clone(),
            &package_digest,
        )
        .unwrap();

        let error = handler
            .execute(json!({
                "op": "select",
                "collection": "people",
                "equals": {"name": "Ada"}
            }))
            .await
            .expect_err("non-indexed-only CSV filter must fail");
        assert!(error.contains("indexed column"));

        let limited_root = tempfile::tempdir().unwrap();
        fs::write(limited_root.path().join("people.csv"), bytes).unwrap();
        let limits = DataQueryLimits {
            max_csv_rows: 1,
            ..DataQueryLimits::default()
        };
        let error = DataQueryHandler::open_csv_indexed_with_limits(
            limited_root.path(),
            "people.csv",
            schema,
            &package_digest,
            limits,
        )
        .err()
        .expect("CSV row cap must fail closed");
        assert!(error.contains("row limit"));
    }

    #[test]
    fn csv_binding_includes_schema_and_header_must_match() {
        let root = tempfile::tempdir().expect("temp root");
        let bytes = b"name,id,active,score\nAda,1,true,1\n";
        fs::write(root.path().join("people.csv"), bytes).unwrap();
        let schema = csv_schema();
        let package_digest = csv_package_digest(bytes, &schema);
        let error = DataQueryHandler::open_csv_indexed(
            root.path(),
            "people.csv",
            schema.clone(),
            &package_digest,
        )
        .err()
        .expect("header order mismatch must fail");
        assert!(error.contains("header"));

        let mut changed = schema;
        changed.columns[1].nullable = true;
        let error =
            DataQueryHandler::open_csv_indexed(root.path(), "people.csv", changed, &package_digest)
                .err()
                .expect("schema tampering must change package digest");
        assert!(error.contains("publication binding"));
    }
}
