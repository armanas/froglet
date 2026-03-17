#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

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
    echo "[strict] OpenClaw plugin checks"
    node --check integrations/openclaw/froglet/index.js
    node --test integrations/openclaw/froglet/test/plugin.test.js
  else
    echo "[strict] skipping OpenClaw plugin checks: node $node_major < 18"
  fi
else
  echo "[strict] skipping OpenClaw plugin checks: node is not installed"
fi

echo "[strict] python unittest with warnings as errors"
python3 -W error -m unittest discover -s python/tests -t . -v

if [[ "${FROGLET_RUN_TOR_INTEGRATION:-0}" == "1" ]]; then
  echo "[strict] tor integration"
  python3 -W error -m unittest -v python.tests.test_tor_integration
fi

if [[ "${FROGLET_RUN_LND_REGTEST:-0}" == "1" ]]; then
  echo "[strict] lnd regtest integration"
  python3 -W error -m unittest -v python.tests.test_lnd_regtest
fi
