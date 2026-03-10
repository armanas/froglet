use mlua::{HookTriggers, Lua, LuaSerdeExt, Result as LuaResult, StdLib, Value as LuaValue};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::{info, warn};
use wasmtime::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

use once_cell::sync::Lazy;
use tokio::sync::Semaphore;

static LUA_CONCURRENCY_SEMAPHORE: Lazy<Semaphore> = Lazy::new(|| Semaphore::new(32));
static WASM_CONCURRENCY_SEMAPHORE: Lazy<Semaphore> = Lazy::new(|| Semaphore::new(16));
const LUA_MAX_INSTRUCTIONS: u32 = 50_000_000;
const LUA_HOOK_GRANULARITY: u32 = 10_000;
const LUA_MAX_HOOK_TICKS: usize = (LUA_MAX_INSTRUCTIONS / LUA_HOOK_GRANULARITY) as usize;
const WASM_FUEL_LIMIT: u64 = 50_000_000;
const WASM_MAX_MEMORY_BYTES: usize = 8 * 1024 * 1024;

/// Initializes and tests the sandbox engines locally to ensure they load properly.
pub fn initialize_engine() {
    info!("Initializing WebAssembly & Lua Sandboxing Engines...");

    // Quick Lua engine self-test
    if let Ok(_) = execute_lua_script("return 1 + 1", None) {
        info!(" ✅ Lua Sandbox engine initialized successfully.");
    } else {
        warn!(" ❌ Lua Sandbox failed to initialize.");
    }

    // Wasmtime engine self-test (just instantiate the config to make sure the JIT works)
    let mut config = Config::new();
    config.consume_fuel(true);
    if let Ok(_) = Engine::new(&config) {
        info!(" ✅ Wasmtime JIT compiler initialized successfully.");
    } else {
        warn!(" ❌ Wasmtime JIT failed to initialize.");
    }
}

/// Executes an arbitrary Lua script in a highly restricted sandbox environment.
/// We intentionally omit the IO, OS, and Package standard libraries.
pub fn execute_lua_script(
    script: &str,
    input: Option<&serde_json::Value>,
) -> LuaResult<serde_json::Value> {
    let _permit = LUA_CONCURRENCY_SEMAPHORE
        .try_acquire()
        .map_err(|_| mlua::Error::RuntimeError("Lua concurrency limit reached".to_string()))?;
    // Only load the safest standard libraries
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        mlua::LuaOptions::new(),
    )?;

    if let Some(input) = input {
        lua.globals().set("input", lua.to_value(input)?)?;
    }

    // Count real VM instructions instead of source lines to make loop limits meaningful.
    let instructions = Arc::new(AtomicUsize::new(0));
    let inst_clone = Arc::clone(&instructions);
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(LUA_HOOK_GRANULARITY),
        move |_, _| {
            if inst_clone.fetch_add(1, Ordering::Relaxed) >= LUA_MAX_HOOK_TICKS {
                return Err(mlua::Error::RuntimeError(
                    "Lua script execution limit exceeded".to_string(),
                ));
            }
            Ok(())
        },
    );

    let result: LuaValue = lua.load(script).eval()?;
    lua.from_value(result)
}

/// Executes a WebAssembly boundary function natively.
/// Absolute memory segregation is intrinsically enforced by Wasmtime.
pub fn execute_wasm_module(
    wasm_bytes: &[u8],
) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    let _permit = WASM_CONCURRENCY_SEMAPHORE
        .try_acquire()
        .map_err(|_| "Wasm concurrency limit reached".to_string())?;
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config)?;
    let module = Module::new(&engine, wasm_bytes)?;

    let limits: StoreLimits = StoreLimitsBuilder::new()
        .memory_size(WASM_MAX_MEMORY_BYTES)
        .instances(1)
        .tables(1)
        .memories(1)
        .trap_on_grow_failure(true)
        .build();

    let mut store = Store::new(&engine, limits);
    store.limiter(|limits| limits);
    store.set_fuel(WASM_FUEL_LIMIT)?;

    let linker = Linker::new(&engine);
    let instance = linker.instantiate(&mut store, &module)?;

    // Try finding the default start function, or a function named "run".
    let run_func = instance
        .get_typed_func::<(), i32>(&mut store, "run")
        .or_else(|_| instance.get_typed_func::<(), i32>(&mut store, "main"))?;

    let outcome = run_func.call(&mut store, ())?;

    Ok(outcome)
}
