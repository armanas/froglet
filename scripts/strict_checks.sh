#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

ensure_mcp_dependencies() {
  local package_dir="integrations/mcp/froglet"
  local marker="${package_dir}/node_modules/@modelcontextprotocol/sdk/package.json"

  if [[ -f "$marker" ]]; then
    return
  fi

  if ! command -v npm >/dev/null 2>&1; then
    echo "[strict] MCP checks require npm to install dependencies" >&2
    exit 1
  fi

  echo "[strict] installing MCP server dependencies"
  npm ci --prefix "$package_dir"
}

echo "[strict] cargo fmt --check"
cargo fmt --all --check

echo "[strict] cargo test with compiler warnings denied"
CARGO_INCREMENTAL=0 RUSTFLAGS="${RUSTFLAGS:-} -D warnings" cargo test --all-targets

if cargo clippy --version >/dev/null 2>&1; then
  echo "[strict] cargo clippy -D warnings"
  cargo clippy --all-targets -- -D warnings
else
  echo "[strict] skipping clippy: cargo-clippy is not installed"
fi

if command -v node >/dev/null 2>&1; then
  node_major=$(node -e 'process.stdout.write(String(process.versions.node.split(".")[0]))')
  if [ "$node_major" -ge 18 ] 2>/dev/null; then
    ensure_mcp_dependencies

    echo "[strict] OpenClaw plugin checks"
    node --check integrations/openclaw/froglet/index.js
    node --check integrations/openclaw/froglet/scripts/doctor.mjs
    node --test integrations/openclaw/froglet/test/plugin.test.js \
      integrations/openclaw/froglet/test/config-profiles.test.mjs \
      integrations/openclaw/froglet/test/doctor.test.mjs \
      integrations/openclaw/froglet/test/froglet-client.test.mjs

    echo "[strict] MCP server checks"
    node --check integrations/mcp/froglet/server.js
    node --test integrations/mcp/froglet/test/server.test.mjs

    if [[ "${FROGLET_RUN_COMPOSE_SMOKE:-0}" == "1" ]]; then
      if ! command -v docker >/dev/null 2>&1; then
        echo "[strict] compose smoke requested but docker is not installed" >&2
        exit 1
      fi

      echo "[strict] compose-backed bot-surface smoke"
      docker compose down --remove-orphans
      docker compose up --build -d
      trap 'docker compose down --remove-orphans' EXIT

      node integrations/openclaw/froglet/test/compose-smoke.mjs
      node integrations/mcp/froglet/test/compose-smoke.mjs
    fi
  else
    echo "[strict] skipping Node integration checks: node $node_major < 18"
  fi
else
  echo "[strict] skipping Node integration checks: node is not installed"
fi

echo "[strict] core python-backed runtime tests with warnings as errors"
python3 -W error -m unittest \
  python.tests.test_protocol \
  python.tests.test_runtime \
  python.tests.test_discovery \
  python.tests.test_jobs \
  python.tests.test_payments \
  python.tests.test_sandbox \
  python.tests.test_security \
  python.tests.test_privacy \
  python.tests.test_hardening \
  python.tests.test_conformance_vectors -v

if [[ "${FROGLET_RUN_TOR_INTEGRATION:-0}" == "1" ]]; then
  echo "[strict] tor integration"
  python3 -W error -m unittest -v python.tests.test_tor_integration
fi

if [[ "${FROGLET_RUN_LND_REGTEST:-0}" == "1" ]]; then
  echo "[strict] lnd regtest integration"
  python3 -W error -m unittest -v python.tests.test_lnd_regtest
fi
