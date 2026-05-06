#!/usr/bin/env bash
set -euo pipefail

repo_url="${FROGLET_FRESH_HOST_REPO_URL:-https://github.com/armanas/froglet.git}"
install_script_url="${FROGLET_FRESH_HOST_INSTALL_URL:-https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh}"
target_agent="${FROGLET_FRESH_HOST_TARGET_AGENT:-claude-code}"
payment_rail="${FROGLET_FRESH_HOST_PAYMENT_RAIL:-lightning}"
workspace="${FROGLET_FRESH_HOST_WORKDIR:-}"
keep_workspace=0
skip_install=0
skip_compose=0
compose_started=0

usage() {
  cat <<'EOF'
Usage:
  scripts/fresh_host_quickstart_smoke.sh [--repo-url URL] [--target-agent claude-code|codex|openclaw|manual] [--payment-rail none|lightning|stripe|x402] [--skip-install] [--skip-compose] [--keep]

Runs the public quickstart in a disposable directory. Full mode requires Docker
with Compose v2 and waits for provider/runtime health on 127.0.0.1:8080/8081.

For a VM proof from a clean shell:
  curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/fresh_host_quickstart_smoke.sh | bash
EOF
}

log() {
  printf '[fresh-host] %s\n' "$*"
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

cleanup() {
  if [[ "$compose_started" -eq 1 && -n "${repo_dir:-}" && -d "$repo_dir" ]]; then
    (cd "$repo_dir" && COMPOSE_PROJECT_NAME="$compose_project" docker compose down --remove-orphans >/dev/null 2>&1 || true)
  fi
  if [[ "$keep_workspace" -eq 0 && -n "${workspace:-}" && -d "$workspace" ]]; then
    rm -rf "$workspace"
  elif [[ -n "${workspace:-}" ]]; then
    log "kept workspace at $workspace"
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo-url)
      [[ $# -ge 2 ]] || fail "--repo-url requires a value"
      repo_url="$2"
      shift 2
      ;;
    --target-agent)
      [[ $# -ge 2 ]] || fail "--target-agent requires a value"
      target_agent="$2"
      shift 2
      ;;
    --payment-rail)
      [[ $# -ge 2 ]] || fail "--payment-rail requires a value"
      payment_rail="$2"
      shift 2
      ;;
    --skip-install)
      skip_install=1
      shift
      ;;
    --skip-compose)
      skip_compose=1
      shift
      ;;
    --keep)
      keep_workspace=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

case "$target_agent" in
  claude-code|codex|openclaw|manual) ;;
  *) fail "unsupported target agent: $target_agent" ;;
esac

case "$payment_rail" in
  none|lightning|stripe|x402) ;;
  *) fail "unsupported payment rail: $payment_rail" ;;
esac

if [[ -z "$workspace" ]]; then
  workspace="$(mktemp -d "${TMPDIR:-/tmp}/froglet-fresh-host.XXXXXX")"
else
  mkdir -p "$workspace"
  workspace="$(cd "$workspace" && pwd)"
fi
trap cleanup EXIT HUP INT TERM

home_dir="$workspace/home"
install_dir="$home_dir/.local/bin"
repo_dir="$workspace/froglet"
compose_project="${FROGLET_FRESH_HOST_COMPOSE_PROJECT:-froglet_fresh_host_$(basename "$workspace" | tr '[:upper:]' '[:lower:]' | tr -cd 'a-z0-9_-')}"
mkdir -p "$home_dir" "$install_dir"

log "workspace: $workspace"
log "compose project: $compose_project"

if [[ "$skip_install" -eq 0 ]]; then
  need_cmd curl
  need_cmd tar
  need_cmd mktemp
  log "installing signed froglet-node into isolated HOME"
  curl -fsSL "$install_script_url" -o "$workspace/install.sh"
  install_env=(HOME="$home_dir" INSTALL_DIR="$install_dir")
  if [[ -n "${VERSION:-}" ]]; then
    install_env+=(VERSION="$VERSION")
  fi
  env "${install_env[@]}" sh "$workspace/install.sh"
  [[ -x "$install_dir/froglet-node" ]] || fail "installed froglet-node is not executable"
  log "installed froglet-node binary is executable"
else
  log "skipping binary installer"
fi

need_cmd git
log "cloning $repo_url"
git clone --depth 1 "$repo_url" "$repo_dir" >/dev/null

if [[ "$target_agent" == "claude-code" || "$target_agent" == "codex" ]]; then
  need_cmd npm
  log "installing local MCP dependencies"
  (cd "$repo_dir" && npm ci --prefix integrations/mcp/froglet)
fi

case "$target_agent" in
  claude-code|codex|openclaw)
    log "generating $target_agent agent config"
    (cd "$repo_dir" && ./scripts/setup-agent.sh --target "$target_agent")
    ;;
  manual)
    log "manual target selected; skipping generated agent config"
    ;;
esac

case "$payment_rail" in
  none)
    log "writing no-payment env snippet"
    mkdir -p "$repo_dir/.froglet/payment"
    printf '%s\n' 'FROGLET_PAYMENT_BACKEND=none' > "$repo_dir/.froglet/payment/none.env"
    ;;
  lightning)
    log "configuring lightning mock payment rail"
    (cd "$repo_dir" && ./scripts/setup-payment.sh lightning)
    ;;
  stripe)
    [[ -n "${FROGLET_STRIPE_SECRET_KEY:-}" ]] || fail "FROGLET_STRIPE_SECRET_KEY is required for --payment-rail stripe"
    log "configuring stripe payment rail"
    (cd "$repo_dir" && ./scripts/setup-payment.sh stripe)
    ;;
  x402)
    [[ -n "${FROGLET_X402_WALLET_ADDRESS:-}" ]] || fail "FROGLET_X402_WALLET_ADDRESS is required for --payment-rail x402"
    log "configuring x402 payment rail"
    (cd "$repo_dir" && ./scripts/setup-payment.sh x402)
    ;;
esac

if [[ "$skip_compose" -eq 1 ]]; then
  if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
    log "validating compose config without starting containers"
    (
      cd "$repo_dir"
      set -a
      # shellcheck disable=SC1090
      . "./.froglet/payment/${payment_rail}.env"
      export FROGLET_HOST_READABLE_CONTROL_TOKEN=true
      set +a
      COMPOSE_PROJECT_NAME="$compose_project" docker compose config >/dev/null
    )
  else
    log "docker compose unavailable; skipped compose config validation"
  fi
  log "quickstart setup smoke passed before compose start"
  exit 0
fi

need_cmd curl
need_cmd docker
docker compose version >/dev/null
docker info >/dev/null

log "starting local provider/runtime stack"
(
  cd "$repo_dir"
  set -a
  # shellcheck disable=SC1090
  . "./.froglet/payment/${payment_rail}.env"
  export FROGLET_HOST_READABLE_CONTROL_TOKEN=true
  set +a
  COMPOSE_PROJECT_NAME="$compose_project" docker compose up --build -d
)
compose_started=1

wait_for_url() {
  local url="$1"
  local label="$2"
  local attempt
  for attempt in $(seq 1 60); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      log "$label healthy"
      return 0
    fi
    sleep 2
  done
  fail "$label did not become healthy at $url"
}

wait_for_url "http://127.0.0.1:8080/health" "provider"
wait_for_url "http://127.0.0.1:8081/health" "runtime"

log "fresh-host quickstart smoke passed"
