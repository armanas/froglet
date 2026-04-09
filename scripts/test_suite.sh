#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# ---------------------------------------------------------------------------
# Color helpers (disabled when NO_COLOR is set or stdout is not a tty)
# ---------------------------------------------------------------------------
if [[ -z "${NO_COLOR:-}" ]] && [[ -t 1 ]]; then
  RED='\033[0;31m'
  GREEN='\033[0;32m'
  YELLOW='\033[0;33m'
  BLUE='\033[0;34m'
  BOLD='\033[1m'
  RESET='\033[0m'
else
  RED='' GREEN='' YELLOW='' BLUE='' BOLD='' RESET=''
fi

DRY_RUN=0
CATEGORIES=()
FAILURES=()
COMPOSE_TEST_ACTIVE=0
COMPOSE_TEST_DATA_ROOT=""
COMPOSE_TEST_PROJECT_NAME=""

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
banner() {
  echo ""
  echo -e "${BLUE}${BOLD}=== $1 ===${RESET}"
}

skip_warn() {
  echo -e "  ${YELLOW}[skip]${RESET} $1"
}

step() {
  if [[ "$DRY_RUN" == "1" ]]; then
    echo -e "  ${BOLD}[dry-run]${RESET} $*"
    return 0
  fi
  echo -e "  ${BOLD}[run]${RESET} $*"
  "$@"
}

compose_test_env() {
  env \
    COMPOSE_PROJECT_NAME="$COMPOSE_TEST_PROJECT_NAME" \
    FROGLET_DATA_ROOT="$COMPOSE_TEST_DATA_ROOT" \
    FROGLET_TEST_DATA_ROOT="$COMPOSE_TEST_DATA_ROOT" \
    FROGLET_AUTH_TOKEN_PATH="$COMPOSE_TEST_DATA_ROOT/runtime/froglet-control.token" \
    FROGLET_BASE_URL="${FROGLET_BASE_URL:-http://127.0.0.1:8080}" \
    FROGLET_PROVIDER_URL="${FROGLET_PROVIDER_URL:-http://127.0.0.1:8080}" \
    FROGLET_RUNTIME_URL="${FROGLET_RUNTIME_URL:-http://127.0.0.1:8081}" \
    FROGLET_PROVIDER_AUTH_TOKEN_PATH="${FROGLET_PROVIDER_AUTH_TOKEN_PATH:-$COMPOSE_TEST_DATA_ROOT/runtime/froglet-control.token}" \
    FROGLET_RUNTIME_AUTH_TOKEN_PATH="${FROGLET_RUNTIME_AUTH_TOKEN_PATH:-$COMPOSE_TEST_DATA_ROOT/runtime/auth.token}" \
    FROGLET_TEST_RESULTS_DIR="${FROGLET_TEST_RESULTS_DIR:-$repo_root/_tmp/test-results}" \
    "$@"
}

compose_test_setup() {
  local label="$1"
  if [[ "$COMPOSE_TEST_ACTIVE" == "1" ]]; then
    compose_test_finish 0 >/dev/null || true
  fi
  local safe_label="${label//[^a-zA-Z0-9]/-}"
  COMPOSE_TEST_DATA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/froglet-${safe_label}-XXXXXX")"
  COMPOSE_TEST_PROJECT_NAME="froglet-${safe_label}-$$-$(date +%s)"
  COMPOSE_TEST_ACTIVE=1
}

compose_test_finish() {
  local rc="$1"
  if [[ "$COMPOSE_TEST_ACTIVE" == "1" ]]; then
    if [[ "$rc" -ne 0 ]]; then
      compose_test_env docker compose ps || true
      compose_test_env docker compose logs --no-color || true
    fi
    compose_test_env docker compose down --remove-orphans -v 2>/dev/null || true
    rm -rf "$COMPOSE_TEST_DATA_ROOT"
    COMPOSE_TEST_ACTIVE=0
    COMPOSE_TEST_DATA_ROOT=""
    COMPOSE_TEST_PROJECT_NAME=""
  fi
  return "$rc"
}

# ---------------------------------------------------------------------------
# Tool availability
# ---------------------------------------------------------------------------
has_cargo() { command -v cargo >/dev/null 2>&1; }
has_python() { command -v python3 >/dev/null 2>&1; }
has_docker() { command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; }
has_cargo_audit() { command -v cargo-audit >/dev/null 2>&1 || cargo audit --version >/dev/null 2>&1; }
has_npm() { command -v npm >/dev/null 2>&1; }

has_node() {
  if ! command -v node >/dev/null 2>&1; then
    return 1
  fi
  local node_major
  node_major=$(node -e 'process.stdout.write(String(process.versions.node.split(".")[0]))')
  [[ "$node_major" -ge 18 ]] 2>/dev/null
}

ensure_mcp_deps() {
  local marker="integrations/mcp/froglet/node_modules/@modelcontextprotocol/sdk/package.json"
  if [[ -f "$marker" ]]; then return; fi
  if ! has_npm; then
    skip_warn "MCP tests require npm to install dependencies"
    return 1
  fi
  step npm ci --prefix integrations/mcp/froglet
}

# ---------------------------------------------------------------------------
# Category: unit
# ---------------------------------------------------------------------------
run_unit() {
  banner "unit"
  local rc=0

  if has_cargo; then
    step cargo test --lib || rc=1
  else
    skip_warn "cargo not found"
  fi

  if has_python; then
    step python3 -W error -m unittest \
      python.tests.test_conformance_vectors -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  if has_node; then
    step node --test \
      integrations/openclaw/froglet/test/plugin.test.js \
      integrations/openclaw/froglet/test/config-profiles.test.mjs \
      integrations/openclaw/froglet/test/doctor.test.mjs \
      integrations/openclaw/froglet/test/froglet-client.test.mjs || rc=1

    if ensure_mcp_deps; then
      step node --test integrations/mcp/froglet/test/server.test.mjs || rc=1
    fi
  else
    skip_warn "node >= 18 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: integration
# ---------------------------------------------------------------------------
run_integration() {
  banner "integration"
  local rc=0

  if has_cargo; then
    step env CARGO_INCREMENTAL=0 RUSTFLAGS="${RUSTFLAGS:-} -D warnings" \
      cargo test --tests || rc=1
  else
    skip_warn "cargo not found"
  fi

  if has_python; then
    step python3 -W error -m unittest \
      python.tests.test_protocol \
      python.tests.test_runtime \
      python.tests.test_jobs \
      python.tests.test_payments \
      python.tests.test_sandbox -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: sast (static analysis)
# ---------------------------------------------------------------------------
run_sast() {
  banner "sast"
  local rc=0

  if has_cargo; then
    step cargo fmt --all --check || rc=1

    if cargo clippy --version >/dev/null 2>&1; then
      step cargo clippy --all-targets -- -D warnings || rc=1
    else
      skip_warn "clippy not installed"
    fi
  else
    skip_warn "cargo not found"
  fi

  if has_node; then
    step node --check integrations/openclaw/froglet/index.js || rc=1
    step node --check integrations/openclaw/froglet/scripts/doctor.mjs || rc=1
    step node --check integrations/mcp/froglet/server.js || rc=1
  else
    skip_warn "node >= 18 not found"
  fi

  if has_python; then
    skip_warn "no core Python SDK source file is compiled in sast; runtime checks cover Python-backed services"
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: security
# ---------------------------------------------------------------------------
run_security() {
  banner "security"
  local rc=0

  if has_python; then
    step python3 -W error -m unittest \
      python.tests.test_security \
      python.tests.test_privacy \
      python.tests.test_hardening -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  if has_cargo; then
    step cargo test crypto::tests || rc=1
  else
    skip_warn "cargo not found"
  fi

  if has_cargo_audit; then
    step cargo audit || rc=1
  else
    skip_warn "cargo-audit not installed (cargo install cargo-audit)"
  fi

  if has_npm; then
    # npm audit exit codes are aggressive (exits non-zero for moderate vulns
    # even with --audit-level=high), so failures here are non-fatal to avoid
    # false positives blocking the security category.
    step npm audit --prefix integrations/mcp/froglet --audit-level=high || true
  else
    skip_warn "npm not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: conformance
# ---------------------------------------------------------------------------
run_conformance() {
  banner "conformance"
  local rc=0

  if has_cargo; then
    step cargo test --test kernel_conformance_vectors || rc=1
  else
    skip_warn "cargo not found"
  fi

  if has_python; then
    step python3 -W error -m unittest \
      python.tests.test_conformance_vectors -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: stress
# ---------------------------------------------------------------------------
run_stress() {
  banner "stress"
  local rc=0

  if has_python; then
    step python3 -W error -m unittest python.tests.test_stress -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: smoke (compose-backed E2E)
# ---------------------------------------------------------------------------
run_smoke() {
  banner "smoke"

  if ! has_docker; then
    skip_warn "smoke tests require Docker"
    return 0
  fi
  if ! has_node; then
    skip_warn "smoke tests require node >= 18"
    return 0
  fi
  if ! ensure_mcp_deps; then
    return 1
  fi

  local rc=0
  if [[ "${FROGLET_TEST_REMOTE_STACK:-0}" == "1" ]]; then
    step node integrations/openclaw/froglet/test/compose-smoke.mjs || rc=1
    step node integrations/mcp/froglet/test/compose-smoke.mjs || rc=1
    return $rc
  fi

  compose_test_setup smoke
  step compose_test_env docker compose up --build -d --wait || rc=1
  if [[ "$rc" -eq 0 ]]; then
    step compose_test_env node integrations/openclaw/froglet/test/compose-smoke.mjs || rc=1
    step compose_test_env node integrations/mcp/froglet/test/compose-smoke.mjs || rc=1
  fi

  compose_test_finish "$rc"
}

# ---------------------------------------------------------------------------
# Category: agentic (model-in-the-loop)
# ---------------------------------------------------------------------------
run_agentic() {
  banner "agentic"

  # Support OPENCLAW_API_KEY with OPENAI_API_KEY fallback
  local api_key="${OPENCLAW_API_KEY:-${OPENAI_API_KEY:-}}"
  if [[ -z "$api_key" ]]; then
    skip_warn "agentic tests require OPENCLAW_API_KEY or OPENAI_API_KEY"
    return 0
  fi
  export OPENAI_API_KEY="$api_key"
  if ! has_node; then
    skip_warn "agentic tests require node >= 18"
    return 0
  fi
  if ! ensure_mcp_deps; then
    return 1
  fi

  local rc=0
  if [[ "${FROGLET_TEST_REMOTE_STACK:-0}" == "1" ]]; then
    step node integrations/openclaw/froglet/test/openai-responses-smoke.mjs \
      --out "${FROGLET_TEST_RESULTS_DIR:-$repo_root/_tmp/test-results}/openclaw-curated-local.json" || rc=1
    return $rc
  fi
  if ! has_docker; then
    skip_warn "agentic tests require Docker or FROGLET_TEST_REMOTE_STACK=1"
    return 0
  fi

  compose_test_setup agentic
  step compose_test_env docker compose up --build -d --wait || rc=1
  if [[ "$rc" -eq 0 ]]; then
    step compose_test_env node integrations/openclaw/froglet/test/openai-responses-smoke.mjs \
      --out "${FROGLET_TEST_RESULTS_DIR:-$repo_root/_tmp/test-results}/openclaw-curated-local.json" || rc=1
  fi

  compose_test_finish "$rc"
}

# ---------------------------------------------------------------------------
# Category: pentest
# ---------------------------------------------------------------------------
run_pentest() {
  banner "pentest"
  local rc=0

  if has_python; then
    step python3 -W error -m unittest python.tests.test_pentest -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: performance (API benchmarking)
# ---------------------------------------------------------------------------
run_performance() {
  banner "performance"
  local rc=0

  if has_python; then
    step python3 -W error -m unittest python.tests.test_bench_api -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: spike (sudden traffic surges)
# ---------------------------------------------------------------------------
run_spike() {
  banner "spike"
  local rc=0

  if has_python; then
    step python3 -W error -m unittest python.tests.test_spike -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: soak (endurance / stability)
# ---------------------------------------------------------------------------
run_soak() {
  banner "soak"
  local rc=0

  if has_python; then
    step python3 -W error -m unittest python.tests.test_soak -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: fuzz (HTTP API fuzzing)
# ---------------------------------------------------------------------------
run_fuzz() {
  banner "fuzz"
  local rc=0

  if has_python; then
    step python3 -W error -m unittest python.tests.test_fuzz_api -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: blackbox (public API testing without internal knowledge)
# ---------------------------------------------------------------------------
run_blackbox() {
  banner "blackbox"
  local rc=0

  if has_python; then
    step python3 -W error -m unittest python.tests.test_blackbox -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: graybox (blackbox + selected white-box unit tests)
# ---------------------------------------------------------------------------
run_graybox() {
  banner "graybox"
  local rc=0

  if has_python; then
    step python3 -W error -m unittest \
      python.tests.test_blackbox \
      python.tests.test_security -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: acceptance (UAT scenarios)
# ---------------------------------------------------------------------------
run_acceptance() {
  banner "acceptance"
  local rc=0

  if has_python; then
    if [[ -n "${FROGLET_ACCEPTANCE_TESTS:-}" ]]; then
      local acceptance_tests=()
      # shellcheck disable=SC2206
      acceptance_tests=(${FROGLET_ACCEPTANCE_TESTS})
      step python3 -W error -m unittest -v "${acceptance_tests[@]}" || rc=1
    else
      step python3 -W error -m unittest python.tests.test_acceptance -v || rc=1
    fi
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: chaos (Docker failure injection)
# ---------------------------------------------------------------------------
run_chaos() {
  banner "chaos"

  if ! has_docker; then
    skip_warn "chaos tests require Docker"
    return 0
  fi

  local rc=0
  if [[ "${FROGLET_TEST_REMOTE_STACK:-0}" == "1" ]]; then
    step bash tests/chaos/chaos_runner.sh || rc=1
    return $rc
  fi
  compose_test_setup chaos
  step compose_test_env bash tests/chaos/chaos_runner.sh || rc=1
  compose_test_finish "$rc"
}

# ---------------------------------------------------------------------------
# Category: exploratory (AI-driven exploratory testing)
# ---------------------------------------------------------------------------
run_exploratory() {
  banner "exploratory"

  local api_key="${OPENCLAW_API_KEY:-${OPENAI_API_KEY:-}}"
  if [[ -z "$api_key" ]]; then
    skip_warn "exploratory tests require OPENCLAW_API_KEY or OPENAI_API_KEY"
    return 0
  fi
  if ! has_node; then
    skip_warn "exploratory tests require node >= 18"
    return 0
  fi
  if ! ensure_mcp_deps; then
    return 1
  fi

  local rc=0
  export OPENAI_API_KEY="$api_key"
  if [[ "${FROGLET_TEST_REMOTE_STACK:-0}" == "1" ]]; then
    step node tests/e2e/agentic_exploratory.mjs \
      --out "${FROGLET_TEST_RESULTS_DIR:-$repo_root/_tmp/test-results}/openclaw-exploratory-local.json" || rc=1
    return $rc
  fi
  if ! has_docker; then
    skip_warn "exploratory tests require Docker or FROGLET_TEST_REMOTE_STACK=1"
    return 0
  fi

  compose_test_setup exploratory
  step compose_test_env docker compose up --build -d --wait || rc=1
  if [[ "$rc" -eq 0 ]]; then
    step compose_test_env node tests/e2e/agentic_exploratory.mjs \
      --out "${FROGLET_TEST_RESULTS_DIR:-$repo_root/_tmp/test-results}/openclaw-exploratory-local.json" || rc=1
  fi

  compose_test_finish "$rc"
}

# ---------------------------------------------------------------------------
# Category: mutation (cargo-mutants)
# ---------------------------------------------------------------------------
has_cargo_mutants() { command -v cargo-mutants >/dev/null 2>&1; }

run_mutation() {
  banner "mutation"

  if ! has_cargo; then
    skip_warn "cargo not found"
    return 0
  fi
  if ! has_cargo_mutants; then
    skip_warn "cargo-mutants not installed (cargo install cargo-mutants)"
    return 0
  fi

  local rc=0
  step cargo mutants --timeout 60 --jobs 4 || rc=1
  return $rc
}

# ---------------------------------------------------------------------------
# Category: vulnscan (dependency vulnerability scanning)
# ---------------------------------------------------------------------------
run_vulnscan() {
  banner "vulnscan"
  local rc=0

  if has_cargo_audit; then
    step cargo audit || rc=1
  else
    skip_warn "cargo-audit not installed"
  fi

  if has_npm; then
    step npm audit --prefix integrations/mcp/froglet --audit-level=high || true
  else
    skip_warn "npm not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: sanity (quick health check)
# ---------------------------------------------------------------------------
run_sanity() {
  banner "sanity"
  local rc=0

  if has_cargo; then
    step cargo test --lib -- --test-threads=1 crypto::tests || rc=1
  else
    skip_warn "cargo not found"
  fi

  if has_python; then
    step python3 -W error -m unittest python.tests.test_conformance_vectors -v || rc=1
  else
    skip_warn "python3 not found"
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Category: regression (all + baseline comparison)
# ---------------------------------------------------------------------------
run_regression() {
  banner "regression"
  local rc=0
  for cat in unit integration sast security conformance blackbox acceptance; do
    run_category "$cat" || rc=1
  done
  return $rc
}

# ---------------------------------------------------------------------------
# Category: e2e (complete user workflows)
# ---------------------------------------------------------------------------
run_e2e() {
  banner "e2e"
  local rc=0
  for cat in smoke blackbox acceptance; do
    run_category "$cat" || rc=1
  done
  return $rc
}

# ---------------------------------------------------------------------------
# Category: canary (acceptance on partial deploy — provider only)
# ---------------------------------------------------------------------------
run_canary() {
  banner "canary"
  local rc=0

  if ! has_docker; then
    skip_warn "canary tests require Docker"
    return 0
  fi

  if ! has_python; then
    skip_warn "canary tests require python3"
    return 0
  fi

  if [[ "${FROGLET_TEST_REMOTE_STACK:-0}" == "1" ]]; then
    step python3 -c '
import json
import urllib.request

def get_json(url: str) -> dict:
    with urllib.request.urlopen(url) as response:
        return json.load(response)

descriptor = get_json("http://127.0.0.1:8080/v1/provider/descriptor")
payload = descriptor["payload"]
assert isinstance(payload["provider_id"], str) and payload["provider_id"], payload
assert isinstance(payload["transport_endpoints"], list) and payload["transport_endpoints"], payload

offers = get_json("http://127.0.0.1:8080/v1/provider/offers")
assert isinstance(offers.get("offers"), list) and offers["offers"], offers
' || rc=1
    return $rc
  fi

  compose_test_setup canary

  step compose_test_env docker compose up --build -d --wait provider || rc=1
  if [[ "$rc" -eq 0 ]]; then
    step compose_test_env python3 -c '
import json
import urllib.request

def get_json(url: str) -> dict:
    with urllib.request.urlopen(url) as response:
        return json.load(response)

descriptor = get_json("http://127.0.0.1:8080/v1/provider/descriptor")
payload = descriptor["payload"]
assert isinstance(payload["provider_id"], str) and payload["provider_id"], payload
assert isinstance(payload["transport_endpoints"], list) and payload["transport_endpoints"], payload

offers = get_json("http://127.0.0.1:8080/v1/provider/offers")
assert isinstance(offers.get("offers"), list) and offers["offers"], offers
' || rc=1
  fi

  compose_test_finish "$rc"
}

# ---------------------------------------------------------------------------
# Meta-category: gcp_rig (GCP-backed test execution on ephemeral VM)
# ---------------------------------------------------------------------------
run_gcp_rig() {
  banner "gcp_rig"

  if [[ -z "${FROGLET_GCP_PROJECT:-}" ]]; then
    skip_warn "gcp_rig requires FROGLET_GCP_PROJECT"
    return 0
  fi
  if ! command -v gcloud >/dev/null 2>&1; then
    skip_warn "gcp_rig requires gcloud CLI"
    return 0
  fi

  # Source GCP instance manager
  source "$repo_root/scripts/gcp_instance.sh"

  local rc=0

  # Step 1: Create and provision the VM
  step gcp_create_instance
  step gcp_wait_ready
  step gcp_deploy_stack

  # Step 2: Run test categories on the VM
  local categories="${GCP_RIG_CATEGORIES:-smoke performance spike soak fuzz blackbox pentest}"
  step gcp_run_test_on_vm "$categories" || rc=1

  # Step 3: Cleanup is handled by EXIT trap in gcp_instance.sh

  return $rc
}

# ---------------------------------------------------------------------------
# Meta-categories (dispatch to run_category for each sub-category)
# ---------------------------------------------------------------------------
run_all() {
  local rc=0
  for cat in unit integration sast security conformance; do
    run_category "$cat" || rc=1
  done
  return $rc
}

run_full() {
  local rc=0
  for cat in unit integration sast security conformance stress smoke agentic; do
    run_category "$cat" || rc=1
  done

  if [[ "${FROGLET_RUN_TOR_INTEGRATION:-0}" == "1" ]] && has_python; then
    banner "tor-integration"
    step python3 -W error -m unittest python.tests.test_tor_integration -v || rc=1
  fi

  if [[ "${FROGLET_RUN_LND_REGTEST:-0}" == "1" ]] && has_python; then
    banner "lnd-regtest"
    step python3 -W error -m unittest python.tests.test_lnd_regtest -v || rc=1
  fi

  return $rc
}

# ---------------------------------------------------------------------------
# Dispatcher
# ---------------------------------------------------------------------------
run_category() {
  local cat="$1"
  case "$cat" in
    unit|integration|sast|security|conformance|stress|smoke|agentic|pentest|\
    performance|spike|soak|fuzz|blackbox|graybox|acceptance|chaos|exploratory|\
    mutation|vulnscan|sanity|regression|e2e|canary|gcp_rig|all|full)
      if ! "run_$cat"; then
        FAILURES+=("$cat")
        return 1
      fi
      ;;
    *)
      echo -e "${RED}Unknown category: $cat${RESET}" >&2
      echo "Run with --list to see available categories." >&2
      exit 1
      ;;
  esac
}

# ---------------------------------------------------------------------------
# --list
# ---------------------------------------------------------------------------
show_list() {
  cat <<'EOF'
Available test categories:

  Core:
  unit          Rust unit tests (--lib), Python unit tests, Node OpenClaw + MCP unit tests
  integration   Rust integration tests (--tests), Python integration tests
  sast          Static analysis: cargo fmt, clippy, node --check
  security      Python security/privacy/hardening, Rust crypto tests, cargo-audit, npm audit
  conformance   Kernel conformance vectors (Rust + Python)
  stress        Python stress tests (concurrent publish + query)
  smoke         Docker Compose E2E: OpenClaw + MCP compose-smoke (requires Docker)
  agentic       Curated blocking OpenAI/OpenClaw prompt suite (requires OPENCLAW_API_KEY or OPENAI_API_KEY)

  Extended:
  performance   API latency benchmarking (p50/p95/p99) and throughput
  spike         Sudden traffic surge testing and recovery measurement
  soak          Endurance testing (sustained load, degradation detection)
  fuzz          HTTP API fuzzing with malformed/oversized/injection payloads
  blackbox      Black box API testing (no internal knowledge)
  graybox       Black box + selected white-box tests
  acceptance    User acceptance testing (UAT business scenarios)
  pentest       Automated penetration testing (auth bypass, injection, replay)
  chaos         Docker failure injection (kill services, network partitions)
  exploratory   Blocking AI exploratory testing with anomaly gating (requires OPENCLAW_API_KEY or OPENAI_API_KEY)
  mutation      Mutation testing via cargo-mutants (requires cargo-mutants)
  vulnscan      Dependency vulnerability scanning (cargo-audit, npm audit)
  sanity        Quick health check (crypto tests + conformance vectors)
  canary        Compose-backed provider canary probe

  Meta-categories:
  all           unit + integration + sast + security + conformance
  full          all + stress + smoke + agentic + tor(if env) + lnd-regtest(if env)
  regression    all + blackbox + acceptance
  e2e           smoke + blackbox + acceptance
  gcp_rig       Provision GCP VM, deploy stack, run extended tests on-VM
                (requires FROGLET_GCP_PROJECT + gcloud CLI)

  Env vars:
  OPENCLAW_API_KEY / OPENAI_API_KEY   For agentic + exploratory tests
  FROGLET_DATA_ROOT                   Override compose-backed test data root
  FROGLET_TEST_RESULTS_DIR            Directory for JSON LLM test artifacts (default _tmp/test-results)
  FROGLET_GCP_PROJECT                 GCP project ID for gcp_rig
  FROGLET_PERF_REQUESTS               Benchmark request count (default 500)
  FROGLET_SOAK_DURATION_MINUTES       Soak test duration (default 5)
  GCP_RIG_CATEGORIES                  Categories to run on GCP VM
  FROGLET_PRICE_EXEC_WASM             Optional compose price override for GCP acceptance lanes
  FROGLET_ACCEPTANCE_TESTS            Optional space-separated unittest targets for acceptance

Usage:
  ./scripts/test_suite.sh                     # runs "all"
  ./scripts/test_suite.sh unit security       # run specific categories
  ./scripts/test_suite.sh --dry-run full      # preview commands without executing
  ./scripts/test_suite.sh --list              # this help
  ./scripts/test_suite.sh fuzz pentest        # run fuzz + penetration tests
  ./scripts/test_suite.sh gcp_rig             # full GCP-backed test rig
EOF
}

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
  case "$1" in
    --list|-l)    show_list; exit 0 ;;
    --dry-run)    DRY_RUN=1; shift ;;
    --help|-h)    show_list; exit 0 ;;
    -*)           echo "Unknown flag: $1" >&2; show_list; exit 1 ;;
    *)            CATEGORIES+=("$1"); shift ;;
  esac
done

if [[ ${#CATEGORIES[@]} -eq 0 ]]; then
  CATEGORIES=(all)
fi

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------
for cat in "${CATEGORIES[@]}"; do
  run_category "$cat" || true
done

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
if [[ ${#FAILURES[@]} -gt 0 ]]; then
  echo -e "${RED}${BOLD}FAILED categories: ${FAILURES[*]}${RESET}"
  exit 1
else
  echo -e "${GREEN}${BOLD}All requested categories passed.${RESET}"
  exit 0
fi
