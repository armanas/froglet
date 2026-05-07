#!/usr/bin/env bash
set -euo pipefail

raw_base="${FROGLET_RAW_BASE:-https://raw.githubusercontent.com/armanas/froglet/main}"
agent_script_url="${FROGLET_FRESH_HOST_AGENT_URL:-$raw_base/scripts/agent-bootstrap.sh}"
target_agent="${FROGLET_FRESH_HOST_TARGET_AGENT:-claude-code}"
image_tag="${FROGLET_FRESH_HOST_IMAGE_TAG:-}"
workspace="${FROGLET_FRESH_HOST_WORKDIR:-}"
keep_workspace=0
skip_start=0
compose_started=0

usage() {
  cat <<'EOF'
Usage:
  scripts/fresh_host_quickstart_smoke.sh [--agent-url URL] [--raw-base URL] [--target-agent claude-code|codex|manual] [--image-tag TAG] [--skip-start] [--keep]

Runs the public no-clone quickstart in an isolated HOME. Full mode requires
Docker with Compose v2 and waits for provider/runtime health on
127.0.0.1:8080/8081.

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
  if [[ "$compose_started" -eq 1 && -n "${compose_project:-}" ]]; then
    docker compose --project-name "$compose_project" down --remove-orphans >/dev/null 2>&1 || true
  fi
  if [[ "$keep_workspace" -eq 0 && -n "${workspace:-}" && -d "$workspace" ]]; then
    rm -rf "$workspace"
  elif [[ -n "${workspace:-}" ]]; then
    log "kept workspace at $workspace"
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --agent-url)
      [[ $# -ge 2 ]] || fail "--agent-url requires a value"
      agent_script_url="$2"
      shift 2
      ;;
    --raw-base)
      [[ $# -ge 2 ]] || fail "--raw-base requires a value"
      raw_base="$2"
      shift 2
      ;;
    --target-agent)
      [[ $# -ge 2 ]] || fail "--target-agent requires a value"
      target_agent="$2"
      shift 2
      ;;
    --image-tag)
      [[ $# -ge 2 ]] || fail "--image-tag requires a value"
      image_tag="$2"
      shift 2
      ;;
    --skip-start)
      skip_start=1
      shift
      ;;
    --payment-rail)
      [[ $# -ge 2 ]] || fail "--payment-rail requires a value"
      [[ "$2" == "none" ]] || fail "fresh bootstrap starts with payment_rail=none; configure paid rails through MCP after health checks"
      shift 2
      ;;
    --skip-install|--skip-compose|--repo-url)
      fail "$1 belonged to the old clone-based quickstart smoke; use --skip-start or --agent-url"
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
  claude-code|codex|manual) ;;
  openclaw)
    fail "OpenClaw still requires the repo-local plugin folder; no-clone bootstrap supports claude-code, codex, or manual"
    ;;
  *) fail "unsupported target agent: $target_agent" ;;
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
bootstrap_dir="$home_dir/.froglet/agent"
data_dir="$home_dir/.froglet/data"
agent_script="$workspace/agent-bootstrap.sh"
bootstrap_json="$workspace/bootstrap.json"
compose_file="$bootstrap_dir/compose.yaml"
compose_project="${FROGLET_FRESH_HOST_COMPOSE_PROJECT:-froglet_fresh_host_$(basename "$workspace" | tr '[:upper:]' '[:lower:]' | tr -cd 'a-z0-9_-')}"

mkdir -p "$home_dir" "$install_dir" "$bootstrap_dir" "$data_dir"

log "workspace: $workspace"
log "compose project: $compose_project"
log "agent bootstrap: $agent_script_url"
log "raw base: $raw_base"
log "target agent: $target_agent"
if [[ -n "$image_tag" ]]; then
  log "image tag override: $image_tag"
fi

need_cmd curl
need_cmd tar
need_cmd mktemp

log "downloading no-clone agent bootstrap"
curl -fsSL "$agent_script_url" -o "$agent_script"
chmod 0755 "$agent_script"
bash -n "$agent_script"

start_flag=1
if [[ "$skip_start" -eq 1 ]]; then
  start_flag=0
else
  need_cmd docker
  docker compose version >/dev/null
  docker info >/dev/null
fi

log "running agent bootstrap in isolated HOME"
bootstrap_env=(
  HOME="$home_dir"
  INSTALL_DIR="$install_dir"
  FROGLET_BOOTSTRAP_DIR="$bootstrap_dir"
  FROGLET_DATA_DIR="$data_dir"
  FROGLET_RAW_BASE="$raw_base"
  FROGLET_AGENT_TARGET="$target_agent"
  FROGLET_BOOTSTRAP_START="$start_flag"
  COMPOSE_PROJECT_NAME="$compose_project"
)
if [[ -n "$image_tag" ]]; then
  bootstrap_env+=(FROGLET_IMAGE_TAG="$image_tag")
fi
env "${bootstrap_env[@]}" bash "$agent_script" >"$bootstrap_json"

[[ -x "$install_dir/froglet-node" ]] || fail "installed froglet-node is not executable"
log "installed froglet-node binary is executable"

case "$target_agent" in
  claude-code)
    [[ -f "$bootstrap_dir/mcp/.mcp.json" ]] || fail "Claude Code MCP config was not written"
    log "Claude Code MCP config written"
    ;;
  codex)
    [[ -f "$bootstrap_dir/mcp/config.toml" ]] || fail "Codex MCP config was not written"
    log "Codex MCP config written"
    ;;
  manual)
    log "manual target selected; MCP config intentionally skipped"
    ;;
esac

if [[ "$skip_start" -eq 1 ]]; then
  log "fresh-host no-clone smoke passed before Docker start"
  exit 0
fi

[[ -f "$compose_file" ]] || fail "bootstrap compose file was not written"
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

log "fresh-host no-clone quickstart smoke passed"
log "stopping smoke stack"
docker compose --project-name "$compose_project" down --remove-orphans >/dev/null 2>&1 || true
compose_started=0
