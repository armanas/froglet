use std::{
    collections::HashMap,
    env, io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::info;
use wasmtime::{
    AsContext, AsContextMut, Caller, Config, Engine, ExternType, Linker, Memory, MemoryType,
    Module, Store, StoreLimits, StoreLimitsBuilder, Trap, ValType,
};

use crate::{
    canonical_json, wasm, wasm_host::WasmExecutionContext, wasm_host::WasmHostEnvironment,
};

pub const WASM_FUEL_LIMIT: u64 = 50_000_000;
pub const WASM_MAX_MEMORY_BYTES: usize = 8 * 1024 * 1024;
pub const WASM_MAX_OUTPUT_BYTES: usize = 128 * 1024;
const WASM_EPOCH_TICK_MILLIS: u64 = 10;
const WASM_PAGE_BYTES: u64 = 64 * 1024;
const WASM_MAX_MEMORY_PAGES: u64 = (WASM_MAX_MEMORY_BYTES as u64) / WASM_PAGE_BYTES;
const WASM_MODULE_CACHE_DEFAULT_CAPACITY: usize = 128;
const WASM_HOST_IMPORT_MODULE: &str = "froglet_host";
const WASM_HOST_IMPORT_CALL_JSON: &str = "call_json";

pub struct ExecutionPermit(OwnedSemaphorePermit);

#[derive(Clone)]
pub struct WasmExecutionOptions {
    pub abi_version: String,
    pub capabilities_granted: Vec<String>,
    pub host_environment: Option<Arc<WasmHostEnvironment>>,
}

impl WasmExecutionOptions {
    pub fn pure_compute() -> Self {
        Self {
            abi_version: wasm::WASM_RUN_JSON_ABI_V1.to_string(),
            capabilities_granted: Vec::new(),
            host_environment: None,
        }
    }
}

struct WasmStoreData {
    limits: StoreLimits,
    host_context: Option<WasmExecutionContext>,
}

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
    module_cache: Mutex<ModuleCache>,
    _epoch_ticker: EpochTicker,
}

impl WasmSandbox {
    pub fn from_env() -> Result<Self, String> {
        Self::new_with_module_cache_capacity(wasm_concurrency_limit(), wasm_module_cache_capacity())
    }

    pub fn new(concurrency_limit: usize) -> Result<Self, String> {
        Self::new_with_module_cache_capacity(concurrency_limit, wasm_module_cache_capacity())
    }

    fn new_with_module_cache_capacity(
        concurrency_limit: usize,
        module_cache_capacity: usize,
    ) -> Result<Self, String> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)
            .map_err(|error| format!("failed to initialize Wasmtime engine: {error}"))?;
        let epoch_ticker = EpochTicker::start(engine.clone())?;

        Ok(Self {
            engine,
            concurrency_semaphore: Arc::new(Semaphore::new(concurrency_limit.max(1))),
            module_cache: Mutex::new(ModuleCache::new(module_cache_capacity)),
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
        self.execute_module_with_options_and_permit(
            wasm_bytes,
            input,
            WasmExecutionOptions::pure_compute(),
            permit,
            timeout,
        )
    }

    pub fn execute_module_with_options(
        &self,
        wasm_bytes: &[u8],
        input: &Value,
        options: WasmExecutionOptions,
        timeout: Duration,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let permit = self
            .try_acquire_execution_permit()
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
        self.execute_module_with_options_and_permit(wasm_bytes, input, options, permit, timeout)
    }

    pub fn execute_module_with_options_and_permit(
        &self,
        wasm_bytes: &[u8],
        input: &Value,
        options: WasmExecutionOptions,
        permit: ExecutionPermit,
        timeout: Duration,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let _permit = permit.0;
        let module = self.load_module(wasm_bytes, &options.abi_version)?;

        let limits: StoreLimits = StoreLimitsBuilder::new()
            .memory_size(WASM_MAX_MEMORY_BYTES)
            .instances(1)
            .tables(1)
            .memories(1)
            .trap_on_grow_failure(true)
            .build();
        let execution_deadline = Instant::now().checked_add(timeout);
        let host_context = options.host_environment.map(|environment| {
            WasmExecutionContext::new(
                environment,
                options.capabilities_granted,
                execution_deadline,
            )
        });
        let mut store = Store::new(
            &self.engine,
            WasmStoreData {
                limits,
                host_context,
            },
        );
        store.limiter(|data| &mut data.limits);
        store.set_fuel(WASM_FUEL_LIMIT)?;
        store.set_epoch_deadline(timeout_to_epoch_ticks(timeout));
        store.epoch_deadline_trap();

        let mut linker = Linker::new(&self.engine);
        if options.abi_version == wasm::WASM_HOST_JSON_ABI_V1 {
            linker
                .func_wrap(
                    WASM_HOST_IMPORT_MODULE,
                    WASM_HOST_IMPORT_CALL_JSON,
                    |mut caller: Caller<'_, WasmStoreData>,
                     ptr: i32,
                     len: i32|
                     -> Result<i64, wasmtime::Error> {
                        host_call_json(&mut caller, ptr, len)
                            .map_err(|error| wasmtime::Error::msg(error.to_string()))
                    },
                )
                .map_err(|error| boxed_message(error.to_string()))?;
        }
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

    #[cfg(test)]
    fn cached_module_count(&self) -> usize {
        self.with_module_cache(|cache| cache.entry_count())
    }

    fn load_module(
        &self,
        wasm_bytes: &[u8],
        abi_version: &str,
    ) -> Result<Module, Box<dyn std::error::Error + Send + Sync>> {
        let cache_key = ModuleCacheKey::new(wasm_bytes, abi_version);
        if let Some(module) = self.with_module_cache(|cache| cache.get(&cache_key)) {
            return Ok(module);
        }

        let module = Module::new(&self.engine, wasm_bytes)?;
        validate_module_policy(&module, abi_version)?;
        self.with_module_cache(|cache| cache.insert(cache_key, module.clone()));
        Ok(module)
    }

    fn with_module_cache<T>(&self, operation: impl FnOnce(&mut ModuleCache) -> T) -> T {
        let mut cache = self
            .module_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        operation(&mut cache)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ModuleCacheKey {
    module_hash: [u8; 32],
    abi_version: String,
}

impl ModuleCacheKey {
    fn new(wasm_bytes: &[u8], abi_version: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(wasm_bytes);
        let digest = hasher.finalize();
        let mut module_hash = [0u8; 32];
        module_hash.copy_from_slice(&digest);
        Self {
            module_hash,
            abi_version: abi_version.to_string(),
        }
    }
}

struct ModuleCache {
    capacity: usize,
    entries: HashMap<ModuleCacheKey, CachedModuleEntry>,
    next_lru_tick: u64,
}

struct CachedModuleEntry {
    module: Module,
    last_used_tick: u64,
}

impl ModuleCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            next_lru_tick: 0,
        }
    }

    fn get(&mut self, key: &ModuleCacheKey) -> Option<Module> {
        let tick = self.next_tick();
        let entry = self.entries.get_mut(key)?;
        entry.last_used_tick = tick;
        Some(entry.module.clone())
    }

    fn insert(&mut self, key: ModuleCacheKey, module: Module) {
        if self.capacity == 0 {
            return;
        }

        let tick = self.next_tick();
        self.entries.insert(
            key,
            CachedModuleEntry {
                module,
                last_used_tick: tick,
            },
        );

        while self.entries.len() > self.capacity {
            let Some(evicted_key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used_tick)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&evicted_key);
        }
    }

    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    fn next_tick(&mut self) -> u64 {
        let tick = self.next_lru_tick;
        self.next_lru_tick = self.next_lru_tick.wrapping_add(1);
        tick
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

pub fn validate_module_bytes_for_abi(module_bytes: &[u8], abi_version: &str) -> Result<(), String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    let engine = Engine::new(&config)
        .map_err(|error| format!("failed to initialize Wasmtime engine: {error}"))?;
    let module = Module::new(&engine, module_bytes)
        .map_err(|error| format!("invalid Wasm module: {error}"))?;
    validate_module_policy(&module, abi_version).map_err(|error| error.to_string())
}

pub fn wasm_concurrency_limit() -> usize {
    concurrency_limit("FROGLET_WASM_CONCURRENCY_LIMIT", 16)
}

fn wasm_module_cache_capacity() -> usize {
    concurrency_limit(
        "FROGLET_WASM_MODULE_CACHE_CAPACITY",
        WASM_MODULE_CACHE_DEFAULT_CAPACITY,
    )
}

fn validate_module_policy(
    module: &Module,
    abi_version: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    match abi_version {
        wasm::WASM_RUN_JSON_ABI_V1 if !imports.is_empty() => {
            return Err(boxed_message(format!(
                "Wasm host imports are not permitted in froglet.wasm.run_json.v1: {}",
                imports.join(", ")
            )));
        }
        wasm::WASM_RUN_JSON_ABI_V1 => {}
        wasm::WASM_HOST_JSON_ABI_V1 => validate_host_imports(module)?,
        _ => {
            return Err(boxed_message(format!(
                "unsupported Wasm ABI for sandbox execution: {abi_version}"
            )));
        }
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

fn validate_host_imports(module: &Module) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut import_count = 0;
    for import in module.imports() {
        import_count += 1;
        if import.module() != WASM_HOST_IMPORT_MODULE || import.name() != WASM_HOST_IMPORT_CALL_JSON
        {
            return Err(boxed_message(format!(
                "unsupported Wasm host import for {}: {}::{}",
                wasm::WASM_HOST_JSON_ABI_V1,
                import.module(),
                import.name()
            )));
        }

        let import_type = import.ty();
        let Some(func_type) = import_type.func() else {
            return Err(boxed_message(format!(
                "{}::{} must be a function import",
                WASM_HOST_IMPORT_MODULE, WASM_HOST_IMPORT_CALL_JSON
            )));
        };
        let params = func_type.params().collect::<Vec<_>>();
        let results = func_type.results().collect::<Vec<_>>();
        let params_ok = params.len() == 2
            && matches!(params[0], ValType::I32)
            && matches!(params[1], ValType::I32);
        let results_ok = results.len() == 1 && matches!(results[0], ValType::I64);
        if !params_ok || !results_ok {
            return Err(boxed_message(format!(
                "{}::{} must have signature (i32, i32) -> i64",
                WASM_HOST_IMPORT_MODULE, WASM_HOST_IMPORT_CALL_JSON
            )));
        }
    }

    if import_count > 1 {
        return Err(boxed_message(format!(
            "{} allows at most one host import",
            wasm::WASM_HOST_JSON_ABI_V1
        )));
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

fn host_call_json(
    caller: &mut Caller<'_, WasmStoreData>,
    ptr: i32,
    len: i32,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    if ptr < 0 || len < 0 {
        return Err(boxed_message(
            "froglet_host.call_json received a negative pointer or length".to_string(),
        ));
    }

    let memory = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .ok_or_else(|| boxed_message("Wasm module must export memory".to_string()))?;
    let request_bytes = read_memory(&memory, caller.as_context(), ptr as usize, len as usize)?;
    // Safety: `memory` is a wasmtime::Memory handle (a stable index into the store's memory
    // table), not a direct slice into linear memory. The `caller_alloc` call below re-enters
    // WASM and may trigger `memory.grow`, which reallocates the backing buffer. This is safe
    // because we re-borrow the data slice via `write_memory` after the alloc returns. The
    // current ABI allows only a single host import (`call_json`), so nested re-entrancy into
    // this function cannot occur. If additional host imports are added in the future, ensure
    // they do not allow re-entrancy that could invalidate memory assumptions.
    let response_bytes = caller
        .data_mut()
        .host_context
        .as_mut()
        .ok_or_else(|| boxed_message("host context is not available".to_string()))?
        .dispatch_json(&request_bytes)
        .map_err(boxed_message)?;
    let response_len_i32 = i32::try_from(response_bytes.len())
        .map_err(|_| boxed_message("host response is too large".to_string()))?;
    let response_ptr = caller_alloc(caller, response_len_i32)?;
    if response_ptr < 0 {
        return Err(boxed_message(
            "Wasm alloc returned a negative pointer for host response".to_string(),
        ));
    }
    write_memory(
        &memory,
        caller.as_context_mut(),
        response_ptr as usize,
        &response_bytes,
    )?;
    Ok(pack_ptr_len(response_ptr, response_len_i32))
}

fn caller_alloc(
    caller: &mut Caller<'_, WasmStoreData>,
    len: i32,
) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    let alloc = caller
        .get_export("alloc")
        .and_then(|export| export.into_func())
        .ok_or_else(|| boxed_message("Wasm module must export alloc".to_string()))?;
    let alloc = alloc
        .typed::<i32, i32>(caller.as_context())
        .map_err(|error| boxed_message(error.to_string()))?;
    alloc
        .call(caller.as_context_mut(), len)
        .map_err(|error| boxed_message(error.to_string()))
}

fn pack_ptr_len(ptr: i32, len: i32) -> i64 {
    ((ptr as u32 as u64) << 32 | (len as u32 as u64)) as i64
}

fn write_memory<T: 'static>(
    memory: &Memory,
    mut store: impl wasmtime::AsContextMut<Data = T>,
    ptr: usize,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let data = memory.data_mut(store.as_context_mut());
    let end = ptr
        .checked_add(bytes.len())
        .ok_or_else(|| boxed_message("Wasm memory write overflow".to_string()))?;
    let target = data
        .get_mut(ptr..end)
        .ok_or_else(|| boxed_message("Wasm alloc returned out-of-bounds pointer".to_string()))?;
    target.copy_from_slice(bytes);
    Ok(())
}

fn read_memory<T: 'static>(
    memory: &Memory,
    store: impl wasmtime::AsContext<Data = T>,
    ptr: usize,
    len: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let data = memory.data(store.as_context());
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
    use crate::{
        config::{WasmPolicy, WasmSqliteHandleConfig, WasmSqlitePolicy},
        wasm_host::WasmHostEnvironment,
    };
    use rusqlite::Connection;
    use serde_json::Value;
    use std::{
        collections::BTreeMap,
        time::{SystemTime, UNIX_EPOCH},
    };
    use wat::parse_str as wat2wasm;

    const VALID_WASM_HEX: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432";
    const INFINITE_WASM_HEX: &str = "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0f02040041100b080003400c000b000b";

    fn test_sandbox() -> WasmSandbox {
        WasmSandbox::new_with_module_cache_capacity(16, 16).expect("sandbox")
    }

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "froglet-sandbox-tests-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
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
    fn wasm_host_abi_can_query_sqlite_via_host_import() {
        let temp_dir = unique_temp_dir("host-db");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("workload.db");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute("CREATE TABLE sample (value INTEGER)", [])
            .unwrap();
        connection
            .execute("INSERT INTO sample (value) VALUES (42)", [])
            .unwrap();
        drop(connection);

        let request_json = r#"{"op":"db.query","request":{"handle":"main","sql":"SELECT value FROM sample","params":[]}}"#;
        let wasm_bytes = wat2wasm(format!(
            r#"(module
                (import "froglet_host" "call_json" (func $call_json (param i32 i32) (result i64)))
                (memory (export "memory") 1)
                (global $heap (mut i32) (i32.const 256))
                (data (i32.const 0) "{request}")
                (func (export "alloc") (param $len i32) (result i32)
                    (local $ptr i32)
                    global.get $heap
                    local.set $ptr
                    global.get $heap
                    local.get $len
                    i32.add
                    global.set $heap
                    local.get $ptr)
                (func (export "run") (param i32 i32) (result i64)
                    i32.const 0
                    i32.const {request_len}
                    call $call_json))"#,
            request = request_json.replace('\\', "\\\\").replace('"', "\\\""),
            request_len = request_json.len(),
        ))
        .unwrap();

        let host_environment = Arc::new(
            WasmHostEnvironment::from_policy(WasmPolicy {
                http: None,
                sqlite: Some(WasmSqlitePolicy {
                    max_queries_per_execution: 4,
                    max_rows_per_query: 10,
                    max_result_bytes: 4_096,
                    handles: BTreeMap::from([(
                        "main".to_string(),
                        WasmSqliteHandleConfig {
                            path: db_path.clone(),
                        },
                    )]),
                }),
            })
            .unwrap(),
        );

        let result = test_sandbox()
            .execute_module_with_options(
                &wasm_bytes,
                &Value::Null,
                WasmExecutionOptions {
                    abi_version: wasm::WASM_HOST_JSON_ABI_V1.to_string(),
                    capabilities_granted: vec![
                        wasm::WASM_CAPABILITY_SQLITE_QUERY_READ_PREFIX.to_string() + "main",
                    ],
                    host_environment: Some(host_environment),
                },
                Duration::from_secs(1),
            )
            .expect("host abi execution should succeed");

        assert_eq!(
            result,
            serde_json::json!({
                "columns": ["value"],
                "rows": [[42]],
            })
        );

        let _ = std::fs::remove_dir_all(temp_dir);
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

    #[test]
    fn wasm_module_cache_is_bounded() {
        let sandbox = WasmSandbox::new_with_module_cache_capacity(2, 1).expect("sandbox");
        let module_a = wat2wasm(
            r#"(module
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32)
                    i32.const 16)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const 2)
                (data (i32.const 0) "42"))"#,
        )
        .unwrap();
        let module_b = wat2wasm(
            r#"(module
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32)
                    i32.const 16)
                (func (export "run") (param i32 i32) (result i64)
                    i64.const 2)
                (data (i32.const 0) "43"))"#,
        )
        .unwrap();

        let _ = sandbox.execute_module(&module_a, &Value::Null, Duration::from_secs(1));
        assert_eq!(sandbox.cached_module_count(), 1);

        let _ = sandbox.execute_module(&module_b, &Value::Null, Duration::from_secs(1));
        assert_eq!(sandbox.cached_module_count(), 1);

        let _ = sandbox.execute_module(&module_b, &Value::Null, Duration::from_secs(1));
        assert_eq!(sandbox.cached_module_count(), 1);
    }
}
