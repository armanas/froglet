# Froglet Test Suite

## Quick Start

```bash
./scripts/test_suite.sh              # runs "all" (unit + integration + sast + security + conformance)
./scripts/test_suite.sh unit         # just unit tests
./scripts/test_suite.sh smoke        # compose-backed E2E (needs Docker)
./scripts/test_suite.sh --list       # show all categories
./scripts/test_suite.sh --dry-run security stress  # preview what would run
```

## Core Categories

### unit

Isolated tests that do not spawn binaries or require external services.

- `cargo test --lib` (Rust unit tests in `src/`)
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
- **Requires:** node >= 18, `OPENCLAW_API_KEY` or `OPENAI_API_KEY`, running Froglet operator

## Extended Categories

### performance

API latency benchmarking — measures p50/p95/p99 latency and throughput for all key endpoints.

- Python: `test_bench_api` (health, publish, query benchmarks)
- **Requires:** python3
- **Tuning:** `FROGLET_PERF_REQUESTS` (default 500), `FROGLET_PERF_CONCURRENCY` (default 40)

### spike

Sudden traffic surge testing — ramps to high concurrency instantly to test burst handling.

- Python: `test_spike` (spike from idle, repeated spikes)
- **Requires:** python3

### soak

Endurance / stability testing — sustained moderate load over configurable duration.

- Python: `test_soak` (monitors latency degradation, error rate, memory growth)
- **Requires:** python3
- **Tuning:** `FROGLET_SOAK_DURATION_MINUTES` (default 5), `FROGLET_SOAK_CONCURRENCY` (default 10)

### fuzz

HTTP API fuzzing — sends malformed, oversized, and injection payloads to all endpoints.

- Python: `test_fuzz_api` (nested JSON, SQL injection, path traversal, binary garbage, etc.)
- **Requires:** python3
- **Verifies:** Server never crashes, always returns valid HTTP responses

### blackbox

Black box API testing — tests the public API with zero knowledge of internals.

- Python: `test_blackbox` (full lifecycle, concurrent ops, error handling, WASM compute)
- **Requires:** python3

### graybox

Combines black box tests with selected white-box security tests.

- Python: `test_blackbox` + `test_security`
- **Requires:** python3

### acceptance

User acceptance testing (UAT) — business-level scenario validation.

- Python: `test_acceptance` (discovery, free compute SLA, priced quote→deal flow, resource limits, data integrity)
- **Requires:** python3
- **Note:** UAT-3 tests the actual 409 → quote → deal payment protocol, not a simplified invoke

### pentest

Automated penetration testing — security exploit attempts.

- Python: `test_pentest` (auth bypass, SQL/command injection, replay attacks, signature manipulation, resource exhaustion, header security)
- **Requires:** python3

### chaos

Docker failure injection — kills services, partitions networks, rapid restarts.

- Bash: `tests/chaos/chaos_runner.sh` (kill_provider, kill_runtime, kill_discovery, restart_all, network_partition, rapid_restarts)
- **Requires:** Docker with running compose stack

### exploratory

AI-driven exploratory testing — uses an LLM to creatively explore the API.

- Node: `tests/e2e/agentic_exploratory.mjs` (30-step exploration with anomaly detection)
- **Requires:** node >= 18, `OPENCLAW_API_KEY` or `OPENAI_API_KEY`

### mutation

Mutation testing — introduces small code changes to verify test quality.

- `cargo mutants` (requires cargo-mutants installed)
- **Requires:** cargo, cargo-mutants

### vulnscan

Dependency vulnerability scanning.

- `cargo audit` + `npm audit`
- **Requires:** cargo-audit and/or npm

### sanity

Quick health check — minimal subset for fast verification.

- Rust: `crypto::tests`
- Python: `test_conformance_vectors`
- **Requires:** cargo, python3

### canary

Acceptance tests on a partial deploy (provider + discovery only, no operator/runtime).

- **Requires:** Docker, python3

## Meta-categories

| Name | Includes |
|---|---|
| `all` | unit, integration, sast, security, conformance |
| `full` | all + stress, smoke, agentic, tor (if env), lnd-regtest (if env) |
| `regression` | all + blackbox, acceptance |
| `e2e` | smoke + blackbox + acceptance |
| `gcp_rig` | Provision GCP VM → deploy stack → run extended tests on-VM |

## GCP Test Rig

The `gcp_rig` category provisions an ephemeral GCP Compute Engine VM, deploys the full Froglet compose stack, runs tests ON the VM (so compose services are on loopback), and destroys the VM on exit.

```bash
FROGLET_GCP_PROJECT=my-project ./scripts/test_suite.sh gcp_rig
FROGLET_GCP_PROJECT=my-project GCP_RIG_CATEGORIES="fuzz pentest" ./scripts/test_suite.sh gcp_rig
```

**Required:** `FROGLET_GCP_PROJECT`, `gcloud` CLI configured.
**Optional:** `FROGLET_GCP_ZONE` (default us-central1-a), `FROGLET_GCP_MACHINE_TYPE` (default e2-standard-4), `GCP_RIG_CATEGORIES` (categories to run on VM, default: performance spike fuzz blackbox acceptance pentest).

## Multi-Node GCP Harness

For end-to-end OpenClaw + Froglet marketplace coverage on GCP, use `scripts/gcp_harness.sh`.
Here `froglet-marketplace` is only a harness role name for the requester/OpenClaw
node; it is not a separate tracked product tree in this repo.

```bash
FROGLET_GCP_PROJECT=bcr1-488220 ./scripts/gcp_harness.sh provision
FROGLET_GCP_PROJECT=bcr1-488220 ./scripts/gcp_harness.sh deploy
FROGLET_GCP_PROJECT=bcr1-488220 ./scripts/gcp_harness.sh seed
FROGLET_GCP_PROJECT=bcr1-488220 ./scripts/gcp_harness.sh run-matrix
FROGLET_GCP_PROJECT=bcr1-488220 ./scripts/gcp_harness.sh run-agentic
FROGLET_GCP_PROJECT=bcr1-488220 ./scripts/gcp_harness.sh collect
FROGLET_GCP_PROJECT=bcr1-488220 ./scripts/gcp_harness.sh destroy
```

The harness provisions five fixed roles on the existing `froglet-harness` VPC:

- `froglet-marketplace` — marketplace requester node, OpenClaw host, local operator/runtime/provider
- `froglet-discovery` — reference discovery
- `froglet-provider-free` — free named/data/project-backed services
- `froglet-provider-paid` — priced, async, and OCI-backed workloads
- `froglet-settlement-lab` — dedicated real-LND regtest runner

Artifacts are written under `_tmp/gcp-harness/<run-id>/`:

- `inventory.json` — node metadata, URLs, token paths, and discovery URL
- `scenario.json` — deterministic tool/protocol/agentic scenarios and oracles
- `results/` — tool matrix, protocol matrix, LND regtest, and agentic outputs
- `collected/` — copied node logs and remote artifacts after `collect`
- `SUMMARY.md` — run summary assembled from the JSON results

Implementation notes:

- Main Froglet binaries run under systemd on the nodes; auxiliary OCI fixtures are seeded separately on the paid provider.
- Provider and discovery publication edges are exposed through per-node TLS reverse proxies on `:443`, with a harness CA distributed through `FROGLET_HTTP_CA_CERT_PATH`. Runtime and operator listeners remain loopback-only.
- `run-matrix` opens a local SSH tunnel to the marketplace operator and executes the OpenClaw tool matrix locally against the real marketplace node, then runs direct protocol checks and the settlement-lab LND regtest.
- `run-agentic` fetches the OpenAI key from Secret Manager using local `gcloud` credentials and injects it only into the remote marketplace process as `OPENCLAW_API_KEY`. Do not hardcode API keys in repo files.

## Environment Variables

| Variable | Category | Purpose |
|---|---|---|
| `OPENCLAW_API_KEY` | agentic, exploratory | API key for model-in-the-loop tests (preferred) |
| `OPENAI_API_KEY` | agentic, exploratory | Fallback API key |
| `FROGLET_GCP_PROJECT` | gcp_rig | GCP project ID |
| `FROGLET_GCP_ZONE` | gcp_rig | GCP zone (default us-central1-a) |
| `FROGLET_GCP_MACHINE_TYPE` | gcp_rig | VM machine type (default e2-standard-4) |
| `GCP_RIG_CATEGORIES` | gcp_rig | Categories to run on VM |
| `FROGLET_GCP_OPENAI_SECRET` | gcp_harness | Secret Manager secret name used by `run-agentic` (default `openclaw-api-key`) |
| `FROGLET_GCP_HARNESS_STATE_DIR` | gcp_harness | Override the local harness state directory |
| `FROGLET_GCP_HARNESS_RUN_ID` | gcp_harness | Override the generated harness run id |
| `FROGLET_PERF_REQUESTS` | performance | Requests per endpoint (default 500) |
| `FROGLET_PERF_CONCURRENCY` | performance | Max parallel requests (default 40) |
| `FROGLET_SOAK_DURATION_MINUTES` | soak | Endurance test duration (default 5) |
| `FROGLET_SOAK_CONCURRENCY` | soak | Sustained load workers (default 10) |
| `FROGLET_RUN_TOR_INTEGRATION` | full | Set to `1` to include Tor integration tests |
| `FROGLET_RUN_LND_REGTEST` | full | Set to `1` to include Lightning regtest tests |
| `NO_COLOR` | all | Disable colored output |

## Testing Category Coverage Matrix

| Category | Type | Status |
|---|---|---|
| Unit Testing | White Box | `unit` |
| Integration Testing | White Box | `integration` |
| System Testing | E2E | `gcp_rig` |
| Acceptance Testing (UAT) | Business | `acceptance` |
| Regression Testing | Aggregate | `regression` |
| Smoke Testing | E2E | `smoke` |
| Functional Testing | Conformance | `conformance` |
| Non-Functional Testing | Performance | `performance` + `soak` |
| Load Testing | Performance | `stress` |
| Stress Testing | Performance | `spike` |
| Spike Testing | Performance | `spike` |
| Soak/Endurance Testing | Stability | `soak` |
| Performance Testing | Benchmarking | `performance` |
| Penetration Testing | Security | `pentest` |
| White Box Testing | Structural | `unit` + `integration` |
| Black Box Testing | Behavioral | `blackbox` |
| Gray Box Testing | Hybrid | `graybox` |
| Security Testing | Security | `security` |
| Vulnerability Scanning | Security | `vulnscan` |
| Fuzz Testing | Robustness | `fuzz` |
| Chaos Testing | Resilience | `chaos` |
| Exploratory Testing | AI-driven | `exploratory` |
| Mutation Testing | Quality | `mutation` |
| Contract Testing | Protocol | `conformance` |
| Canary Testing | Deployment | `canary` |
| Sanity Testing | Quick Check | `sanity` |
| Static Testing | Analysis | `sast` |
| Dynamic Testing | Runtime | All runtime categories |
| End-to-End Testing | Workflow | `e2e` |

## Adding New Tests

- **Rust unit test:** Add `#[test]` in `src/*.rs` — picked up automatically by `unit`
- **Rust integration test:** Add a file in `tests/*.rs` — picked up automatically by `integration`
- **Core Python-backed test:** Create `python/tests/test_<name>.py` only for node/runtime/security/conformance coverage that remains part of the core repo, then add the module to the appropriate category in `scripts/test_suite.sh`
- **Node test:** Add `*.test.{js,mjs}` in `integrations/*/froglet/test/`, then add to the appropriate category in `scripts/test_suite.sh`
- **Chaos scenario:** Add a function in `tests/chaos/chaos_runner.sh` and register it in `ALL_SCENARIOS`
- **Penetration test:** Add test class in `python/tests/test_pentest.py`

## Legacy Runner

`scripts/strict_checks.sh` is still used by CI (`.github/workflows/ci.yml`). It runs sast + all Rust tests + all Node tests + the core Python-backed runtime/security/conformance modules sequentially. `test_suite.sh` is the organized superset with per-category control.
