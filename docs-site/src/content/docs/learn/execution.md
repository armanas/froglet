---
title: Execution
description: How workloads run — WASM, Python, containers, and builtin services.
---

## What is a workload?

A workload is the computation the requester wants executed:

| Type | Description | Example |
|------|-------------|---------|
| WASM module | WebAssembly binary in a sandbox | Custom compute function |
| Python script | Inline Python code | Data processing |
| Container | OCI container image | Complex application |
| Builtin service | Handler registered on the node | Marketplace search |

<div class="learn-grid two">
  <div class="learn-card">
    <span class="learn-kicker">The invariant</span>
    <p>Every runtime still feeds the same deal flow. Execution is swappable, but the signed receipt stays the final proof surface.</p>
  </div>
  <div class="learn-card">
    <span class="learn-kicker">Why this matters</span>
    <p>Requesters can compare services by the same evidence model even when providers use different runtime stacks.</p>
  </div>
</div>

## The WASM sandbox

**WebAssembly (WASM)** is a binary instruction format designed to run in a sandboxed environment. The provider runs requester code with strict limits:

| Limit | Default | Purpose |
|-------|---------|---------|
| Fuel | 50,000,000 units | Bounds computation time |
| Memory | 8 MB | Prevents memory exhaustion |
| Output | 128 KB | Bounds response size |
| Timeout | 10 seconds | Wall-clock deadline |

WASM modules can optionally access **host capabilities**:

- `net.http.fetch` — make HTTP requests (policy-controlled)
- `db.sqlite.query.read` — query SQLite databases (read-only)

<div class="learn-sequence four">
  <div class="learn-sequence-step">
    <strong>Declare</strong>
    <small>The offer states runtime, limits, and capability profile.</small>
  </div>
  <div class="learn-sequence-step">
    <strong>Dispatch</strong>
    <small>The provider selects the matching runtime adapter for the workload.</small>
  </div>
  <div class="learn-sequence-step">
    <strong>Execute</strong>
    <small>The runtime enforces memory, output, and time bounds.</small>
  </div>
  <div class="learn-sequence-step">
    <strong>Receipt</strong>
    <small>The outcome hash and settlement state become signed evidence.</small>
  </div>
</div>

## BuiltinServiceHandler

For services that run in-process (like the marketplace), froglet provides a plugin trait:

```rust
trait BuiltinServiceHandler {
    fn execute(&self, input: JSON) -> Result<JSON, Error>;
}
```

A node registers handlers at startup in `AppState.builtin_services`. When a deal targets a builtin offer, the execution dispatch calls the handler directly — JSON in, JSON out.

The handler owns its state (database pools, caches, HTTP clients). This is how the marketplace serves search queries through the standard deal flow without sandbox overhead.

## Execution dispatch

When a deal is accepted, the provider dispatches based on the workload type:

```
match (runtime, package_kind):
  (Wasm, InlineModule)  → WasmSandbox.execute()
  (Python, InlineSource) → run_python_execution()
  (Container, OciImage)  → run_container_execution()
  (Builtin, Builtin)     → dispatch_builtin_workload()
```

All paths produce a result that is hashed and included in the signed Receipt.
