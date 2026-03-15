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

echo "[strict] python unittest with warnings as errors"
python3 -W error -m unittest -v

if [[ "${FROGLET_RUN_TOR_INTEGRATION:-0}" == "1" ]]; then
  echo "[strict] tor integration"
  python3 -W error -m unittest -v test_tor_integration.py
fi

if [[ "${FROGLET_RUN_LND_REGTEST:-0}" == "1" ]]; then
  echo "[strict] lnd regtest integration"
  python3 -W error -m unittest -v test_lnd_regtest.py
fi
