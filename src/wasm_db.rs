use std::{
    env,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rusqlite::{
    Connection, ErrorCode, OpenFlags,
    types::{Value as SqlValue, ValueRef},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{config::WasmSqlitePolicy, wasm::WASM_CAPABILITY_SQLITE_QUERY_READ_PREFIX};

const DEFAULT_SQLITE_MAX_CELL_BYTES: usize = 64 * 1024;
const SQLITE_DEADLINE_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, Deserialize)]
pub struct DbQueryRequest {
    pub handle: String,
    pub sql: String,
    #[serde(default)]
    pub params: Vec<Value>,
}

pub fn query(
    policy: &WasmSqlitePolicy,
    granted_capabilities: &[String],
    db_queries_used: &mut u32,
    request: DbQueryRequest,
    deadline: Option<Instant>,
) -> Result<Value, String> {
    if *db_queries_used >= policy.max_queries_per_execution {
        return Err("Wasm DB query limit exceeded".to_string());
    }
    *db_queries_used = db_queries_used.saturating_add(1);
    enforce_deadline(deadline)?;

    let capability = format!(
        "{WASM_CAPABILITY_SQLITE_QUERY_READ_PREFIX}{}",
        request.handle
    );
    if !granted_capabilities
        .iter()
        .any(|granted| granted == &capability)
    {
        return Err(format!("missing granted capability '{capability}'"));
    }

    let handle = policy.handles.get(&request.handle).ok_or_else(|| {
        format!(
            "sqlite handle '{}' is not configured on this provider",
            request.handle
        )
    })?;
    let connection = Connection::open_with_flags(
        &handle.path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("failed to open sqlite handle '{}': {error}", request.handle))?;
    let _deadline_interrupt = QueryDeadlineInterrupt::start(&connection, deadline);
    let max_cell_bytes = sqlite_max_cell_bytes(policy);

    reject_unsafe_sql(&request.sql)?;

    let params = request
        .params
        .iter()
        .map(json_value_to_sql)
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection
        .prepare(&request.sql)
        .map_err(|error| format!("sqlite prepare failed: {error}"))?;
    let columns = statement
        .column_names()
        .iter()
        .map(|column| column.to_string())
        .collect::<Vec<_>>();
    let mut encoded_size = base_response_size(&columns)?;
    let column_count = columns.len();
    let mut rows = statement
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|error| map_sqlite_error(error, deadline, "sqlite query failed"))?;
    let mut results = Vec::new();

    loop {
        enforce_deadline(deadline)?;
        let row = rows
            .next()
            .map_err(|error| map_sqlite_error(error, deadline, "sqlite row iteration failed"))?;
        let Some(row) = row else {
            break;
        };
        if results.len() >= policy.max_rows_per_query {
            return Err("sqlite query returned too many rows".to_string());
        }

        let mut values = Vec::with_capacity(column_count);
        let mut row_encoded_size = 2usize;
        for index in 0..column_count {
            let value = row
                .get_ref(index)
                .map_err(|error| format!("sqlite column decoding failed: {error}"))?;
            let json_value = sql_value_to_json(value, max_cell_bytes)?;
            row_encoded_size = row_encoded_size
                .checked_add(
                    serde_json::to_vec(&json_value)
                        .map_err(|error| {
                            format!("failed to encode sqlite column value to json: {error}")
                        })?
                        .len(),
                )
                .ok_or_else(|| "sqlite query result exceeds provider policy limit".to_string())?;
            if index > 0 {
                row_encoded_size = row_encoded_size.checked_add(1).ok_or_else(|| {
                    "sqlite query result exceeds provider policy limit".to_string()
                })?;
            }
            values.push(json_value);
        }

        let comma_size = usize::from(!results.is_empty());
        let next_encoded_size = encoded_size
            .checked_add(comma_size)
            .and_then(|size| size.checked_add(row_encoded_size))
            .ok_or_else(|| "sqlite query result exceeds provider policy limit".to_string())?;
        if next_encoded_size > policy.max_result_bytes {
            return Err("sqlite query result exceeds provider policy limit".to_string());
        }
        encoded_size = next_encoded_size;
        results.push(values);
    }

    let response = json!({
        "columns": columns,
        "rows": results,
    });
    let encoded = serde_json::to_vec(&response)
        .map_err(|error| format!("failed to encode sqlite response: {error}"))?;
    if encoded.len() > policy.max_result_bytes {
        return Err("sqlite query result exceeds provider policy limit".to_string());
    }

    Ok(response)
}

struct QueryDeadlineInterrupt {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl QueryDeadlineInterrupt {
    fn start(connection: &Connection, deadline: Option<Instant>) -> Option<Self> {
        let deadline = deadline?;
        if deadline.checked_duration_since(Instant::now()).is_none() {
            connection.get_interrupt_handle().interrupt();
            return None;
        }

        let interrupt_handle = connection.get_interrupt_handle();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let worker = thread::Builder::new()
            .name("froglet-wasm-sqlite-deadline".to_string())
            .spawn(move || {
                loop {
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    match deadline.checked_duration_since(Instant::now()) {
                        Some(remaining) if remaining > SQLITE_DEADLINE_POLL_INTERVAL => {
                            thread::sleep(SQLITE_DEADLINE_POLL_INTERVAL);
                        }
                        Some(remaining) if remaining > Duration::ZERO => {
                            thread::sleep(remaining);
                        }
                        _ => {
                            interrupt_handle.interrupt();
                            break;
                        }
                    }
                }
            })
            .ok()?;

        Some(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for QueryDeadlineInterrupt {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn reject_unsafe_sql(sql: &str) -> Result<(), String> {
    // Block semicolons to prevent multi-statement execution.
    if sql.contains(';') {
        return Err(
            "multi-statement queries are not permitted for wasm sqlite queries".to_string(),
        );
    }
    // Strip SQL comments before keyword analysis. The primary defense against
    // write operations is SQLITE_OPEN_READ_ONLY; this check is defense-in-depth.
    let stripped = strip_sql_comments(sql);
    let normalized = stripped.to_ascii_uppercase();
    let normalized = normalized.trim();
    // Only allow SELECT statements. Block ATTACH, PRAGMA, multi-statement, and DDL/DML.
    if !normalized.starts_with("SELECT") {
        return Err("only SELECT statements are permitted for wasm sqlite queries".to_string());
    }
    // Block ATTACH even inside subqueries or CTEs.
    let tokens: Vec<&str> = normalized.split_ascii_whitespace().collect();
    for token in &tokens {
        if matches!(*token, "ATTACH" | "PRAGMA" | "DETACH") {
            return Err(format!(
                "the SQL keyword '{}' is not permitted in wasm sqlite queries",
                token.to_ascii_lowercase()
            ));
        }
    }
    Ok(())
}

/// Strips `-- line comments` and `/* block comments */` from SQL text.
fn strip_sql_comments(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'-' && bytes[i + 1] == b'-' {
            // Skip until end of line.
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // Skip until closing */.
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2; // skip */
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

fn sqlite_max_cell_bytes(policy: &WasmSqlitePolicy) -> usize {
    match env::var("FROGLET_WASM_SQLITE_MAX_CELL_BYTES") {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|size| *size > 0)
            .unwrap_or(DEFAULT_SQLITE_MAX_CELL_BYTES)
            .min(policy.max_result_bytes.max(1)),
        Err(_) => DEFAULT_SQLITE_MAX_CELL_BYTES.min(policy.max_result_bytes.max(1)),
    }
}

fn base_response_size(columns: &[String]) -> Result<usize, String> {
    serde_json::to_vec(&json!({
        "columns": columns,
        "rows": [],
    }))
    .map(|bytes| bytes.len())
    .map_err(|error| format!("failed to encode sqlite response: {error}"))
}

fn enforce_deadline(deadline: Option<Instant>) -> Result<(), String> {
    if deadline.is_some() && deadline_remaining(deadline).is_none() {
        Err("sqlite query deadline exceeded".to_string())
    } else {
        Ok(())
    }
}

fn deadline_remaining(deadline: Option<Instant>) -> Option<Duration> {
    deadline.and_then(|limit| limit.checked_duration_since(Instant::now()))
}

fn map_sqlite_error(error: rusqlite::Error, deadline: Option<Instant>, prefix: &str) -> String {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(ref inner, _)
            if inner.code == ErrorCode::OperationInterrupted
    ) && deadline_expired(deadline)
    {
        return "sqlite query deadline exceeded".to_string();
    }
    format!("{prefix}: {error}")
}

fn deadline_expired(deadline: Option<Instant>) -> bool {
    deadline.is_some() && deadline_remaining(deadline).is_none()
}

fn json_value_to_sql(value: &Value) -> Result<SqlValue, String> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::Bool(boolean) => Ok(SqlValue::Integer(i64::from(*boolean))),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Ok(SqlValue::Integer(integer))
            } else if let Some(unsigned) = number.as_u64() {
                let integer = i64::try_from(unsigned)
                    .map_err(|_| "sqlite parameter u64 exceeds i64 range".to_string())?;
                Ok(SqlValue::Integer(integer))
            } else if let Some(float) = number.as_f64() {
                Ok(SqlValue::Real(float))
            } else {
                Err("unsupported sqlite numeric parameter".to_string())
            }
        }
        Value::String(text) => Ok(SqlValue::Text(text.clone())),
        Value::Array(_) | Value::Object(_) => {
            Err("sqlite parameters must be null, boolean, number, or string".to_string())
        }
    }
}

fn sql_value_to_json(value: ValueRef<'_>, max_cell_bytes: usize) -> Result<Value, String> {
    match value {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(integer) => Ok(json!(integer)),
        ValueRef::Real(float) => Ok(json!(float)),
        ValueRef::Text(text) => {
            if text.len() > max_cell_bytes {
                return Err("sqlite text column exceeds per-cell provider policy limit".to_string());
            }
            Ok(Value::String(String::from_utf8(text.to_vec()).map_err(
                |_| "sqlite text column is not valid utf-8".to_string(),
            )?))
        }
        ValueRef::Blob(bytes) => {
            if bytes.len() > max_cell_bytes {
                return Err("sqlite blob column exceeds per-cell provider policy limit".to_string());
            }
            Ok(json!({
                "base64": BASE64_STANDARD.encode(bytes),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WasmSqliteHandleConfig, WasmSqlitePolicy};
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "froglet-wasm-db-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn missing_capability_is_rejected() {
        let temp_dir = unique_temp_dir("missing-capability");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("test.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute("CREATE TABLE sample (value INTEGER)", [])
            .unwrap();

        let policy = WasmSqlitePolicy {
            max_queries_per_execution: 1,
            max_rows_per_query: 10,
            max_result_bytes: 1_024,
            handles: BTreeMap::from([(
                "main".to_string(),
                WasmSqliteHandleConfig {
                    path: db_path.clone(),
                },
            )]),
        };

        let mut db_queries_used = 0;
        let error = query(
            &policy,
            &[],
            &mut db_queries_used,
            DbQueryRequest {
                handle: "main".to_string(),
                sql: "SELECT 1".to_string(),
                params: Vec::new(),
            },
            None,
        )
        .expect_err("expected missing capability failure");
        assert!(
            error.contains("missing granted capability"),
            "unexpected error: {error}"
        );

        let _ = std::fs::remove_file(db_path);
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn deadline_exceeded_is_rejected_before_query_executes() {
        let temp_dir = unique_temp_dir("deadline");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("deadline.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute("CREATE TABLE sample (value INTEGER)", [])
            .unwrap();

        let policy = WasmSqlitePolicy {
            max_queries_per_execution: 1,
            max_rows_per_query: 10,
            max_result_bytes: 1_024,
            handles: BTreeMap::from([(
                "main".to_string(),
                WasmSqliteHandleConfig {
                    path: db_path.clone(),
                },
            )]),
        };

        let mut db_queries_used = 0;
        let error = query(
            &policy,
            &[format!("{WASM_CAPABILITY_SQLITE_QUERY_READ_PREFIX}main")],
            &mut db_queries_used,
            DbQueryRequest {
                handle: "main".to_string(),
                sql: "SELECT 1".to_string(),
                params: Vec::new(),
            },
            Some(Instant::now()),
        )
        .expect_err("expected deadline failure");

        assert!(error.contains("deadline"), "unexpected error: {error}");

        let _ = std::fs::remove_file(db_path);
        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
