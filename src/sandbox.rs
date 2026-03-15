use std::{
    env, io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::info;
use wasmtime::{
    Config, Engine, ExternType, Linker, Memory, MemoryType, Module, Store, StoreLimits,
    StoreLimitsBuilder, Trap,
};

use crate::canonical_json;

pub const WASM_FUEL_LIMIT: u64 = 50_000_000;
pub const WASM_MAX_MEMORY_BYTES: usize = 8 * 1024 * 1024;
pub const WASM_MAX_OUTPUT_BYTES: usize = 128 * 1024;
const WASM_EPOCH_TICK_MILLIS: u64 = 10;
const WASM_PAGE_BYTES: u64 = 64 * 1024;
const WASM_MAX_MEMORY_PAGES: u64 = (WASM_MAX_MEMORY_BYTES as u64) / WASM_PAGE_BYTES;

pub struct ExecutionPermit(OwnedSemaphorePermit);

struct EpochTicker {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl EpochTicker {
    fn start(engine: Engine) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let handle = thread::Builder::new()
            .name("froglet-wasm-epoch".to_string())
            .spawn(move || {
                while !stop_flag.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(WASM_EPOCH_TICK_MILLIS));
                    engine.increment_epoch();
                }
            })
            .map_err(|error| format!("failed to start Wasm epoch ticker: {error}"))?;

        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub struct WasmSandbox {
    engine: Engine,
    concurrency_semaphore: Arc<Semaphore>,
    _epoch_ticker: EpochTicker,
}

impl WasmSandbox {
    pub fn from_env() -> Result<Self, String> {
        Self::new(wasm_concurrency_limit())
    }

    pub fn new(concurrency_limit: usize) -> Result<Self, String> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)
            .map_err(|error| format!("failed to initialize Wasmtime engine: {error}"))?;
        let epoch_ticker = EpochTicker::start(engine.clone())?;

        Ok(Self {
            engine,
            concurrency_semaphore: Arc::new(Semaphore::new(concurrency_limit.max(1))),
            _epoch_ticker: epoch_ticker,
        })
    }

    pub fn warm_up(&self) {
        let _ = &self.engine;
        info!("Initialized Wasmtime JIT compiler.");
    }

    pub fn try_acquire_execution_permit(&self) -> Result<ExecutionPermit, String> {
        self.concurrency_semaphore
            .clone()
            .try_acquire_owned()
            .map(ExecutionPermit)
            .map_err(|_| "Wasm concurrency limit reached".to_string())
    }

    pub fn execute_module(
        &self,
        wasm_bytes: &[u8],
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let permit = self
            .try_acquire_execution_permit()
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
        self.execute_module_with_permit(wasm_bytes, input, permit, timeout)
    }

    pub fn execute_module_with_permit(
        &self,
        wasm_bytes: &[u8],
        input: &Value,
        permit: ExecutionPermit,
        timeout: Duration,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let _permit = permit.0;
        let module = Module::new(&self.engine, wasm_bytes)?;
        validate_module_policy(&module)?;

        let limits: StoreLimits = StoreLimitsBuilder::new()
            .memory_size(WASM_MAX_MEMORY_BYTES)
            .instances(1)
            .tables(1)
            .memories(1)
            .trap_on_grow_failure(true)
            .build();

        let mut store = Store::new(&self.engine, limits);
        store.limiter(|limits| limits);
        store.set_fuel(WASM_FUEL_LIMIT)?;
        store.set_epoch_deadline(timeout_to_epoch_ticks(timeout));
        store.epoch_deadline_trap();

        let linker = Linker::new(&self.engine);
        let instance = linker.instantiate(&mut store, &module)?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| boxed_message("Wasm module must export memory".to_string()))?;
        let alloc_func = instance
            .get_typed_func::<i32, i32>(&mut store, "alloc")
            .map_err(|error| normalize_wasm_error(error, timeout))?;
        let run_func = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "run")
            .map_err(|error| normalize_wasm_error(error, timeout))?;
        let dealloc_func = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "dealloc")
            .ok();

        let input_bytes =
            canonical_json::to_vec(input).map_err(|error| boxed_message(error.to_string()))?;
        let input_len_i32 = i32::try_from(input_bytes.len())
            .map_err(|_| boxed_message("Wasm input too large".to_string()))?;
        let input_ptr = alloc_func
            .call(&mut store, input_len_i32)
            .map_err(|error| normalize_wasm_error(error, timeout))?;

        if input_ptr < 0 {
            return Err(boxed_message(
                "Wasm alloc returned a negative pointer".to_string(),
            ));
        }

        write_memory(&memory, &mut store, input_ptr as usize, &input_bytes)?;

        let packed = run_func
            .call(&mut store, (input_ptr, input_len_i32))
            .map_err(|error| normalize_wasm_error(error, timeout))?;

        if let Some(dealloc_func) = &dealloc_func {
            let _ = dealloc_func.call(&mut store, (input_ptr, input_len_i32));
        }

        let packed = packed as u64;
        let result_ptr = (packed >> 32) as usize;
        let result_len = (packed & 0xffff_ffff) as usize;
        if result_len > WASM_MAX_OUTPUT_BYTES {
            return Err(boxed_message(
                "Wasm module output size limit exceeded".to_string(),
            ));
        }
        let result_bytes = read_memory(&memory, &mut store, result_ptr, result_len)?;

        if let Some(dealloc_func) = &dealloc_func
            && let (Ok(result_ptr_i32), Ok(result_len_i32)) =
                (i32::try_from(result_ptr), i32::try_from(result_len))
        {
            let _ = dealloc_func.call(&mut store, (result_ptr_i32, result_len_i32));
        }

        let result_text = String::from_utf8(result_bytes)
            .map_err(|_| boxed_message("Wasm result is not valid UTF-8 JSON".to_string()))?;
        let result =
            serde_json::from_str(&result_text).map_err(|error| boxed_message(error.to_string()))?;

        Ok(result)
    }
}

fn concurrency_limit(name: &str, default: usize) -> usize {
    match env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| *limit > 0)
            .unwrap_or(default),
        Err(_) => default,
    }
}

pub fn wasm_concurrency_limit() -> usize {
    concurrency_limit("FROGLET_WASM_CONCURRENCY_LIMIT", 16)
}

fn validate_module_policy(module: &Module) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let imports: Vec<String> = module
        .imports()
        .map(|import| {
            format!(
                "{}::{} ({})",
                import.module(),
                import.name(),
                extern_type_name(&import.ty())
            )
        })
        .collect();
    if !imports.is_empty() {
        return Err(boxed_message(format!(
            "Wasm host imports are not permitted in froglet.wasm.run_json.v1: {}",
            imports.join(", ")
        )));
    }

    let exported_memories: Vec<(String, MemoryType)> = module
        .exports()
        .filter_map(|export| {
            export
                .ty()
                .memory()
                .cloned()
                .map(|memory| (export.name().to_string(), memory))
        })
        .collect();
    if exported_memories.len() > 1 {
        return Err(boxed_message(
            "Wasm module must not export more than one memory".to_string(),
        ));
    }
    if let Some((name, memory)) = exported_memories.first() {
        if name != "memory" {
            return Err(boxed_message(
                "Wasm module must export its linear memory as \"memory\"".to_string(),
            ));
        }
        validate_memory_type(memory)?;
    }

    Ok(())
}

fn validate_memory_type(
    memory: &MemoryType,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if memory.is_64() {
        return Err(boxed_message(
            "64-bit Wasm memories are not permitted in froglet.wasm.run_json.v1".to_string(),
        ));
    }
    if memory.is_shared() {
        return Err(boxed_message(
            "shared Wasm memories are not permitted in froglet.wasm.run_json.v1".to_string(),
        ));
    }
    if memory.minimum() > WASM_MAX_MEMORY_PAGES {
        return Err(boxed_message(format!(
            "Wasm module initial memory limit exceeded: {} pages > {} pages",
            memory.minimum(),
            WASM_MAX_MEMORY_PAGES
        )));
    }
    if let Some(maximum) = memory.maximum()
        && maximum > WASM_MAX_MEMORY_PAGES
    {
        return Err(boxed_message(format!(
            "Wasm module declared memory maximum exceeds v1 limit: {} pages > {} pages",
            maximum, WASM_MAX_MEMORY_PAGES
        )));
    }

    Ok(())
}

fn extern_type_name(ty: &ExternType) -> &'static str {
    match ty {
        ExternType::Func(_) => "func",
        ExternType::Global(_) => "global",
        ExternType::Table(_) => "table",
        ExternType::Memory(_) => "memory",
        ExternType::Tag(_) => "tag",
    }
}

fn timeout_to_epoch_ticks(timeout: Duration) -> u64 {
    let millis = timeout.as_millis().max(1) as u64;
    millis.div_ceil(WASM_EPOCH_TICK_MILLIS).max(1)
}

fn is_wasm_timeout_message(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("interrupt")
        || normalized.contains("epoch deadline")
        || normalized.contains("deadline exceeded")
}

fn normalize_wasm_error(
    error: wasmtime::Error,
    timeout: Duration,
) -> Box<dyn std::error::Error + Send + Sync> {
    let message = error.to_string();
    let debug_message = format!("{error:?}");
    let trap = error.downcast_ref::<Trap>().copied();
    if matches!(trap, Some(Trap::Interrupt))
        || is_wasm_timeout_message(&message)
        || is_wasm_timeout_message(&debug_message)
    {
        boxed_message(format!(
            "Wasm module wall-clock timeout exceeded after {}s",
            timeout.as_secs()
        ))
    } else if matches!(trap, Some(Trap::OutOfFuel)) {
        boxed_message("Wasm module execution limit exceeded".to_string())
    } else {
        boxed_message(message)
    }
}

fn write_memory(
    memory: &Memory,
    store: &mut Store<StoreLimits>,
    ptr: usize,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let data = memory.data_mut(store);
    let end = ptr
        .checked_add(bytes.len())
        .ok_or_else(|| boxed_message("Wasm memory write overflow".to_string()))?;
    let target = data
        .get_mut(ptr..end)
        .ok_or_else(|| boxed_message("Wasm alloc returned out-of-bounds pointer".to_string()))?;
    target.copy_from_slice(bytes);
    Ok(())
}

fn read_memory(
    memory: &Memory,
    store: &mut Store<StoreLimits>,
    ptr: usize,
    len: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let data = memory.data(store);
    let end = ptr
        .checked_add(len)
        .ok_or_else(|| boxed_message("Wasm memory read overflow".to_string()))?;
    let slice = data
        .get(ptr..end)
        .ok_or_else(|| boxed_message("Wasm result pointer is out of bounds".to_string()))?;
    Ok(slice.to_vec())
}

fn boxed_message(message: String) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(io::Error::other(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use wat::parse_str as wat2wasm;

    const VALID_WASM_HEX: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";
    const INFINITE_WASM_HEX: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0f02040041100b080003400c000b000b";

    fn test_sandbox() -> WasmSandbox {
        WasmSandbox::new(16).expect("sandbox")
    }

    #[test]
    fn wasm_wall_clock_timeout_is_reported() {
        let wasm_bytes = hex::decode(INFINITE_WASM_HEX).unwrap();
        let error = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::ZERO)
            .expect_err("expected timeout");
        assert!(
            error.to_string().to_ascii_lowercase().contains("timeout"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn wasm_json_abi_returns_json_value() {
        let wasm_bytes = hex::decode(VALID_WASM_HEX).unwrap();
        let result = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::from_secs(1))
            .unwrap();
        assert_eq!(result, Value::from(42));
    }

    #[test]
    fn wasm_requires_memory_export() {
        let wasm_bytes = wat2wasm(
            r#"(module
                (func (export "alloc") (param i32) (result i32)
                    local.get 0
                    drop
                    i32.const 0)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const 0))"#,
        )
        .unwrap();

        let error = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::from_secs(1))
            .expect_err("expected missing memory export");
        assert!(
            error.to_string().contains("export memory"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn wasm_rejects_negative_alloc_pointer() {
        let wasm_bytes = wat2wasm(
            r#"(module
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32)
                    i32.const -1)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const 0))"#,
        )
        .unwrap();

        let error = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::from_secs(1))
            .expect_err("expected negative alloc pointer failure");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("negative pointer"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn wasm_rejects_non_utf8_json_output() {
        let wasm_bytes = wat2wasm(
            r#"(module
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32)
                    i32.const 16)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const 1)
                (data (i32.const 0) "\ff"))"#,
        )
        .unwrap();

        let error = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::from_secs(1))
            .expect_err("expected invalid utf-8 failure");
        assert!(
            error.to_string().to_ascii_lowercase().contains("utf-8"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn wasm_rejects_non_json_output() {
        let wasm_bytes = wat2wasm(
            r#"(module
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32)
                    i32.const 16)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const 4)
                (data (i32.const 0) "nope"))"#,
        )
        .unwrap();

        let error = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::from_secs(1))
            .expect_err("expected invalid json failure");
        assert!(
            error.to_string().to_ascii_lowercase().contains("expected"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn wasm_rejects_output_that_exceeds_limit() {
        let oversized_json = format!("\"{}\"", "a".repeat(WASM_MAX_OUTPUT_BYTES));
        let payload = oversized_json.replace('\\', "\\\\").replace('"', "\\\"");
        let memory_pages = oversized_json.len().div_ceil(65_536);
        let wasm_bytes = wat2wasm(format!(
            r#"(module
                (memory (export "memory") {memory_pages})
                (func (export "alloc") (param i32) (result i32)
                    i32.const 16)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const {result_len})
                (data (i32.const 0) "{payload}"))"#,
            memory_pages = memory_pages.max(1),
            result_len = oversized_json.len(),
            payload = payload,
        ))
        .unwrap();

        let error = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::from_secs(1))
            .expect_err("expected oversized output failure");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("limit exceeded"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn wasm_rejects_host_imports() {
        let wasm_bytes = wat2wasm(
            r#"(module
                (import "env" "sleep_ms" (func $sleep_ms (param i32)))
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32)
                    i32.const 16)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const 0))"#,
        )
        .unwrap();

        let error = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::from_secs(1))
            .expect_err("expected host import rejection");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("host imports are not permitted"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn wasm_rejects_initial_memory_above_limit() {
        let wasm_bytes = wat2wasm(
            r#"(module
                (memory (export "memory") 129)
                (func (export "alloc") (param i32) (result i32)
                    i32.const 16)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const 0))"#,
        )
        .unwrap();

        let error = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::from_secs(1))
            .expect_err("expected oversized initial memory rejection");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("initial memory limit exceeded"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn wasm_rejects_declared_memory_maximum_above_limit() {
        let wasm_bytes = wat2wasm(
            r#"(module
                (memory (export "memory") 1 256)
                (func (export "alloc") (param i32) (result i32)
                    i32.const 16)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const 0))"#,
        )
        .unwrap();

        let error = test_sandbox()
            .execute_module(&wasm_bytes, &Value::Null, Duration::from_secs(1))
            .expect_err("expected oversized maximum memory rejection");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("declared memory maximum exceeds"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn independent_sandboxes_do_not_share_execution_permits() {
        let first = WasmSandbox::new(1).expect("first sandbox");
        let second = WasmSandbox::new(1).expect("second sandbox");

        let _first_permit = first.try_acquire_execution_permit().expect("first permit");
        let second_permit = second
            .try_acquire_execution_permit()
            .expect("second sandbox permit should be independent");
        drop(second_permit);
    }
}
