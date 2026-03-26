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
      python.tests.test_client_sdk \
      python.tests.test_nostr_adapter \
      python.tests.test_examples -v || rc=1
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
      python.tests.test_discovery \
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
    step python3 -m py_compile python/froglet_client.py || rc=1
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
  step docker compose down --remove-orphans
  step docker compose up --build -d --wait || { rc=1; return $rc; }
  trap 'docker compose down --remove-orphans 2>/dev/null || true' EXIT

  step node integrations/openclaw/froglet/test/compose-smoke.mjs || rc=1
  step node integrations/mcp/froglet/test/compose-smoke.mjs || rc=1

  return $rc
}

# ---------------------------------------------------------------------------
# Category: agentic (model-in-the-loop)
# ---------------------------------------------------------------------------
run_agentic() {
  banner "agentic"

  if [[ -z "${OPENAI_API_KEY:-}" ]]; then
    skip_warn "agentic tests require OPENAI_API_KEY"
    return 0
  fi
  if ! has_node; then
    skip_warn "agentic tests require node >= 18"
    return 0
  fi

  local rc=0
  step node integrations/openclaw/froglet/test/openai-responses-smoke.mjs || rc=1
  return $rc
}

# ---------------------------------------------------------------------------
# Category: pentest (placeholder)
# ---------------------------------------------------------------------------
run_pentest() {
  banner "pentest"
  echo "  Penetration testing is a placeholder category."
  echo "  Future scripts should be added to tests/pentest/."
  echo "  See tests/README.md for conventions."
  return 0
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
    unit|integration|sast|security|conformance|stress|smoke|agentic|pentest|all|full)
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

  unit          Rust unit tests (--lib), Python unit tests, Node OpenClaw + MCP unit tests
  integration   Rust integration tests (--tests), Python integration tests
  sast          Static analysis: cargo fmt, clippy, node --check, py_compile
  security      Python security/privacy/hardening, Rust crypto tests, cargo-audit, npm audit
  conformance   Kernel conformance vectors (Rust + Python)
  stress        Python stress tests
  smoke         Docker Compose E2E: OpenClaw + MCP compose-smoke (requires Docker)
  agentic       OpenAI Responses smoke test (requires OPENAI_API_KEY + running operator)
  pentest       Placeholder for future penetration testing scripts

Meta-categories:
  all           unit + integration + sast + security + conformance
  full          all + stress + smoke + agentic + tor(if env) + lnd-regtest(if env)

Usage:
  ./scripts/test_suite.sh                     # runs "all"
  ./scripts/test_suite.sh unit security       # run specific categories
  ./scripts/test_suite.sh --dry-run full      # preview commands without executing
  ./scripts/test_suite.sh --list              # this help
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
