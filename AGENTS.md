# AGENTS.md

## Scope
- These instructions apply to the entire repository unless a deeper `AGENTS.md` overrides them.

## Project Overview
- Core Froglet implementation is in Rust under `src/`, with binary entrypoints in `src/bin/`.
- Rust integration and conformance tests live in `tests/`.
- Python client helpers, adapters, and unittests live in `python/`.
- The OpenClaw/NemoClaw plugin lives in `integrations/openclaw/froglet/`.
- Protocol and operator docs live in `docs/KERNEL.md`, `docs/`, and `conformance/`.

## Change Boundaries
- Treat `docs/KERNEL.md` as the authoritative kernel specification. Do not change canonical artifact payloads, hashing, signing bytes, state transitions, or Lightning settlement bindings without explicit interoperability justification and prior discussion.
- If you touch signing, hashing, or artifact structure, update `conformance/kernel_v1.json` and explain why in the change summary.
- Keep kernel changes small. Prefer adapters, higher layers, or integrations unless a change truly belongs in the signed protocol/runtime core.
- Public code must not depend on ignored `private/` incubations.

## Working Style
- Prefer editing existing files over adding new ones.
- Keep changes scoped to the task; avoid broad refactors unless they are required.
- Match existing Rust, Python, and JavaScript patterns already used nearby.
- Do not commit generated artifacts, local databases, or scratch outputs from `target/`, `_tmp/`, `data/`, `node.db*`, `higher_layers/`, coverage caches, or IDE folders.

## Key Paths
- `src/`: Rust library code for protocol, runtime, provider, discovery, operator, storage, and execution.
- `src/bin/`: Executable entrypoints for `froglet-runtime`, `froglet-provider`, `froglet-discovery`, and `froglet-operator`.
- `tests/`: Rust integration tests such as runtime routes, payments/discovery, and kernel conformance vectors.
- `python/`: Python SDK-style helpers plus `python/tests/`.
- `integrations/shared/froglet-lib/`: Shared HTTP client and utilities used by both OpenClaw plugin and MCP server.
- `integrations/openclaw/froglet/`: OpenClaw/NemoClaw JavaScript plugin, scripts, and Node-based tests.
- `integrations/mcp/froglet/`: MCP server for external agent hosts (Claude Code, Cursor, Windsurf, etc.).
- `scripts/strict_checks.sh`: Full validation matrix used before PRs.
- `compose*.yaml`: Local multi-service compose setups.

## Validation
- Start with the smallest relevant check for the area you changed, then expand outward.
- Rust formatting: `cargo fmt --all --check`
- Rust tests with warnings denied: `CARGO_INCREMENTAL=0 RUSTFLAGS="-D warnings" cargo test --all-targets`
- Clippy: `cargo clippy --all-targets -- -D warnings`
- Python tests: `python3 -W error -m unittest discover -s python/tests -t . -v`
- OpenClaw plugin checks: `node --check integrations/openclaw/froglet/index.js`
- Additional plugin checks: `node --check integrations/openclaw/froglet/scripts/doctor.mjs`
- Plugin tests: `node --test integrations/openclaw/froglet/test/plugin.test.js integrations/openclaw/froglet/test/config-profiles.test.mjs integrations/openclaw/froglet/test/doctor.test.mjs integrations/openclaw/froglet/test/froglet-client.test.mjs`
- MCP server check: `node --check integrations/mcp/froglet/server.js`
- MCP server tests: `node --test integrations/mcp/froglet/test/server.test.mjs`
- Full repo check: `./scripts/strict_checks.sh`

## Setup Notes
- Python dependencies: `python3 -m pip install -r python/requirements.txt`
- The Node-based integration checks expect Node.js 18+.
- Use the root `Cargo.toml` for Rust builds and tests.

## Quick Start
- Discovery: `cargo run --bin froglet-discovery`
- Provider: `cargo run --bin froglet-provider`
- Runtime: `cargo run --bin froglet-runtime`
- Operator: `cargo run --bin froglet-operator`
