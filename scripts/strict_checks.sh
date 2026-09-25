#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

strict_rustflags="${RUSTFLAGS:-}"
case " ${strict_rustflags} " in
  *" -D warnings "*) ;;
  *) strict_rustflags="${strict_rustflags:+${strict_rustflags} }-D warnings" ;;
esac

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
CARGO_INCREMENTAL=0 RUSTFLAGS="$strict_rustflags" cargo test --locked --workspace --all-targets

# The offline verifier must keep building without default features, and for
# wasm32, or the browser/npm distribution silently rots.
echo "[strict] froglet-protocol builds dependency-light and for wasm32"
CARGO_INCREMENTAL=0 cargo check -p froglet-protocol --no-default-features
if rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown; then
  CARGO_INCREMENTAL=0 cargo check -p froglet-protocol --no-default-features \
    --target wasm32-unknown-unknown
else
  echo "[strict] skipping wasm32 check: target wasm32-unknown-unknown is not installed"
fi

# conformance/kernel_v1.json is frozen: "signed froglet/v1 artifacts verify
# forever" is only true while these bytes never change.
echo "[strict] frozen kernel conformance vectors are unmodified"
if git rev-parse --git-dir >/dev/null 2>&1; then
  git diff --exit-code -- conformance/kernel_v1.json
  git diff --cached --exit-code -- conformance/kernel_v1.json
else
  echo "[strict] skipping frozen-fixture check: not a git checkout"
fi

if cargo clippy --version >/dev/null 2>&1; then
  echo "[strict] cargo clippy -D warnings"
  cargo clippy --locked --workspace --all-targets -- -D warnings
else
  echo "[strict] skipping clippy: cargo-clippy is not installed"
fi

echo "[strict] installer and release helper shell syntax"
sh -n scripts/install.sh
bash -n scripts/gitleaks_gate.sh
bash -n scripts/agent-bootstrap.sh
bash -n scripts/setup-agent.sh
bash -n scripts/setup-payment.sh
bash -n scripts/fresh_host_quickstart_smoke.sh
sh -n docs-site/public/agent
bash -n scripts/deploy_gcp_single_vm.sh
bash -n scripts/gpu_smoke.sh
bash -n scripts/package_release_assets.sh
bash -n scripts/verify_release_assets.sh
bash -n scripts/smoke_install_from_assets.sh
bash -n scripts/release_gate.sh

if [[ "${FROGLET_SKIP_GITLEAKS:-0}" != "1" ]]; then
  echo "[strict] gitleaks publication gate"
  ./scripts/gitleaks_gate.sh
fi

if command -v node >/dev/null 2>&1; then
  node_major=$(node -e 'process.stdout.write(String(process.versions.node.split(".")[0]))')
  if [ "$node_major" -ge 18 ] 2>/dev/null; then
    ensure_mcp_dependencies

    echo "[strict] OpenClaw plugin checks"
    node --check integrations/openclaw/froglet/index.js
    node --check integrations/openclaw/froglet/scripts/doctor.mjs
    node --test integrations/openclaw/froglet/test/plugin.test.js \
      integrations/openclaw/froglet/test/*.test.mjs

    echo "[strict] MCP server checks"
    node --check integrations/mcp/froglet/server.js
    node --test integrations/mcp/froglet/test/*.test.mjs

    echo "[strict] shared froglet-lib checks"
    node --check integrations/shared/froglet-lib/froglet-client.js
    node --check integrations/shared/froglet-lib/url-safety.js
    node --test integrations/shared/froglet-lib/test/*.test.mjs

    if [[ "${FROGLET_RUN_COMPOSE_SMOKE:-0}" == "1" ]]; then
      if ! command -v docker >/dev/null 2>&1; then
        echo "[strict] compose smoke requested but docker is not installed" >&2
        exit 1
      fi

      echo "[strict] compose-backed bot-surface smoke"
      docker compose -f compose.yaml -f compose.ci.yaml down --remove-orphans
      docker compose -f compose.yaml -f compose.ci.yaml up --build -d --wait
      trap 'docker compose -f compose.yaml -f compose.ci.yaml down --remove-orphans' EXIT

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
  python.tests.test_jobs \
  python.tests.test_payments \
  python.tests.test_sandbox \
  python.tests.test_acceptance \
  python.tests.test_pentest \
  python.tests.test_security \
  python.tests.test_privacy \
  python.tests.test_hardening \
  python.tests.test_install_script \
  python.tests.test_setup_scripts \
  python.tests.test_conformance_vectors -v

echo "[strict] standalone python verifier package"
(
  cd python/froglet-verify
  python3 -W error -m unittest discover -s tests -v
)
echo "[strict] python verifier reproduces every conformance fixture"
PYTHONPATH="python/froglet-verify" python3 -m froglet_verify.conformance conformance/

if [[ "${FROGLET_RUN_TOR_INTEGRATION:-0}" == "1" ]]; then
  echo "[strict] tor integration"
  python3 -W error -m unittest -v python.tests.test_tor_integration
fi

if [[ "${FROGLET_RUN_LINUX_SANDBOX_TESTS:-0}" == "1" ]]; then
  # The landlock+seccomp sandbox tests need Linux kernel capabilities the
  # default GitHub Actions runner does not grant (CAP_SYS_ADMIN-equivalent
  # privileges for seccomp/landlock syscalls). On a capable runner — a
  # self-hosted runner, a bare Linux VM, or local Linux with the right
  # priv set — export FROGLET_RUN_LINUX_SANDBOX_TESTS=1 to exercise them.
  echo "[strict] linux sandbox tests (landlock + seccomp)"
  CARGO_INCREMENTAL=0 RUSTFLAGS="$strict_rustflags" \
    cargo test --locked --workspace --all-targets -- --ignored \
      python_sandbox::tests:: \
      service_addressed_python_execution_runs_from_redacted_service_record
fi

if [[ "${FROGLET_RUN_LND_REGTEST:-0}" == "1" ]]; then
  echo "[strict] lnd regtest integration"
  python3 -W error -m unittest -v python.tests.test_lnd_regtest
fi
