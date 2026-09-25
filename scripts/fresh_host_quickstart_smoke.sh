#!/usr/bin/env bash
set -euo pipefail

repo="${FROGLET_INSTALL_REPO:-armanas/froglet}"
raw_base="${FROGLET_RAW_BASE:-}"
agent_script_url="${FROGLET_FRESH_HOST_AGENT_URL:-}"
agent_script_sha256="${FROGLET_FRESH_HOST_AGENT_SHA256:-}"
target_agent="${FROGLET_FRESH_HOST_TARGET_AGENT:-claude-code}"
release_tag="${FROGLET_FRESH_HOST_RELEASE_TAG:-${FROGLET_FRESH_HOST_IMAGE_TAG:-}}"
bootstrap_mode="${FROGLET_FRESH_HOST_MODE:-native}"
workspace="${FROGLET_FRESH_HOST_WORKDIR:-}"
data_fixture="${FROGLET_FRESH_HOST_DATA_FIXTURE:-}"
keep_workspace=0
skip_start=0
compose_started=0
native_started=0
native_service_label="dev.froglet.node"

usage() {
  cat <<'EOF'
Usage:
  scripts/fresh_host_quickstart_smoke.sh [--agent-url URL --agent-sha256 HEX] [--raw-base URL] [--target-agent claude-code|codex|manual] [--release-tag TAG] [--mode native|docker] [--data-fixture JSON] [--skip-start] [--keep]

Runs the public no-clone quickstart in an isolated HOME. Native mode is the
default and needs no Python, jq, Node.js, or Docker. Docker mode exercises the
released dual-role fallback image. Both wait for provider/runtime health, prove
native MCP attachment without seeding services, and require transient data
proof cleanup before success.

Run this file from a trusted checkout. By default it resolves the immutable
GitHub release and verifies the uploaded agent-bootstrap.sh asset digest before
execution. Custom --agent-url inputs require an explicit --agent-sha256 pin.
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

file_sha256() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$path" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$path" | sed 's/^.*= //'
  else
    fail "sha256sum, shasum, or openssl is required"
  fi
}

resolve_immutable_bootstrap() {
  local metadata="$1"
  local resolved_url tag_values immutable_values matches count state digest
  if [[ -z "$release_tag" ]]; then
    resolved_url="$(
      curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 \
        -o /dev/null -w '%{url_effective}' \
        "https://github.com/$repo/releases/latest"
    )"
    resolved_url="${resolved_url%/}"
    release_tag="${resolved_url##*/}"
  elif [[ "$release_tag" != v* ]]; then
    release_tag="v$release_tag"
  fi
  [[ "$release_tag" =~ ^v[0-9A-Za-z][0-9A-Za-z.+-]*$ ]] || \
    fail "invalid release tag: $release_tag"

  curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2026-03-10' \
    "https://api.github.com/repos/$repo/releases/tags/$release_tag" \
    -o "$metadata"
  immutable_values="$(
    sed -n 's/^[[:space:]][[:space:]]"immutable"[[:space:]]*:[[:space:]]*\([a-z][a-z]*\)[,]*[[:space:]]*$/\1/p' \
      "$metadata"
  )"
  [[ "$immutable_values" == true ]] || \
    fail "release $release_tag is not a unique immutable GitHub release"
  tag_values="$(
    sed -n 's/^[[:space:]][[:space:]]"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)"[,]*[[:space:]]*$/\1/p' \
      "$metadata"
  )"
  [[ "$tag_values" == "$release_tag" ]] || \
    fail "release metadata tag does not match $release_tag"
  matches="$(
    awk '
      /^    \{[[:space:]]*$/ { in_asset=1; name=""; digest=""; state=""; next }
      in_asset && /^      "name"[[:space:]]*:/ { value=$0; sub(/^      "name"[[:space:]]*:[[:space:]]*"/,"",value); sub(/"[,]?[[:space:]]*$/,"",value); name=value; next }
      in_asset && /^      "digest"[[:space:]]*:/ { value=$0; sub(/^      "digest"[[:space:]]*:[[:space:]]*"/,"",value); sub(/"[,]?[[:space:]]*$/,"",value); digest=value; next }
      in_asset && /^      "state"[[:space:]]*:/ { value=$0; sub(/^      "state"[[:space:]]*:[[:space:]]*"/,"",value); sub(/"[,]?[[:space:]]*$/,"",value); state=value; next }
      in_asset && /^    \}[,]?[[:space:]]*$/ { if (name=="agent-bootstrap.sh") print digest "|" state; in_asset=0 }
    ' "$metadata"
  )"
  count="$(printf '%s\n' "$matches" | sed '/^$/d' | wc -l | tr -d ' ')"
  [[ "$count" == 1 ]] || \
    fail "immutable release must contain exactly one agent-bootstrap.sh asset"
  digest="${matches%%|*}"
  state="${matches#*|}"
  [[ "$state" == uploaded ]] || fail "agent-bootstrap.sh asset is not uploaded"
  [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || \
    fail "agent-bootstrap.sh asset has no valid SHA-256 API digest"
  agent_script_sha256="${digest#sha256:}"
  agent_script_url="https://github.com/$repo/releases/download/$release_tag/agent-bootstrap.sh"
}

cleanup() {
  local exit_status=$?
  local cleanup_ok=1
  if [[ "${skip_start:-0}" -eq 0 && "${bootstrap_mode:-}" == "docker" \
    && -n "${compose_project:-}" && -f "${compose_file:-}" ]]; then
    if ! docker compose --project-name "$compose_project" -f "$compose_file" \
      down --remove-orphans >/dev/null 2>&1; then
      log "cleanup warning: Docker smoke stack could not be stopped"
      cleanup_ok=0
    fi
  fi
  if [[ -x "${bootstrap_dir:-}/froglet-service.sh" \
    && ( "$native_started" -eq 1 || -f "$bootstrap_dir/current-release" \
      || -L "$bootstrap_dir/current" ) ]]; then
    if ! HOME="$home_dir" \
      FROGLET_BOOTSTRAP_DIR="$bootstrap_dir" \
      FROGLET_DATA_DIR="$data_dir" \
      INSTALL_DIR="$install_dir" \
        bash "$bootstrap_dir/froglet-service.sh" uninstall >/dev/null 2>&1; then
      log "cleanup warning: native Froglet service could not be uninstalled"
      cleanup_ok=0
    fi
  fi
  if [[ "$cleanup_ok" -eq 1 && "$keep_workspace" -eq 0 \
    && -n "${workspace:-}" && -d "$workspace" ]]; then
    rm -rf "$workspace"
  elif [[ -n "${workspace:-}" ]]; then
    if [[ "$cleanup_ok" -eq 0 ]]; then
      log "kept workspace after incomplete lifecycle cleanup at $workspace"
    else
      log "kept workspace at $workspace"
    fi
  fi
  if [[ "$exit_status" -eq 0 && "$cleanup_ok" -eq 0 ]]; then
    return 1
  fi
  return "$exit_status"
}

ensure_native_service_slot_is_free() {
  case "$(uname -s 2>/dev/null || true)" in
    Darwin)
      if launchctl print "gui/$(id -u)/$native_service_label" >/dev/null 2>&1; then
        fail "native smoke refuses to replace an existing $native_service_label launchd service"
      fi
      ;;
    Linux)
      if command -v systemctl >/dev/null 2>&1 \
        && systemctl --user is-active froglet.service >/dev/null 2>&1; then
        fail "native smoke refuses to replace an existing froglet.service user service"
      fi
      ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --agent-url)
      [[ $# -ge 2 ]] || fail "--agent-url requires a value"
      agent_script_url="$2"
      shift 2
      ;;
    --agent-sha256)
      [[ $# -ge 2 ]] || fail "--agent-sha256 requires a value"
      agent_script_sha256="$2"
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
    --release-tag|--image-tag)
      [[ $# -ge 2 ]] || fail "$1 requires a value"
      release_tag="$2"
      shift 2
      ;;
    --mode)
      [[ $# -ge 2 ]] || fail "--mode requires native or docker"
      bootstrap_mode="$2"
      shift 2
      ;;
    --data-fixture)
      [[ $# -ge 2 ]] || fail "--data-fixture requires a JSON file"
      data_fixture="$2"
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

case "$bootstrap_mode" in
  native|docker) ;;
  *) fail "--mode must be native or docker" ;;
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
agent_project_dir="$workspace/project"
agent_script="$workspace/agent-bootstrap.sh"
install_plan_json="$workspace/install-plan.json"
bootstrap_json="$workspace/bootstrap.json"
release_metadata="$workspace/release-api.json"
compose_file="$bootstrap_dir/compose.yaml"
compose_project="${FROGLET_FRESH_HOST_COMPOSE_PROJECT:-froglet_fresh_host_$(basename "$workspace" | tr '[:upper:]' '[:lower:]' | tr -cd 'a-z0-9_-')}"

if [[ -n "$data_fixture" ]]; then
  [[ -f "$data_fixture" ]] || fail "data fixture is not a regular file: $data_fixture"
  data_fixture="$(cd "$(dirname "$data_fixture")" && pwd)/$(basename "$data_fixture")"
else
  data_fixture="$workspace/user-supplied-fixture.json"
  cat >"$data_fixture" <<'EOF'
[
  {"owner":"fresh-host-smoke","value":17},
  {"owner":"fresh-host-smoke","value":23}
]
EOF
fi

need_cmd curl
need_cmd tar
need_cmd mktemp

if [[ -z "$agent_script_url" ]]; then
  resolve_immutable_bootstrap "$release_metadata"
else
  [[ "$agent_script_url" == https://* ]] || \
    fail "custom --agent-url must use https://"
  [[ "$agent_script_sha256" =~ ^[0-9a-f]{64}$ ]] || \
    fail "custom --agent-url requires a lowercase SHA-256 via --agent-sha256"
fi

log "workspace: $workspace"
log "compose project: $compose_project"
log "agent bootstrap: $agent_script_url"
if [[ -n "$raw_base" ]]; then
  log "raw base override: $raw_base"
fi
log "target agent: $target_agent"
log "bootstrap mode: $bootstrap_mode"
log "user data fixture: $data_fixture"
if [[ -n "$release_tag" ]]; then
  log "release tag: $release_tag"
fi

log "downloading no-clone agent bootstrap"
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 \
  "$agent_script_url" -o "$agent_script"
[[ "$(file_sha256 "$agent_script")" == "$agent_script_sha256" ]] || \
  fail "agent-bootstrap.sh SHA-256 does not match the approved release asset"
chmod 0755 "$agent_script"
bash -n "$agent_script"

start_flag=1
if [[ "$skip_start" -eq 1 ]]; then
  start_flag=0
elif [[ "$bootstrap_mode" == "docker" ]]; then
  need_cmd docker
  docker compose version >/dev/null
  docker info >/dev/null
else
  ensure_native_service_slot_is_free
fi

log "planning agent bootstrap in isolated HOME (non-mutating)"
bootstrap_env=(
  HOME="$home_dir"
  INSTALL_DIR="$install_dir"
  FROGLET_BOOTSTRAP_DIR="$bootstrap_dir"
  FROGLET_DATA_DIR="$data_dir"
  FROGLET_AGENT_PROJECT_DIR="$agent_project_dir"
  FROGLET_AGENT_TARGET="$target_agent"
  FROGLET_BOOTSTRAP_MODE="$bootstrap_mode"
  FROGLET_BOOTSTRAP_START="$start_flag"
  FROGLET_BOOTSTRAP_DATA_FIXTURE="$data_fixture"
  COMPOSE_PROJECT_NAME="$compose_project"
)
if [[ -n "$raw_base" ]]; then
  bootstrap_env+=(FROGLET_RAW_BASE="$raw_base")
fi
if [[ -n "$release_tag" ]]; then
  bootstrap_env+=(VERSION="$release_tag")
fi
env "${bootstrap_env[@]}" bash "$agent_script" plan >"$install_plan_json"
approval_hash="$(
  sed -n 's/^[[:space:]]*"install_approval_hash"[[:space:]]*:[[:space:]]*"\([0-9a-f]*\)"[,]*[[:space:]]*$/\1/p' \
    "$install_plan_json"
)"
[[ "$approval_hash" =~ ^[0-9a-f]{64}$ ]] || \
  fail "install plan did not return one canonical approval hash"
[[ ! -e "$install_dir" ]] || fail "install plan mutated the binary directory"
[[ ! -e "$bootstrap_dir" ]] || fail "install plan mutated the bootstrap directory"
[[ ! -e "$data_dir" ]] || fail "install plan mutated the data directory"
[[ ! -e "$agent_project_dir" ]] || fail "install plan mutated the agent project"
log "test harness approving exact native install plan: $approval_hash"
env "${bootstrap_env[@]}" bash "$agent_script" execute "$approval_hash" >"$bootstrap_json"

[[ -x "$install_dir/froglet-node" ]] || fail "installed froglet-node is not executable"
log "installed froglet-node binary is executable"

case "$target_agent" in
  claude-code)
    [[ -f "$agent_project_dir/.mcp.json" ]] || fail "Claude Code MCP config was not installed directly"
    log "Claude Code MCP config written"
    ;;
  codex)
    [[ -f "$agent_project_dir/.codex/config.toml" ]] || fail "Codex MCP config was not installed directly"
    log "Codex MCP config written"
    ;;
  manual)
    log "manual target selected; MCP config intentionally skipped"
    ;;
esac

if [[ "$skip_start" -eq 1 ]]; then
  log "fresh-host no-clone smoke passed before service start"
  exit 0
fi

if [[ "$bootstrap_mode" == "docker" ]]; then
  [[ -f "$compose_file" ]] || fail "bootstrap compose file was not written"
  compose_started=1
else
  [[ -x "$bootstrap_dir/froglet-service.sh" ]] || fail "native lifecycle command was not installed"
  [[ -f "$bootstrap_dir/local-proof.json" ]] || fail "native local proof was not recorded"
  native_started=1
fi

data_proof_file="$bootstrap_dir/data-local-proof.json"
[[ -f "$data_proof_file" ]] || fail "read-only data fixture proof was not recorded"
grep -F '"service_id": "froglet-install-data"' "$data_proof_file" >/dev/null || \
  fail "data fixture proof omitted the expected service"
grep -F "\"release\": \"$(sed -n 's/.*\"release\": \"\([^\"]*\)\".*/\1/p' "$bootstrap_json" | head -1)\"" \
  "$data_proof_file" >/dev/null || fail "data fixture proof omitted the exact release"
grep -F "\"state_path\": \"$data_dir\"" "$data_proof_file" >/dev/null || \
  fail "data fixture proof omitted the persistent state path"
log "user-supplied read-only data fixture publication and invocation proved"
evidence_dir="$workspace/evidence"
mkdir -p "$evidence_dir"
cp "$data_proof_file" "$evidence_dir/data-local-proof.json"
if [[ -f "$bootstrap_dir/local-proof.json" ]]; then
  cp "$bootstrap_dir/local-proof.json" "$evidence_dir/local-proof.json"
fi
chmod 0600 "$evidence_dir"/*.json
log "copied local proof evidence to $evidence_dir"

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
if [[ "$compose_started" -eq 1 ]]; then
  log "stopping smoke stack"
  docker compose --project-name "$compose_project" -f "$compose_file" \
    down --remove-orphans >/dev/null
  compose_started=0
  printf 'mode=docker\nstatus=stopped\n' >"$workspace/lifecycle-cleanup.txt"
else
  log "stopping native service"
  HOME="$home_dir" \
  FROGLET_BOOTSTRAP_DIR="$bootstrap_dir" \
  FROGLET_DATA_DIR="$data_dir" \
  INSTALL_DIR="$install_dir" \
    bash "$bootstrap_dir/froglet-service.sh" uninstall >/dev/null
  native_started=0
  [[ ! -e "$install_dir/froglet-node" ]] || fail "native uninstall left the installed binary link"
  [[ ! -e "$bootstrap_dir" ]] || fail "native uninstall left the bootstrap directory"
  [[ ! -e "$home_dir/Library/LaunchAgents/$native_service_label.plist" ]] || \
    fail "native uninstall left the launchd definition"
  if [[ "$(uname -s 2>/dev/null || true)" == "Darwin" ]] \
    && launchctl print "gui/$(id -u)/$native_service_label" >/dev/null 2>&1; then
    launchctl bootout "gui/$(id -u)/$native_service_label" >/dev/null 2>&1 || true
    launchctl print "gui/$(id -u)/$native_service_label" >/dev/null 2>&1 \
      && fail "native uninstall left the launchd service loaded"
  fi
  printf 'mode=native\nstatus=uninstalled\n' >"$workspace/lifecycle-cleanup.txt"
  log "native lifecycle cleanup proved"
fi
