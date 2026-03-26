# Froglet Test Suite

## Quick Start

```bash
./scripts/test_suite.sh              # runs "all" (unit + integration + sast + security + conformance)
./scripts/test_suite.sh unit         # just unit tests
./scripts/test_suite.sh smoke        # compose-backed E2E (needs Docker)
./scripts/test_suite.sh --list       # show all categories
./scripts/test_suite.sh --dry-run security stress  # preview what would run
```

## Categories

### unit

Isolated tests that do not spawn binaries or require external services.

- `cargo test --lib` (Rust unit tests in `src/`)
- Python: `test_client_sdk`, `test_nostr_adapter`, `test_examples`
- Node: OpenClaw plugin tests (plugin, config-profiles, doctor, froglet-client) + MCP server tests
- **Requires:** cargo, python3, node >= 18

### integration

Tests that build and spawn Froglet binaries or exercise cross-module paths.

- `cargo test --tests` (Rust integration tests in `tests/`)
- Python: `test_protocol`, `test_runtime`, `test_discovery`, `test_jobs`, `test_payments`, `test_sandbox`
- **Requires:** cargo, python3

### sast

Static analysis and syntax checks.

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings`
- `node --check` on `index.js`, `doctor.mjs`, `server.js`
- `python3 -m py_compile froglet_client.py`
- **Requires:** cargo + rustfmt + clippy, node >= 18, python3

### security

Security-focused tests and dependency audits.

- Python: `test_security` (signatures, signing), `test_privacy` (encryption), `test_hardening` (sandboxing)
- Rust: `crypto::tests` module
- `cargo audit` (if cargo-audit installed)
- `npm audit --audit-level=high` (MCP package)
- **Requires:** cargo, python3; cargo-audit and npm are optional

### conformance

Protocol conformance against canonical test vectors.

- `cargo test --test kernel_conformance_vectors`
- Python: `test_conformance_vectors`
- Both read `conformance/kernel_v1.json`
- **Requires:** cargo, python3

### stress

Load and concurrency testing.

- Python: `test_stress` (concurrent requests)
- **Requires:** python3

### smoke

End-to-end tests against a full Docker Compose stack (discovery + provider + operator + runtime).

- Starts compose, waits for health, runs OpenClaw and MCP compose-smoke.mjs
- **Requires:** Docker, node >= 18

### agentic

Model-in-the-loop testing via OpenAI Responses API.

- `openai-responses-smoke.mjs` — sends prompts to a model, model calls Froglet tools, harness validates
- **Requires:** node >= 18, `OPENAI_API_KEY`, running Froglet operator

### pentest

Placeholder for future penetration testing scripts. Add scripts to `tests/pentest/`.

## Meta-categories

| Name | Includes |
|---|---|
| `all` | unit, integration, sast, security, conformance |
| `full` | all + stress, smoke, agentic, tor (if env), lnd-regtest (if env) |

## Environment Variables

| Variable | Category | Purpose |
|---|---|---|
| `OPENAI_API_KEY` | agentic | OpenAI API key for model-in-the-loop tests |
| `FROGLET_RUN_TOR_INTEGRATION` | full | Set to `1` to include Tor integration tests |
| `FROGLET_RUN_LND_REGTEST` | full | Set to `1` to include Lightning regtest tests |
| `NO_COLOR` | all | Disable colored output |

## Adding New Tests

- **Rust unit test:** Add `#[test]` in `src/*.rs` — picked up automatically by `unit`
- **Rust integration test:** Add a file in `tests/*.rs` — picked up automatically by `integration`
- **Python test:** Create `python/tests/test_<name>.py`, then add the module to the appropriate category in `scripts/test_suite.sh`
- **Node test:** Add `*.test.{js,mjs}` in `integrations/*/froglet/test/`, then add to the appropriate category in `scripts/test_suite.sh`
- **Penetration test:** Add scripts to `tests/pentest/` and wire them in the `run_pentest()` function

## Legacy Runner

`scripts/strict_checks.sh` is still used by CI (`.github/workflows/ci.yml`). It runs sast + all Rust tests + all Node tests + all Python tests sequentially. `test_suite.sh` is the organized superset with per-category control.
