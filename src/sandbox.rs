use mlua::{HookTriggers, Lua, LuaSerdeExt, Result as LuaResult, StdLib, Value as LuaValue};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    env,
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{info, warn};
use wasmtime::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

use once_cell::sync::Lazy;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

static LUA_CONCURRENCY_SEMAPHORE: Lazy<Arc<Semaphore>> = Lazy::new(|| {
    Arc::new(Semaphore::new(concurrency_limit(
        "FROGLET_LUA_CONCURRENCY_LIMIT",
        32,
    )))
});
static WASM_CONCURRENCY_SEMAPHORE: Lazy<Arc<Semaphore>> = Lazy::new(|| {
    Arc::new(Semaphore::new(concurrency_limit(
        "FROGLET_WASM_CONCURRENCY_LIMIT",
        16,
    )))
});
const LUA_MAX_INSTRUCTIONS: u32 = 50_000_000;
const LUA_HOOK_GRANULARITY: u32 = 10_000;
const LUA_MAX_HOOK_TICKS: usize = (LUA_MAX_INSTRUCTIONS / LUA_HOOK_GRANULARITY) as usize;
const WASM_FUEL_LIMIT: u64 = 50_000_000;
const WASM_MAX_MEMORY_BYTES: usize = 8 * 1024 * 1024;
const WASM_EPOCH_TICK_MILLIS: u64 = 10;

static WASM_ENGINE: Lazy<Engine> = Lazy::new(|| {
    let mut config = Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    Engine::new(&config).expect("failed to initialize Wasmtime engine")
});
static WASM_EPOCH_TICKER: Lazy<()> = Lazy::new(|| {
    let engine = WASM_ENGINE.clone();
    std::thread::Builder::new()
        .name("froglet-wasm-epoch".to_string())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(WASM_EPOCH_TICK_MILLIS));
            engine.increment_epoch();
        })
        .expect("failed to start Wasm epoch ticker");
});

pub enum ExecutionPermit {
    Lua(OwnedSemaphorePermit),
    Wasm(OwnedSemaphorePermit),
}

/// Initializes and tests the sandbox engines locally to ensure they load properly.
pub fn initialize_engine() {
    info!("Initializing WebAssembly & Lua Sandboxing Engines...");

    // Quick Lua engine self-test
    if let Ok(_) = execute_lua_script("return 1 + 1", None, Duration::from_secs(1)) {
        info!(" ✅ Lua Sandbox engine initialized successfully.");
    } else {
        warn!(" ❌ Lua Sandbox failed to initialize.");
    }

    // Touch the shared engine so the JIT and epoch ticker are initialized eagerly.
    let _ = wasm_engine();
    info!(" ✅ Wasmtime JIT compiler initialized successfully.");
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

pub fn try_acquire_lua_execution_permit() -> Result<ExecutionPermit, String> {
    LUA_CONCURRENCY_SEMAPHORE
        .clone()
        .try_acquire_owned()
        .map(ExecutionPermit::Lua)
        .map_err(|_| "Lua concurrency limit reached".to_string())
}

pub fn try_acquire_wasm_execution_permit() -> Result<ExecutionPermit, String> {
    WASM_CONCURRENCY_SEMAPHORE
        .clone()
        .try_acquire_owned()
        .map(ExecutionPermit::Wasm)
        .map_err(|_| "Wasm concurrency limit reached".to_string())
}

/// Executes an arbitrary Lua script in a highly restricted sandbox environment.
/// We intentionally omit the IO, OS, and Package standard libraries.
pub fn execute_lua_script(
    script: &str,
    input: Option<&serde_json::Value>,
    timeout: Duration,
) -> LuaResult<serde_json::Value> {
    let permit = try_acquire_lua_execution_permit()
        .map_err(|error| mlua::Error::RuntimeError(error.to_string()))?;
    execute_lua_script_with_permit(script, input, permit, timeout)
}

pub fn execute_lua_script_with_permit(
    script: &str,
    input: Option<&serde_json::Value>,
    permit: ExecutionPermit,
    timeout: Duration,
) -> LuaResult<serde_json::Value> {
    let _permit = match permit {
        ExecutionPermit::Lua(permit) => permit,
        ExecutionPermit::Wasm(_) => {
            return Err(mlua::Error::RuntimeError(
                "mismatched execution permit for Lua workload".to_string(),
            ));
        }
    };
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
    let started_at = Instant::now();
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(LUA_HOOK_GRANULARITY),
        move |_, _| {
            if started_at.elapsed() >= timeout {
                return Err(mlua::Error::RuntimeError(format!(
                    "Lua script wall-clock timeout exceeded after {}s",
                    timeout.as_secs()
                )));
            }
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
    timeout: Duration,
) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    let permit = try_acquire_wasm_execution_permit()
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
    execute_wasm_module_with_permit(wasm_bytes, permit, timeout)
}

pub fn execute_wasm_module_with_permit(
    wasm_bytes: &[u8],
    permit: ExecutionPermit,
    timeout: Duration,
) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    let _permit = match permit {
        ExecutionPermit::Wasm(permit) => permit,
        ExecutionPermit::Lua(_) => {
            return Err("mismatched execution permit for Wasm workload".into());
        }
    };
    let engine = wasm_engine();
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
    store.set_epoch_deadline(timeout_to_epoch_ticks(timeout));
    store.epoch_deadline_trap();

    let linker = Linker::new(&engine);
    let instance = linker.instantiate(&mut store, &module)?;

    // Try finding the default start function, or a function named "run".
    let run_func = instance
        .get_typed_func::<(), i32>(&mut store, "run")
        .or_else(|_| instance.get_typed_func::<(), i32>(&mut store, "main"))?;

    let outcome = run_func.call(&mut store, ()).map_err(|error| {
        let message = error.to_string();
        if is_wasm_timeout_message(&message) {
            boxed_message(format!(
                "Wasm module wall-clock timeout exceeded after {}s",
                timeout.as_secs()
            ))
        } else {
            boxed_message(message)
        }
    })?;

    Ok(outcome)
}

fn wasm_engine() -> &'static Engine {
    Lazy::force(&WASM_EPOCH_TICKER);
    &WASM_ENGINE
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

fn boxed_message(message: String) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(io::Error::other(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_wall_clock_timeout_is_reported() {
        let error = execute_lua_script("while true do end", None, Duration::ZERO)
            .expect_err("expected timeout");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("timeout"),
            "unexpected error: {error}"
        );
    }
}
