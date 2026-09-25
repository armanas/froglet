#!/bin/sh
set -eu

REPO="${FROGLET_INSTALL_REPO:-armanas/froglet}"
RAW_BASE_OVERRIDE="${FROGLET_RAW_BASE:-}"
RAW_BASE="${RAW_BASE_OVERRIDE:-https://raw.githubusercontent.com/$REPO/main}"
DOWNLOAD_BASE_URL="${FROGLET_INSTALL_BASE_URL:-https://github.com/$REPO/releases/download}"
GITHUB_API_BASE_URL="${FROGLET_GITHUB_API_BASE_URL:-https://api.github.com}"
BOOTSTRAP_DIR="${FROGLET_BOOTSTRAP_DIR:-$HOME/.froglet/agent}"
DATA_DIR="${FROGLET_DATA_DIR:-$HOME/.froglet/data}"
BIN_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
INSTALL_VERSION="${VERSION:-${FROGLET_IMAGE_TAG:-}}"
PROVIDER_IMAGE="${FROGLET_PROVIDER_IMAGE:-}"
RUNTIME_IMAGE="${FROGLET_RUNTIME_IMAGE:-}"
DUAL_IMAGE="${FROGLET_DUAL_IMAGE:-}"
MCP_IMAGE="${FROGLET_MCP_IMAGE:-}"
RELEASE_MANIFEST="$BOOTSTRAP_DIR/release-manifest.json"
SERVICE_SCRIPT="$BOOTSTRAP_DIR/froglet-service.sh"
LIFECYCLE_LOCK_DIR="$BOOTSTRAP_DIR.lifecycle.lock"
TRANSIENT_PROOF_MARKER="$DATA_DIR/froglet-install-proof-published"
AGENT_TARGET="${FROGLET_AGENT_TARGET:-claude-code}"
AGENT_PROJECT_DIR="${FROGLET_AGENT_PROJECT_DIR:-$PWD}"
BOOTSTRAP_MODE="${FROGLET_BOOTSTRAP_MODE:-auto}"
PROVIDER_URL="${FROGLET_PROVIDER_URL:-http://127.0.0.1:8080}"
RUNTIME_URL="${FROGLET_RUNTIME_URL:-http://127.0.0.1:8081}"
NETWORK_MODE="${FROGLET_NETWORK_MODE:-clearnet}"
MARKETPLACE_URL="${FROGLET_MARKETPLACE_URL:-https://marketplace.froglet.dev}"
# Reserve the official identity-derived relay endpoint by default.  The core
# keeps this configuration dormant until an exact durable publication grant
# exists, so planning it opens no socket and exposes no service.  Operators
# can explicitly opt out by setting both variables to the empty string; use
# the `-` (not `:-`) expansion so that distinction survives into the approval
# contract.
RELAY_URL="${FROGLET_RELAY_URL-wss://relay.froglet.dev/v1/tunnel}"
RELAY_PUBLIC_SUFFIX="${FROGLET_RELAY_PUBLIC_SUFFIX-relay.froglet.dev}"
RELAY_CONFIGURED=false
# Requester spend policy (buyer side), carried on the single spawn command so
# paid buying needs no post-install config edit. Unset keeps the daemon's
# fail-closed default: every paid deal is refused until a budget exists.
SPEND_BUDGET_MSAT="${FROGLET_REQUESTER_SPEND_BUDGET_MSAT:-}"
MAX_DEAL_MSAT="${FROGLET_REQUESTER_MAX_DEAL_MSAT:-}"
START_STACK="${FROGLET_BOOTSTRAP_START:-1}"
HEALTH_ATTEMPTS="${FROGLET_HEALTH_ATTEMPTS:-60}"
HEALTH_INTERVAL_SECS="${FROGLET_HEALTH_INTERVAL_SECS:-1}"
GH_ATTESTATION_MODE="${FROGLET_GH_ATTESTATION_MODE:-auto}"
COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-${FROGLET_COMPOSE_PROJECT_NAME:-froglet_agent}}"
MCP_DOCKER_NETWORK="${FROGLET_MCP_DOCKER_NETWORK:-${COMPOSE_PROJECT_NAME}_default}"
DATA_FIXTURE_INPUT="${FROGLET_BOOTSTRAP_DATA_FIXTURE:-}"
DATA_SERVICE_ID="${FROGLET_BOOTSTRAP_DATA_SERVICE_ID:-froglet-install-data}"
INSTALL_ACTION="${1:-plan}"
SUPPLIED_APPROVAL_HASH="${2:-${FROGLET_INSTALL_APPROVAL_HASH:-}}"
MANIFEST_SHA256_PIN="${FROGLET_RELEASE_MANIFEST_SHA256:-}"
TRUSTED_MANIFEST_PIN="${FROGLET_TRUSTED_MANIFEST_PIN:-0}"
bootstrap_complete=false
native_activation_attempted=false
existing_native=false
existing_native_fingerprint=missing
docker_activation_attempted=false
install_mode=""
staging_dir=""
plan_tmp_dir=""
approved_installer=""
approved_service_script=""
approved_setup_agent_script=""
approved_setup_payment_script=""
approved_data_fixture=""
transient_proof_root=""
transient_proof_node_bin=""
transient_proof_baseline_feed=""
transient_proof_published=false
transient_proof_cleanup_proved=false
lifecycle_lock_owned=false
agent_config_receipt="$BOOTSTRAP_DIR/agent-config-receipt-$$.json"
LIFECYCLE_LOCK_TOKEN=""
export COMPOSE_PROJECT_NAME

log() {
  printf '%s\n' "$*" >&2
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

cleanup_failed_bootstrap() {
  status=$?
  if [ "$bootstrap_complete" = "true" ]; then
    release_lifecycle_lock
    return "$status"
  fi

  if [ "$transient_proof_published" = "true" ] \
    && [ "$transient_proof_cleanup_proved" != "true" ]; then
    log "bootstrap failed after temporary proof publication; attempting exact unpublish before service teardown"
    if compensate_transient_proof; then
      log "temporary proof compensation restored the active-offer catalog baseline"
    else
      log "warning: temporary proof compensation could not be proved; service teardown will continue and manual cleanup is required for $DATA_SERVICE_ID"
      mkdir -p "$DATA_DIR"
      (
        umask 077
        printf 'service_id=%s\nreason=temporary proof unpublish could not be proved\n' \
          "$DATA_SERVICE_ID" >"$DATA_DIR/froglet-install-compensation-required"
      ) || true
    fi
  fi

  if [ -n "$agent_config_receipt" ] && [ -f "$agent_config_receipt" ] && [ -x "$BIN_DIR/froglet-node" ]; then
    "$BIN_DIR/froglet-node" configure-agent --rollback-receipt "$agent_config_receipt" >&2 || \
      log "warning: agent config changed after setup; leaving the user's edits and backup intact"
  fi

  if [ "$docker_activation_attempted" = "true" \
    ] && [ -n "${compose_file:-}" ] && [ -f "$compose_file" ]; then
    log "bootstrap failed; stopping the partially activated Docker service"
    COMPOSE_PROJECT_NAME="$COMPOSE_PROJECT_NAME" \
      docker_compose -f "$compose_file" down --remove-orphans >/dev/null 2>&1 || \
      log "warning: failed to stop the partially activated Docker service"
  fi
  if [ "$native_activation_attempted" = "true" ] && [ -x "$SERVICE_SCRIPT" ]; then
    log "bootstrap failed; uninstalling the partially activated native service"
    FROGLET_BOOTSTRAP_DIR="$BOOTSTRAP_DIR" \
    FROGLET_DATA_DIR="$DATA_DIR" \
    INSTALL_DIR="$BIN_DIR" \
      bash "$SERVICE_SCRIPT" uninstall >/dev/null 2>&1 || \
      log "warning: failed to uninstall the partially activated native service"
  fi
  if [ -n "$staging_dir" ]; then
    rm -rf "$staging_dir"
  fi
  if [ -n "$plan_tmp_dir" ]; then
    rm -rf "$plan_tmp_dir"
  fi
  if [ -n "$transient_proof_root" ]; then
    rm -rf "$transient_proof_root"
    transient_proof_root=""
  fi
  release_lifecycle_lock
  return "$status"
}

trap cleanup_failed_bootstrap EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

write_lifecycle_lock_owner() {
  chmod 0700 "$LIFECYCLE_LOCK_DIR" || fail "could not secure lifecycle lock directory"
  lifecycle_started="$(date +%s)"
  case "$lifecycle_started" in
    ''|*[!0-9]*) fail "could not determine lifecycle lock timestamp" ;;
  esac
  if ! (
    umask 077
    printf 'pid=%s\nstarted=%s\ntoken=%s\n' \
      "$$" "$lifecycle_started" "$LIFECYCLE_LOCK_TOKEN" \
      >"$LIFECYCLE_LOCK_DIR/owner"
  ); then
    fail "could not write lifecycle lock owner metadata"
  fi
}

release_lifecycle_lock() {
  [ "${lifecycle_lock_owned:-false}" = "true" ] || return 0
  lifecycle_lock_owned=false
  [ -d "$LIFECYCLE_LOCK_DIR" ] || return 0
  lifecycle_owner_token="$(sed -n 's/^token=//p' "$LIFECYCLE_LOCK_DIR/owner" 2>/dev/null || true)"
  [ "$lifecycle_owner_token" = "$LIFECYCLE_LOCK_TOKEN" ] || return 0
  rm -f "$LIFECYCLE_LOCK_DIR/owner"
  rmdir "$LIFECYCLE_LOCK_DIR" 2>/dev/null || true
}

acquire_lifecycle_lock() {
  LIFECYCLE_LOCK_TOKEN="bootstrap-$$-$SUPPLIED_APPROVAL_HASH"
  export FROGLET_LIFECYCLE_LOCK_TOKEN="$LIFECYCLE_LOCK_TOKEN"
  mkdir -p "$(dirname "$BOOTSTRAP_DIR")"
  if mkdir "$LIFECYCLE_LOCK_DIR" 2>/dev/null; then
    lifecycle_lock_owned=true
    write_lifecycle_lock_owner
    return 0
  fi

  lifecycle_owner_file="$LIFECYCLE_LOCK_DIR/owner"
  [ -f "$lifecycle_owner_file" ] || \
    fail "another Froglet install is acquiring the lifecycle lock; retry later"
  lifecycle_owner_pid="$(sed -n 's/^pid=//p' "$lifecycle_owner_file")"
  lifecycle_owner_started="$(sed -n 's/^started=//p' "$lifecycle_owner_file")"
  lifecycle_owner_token="$(sed -n 's/^token=//p' "$lifecycle_owner_file")"
  case "$lifecycle_owner_pid" in
    ''|*[!0-9]*) fail "lifecycle lock has invalid owner metadata; inspect $LIFECYCLE_LOCK_DIR" ;;
  esac
  case "$lifecycle_owner_started" in
    ''|*[!0-9]*) fail "lifecycle lock has invalid timestamp metadata; inspect $LIFECYCLE_LOCK_DIR" ;;
  esac
  if kill -0 "$lifecycle_owner_pid" 2>/dev/null; then
    fail "another Froglet lifecycle mutation is active (pid $lifecycle_owner_pid)"
  fi

  lifecycle_now="$(date +%s)"
  case "$lifecycle_now" in
    ''|*[!0-9]*) fail "could not determine lifecycle lock age" ;;
  esac
  lifecycle_age=$((lifecycle_now - lifecycle_owner_started))
  [ "$lifecycle_age" -ge 900 ] || \
    fail "a recent stale Froglet lifecycle lock exists; retry later or inspect $LIFECYCLE_LOCK_DIR"
  log "reclaiming stale Froglet lifecycle lock from pid $lifecycle_owner_pid"
  lifecycle_stale_dir="$LIFECYCLE_LOCK_DIR.stale.$$"
  mv "$LIFECYCLE_LOCK_DIR" "$lifecycle_stale_dir" 2>/dev/null || \
    fail "another Froglet lifecycle mutation won the stale-lock race"
  lifecycle_moved_owner="$(cat "$lifecycle_stale_dir/owner" 2>/dev/null || true)"
  lifecycle_expected_owner="$(printf 'pid=%s\nstarted=%s\ntoken=%s' \
    "$lifecycle_owner_pid" "$lifecycle_owner_started" "$lifecycle_owner_token")"
  if [ "$lifecycle_moved_owner" != "$lifecycle_expected_owner" ]; then
    mv "$lifecycle_stale_dir" "$LIFECYCLE_LOCK_DIR" 2>/dev/null || true
    fail "lifecycle lock owner changed during stale-lock reclamation"
  fi
  if ! mkdir "$LIFECYCLE_LOCK_DIR" 2>/dev/null; then
    rm -f "$lifecycle_stale_dir/owner"
    rmdir "$lifecycle_stale_dir" 2>/dev/null || true
    fail "another Froglet lifecycle mutation won the lock race"
  fi
  lifecycle_lock_owned=true
  rm -f "$lifecycle_stale_dir/owner"
  rmdir "$lifecycle_stale_dir" 2>/dev/null || \
    fail "stale lifecycle lock contained unexpected files: $lifecycle_stale_dir"
  write_lifecycle_lock_owner
}

curl_https() {
  curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 "$@"
}

download_to_file() {
  url="$1"
  output="$2"
  case "$url" in
    https://*) curl_https "$url" -o "$output" ;;
    file://*)
      [ "$TRUSTED_MANIFEST_PIN" = 1 ] || \
        fail "file:// install inputs require explicit FROGLET_TRUSTED_MANIFEST_PIN=1"
      curl -fsSL "$url" -o "$output"
      ;;
    *) fail "install material URL must use https://: $url" ;;
  esac
}

require_absolute_path() {
  case "$2" in
    /*) ;;
    *) fail "$1 must be an absolute path: $2" ;;
  esac
}

normalize_release_tag() {
  case "$1" in
    v*) printf '%s' "$1" ;;
    *) printf 'v%s' "$1" ;;
  esac
}

validate_release_tag() {
  printf '%s' "$1" | grep -Eq '^v[0-9A-Za-z][0-9A-Za-z.+-]*$' || \
    fail "invalid release tag: $1"
}

resolve_latest_release_tag() {
  resolved_url="$(
    curl_https -o /dev/null -w '%{url_effective}' \
      "https://github.com/$REPO/releases/latest"
  )"
  resolved_url="$(printf '%s' "$resolved_url" | sed 's:/*$::')"
  resolved_tag="${resolved_url##*/}"
  validate_release_tag "$resolved_tag"
  printf '%s' "$resolved_tag"
}

reject_control_chars() {
  name="$1"
  value="$2"
  newline='
'
  case "$value" in
    *"$newline"*)
      fail "$name must not contain control characters"
      ;;
  esac
  if printf '%s' "$value" | grep '[[:cntrl:]]' >/dev/null 2>&1; then
    fail "$name must not contain control characters"
  fi
}

json_escape() {
  reject_control_chars "json value" "$1"
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

yaml_quote() {
  reject_control_chars "$1" "$2"
  printf '"%s"' "$(printf '%s' "$2" | sed 's/\\/\\\\/g; s/"/\\"/g')"
}

validate_relay_url() {
  value="$1"
  [ -n "$value" ] || return 0
  case "$value" in
    wss://*)
      authority="${value#wss://}"
      authority="${authority%%/*}"
      [ -n "$authority" ] || fail "FROGLET_RELAY_URL must include a host"
      case "$authority" in
        *@*) fail "FROGLET_RELAY_URL must not contain credentials" ;;
      esac
      ;;
    ws://*)
      authority="${value#ws://}"
      authority="${authority%%/*}"
      case "$authority" in
        127.0.0.1|localhost|'[::1]') ;;
        127.0.0.1:*|localhost:*|'[::1]':*)
          port="${authority##*:}"
          case "$port" in
            ''|*[!0-9]*) fail "loopback FROGLET_RELAY_URL has an invalid port" ;;
          esac
          ;;
        *) fail "FROGLET_RELAY_URL must use wss:// unless it points to loopback ws://" ;;
      esac
      ;;
    *) fail "FROGLET_RELAY_URL must use wss:// unless it points to loopback ws://" ;;
  esac
}

validate_relay_public_suffix() {
  value="$1"
  [ -z "$value" ] && return 0
  printf '%s' "$value" | grep -Eq \
    '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$' || \
    fail "FROGLET_RELAY_PUBLIC_SUFFIX must be a lowercase DNS suffix"
}

validate_network_mode() {
  case "$NETWORK_MODE" in
    clearnet|tor|dual) ;;
    *) fail "FROGLET_NETWORK_MODE must be clearnet, tor, or dual" ;;
  esac
}

validate_http_url() {
  name="$1"
  value="$2"
  reject_control_chars "$name" "$value"
  case "$value" in
    *'"'*|*'\'*|*'$'*|*'`'*) fail "$name contains an unsupported character" ;;
    http://*) authority="${value#http://}" ;;
    https://*) authority="${value#https://}" ;;
    *) fail "$name must use http:// or https://" ;;
  esac
  authority="${authority%%/*}"
  [ -n "$authority" ] || fail "$name must include a host"
  case "$authority" in
    *@*) fail "$name must not contain credentials" ;;
  esac
}

docker_compose() {
  if docker compose version >/dev/null 2>&1; then
    docker compose "$@"
    return 0
  fi
  if command -v docker-compose >/dev/null 2>&1; then
    docker-compose "$@"
    return 0
  fi
  fail "Docker Compose v2 is required"
}

select_bootstrap_mode() {
  case "$BOOTSTRAP_MODE" in
    native|docker) printf '%s' "$BOOTSTRAP_MODE"; return 0 ;;
    auto) ;;
    *) fail "FROGLET_BOOTSTRAP_MODE must be auto, native, or docker" ;;
  esac

  case "$(uname -s 2>/dev/null || true)" in
    Darwin)
      if command -v launchctl >/dev/null 2>&1; then
        printf 'native'
        return 0
      fi
      ;;
    Linux)
      if command -v systemctl >/dev/null 2>&1; then
        printf 'native'
        return 0
      fi
      ;;
  esac
  printf 'docker'
}

wait_for_url() {
  url="$1"
  label="$2"
  attempts="$HEALTH_ATTEMPTS"
  interval="$HEALTH_INTERVAL_SECS"
  printf '%s' "$attempts" | grep -Eq '^[1-9][0-9]*$' || \
    fail "FROGLET_HEALTH_ATTEMPTS must be a positive integer"
  printf '%s' "$interval" | grep -Eq '^(0|[1-9][0-9]*)([.][0-9]+)?$' || \
    fail "FROGLET_HEALTH_INTERVAL_SECS must be a non-negative number"
  attempt=1
  while [ "$attempt" -le "$attempts" ]; do
    if curl -fsS --max-time 5 "$url" >/dev/null 2>&1; then
      log "$label healthy"
      return 0
    fi
    sleep "$interval"
    attempt=$((attempt + 1))
  done
  fail "$label did not become healthy at $url"
}

manifest_value() {
  local key="$1"
  local manifest="$2"
  local values count
  values="$(
    sed -n 's/^[[:space:]]*"'"$key"'"[[:space:]]*:[[:space:]]*"\([^"]*\)"[,]\{0,1\}[[:space:]]*$/\1/p' \
      "$manifest"
  )"
  count="$(printf '%s\n' "$values" | sed '/^$/d' | wc -l | tr -d ' ')"
  [ "$count" = "1" ] || fail "release manifest must contain exactly one string field: $key"
  printf '%s' "$values"
}

require_source_revision() {
  printf '%s' "$1" | grep -Eq '^[0-9a-f]{40}([0-9a-f]{24})?$' || \
    fail "release manifest source_revision is not a full source digest"
}

require_immutable_image() {
  local name="$1"
  local value="$2"
  printf '%s' "$value" | grep -Eq \
    '^[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[0-9a-f]{64}$' || \
    fail "$name must be an immutable OCI sha256 digest reference"
}

require_sha256() {
  name="$1"
  value="$2"
  printf '%s' "$value" | grep -Eq '^[0-9a-f]{64}$' || \
    fail "$name must be a lowercase SHA-256 digest"
}

file_sha256() {
  path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | awk '{print $1}'
    return 0
  fi
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$path" | awk '{print $1}'
    return 0
  fi
  if command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$path" | sed 's/^.*= //'
    return 0
  fi
  fail "missing required checksum tool: sha256sum, shasum, or openssl"
}

verify_file_sha256() {
  path="$1"
  expected="$2"
  actual="$(file_sha256 "$path")"
  [ "$actual" = "$expected" ] || \
    fail "SHA-256 mismatch for $(basename "$path")"
}

detect_install_platform() {
  os_name="$(uname -s 2>/dev/null || true)"
  arch_name="$(uname -m 2>/dev/null || true)"
  case "$os_name" in
    Linux) platform=linux ;;
    Darwin) platform=darwin ;;
    *) fail "unsupported operating system: $os_name" ;;
  esac
  case "$arch_name" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=arm64 ;;
    *) fail "unsupported architecture: $arch_name" ;;
  esac
  if [ "$platform" = darwin ] && [ "$arch" != arm64 ]; then
    fail "macOS x86_64 is not supported by the binary installer"
  fi
  printf '%s %s\n' "$platform" "$arch"
}

validate_release_metadata() {
  release_json="$1"
  release_tag="$2"
  immutable_values="$(
    sed -n 's/^[[:space:]][[:space:]]"immutable"[[:space:]]*:[[:space:]]*\([a-z][a-z]*\)[,]\{0,1\}[[:space:]]*$/\1/p' \
      "$release_json"
  )"
  immutable_count="$(printf '%s\n' "$immutable_values" | sed '/^$/d' | wc -l | tr -d ' ')"
  [ "$immutable_count" = 1 ] || \
    fail "GitHub release metadata must contain exactly one top-level immutable field"
  [ "$immutable_values" = true ] || \
    fail "GitHub release $release_tag is mutable; refusing it as an install trust root"

  tag_values="$(
    sed -n 's/^[[:space:]][[:space:]]"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)"[,]\{0,1\}[[:space:]]*$/\1/p' \
      "$release_json"
  )"
  tag_count="$(printf '%s\n' "$tag_values" | sed '/^$/d' | wc -l | tr -d ' ')"
  [ "$tag_count" = 1 ] || \
    fail "GitHub release metadata must contain exactly one top-level tag_name field"
  [ "$tag_values" = "$release_tag" ] || \
    fail "GitHub release metadata tag does not match requested release $release_tag"
}

release_asset_sha256() {
  release_json="$1"
  wanted_asset="$2"
  asset_matches="$(
    awk -v wanted="$wanted_asset" '
      /^    \{[[:space:]]*$/ {
        in_asset = 1
        name = ""
        digest = ""
        state = ""
        next
      }
      in_asset && /^      "name"[[:space:]]*:/ {
        value = $0
        sub(/^      "name"[[:space:]]*:[[:space:]]*"/, "", value)
        sub(/"[,]?[[:space:]]*$/, "", value)
        name = value
        next
      }
      in_asset && /^      "digest"[[:space:]]*:/ {
        value = $0
        sub(/^      "digest"[[:space:]]*:[[:space:]]*"/, "", value)
        sub(/"[,]?[[:space:]]*$/, "", value)
        digest = value
        next
      }
      in_asset && /^      "state"[[:space:]]*:/ {
        value = $0
        sub(/^      "state"[[:space:]]*:[[:space:]]*"/, "", value)
        sub(/"[,]?[[:space:]]*$/, "", value)
        state = value
        next
      }
      in_asset && /^    \}[,]?[[:space:]]*$/ {
        if (name == wanted) {
          print "asset|" digest "|" state
        }
        in_asset = 0
      }
    ' "$release_json"
  )"
  asset_count="$(printf '%s\n' "$asset_matches" | sed -n '/^asset|/p' | wc -l | tr -d ' ')"
  [ "$asset_count" = 1 ] || \
    fail "GitHub immutable release must contain exactly one $wanted_asset asset; found $asset_count"
  asset_digest="$(printf '%s\n' "$asset_matches" | sed -n 's/^asset|\([^|]*\)|.*$/\1/p')"
  asset_state="$(printf '%s\n' "$asset_matches" | sed -n 's/^asset|[^|]*|//p')"
  [ "$asset_state" = uploaded ] || \
    fail "GitHub $wanted_asset asset state must be uploaded"
  case "$asset_digest" in
    sha256:*) asset_sha256="${asset_digest#sha256:}" ;;
    *) fail "GitHub $wanted_asset asset digest must use sha256" ;;
  esac
  require_sha256 "GitHub $wanted_asset asset digest" "$asset_sha256"
  printf '%s\n' "$asset_sha256"
}

shell_quote() {
  reject_control_chars "shell value" "$1"
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

validate_agent_target() {
  case "$AGENT_TARGET" in
    claude-code|codex|manual) ;;
    *) fail "FROGLET_AGENT_TARGET must be claude-code, codex, or manual" ;;
  esac
}

validate_payment_profile() {
  PAYMENT_BACKEND="${FROGLET_PAYMENT_BACKEND:-none}"
  PAYMENT_MODE="none"
  PAYMENT_SECRET_SOURCES="none"
  case "$PAYMENT_BACKEND" in
    none) ;;
    lightning)
      PAYMENT_MODE="${FROGLET_LIGHTNING_MODE:-mock}"
      case "$PAYMENT_MODE" in
        mock) PAYMENT_SECRET_SOURCES=none ;;
        lnd_rest)
          PAYMENT_SECRET_SOURCES="FROGLET_LIGHTNING_MACAROON_PATH,FROGLET_LIGHTNING_TLS_CERT_PATH"
          ;;
        phoenixd)
          PAYMENT_SECRET_SOURCES=FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD
          ;;
        *) fail "FROGLET_LIGHTNING_MODE must be mock, lnd_rest, or phoenixd" ;;
      esac
      ;;
    stripe)
      PAYMENT_MODE="${FROGLET_STRIPE_MODE:-direct}"
      PAYMENT_SECRET_SOURCES="FROGLET_STRIPE_SECRET_KEY,FROGLET_STRIPE_WEBHOOK_SECRET"
      ;;
    x402)
      PAYMENT_MODE="${FROGLET_X402_NETWORK:-base}"
      PAYMENT_SECRET_SOURCES=none
      ;;
    *) fail "FROGLET_PAYMENT_BACKEND must be none, lightning, stripe, or x402" ;;
  esac
  reject_control_chars FROGLET_PAYMENT_BACKEND "$PAYMENT_BACKEND"
  reject_control_chars payment_mode "$PAYMENT_MODE"
}

resolve_service_manager() {
  if [ "$install_mode" = docker ]; then
    printf 'docker-compose'
    return 0
  fi
  if [ -n "${FROGLET_SERVICE_MANAGER:-}" ]; then
    case "$FROGLET_SERVICE_MANAGER" in
      systemd|launchd) printf '%s' "$FROGLET_SERVICE_MANAGER" ;;
      *) fail "FROGLET_SERVICE_MANAGER must be systemd or launchd" ;;
    esac
    return 0
  fi
  case "$(uname -s 2>/dev/null || true)" in
    Linux) printf systemd ;;
    Darwin) printf launchd ;;
    *) fail "native lifecycle supports Ubuntu/systemd and macOS/launchd" ;;
  esac
}

build_payment_profile_fingerprint() {
  payment_contract="$plan_tmp_dir/payment-profile.txt"
  umask 077
  {
    printf 'backend=%s\n' "$PAYMENT_BACKEND"
    printf 'mode=%s\n' "$PAYMENT_MODE"
    case "$PAYMENT_BACKEND:$PAYMENT_MODE" in
      lightning:lnd_rest)
        lightning_url="${FROGLET_LIGHTNING_REST_URL:-}"
        macaroon_path="${FROGLET_LIGHTNING_MACAROON_PATH:-}"
        tls_path="${FROGLET_LIGHTNING_TLS_CERT_PATH:-}"
        reject_control_chars FROGLET_LIGHTNING_REST_URL "$lightning_url"
        reject_control_chars FROGLET_LIGHTNING_MACAROON_PATH "$macaroon_path"
        reject_control_chars FROGLET_LIGHTNING_TLS_CERT_PATH "$tls_path"
        [ -n "$lightning_url" ] || fail "FROGLET_LIGHTNING_REST_URL is required for the approved lnd_rest profile"
        [ -f "$macaroon_path" ] || fail "FROGLET_LIGHTNING_MACAROON_PATH must name a readable file before approval"
        if [ -n "$tls_path" ]; then
          [ -f "$tls_path" ] || fail "FROGLET_LIGHTNING_TLS_CERT_PATH must name a readable file before approval"
        fi
        printf 'rest_url=%s\n' "$lightning_url"
        printf 'macaroon_path=%s\n' "$macaroon_path"
        printf 'macaroon_sha256=%s\n' "$(file_sha256 "$macaroon_path")"
        printf 'tls_cert_path=%s\n' "$tls_path"
        if [ -n "$tls_path" ]; then
          printf 'tls_cert_sha256=%s\n' "$(file_sha256 "$tls_path")"
        fi
        printf 'request_timeout_secs=%s\n' "${FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS:-5}"
        ;;
      lightning:phoenixd)
        phoenix_url="${FROGLET_LIGHTNING_PHOENIXD_URL:-}"
        phoenix_password="${FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD:-}"
        reject_control_chars FROGLET_LIGHTNING_PHOENIXD_URL "$phoenix_url"
        reject_control_chars FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD "$phoenix_password"
        [ -n "$phoenix_url" ] || fail "FROGLET_LIGHTNING_PHOENIXD_URL is required for the approved phoenixd profile"
        [ -n "$phoenix_password" ] || fail "FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD is required for the approved phoenixd profile"
        printf 'url=%s\n' "$phoenix_url"
        printf 'password=%s\n' "$phoenix_password"
        printf 'request_timeout_secs=%s\n' "${FROGLET_LIGHTNING_PHOENIXD_REQUEST_TIMEOUT_SECS:-15}"
        printf 'mainnet_confirm=%s\n' "${FROGLET_LIGHTNING_PHOENIXD_MAINNET_CONFIRM:-}"
        ;;
      stripe:*)
        stripe_secret="${FROGLET_STRIPE_SECRET_KEY:-}"
        stripe_webhook="${FROGLET_STRIPE_WEBHOOK_SECRET:-}"
        reject_control_chars FROGLET_STRIPE_SECRET_KEY "$stripe_secret"
        reject_control_chars FROGLET_STRIPE_WEBHOOK_SECRET "$stripe_webhook"
        [ -n "$stripe_secret" ] || fail "FROGLET_STRIPE_SECRET_KEY is required for the approved Stripe profile"
        printf 'secret_key=%s\n' "$stripe_secret"
        printf 'webhook_secret=%s\n' "$stripe_webhook"
        printf 'api_version=%s\n' "${FROGLET_STRIPE_API_VERSION:-2026-04-22.preview}"
        printf 'live_confirm=%s\n' "${FROGLET_STRIPE_LIVE_CONFIRM:-}"
        ;;
      x402:*)
        x402_wallet="${FROGLET_X402_WALLET_ADDRESS:-}"
        x402_facilitator_url="${FROGLET_X402_FACILITATOR_URL:-}"
        reject_control_chars FROGLET_X402_WALLET_ADDRESS "$x402_wallet"
        reject_control_chars FROGLET_X402_FACILITATOR_URL "$x402_facilitator_url"
        [ -n "$x402_wallet" ] || fail "FROGLET_X402_WALLET_ADDRESS is required for the approved x402 profile"
        [ -n "$x402_facilitator_url" ] || fail "FROGLET_X402_FACILITATOR_URL is required for the approved x402 profile"
        printf 'wallet_address=%s\n' "$x402_wallet"
        printf 'network=%s\n' "${FROGLET_X402_NETWORK:-base}"
        printf 'facilitator_url=%s\n' "$x402_facilitator_url"
        ;;
    esac
  } >"$payment_contract"
  PAYMENT_PROFILE_SHA256="$(file_sha256 "$payment_contract")"
  require_sha256 payment_profile_sha256 "$PAYMENT_PROFILE_SHA256"
}

resolve_install_contract() {
  case "$0" in
    /*) SELF_PATH="$0" ;;
    *)
      [ -f "$0" ] || \
        fail "agent-bootstrap.sh must be downloaded to a file before planning; pipe-to-shell is not supported"
      SELF_PATH="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
      ;;
  esac
  [ -f "$SELF_PATH" ] || \
    fail "agent-bootstrap.sh must be downloaded to a file before planning; pipe-to-shell is not supported"
  BOOTSTRAP_SHA256="$(file_sha256 "$SELF_PATH")"
  require_sha256 bootstrap_script_sha256 "$BOOTSTRAP_SHA256"

  if [ -n "$INSTALL_VERSION" ]; then
    RELEASE_TAG="$(normalize_release_tag "$INSTALL_VERSION")"
  else
    RELEASE_TAG="$(resolve_latest_release_tag)"
  fi
  validate_release_tag "$RELEASE_TAG"
  INSTALL_VERSION="$RELEASE_TAG"

  set -- $(detect_install_platform)
  PLAN_PLATFORM="$1"
  PLAN_ARCH="$2"
  plan_tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/froglet-install-plan.XXXXXX")"
  chmod 0700 "$plan_tmp_dir"
  build_payment_profile_fingerprint
  if [ -n "$DATA_FIXTURE_INPUT" ]; then
    [ -f "$DATA_FIXTURE_INPUT" ] || \
      fail "FROGLET_BOOTSTRAP_DATA_FIXTURE is not a regular file: $DATA_FIXTURE_INPUT"
    approved_data_fixture="$plan_tmp_dir/data-fixture"
    cp "$DATA_FIXTURE_INPUT" "$approved_data_fixture"
    chmod 0600 "$approved_data_fixture"
    DATA_FIXTURE_SHA256="$(file_sha256 "$approved_data_fixture")"
    require_sha256 data_fixture_sha256 "$DATA_FIXTURE_SHA256"
  else
    DATA_FIXTURE_SHA256=generated-starter-fixture
  fi
  plan_manifest="$plan_tmp_dir/release-manifest.json"
  plan_release_json="$plan_tmp_dir/github-release.json"
  download_to_file "$DOWNLOAD_BASE_URL/$RELEASE_TAG/release-manifest.json" "$plan_manifest"

  if [ -n "$MANIFEST_SHA256_PIN" ]; then
    [ "$TRUSTED_MANIFEST_PIN" = 1 ] || \
      fail "FROGLET_RELEASE_MANIFEST_SHA256 requires explicit FROGLET_TRUSTED_MANIFEST_PIN=1"
    require_sha256 FROGLET_RELEASE_MANIFEST_SHA256 "$MANIFEST_SHA256_PIN"
    verify_file_sha256 "$plan_manifest" "$MANIFEST_SHA256_PIN"
    PLAN_MANIFEST_SHA256="$MANIFEST_SHA256_PIN"
    RELEASE_TRUST="explicit-trusted-manifest-pin"
  else
    curl_https \
      -H 'Accept: application/vnd.github+json' \
      -H 'X-GitHub-Api-Version: 2026-03-10' \
      "$GITHUB_API_BASE_URL/repos/$REPO/releases/tags/$RELEASE_TAG" \
      -o "$plan_release_json"
    validate_release_metadata "$plan_release_json" "$RELEASE_TAG"
    PLAN_MANIFEST_SHA256="$(release_asset_sha256 "$plan_release_json" release-manifest.json)"
    verify_file_sha256 "$plan_manifest" "$PLAN_MANIFEST_SHA256"
    RELEASE_TRUST=github-immutable-release
  fi

  manifest_schema="$(manifest_value schema "$plan_manifest")"
  manifest_release="$(manifest_value release "$plan_manifest")"
  manifest_repository="$(manifest_value source_repository "$plan_manifest")"
  SOURCE_REVISION="$(manifest_value source_revision "$plan_manifest")"
  source_ref="$(manifest_value source_ref "$plan_manifest")"
  signer_workflow="$(manifest_value attestation_signer_workflow "$plan_manifest")"
  PLAN_BOOTSTRAP_ASSET="$(manifest_value agent_bootstrap_asset "$plan_manifest")"
  manifest_bootstrap_sha256="$(manifest_value agent_bootstrap_sha256 "$plan_manifest")"
  [ "$manifest_schema" = froglet.release-bundle.v1 ] || \
    fail "unsupported release manifest schema: $manifest_schema"
  [ "$manifest_release" = "$RELEASE_TAG" ] || \
    fail "release manifest tag does not match requested release $RELEASE_TAG"
  [ "$manifest_repository" = "$REPO" ] || \
    fail "release manifest repository does not match $REPO"
  require_source_revision "$SOURCE_REVISION"
  [ "$source_ref" = "refs/tags/$RELEASE_TAG" ] || \
    fail "release manifest source_ref does not match $RELEASE_TAG"
  [ "$signer_workflow" = "$REPO/.github/workflows/release.yml" ] || \
    fail "release manifest signer workflow does not match the trusted release workflow"
  [ "$PLAN_BOOTSTRAP_ASSET" = agent-bootstrap.sh ] || \
    fail "release manifest agent_bootstrap_asset must be agent-bootstrap.sh"
  require_sha256 agent_bootstrap_sha256 "$manifest_bootstrap_sha256"
  [ "$BOOTSTRAP_SHA256" = "$manifest_bootstrap_sha256" ] || \
    fail "running bootstrap SHA-256 does not match the verified release manifest"
  if [ "$RELEASE_TRUST" = github-immutable-release ]; then
    api_bootstrap_sha256="$(release_asset_sha256 "$plan_release_json" "$PLAN_BOOTSTRAP_ASSET")"
    [ "$api_bootstrap_sha256" = "$manifest_bootstrap_sha256" ] || \
      fail "GitHub bootstrap asset digest does not match the verified release manifest"
  fi

  binary_asset_key="binary_froglet_node_${PLAN_PLATFORM}_${PLAN_ARCH}_asset"
  binary_digest_key="binary_froglet_node_${PLAN_PLATFORM}_${PLAN_ARCH}_sha256"
  PLAN_BINARY_ASSET="$(manifest_value "$binary_asset_key" "$plan_manifest")"
  PLAN_BINARY_SHA256="$(manifest_value "$binary_digest_key" "$plan_manifest")"
  expected_binary_asset="froglet-node-${RELEASE_TAG}-${PLAN_PLATFORM}-${PLAN_ARCH}.tar.gz"
  [ "$PLAN_BINARY_ASSET" = "$expected_binary_asset" ] || \
    fail "release manifest asset does not match requested platform: $PLAN_BINARY_ASSET"
  require_sha256 "$binary_digest_key" "$PLAN_BINARY_SHA256"
  if [ "$RELEASE_TRUST" = github-immutable-release ]; then
    api_binary_sha256="$(release_asset_sha256 "$plan_release_json" "$PLAN_BINARY_ASSET")"
    [ "$api_binary_sha256" = "$PLAN_BINARY_SHA256" ] || \
      fail "GitHub asset digest for $PLAN_BINARY_ASSET does not match the verified release manifest"
  fi

  manifest_provider_image="$(manifest_value image_provider "$plan_manifest")"
  manifest_runtime_image="$(manifest_value image_runtime "$plan_manifest")"
  manifest_dual_image="$(manifest_value image_dual "$plan_manifest")"
  manifest_mcp_image="$(manifest_value image_mcp "$plan_manifest")"
  PROVIDER_IMAGE="${PROVIDER_IMAGE:-$manifest_provider_image}"
  RUNTIME_IMAGE="${RUNTIME_IMAGE:-$manifest_runtime_image}"
  DUAL_IMAGE="${DUAL_IMAGE:-$manifest_dual_image}"
  MCP_IMAGE="${MCP_IMAGE:-$manifest_mcp_image}"
  require_immutable_image FROGLET_PROVIDER_IMAGE "$PROVIDER_IMAGE"
  require_immutable_image FROGLET_RUNTIME_IMAGE "$RUNTIME_IMAGE"
  require_immutable_image FROGLET_DUAL_IMAGE "$DUAL_IMAGE"
  require_immutable_image FROGLET_MCP_IMAGE "$MCP_IMAGE"

  if [ -n "$RAW_BASE_OVERRIDE" ]; then
    [ "$RELEASE_TRUST" = explicit-trusted-manifest-pin ] || \
      fail "FROGLET_RAW_BASE is allowed only with explicit FROGLET_TRUSTED_MANIFEST_PIN=1"
    APPROVED_RAW_BASE="$RAW_BASE_OVERRIDE"
  else
    APPROVED_RAW_BASE="https://raw.githubusercontent.com/$REPO/$SOURCE_REVISION"
  fi
  reject_control_chars FROGLET_RAW_BASE "$APPROVED_RAW_BASE"
  approved_installer="$plan_tmp_dir/install.sh"
  approved_service_script="$plan_tmp_dir/froglet-service.sh"
  approved_setup_agent_script="$plan_tmp_dir/setup-agent.sh"
  approved_setup_payment_script="$plan_tmp_dir/setup-payment.sh"
  download_to_file "$APPROVED_RAW_BASE/scripts/install.sh" "$approved_installer"
  download_to_file "$APPROVED_RAW_BASE/scripts/froglet-service.sh" "$approved_service_script"
  download_to_file "$APPROVED_RAW_BASE/scripts/setup-agent.sh" "$approved_setup_agent_script"
  download_to_file "$APPROVED_RAW_BASE/scripts/setup-payment.sh" "$approved_setup_payment_script"
  INSTALLER_SHA256="$(file_sha256 "$approved_installer")"
  SERVICE_SCRIPT_SHA256="$(file_sha256 "$approved_service_script")"
  SETUP_AGENT_SHA256="$(file_sha256 "$approved_setup_agent_script")"
  SETUP_PAYMENT_SHA256="$(file_sha256 "$approved_setup_payment_script")"
  require_sha256 install_script_sha256 "$INSTALLER_SHA256"
  require_sha256 service_script_sha256 "$SERVICE_SCRIPT_SHA256"
  require_sha256 setup_agent_script_sha256 "$SETUP_AGENT_SHA256"
  require_sha256 setup_payment_script_sha256 "$SETUP_PAYMENT_SHA256"

  SERVICE_MANAGER="$(resolve_service_manager)"
  case "$SERVICE_MANAGER" in
    systemd)
      SERVICE_MANAGER_PATH="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/froglet.service"
      PROCESS_MANAGER_IMPACT="write user systemd unit; enable and start only when FROGLET_BOOTSTRAP_START=1"
      ;;
    launchd)
      SERVICE_MANAGER_PATH="$HOME/Library/LaunchAgents/dev.froglet.node.plist"
      PROCESS_MANAGER_IMPACT="write user launchd plist; bootstrap and kickstart only when FROGLET_BOOTSTRAP_START=1"
      ;;
    docker-compose)
      SERVICE_MANAGER_PATH="$BOOTSTRAP_DIR/compose.yaml"
      PROCESS_MANAGER_IMPACT="write Docker Compose project; create containers only when FROGLET_BOOTSTRAP_START=1"
      ;;
  esac
  case "$AGENT_TARGET" in
    claude-code) AGENT_CONFIG_PATH="$AGENT_PROJECT_DIR/.mcp.json" ;;
    codex) AGENT_CONFIG_PATH="$AGENT_PROJECT_DIR/.codex/config.toml" ;;
    manual) AGENT_CONFIG_PATH="none" ;;
  esac
  AGENT_CONFIG_SHA256=missing
  if [ "$AGENT_CONFIG_PATH" != none ]; then
    [ ! -L "$AGENT_CONFIG_PATH" ] || fail "agent configuration must not be a symlink"
    if [ -e "$AGENT_CONFIG_PATH" ]; then
      [ -f "$AGENT_CONFIG_PATH" ] || fail "agent configuration must be a regular file"
      AGENT_CONFIG_SHA256="$(file_sha256 "$AGENT_CONFIG_PATH")"
    fi
  fi
  existing_native=false
  existing_native_fingerprint=missing
  if [ -d "$LIFECYCLE_LOCK_DIR" ]; then
    owner_file="$LIFECYCLE_LOCK_DIR/owner"
    if [ ! -f "$owner_file" ] || kill -0 "$(sed -n 's/^pid=//p' "$owner_file")" 2>/dev/null; then
      fail "another Froglet lifecycle mutation is active; retry after its lifecycle lock is released"
    fi
  fi
  if [ "$install_mode" = native ] && { [ -L "$BOOTSTRAP_DIR/current" ] || [ -e "$BOOTSTRAP_DIR/current" ]; }; then
    [ -e "$BOOTSTRAP_DIR/current" ] || fail "existing installation has a broken current link; recover it before setup"
  fi
  if [ "$install_mode" = native ] && [ -e "$BOOTSTRAP_DIR/current" ]; then
    [ -L "$BOOTSTRAP_DIR/current" ] && [ -x "$BOOTSTRAP_DIR/current/froglet-node" ] || fail "existing installation is incomplete; run its status/doctor before setup"
    [ "$(manifest_value release "$BOOTSTRAP_DIR/current/release-manifest.json")" = "$RELEASE_TAG" ] || fail "existing installation uses another release; use froglet-service.sh upgrade first (it preserves identity and rolls back failed health checks), then reconnect this agent"
    [ "$(file_sha256 "$BOOTSTRAP_DIR/current/release-manifest.json")" = "$PLAN_MANIFEST_SHA256" ] || fail "existing release manifest differs from the approved immutable release"
    [ -f "$BOOTSTRAP_DIR/native.env" ] || fail "existing native environment is missing; recover it before reconnecting"
    existing_native=true
    existing_native_fingerprint="$(file_sha256 "$BOOTSTRAP_DIR/native.env")"
    PROCESS_MANAGER_IMPACT="reuse existing installation; no service restart, payment changes, or service definition replacement"
  fi
  if [ "$install_mode" = native ] && [ "$existing_native" != true ]; then
    [ ! -e "$BOOTSTRAP_DIR" ] && [ ! -L "$BOOTSTRAP_DIR" ] || fail "installation destination is already occupied without a complete native installation; preserve it and choose a new bootstrap directory or recover the existing installation"
    [ ! -e "$BIN_DIR/froglet-node" ] && [ ! -L "$BIN_DIR/froglet-node" ] || fail "an unmanaged froglet-node already occupies the installation path; preserve it and select a different INSTALL_DIR"
    [ ! -e "$SERVICE_MANAGER_PATH" ] && [ ! -L "$SERVICE_MANAGER_PATH" ] || fail "a service definition already exists; recover the existing installation before setup"
  fi
  EXECUTE_COMMAND="sh $(shell_quote "$SELF_PATH") execute \"\$FROGLET_INSTALL_APPROVAL_HASH\""

  contract="$plan_tmp_dir/approval-contract.txt"
  {
    printf '%s\n' 'schema=froglet.native-install-approval.v1'
    printf 'repository=%s\n' "$REPO"
    printf 'release_tag=%s\n' "$RELEASE_TAG"
    printf 'release_trust=%s\n' "$RELEASE_TRUST"
    printf 'release_manifest_sha256=%s\n' "$PLAN_MANIFEST_SHA256"
    printf 'source_revision=%s\n' "$SOURCE_REVISION"
    printf 'platform=%s\n' "$PLAN_PLATFORM"
    printf 'architecture=%s\n' "$PLAN_ARCH"
    printf 'binary_asset=%s\n' "$PLAN_BINARY_ASSET"
    printf 'binary_asset_sha256=%s\n' "$PLAN_BINARY_SHA256"
    printf 'bootstrap_path=%s\n' "$SELF_PATH"
    printf 'bootstrap_asset=%s\n' "$PLAN_BOOTSTRAP_ASSET"
    printf 'bootstrap_script_sha256=%s\n' "$BOOTSTRAP_SHA256"
    printf 'script_base=%s\n' "$APPROVED_RAW_BASE"
    printf 'install_script_sha256=%s\n' "$INSTALLER_SHA256"
    printf 'service_script_sha256=%s\n' "$SERVICE_SCRIPT_SHA256"
    printf 'setup_agent_script_sha256=%s\n' "$SETUP_AGENT_SHA256"
    printf 'setup_payment_script_sha256=%s\n' "$SETUP_PAYMENT_SHA256"
    printf 'install_mode=%s\n' "$install_mode"
    printf 'existing_native=%s\n' "$existing_native"
    printf 'existing_native_fingerprint=%s\n' "$existing_native_fingerprint"
    printf 'agent_target=%s\n' "$AGENT_TARGET"
    printf 'agent_project_dir=%s\n' "$AGENT_PROJECT_DIR"
    printf 'payment_backend=%s\n' "$PAYMENT_BACKEND"
    printf 'payment_mode=%s\n' "$PAYMENT_MODE"
    printf 'payment_secret_sources=%s\n' "$PAYMENT_SECRET_SOURCES"
    printf 'payment_profile_sha256=%s\n' "$PAYMENT_PROFILE_SHA256"
    printf 'network_mode=%s\n' "$NETWORK_MODE"
    printf 'marketplace_url=%s\n' "$MARKETPLACE_URL"
    printf 'relay_url=%s\n' "$RELAY_URL"
    printf 'relay_public_suffix=%s\n' "$RELAY_PUBLIC_SUFFIX"
    printf 'relay_configured=%s\n' "$RELAY_CONFIGURED"
    printf '%s\n' 'public_exposure_before_approval=false'
    printf 'provider_url=%s\n' "$PROVIDER_URL"
    printf 'runtime_url=%s\n' "$RUNTIME_URL"
    printf 'requester_spend_budget_msat=%s\n' "$SPEND_BUDGET_MSAT"
    printf 'requester_max_deal_msat=%s\n' "$MAX_DEAL_MSAT"
    printf 'start_stack=%s\n' "$START_STACK"
    printf '%s\n' 'seeded_demo_services=none'
    printf '%s\n' 'seeded_public_services=none'
    printf 'temporary_local_proof_service=%s\n' "$DATA_SERVICE_ID"
    printf 'temporary_proof_state_marker=%s\n' "$TRANSIENT_PROOF_MARKER"
    printf 'data_fixture_path=%s\n' "$DATA_FIXTURE_INPUT"
    printf 'data_fixture_sha256=%s\n' "$DATA_FIXTURE_SHA256"
    printf 'data_service_id=%s\n' "$DATA_SERVICE_ID"
    printf 'compose_project_name=%s\n' "$COMPOSE_PROJECT_NAME"
    printf 'mcp_docker_network=%s\n' "$MCP_DOCKER_NETWORK"
    printf 'provider_image=%s\n' "$PROVIDER_IMAGE"
    printf 'runtime_image=%s\n' "$RUNTIME_IMAGE"
    printf 'dual_image=%s\n' "$DUAL_IMAGE"
    printf 'mcp_image=%s\n' "$MCP_IMAGE"
    printf 'health_attempts=%s\n' "$HEALTH_ATTEMPTS"
    printf 'health_interval_secs=%s\n' "$HEALTH_INTERVAL_SECS"
    printf 'gh_attestation_mode=%s\n' "$GH_ATTESTATION_MODE"
    printf 'bootstrap_dir=%s\n' "$BOOTSTRAP_DIR"
    printf 'lifecycle_lock_dir=%s\n' "$LIFECYCLE_LOCK_DIR"
    printf 'data_dir=%s\n' "$DATA_DIR"
    printf 'binary_path=%s/froglet-node\n' "$BIN_DIR"
    printf 'agent_config_path=%s\n' "$AGENT_CONFIG_PATH"
    printf 'agent_config_sha256=%s\n' "$AGENT_CONFIG_SHA256"
    printf 'service_manager=%s\n' "$SERVICE_MANAGER"
    printf 'service_manager_path=%s\n' "$SERVICE_MANAGER_PATH"
    printf 'process_manager_impact=%s\n' "$PROCESS_MANAGER_IMPACT"
    printf 'execute_command=%s\n' "$EXECUTE_COMMAND"
  } >"$contract"
  APPROVAL_HASH="$(file_sha256 "$contract")"
  require_sha256 install_approval_hash "$APPROVAL_HASH"
  APPROVED_EXECUTE_COMMAND="sh $(shell_quote "$SELF_PATH") execute $(shell_quote "$APPROVAL_HASH")"
}

print_install_plan() {
  cat <<EOF
{
  "status": "approval_required",
  "schema": "froglet.native-install-approval.v1",
  "repository": "$(json_escape "$REPO")",
  "release_tag": "$(json_escape "$RELEASE_TAG")",
  "release_trust": "$(json_escape "$RELEASE_TRUST")",
  "release_manifest_sha256": "$PLAN_MANIFEST_SHA256",
  "source_revision": "$SOURCE_REVISION",
  "platform": "$PLAN_PLATFORM",
  "architecture": "$PLAN_ARCH",
  "binary_asset": "$(json_escape "$PLAN_BINARY_ASSET")",
  "binary_asset_sha256": "$PLAN_BINARY_SHA256",
  "bootstrap_asset": "$PLAN_BOOTSTRAP_ASSET",
  "bootstrap_script_path": "$(json_escape "$SELF_PATH")",
  "bootstrap_script_sha256": "$BOOTSTRAP_SHA256",
  "script_base": "$(json_escape "$APPROVED_RAW_BASE")",
  "install_script_sha256": "$INSTALLER_SHA256",
  "service_script_sha256": "$SERVICE_SCRIPT_SHA256",
  "setup_agent_script_sha256": "$SETUP_AGENT_SHA256",
  "setup_payment_script_sha256": "$SETUP_PAYMENT_SHA256",
  "install_mode": "$install_mode",
  "agent_target": "$AGENT_TARGET",
  "agent_config_before_sha256": "$AGENT_CONFIG_SHA256",
  "agent_configuration_change": {
    "operation": "merge only froglet; preserve unrelated servers, settings, and TOML comments",
    "target": "$(json_escape "$AGENT_TARGET")",
    "native_command": $([ "$install_mode" = native ] && [ "$AGENT_TARGET" != manual ] && printf '"%s"' "$(json_escape "$BIN_DIR/froglet-node")" || printf null),
    "native_args": ["mcp"],
    "implementation_sha256": "$SETUP_AGENT_SHA256",
    "provider_url": "$(json_escape "$PROVIDER_URL")",
    "runtime_url": "$(json_escape "$RUNTIME_URL")",
    "data_dir": "$(json_escape "$DATA_DIR")",
    "provider_token_file": "$(json_escape "$DATA_DIR/runtime/froglet-control.token")",
    "runtime_token_file": "$(json_escape "$DATA_DIR/runtime/auth.token")",
    "backup": "private content-addressed backup before a changed configuration is replaced",
    "drift_policy": "reject intervening edits and request a new plan"
  },
  "agent_config_change": "merge only the Froglet server entry; preserve other settings",
  "agent_project_dir": "$(json_escape "$AGENT_PROJECT_DIR")",
  "network_mode": "$(json_escape "$NETWORK_MODE")",
  "marketplace_url": "$(json_escape "$MARKETPLACE_URL")",
  "relay_url": "$(json_escape "$RELAY_URL")",
  "relay_public_suffix": "$(json_escape "$RELAY_PUBLIC_SUFFIX")",
  "relay_configured": $RELAY_CONFIGURED,
  "relay_transport_activation_granted": false,
  "relay_connected_before_approval": false,
  "provider_url": "$(json_escape "$PROVIDER_URL")",
  "runtime_url": "$(json_escape "$RUNTIME_URL")",
  "public_exposure_before_approval": false,
  "start_stack": $([ "$START_STACK" = 1 ] && printf true || printf false),
  "start_behavior": "$([ "$START_STACK" = 1 ] && printf 'install, start, run non-seeding MCP health, capture the active-offer catalog baseline, publish and invoke one transient local data proof, confirmed-unpublish it, then require the same active offer IDs' || printf 'install service definition without starting or publishing')",
  "health_attempts": $HEALTH_ATTEMPTS,
  "health_interval_secs": $HEALTH_INTERVAL_SECS,
  "github_attestation_mode": "$(json_escape "$GH_ATTESTATION_MODE")",
  "requester_spend_budget_msat": ${SPEND_BUDGET_MSAT:-null},
  "requester_max_deal_msat": ${MAX_DEAL_MSAT:-null},
  "seeded_demo_services": [],
  "seeded_public_services": [],
  "temporary_local_proof_service": "$(json_escape "$DATA_SERVICE_ID")",
  "temporary_proof_state_marker": "$(json_escape "$TRANSIENT_PROOF_MARKER")",
  "payment_backend": "$PAYMENT_BACKEND",
  "payment_mode": "$(json_escape "$PAYMENT_MODE")",
  "payment_secret_sources": "$(json_escape "$PAYMENT_SECRET_SOURCES")",
  "payment_profile_sha256": "$PAYMENT_PROFILE_SHA256",
  "data_fixture": {
    "path": "$(json_escape "$DATA_FIXTURE_INPUT")",
    "sha256": "$(json_escape "$DATA_FIXTURE_SHA256")",
    "service_id": "$(json_escape "$DATA_SERVICE_ID")"
  },
  "images": {
    "provider": "$(json_escape "$PROVIDER_IMAGE")",
    "runtime": "$(json_escape "$RUNTIME_IMAGE")",
    "dual": "$(json_escape "$DUAL_IMAGE")",
    "mcp": "$(json_escape "$MCP_IMAGE")"
  },
  "compose_project_name": "$(json_escape "$COMPOSE_PROJECT_NAME")",
  "mcp_docker_network": "$(json_escape "$MCP_DOCKER_NETWORK")",
  "paths": {
    "binary": "$(json_escape "$BIN_DIR/froglet-node")",
    "bootstrap_dir": "$(json_escape "$BOOTSTRAP_DIR")",
    "data_dir": "$(json_escape "$DATA_DIR")",
    "agent_config": "$(json_escape "$AGENT_CONFIG_PATH")",
    "service_manager_config": "$(json_escape "$SERVICE_MANAGER_PATH")"
  },
  "persistent_paths": [
    "$(json_escape "$BIN_DIR/froglet-node")",
    "$(json_escape "$BOOTSTRAP_DIR")",
    "$(json_escape "$DATA_DIR")",
    "$(json_escape "$AGENT_CONFIG_PATH")",
    "$(json_escape "$SERVICE_MANAGER_PATH")"
  ],
  "lifecycle_lock": {
    "path": "$(json_escape "$LIFECYCLE_LOCK_DIR")",
    "scope": "bootstrap execute and native lifecycle mutations",
    "created_during_plan": false
  },
  "service_manager": "$SERVICE_MANAGER",
  "process_manager_impact": "$(json_escape "$PROCESS_MANAGER_IMPACT")",
  "existing_installation_reused": $existing_native,
  "execute_command_template": "$(json_escape "$EXECUTE_COMMAND")",
  "execute_command": "$(json_escape "$APPROVED_EXECUTE_COMMAND")",
  "install_approval_hash": "$APPROVAL_HASH"
}
EOF
}

validate_service_id() {
  value="$1"
  printf '%s' "$value" | grep -Eq '^[a-z0-9][a-z0-9-]{0,62}$' || \
    fail "FROGLET_BOOTSTRAP_DATA_SERVICE_ID must be 1-63 lowercase letters, digits, or interior hyphens"
  case "$value" in
    *-) fail "FROGLET_BOOTSTRAP_DATA_SERVICE_ID must not end with a hyphen" ;;
  esac
}

configure_payment() {
  local payment_backend="${FROGLET_PAYMENT_BACKEND:-none}"
  local tmp_payment_script="$BOOTSTRAP_DIR/setup-payment.sh"
  case "$payment_backend" in
    none|lightning|stripe|x402) ;;
    *)
      fail "FROGLET_PAYMENT_BACKEND must be none, lightning, stripe, or x402"
      ;;
  esac

  if [ "$payment_backend" != "none" ]; then
    log "Configuring and verifying payment rail: $payment_backend..."
    cp "$approved_setup_payment_script" "$tmp_payment_script"
    verify_file_sha256 "$tmp_payment_script" "$SETUP_PAYMENT_SHA256"
    chmod 0755 "$tmp_payment_script"

    if [ "$payment_backend" = "lightning" ]; then
      lightning_mode="${FROGLET_LIGHTNING_MODE:-mock}"
      case "$lightning_mode" in
        mock|lnd_rest|phoenixd) ;;
        *)
          fail "FROGLET_LIGHTNING_MODE must be mock, lnd_rest, or phoenixd"
          ;;
      esac
      if ! "$tmp_payment_script" "$payment_backend" --mode "$lightning_mode" --out "$BOOTSTRAP_DIR/payment.env"; then
        fail "Payment rail verification failed. Please check your credentials."
      fi
    elif ! "$tmp_payment_script" "$payment_backend" --out "$BOOTSTRAP_DIR/payment.env"; then
      fail "Payment rail verification failed. Please check your credentials."
    fi
  else
    mkdir -p "$BOOTSTRAP_DIR"
    payment_tmp="$BOOTSTRAP_DIR/.payment.env.$$"
    umask 077
    printf '%s\n' "FROGLET_PAYMENT_BACKEND=none" >"$payment_tmp"
    chmod 0600 "$payment_tmp"
    mv -f "$payment_tmp" "$BOOTSTRAP_DIR/payment.env"
  fi
  chmod 0600 "$BOOTSTRAP_DIR/payment.env"
}

require_msat_or_empty() {
  # $1 = env var name, $2 = value. Positive-integer msat or empty.
  case "$2" in
    '') ;;
    0|0[0-9]*|*[!0-9]*) fail "$1 must be a positive integer (millisatoshis), got: $2" ;;
  esac
}

write_compose() {
  mkdir -p "$BOOTSTRAP_DIR" "$DATA_DIR"
  require_msat_or_empty FROGLET_REQUESTER_SPEND_BUDGET_MSAT "$SPEND_BUDGET_MSAT"
  require_msat_or_empty FROGLET_REQUESTER_MAX_DEAL_MSAT "$MAX_DEAL_MSAT"
  runtime_spend_env=""
  if [ -n "$SPEND_BUDGET_MSAT" ]; then
    runtime_spend_env="      FROGLET_REQUESTER_SPEND_BUDGET_MSAT: \"$SPEND_BUDGET_MSAT\"
"
  fi
  if [ -n "$MAX_DEAL_MSAT" ]; then
    runtime_spend_env="${runtime_spend_env}      FROGLET_REQUESTER_MAX_DEAL_MSAT: \"$MAX_DEAL_MSAT\"
"
  fi
  relay_env=""
  if [ -n "$RELAY_URL" ]; then
    relay_env="${relay_env}      FROGLET_RELAY_URL: $(yaml_quote FROGLET_RELAY_URL "$RELAY_URL")
      FROGLET_RELAY_PUBLIC_SUFFIX: $(yaml_quote FROGLET_RELAY_PUBLIC_SUFFIX "$RELAY_PUBLIC_SUFFIX")
"
  fi
  cat >"$BOOTSTRAP_DIR/compose.yaml" <<EOF
services:
  froglet:
    image: $(yaml_quote FROGLET_DUAL_IMAGE "$DUAL_IMAGE")
    env_file:
      - payment.env
    environment:
      FROGLET_DATA_ROOT: /data
      FROGLET_NETWORK_MODE: $(yaml_quote FROGLET_NETWORK_MODE "$NETWORK_MODE")
      FROGLET_PUBLIC_BASE_URL: http://froglet:8080
      FROGLET_RUNTIME_PROVIDER_BASE_URL: http://froglet:8080
      FROGLET_MARKETPLACE_URL: $(yaml_quote FROGLET_MARKETPLACE_URL "$MARKETPLACE_URL")
${runtime_spend_env}${relay_env}      FROGLET_HOST_READABLE_CONTROL_TOKEN: "true"
    ports:
      - "127.0.0.1:8080:8080"
      - "127.0.0.1:8081:8081"
    volumes:
      - $(yaml_quote FROGLET_DATA_DIR "$DATA_DIR:/data")
    healthcheck:
      test: ["CMD-SHELL", "curl -fsS http://127.0.0.1:8080/health >/dev/null && curl -fsS http://127.0.0.1:8081/health >/dev/null"]
      interval: 10s
      timeout: 3s
      retries: 10
      start_period: 5s
    networks:
      default:
        aliases: [provider, runtime]
EOF
}

active_offer_hash_snapshot() {
  feed_path="$1"
  feed_normalized="$(tr -d '[:space:]' <"$feed_path")"
  case "$feed_normalized" in
    \{*\}) ;;
    *) return 1 ;;
  esac
  feed_field='"active_offer_hashes":'
  feed_remainder="$feed_normalized"
  feed_field_count=0
  while :; do
    case "$feed_remainder" in
      *"$feed_field"*)
        feed_field_count=$((feed_field_count + 1))
        feed_remainder="${feed_remainder#*"$feed_field"}"
        ;;
      *) break ;;
    esac
  done
  [ "$feed_field_count" = 1 ] || return 1
  feed_remainder="${feed_normalized#*"$feed_field"}"
  case "$feed_remainder" in
    \[*\]*) ;;
    *) return 1 ;;
  esac
  feed_snapshot_prefix="${feed_remainder%%]*}"
  printf '%s]\n' "$feed_snapshot_prefix"
}

require_active_offer_feed_shape() {
  active_offer_hash_snapshot "$1" >/dev/null
}

active_offer_id_snapshot() {
  feed_path="$1"
  feed_normalized="$(tr -d '[:space:]' <"$feed_path")"
  case "$feed_normalized" in
    *'"has_more":false'*) ;;
    *) return 1 ;;
  esac
  offer_ids="$({
    tr ',' '\n' <"$feed_path" \
      | sed -n 's/.*"offer_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
      | LC_ALL=C sort -u
  })"
  [ -n "$offer_ids" ] || return 1
  printf '%s\n' "$offer_ids"
}

require_same_active_offer_catalog() {
  [ "$(active_offer_id_snapshot "$1")" = "$(active_offer_id_snapshot "$2")" ]
}

mark_transient_proof_cleanup_intent() {
  proof_marker_tmp="$TRANSIENT_PROOF_MARKER.tmp.$$"
  mkdir -p "$DATA_DIR"
  (
    umask 077
    printf 'state=cleanup-required\nservice_id=%s\nrelease=%s\n' \
      "$DATA_SERVICE_ID" "$RELEASE_TAG" \
      >"$proof_marker_tmp"
  ) || return 1
  chmod 0600 "$proof_marker_tmp" || return 1
  mv -f "$proof_marker_tmp" "$TRANSIENT_PROOF_MARKER" || return 1
}

compensate_transient_proof() {
  [ "$transient_proof_published" = "true" ] || return 0
  [ -n "$transient_proof_root" ] && [ -n "$transient_proof_node_bin" ] || return 1
  compensation_request="$transient_proof_root/unpublish-request.json"
  compensation_output="$transient_proof_root/unpublish.json"
  compensation_feed="$transient_proof_root/final-feed.json"
  cat >"$compensation_request" <<EOF
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"froglet","arguments":{"action":"publication_unpublish","service_id":"$DATA_SERVICE_ID","confirm_service_id":"$DATA_SERVICE_ID"}}}
EOF
  if ! FROGLET_DAEMON_URL="$PROVIDER_URL" \
    FROGLET_PROVIDER_CONTROL_TOKEN_PATH="$DATA_DIR/runtime/froglet-control.token" \
    FROGLET_DATA_DIR="$DATA_DIR" \
      "$transient_proof_node_bin" mcp \
        <"$compensation_request" >"$compensation_output"; then
    return 1
  fi
  if ! grep -Eq '"isError"[[:space:]]*:[[:space:]]*false' "$compensation_output"; then
    grep -Eq '"isError"[[:space:]]*:[[:space:]]*true' "$compensation_output" \
      && grep -F 'HTTP 404' "$compensation_output" >/dev/null || return 1
  fi
  if ! curl -fsS --max-time 5 "$PROVIDER_URL/v1/feed?limit=100" >"$compensation_feed"; then
    return 1
  fi
  require_active_offer_feed_shape "$compensation_feed" || return 1
  if [ -n "$transient_proof_baseline_feed" ]; then
    require_same_active_offer_catalog \
      "$transient_proof_baseline_feed" "$compensation_feed" || return 1
  fi
  rm -f "$TRANSIENT_PROOF_MARKER" || return 1
  transient_proof_cleanup_proved=true
  transient_proof_published=false
  transient_proof_baseline_feed=""
  return 0
}

publish_and_prove_data_fixture() {
  node_bin="$1"
  proof_root="$BOOTSTRAP_DIR/proofs/$RELEASE_TAG/native-data"
  transient_proof_root="$proof_root"
  staged_fixture="$proof_root/fixture.json"
  service_manifest="$proof_root/froglet-service.toml"
  publication_output="$proof_root/publication.json"
  invocation_output="$proof_root/invocation.json"
  unpublish_request="$proof_root/unpublish-request.json"
  unpublish_output="$proof_root/unpublish.json"
  final_feed_output="$proof_root/final-feed.json"
  preflight_status_request="$proof_root/preflight-status-request.json"
  preflight_status_output="$proof_root/preflight-status.json"
  preflight_feed_output="$proof_root/preflight-feed.json"
  preflight_offer_ids_output="$proof_root/preflight-offer-ids.txt"
  final_offer_ids_output="$proof_root/final-offer-ids.txt"
  data_proof_tmp="$BOOTSTRAP_DIR/data-local-proof.json.tmp.$$"

  validate_service_id "$DATA_SERVICE_ID"
  transient_proof_node_bin="$node_bin"
  transient_proof_baseline_feed=""
  transient_proof_published=false
  transient_proof_cleanup_proved=false
  rm -rf "$proof_root"
  mkdir -p "$proof_root"
  if [ -e "$TRANSIENT_PROOF_MARKER" ]; then
    marker_service_id="$(sed -n 's/^service_id=//p' "$TRANSIENT_PROOF_MARKER")"
    [ "$marker_service_id" = "$DATA_SERVICE_ID" ] || \
      fail "temporary proof cleanup marker does not match the approved service ID: $TRANSIENT_PROOF_MARKER"
    transient_proof_published=true
    log "recovering a durable temporary-proof cleanup intent before new publication"
    compensate_transient_proof || \
      fail "previous temporary proof cleanup intent could not be resolved; marker preserved at $TRANSIENT_PROOF_MARKER"
  fi
  if ! curl -fsS --max-time 5 "$PROVIDER_URL/v1/feed?limit=100" >"$preflight_feed_output"; then
    fail "could not capture the provider active-offer catalog baseline before temporary proof publication"
  fi
  require_active_offer_feed_shape "$preflight_feed_output" || \
    fail "temporary install proof requires exactly one valid active_offer_hashes feed field"
  active_offer_id_snapshot "$preflight_feed_output" >"$preflight_offer_ids_output" || \
    fail "temporary install proof requires one complete page of active Offer artifacts"
  transient_proof_baseline_feed="$preflight_feed_output"
  cat >"$preflight_status_request" <<EOF
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"froglet","arguments":{"action":"publication_status","service_id":"$DATA_SERVICE_ID"}}}
EOF
  if ! FROGLET_DAEMON_URL="$PROVIDER_URL" \
    FROGLET_PROVIDER_CONTROL_TOKEN_PATH="$DATA_DIR/runtime/froglet-control.token" \
    FROGLET_DATA_DIR="$DATA_DIR" \
      "$node_bin" mcp <"$preflight_status_request" >"$preflight_status_output"; then
    fail "could not prove temporary proof service lifecycle absence"
  fi
  grep -Eq '"isError"[[:space:]]*:[[:space:]]*true' "$preflight_status_output" \
    && grep -F 'HTTP 404' "$preflight_status_output" >/dev/null || \
    fail "temporary proof service ID already exists or lifecycle absence could not be proved: $DATA_SERVICE_ID"

  if [ -n "$DATA_FIXTURE_INPUT" ]; then
    [ -f "$approved_data_fixture" ] || \
      fail "approved data fixture staging disappeared before activation"
    verify_file_sha256 "$approved_data_fixture" "$DATA_FIXTURE_SHA256"
    fixture_source="$DATA_FIXTURE_INPUT"
  else
    fixture_source="generated starter fixture"
  fi

  if [ -n "$DATA_FIXTURE_INPUT" ]; then
    cp "$approved_data_fixture" "$staged_fixture"
  else
    cat >"$staged_fixture" <<'EOF'
[
  {"id":"froglet-proof","status":"ready","source":"generated starter fixture"},
  {"id":"froglet-example","status":"example","source":"generated starter fixture"}
]
EOF
  fi
  chmod 0600 "$staged_fixture"
  staged_fixture_sha256="$(file_sha256 "$staged_fixture")"
  require_sha256 staged_fixture_sha256 "$staged_fixture_sha256"

  cat >"$service_manifest" <<EOF
schema_version = "froglet-service/v4"
service_id = "$DATA_SERVICE_ID"
summary = "Read-only Froglet installation data proof"
runtime = "builtin"
package_kind = "builtin"
entrypoint_kind = "builtin"
contract_version = "froglet.builtin.data_query.json.v1"
publication_state = "active"
verification = { input = { op = "select", collection = "rows", limit = 1 } }

[hosting]
default = "local"

[settlement]
method = "none"

[price]
sats = 0
currency = "sat"

[data]
path = "fixture.json"
format = "json"
EOF
  chmod 0600 "$service_manifest"

  log "Publishing and locally verifying the staged read-only JSON fixture..."
  mark_transient_proof_cleanup_intent || \
    fail "temporary proof cleanup intent could not be recorded before publication"
  transient_proof_published=true
  if ! (
    cd "$proof_root"
    FROGLET_DAEMON_URL="$PROVIDER_URL" \
    FROGLET_PROVIDER_CONTROL_TOKEN_PATH="$DATA_DIR/runtime/froglet-control.token" \
      "$node_bin" publish --host local --json
  ) >"$publication_output"; then
    fail "native data fixture did not pass publication-time local verification"
  fi
  grep -Eq '"local_verification"[[:space:]]*:[[:space:]]*\{' "$publication_output" || \
    fail "native data publication omitted local verification evidence"
  grep -Eq '"publication_revision"[[:space:]]*:[[:space:]]*\{' "$publication_output" || \
    fail "native data publication omitted its signed Publication Revision"

  log "Invoking the staged read-only JSON fixture through the runtime deal path..."
  if ! FROGLET_DAEMON_URL="$PROVIDER_URL" \
    FROGLET_RUNTIME_URL="$RUNTIME_URL" \
    FROGLET_RUNTIME_AUTH_TOKEN_PATH="$DATA_DIR/runtime/auth.token" \
    FROGLET_DATA_DIR="$DATA_DIR" \
      "$node_bin" invoke "$DATA_SERVICE_ID" \
        '{"op":"select","collection":"rows","limit":1}' \
        --json >"$invocation_output"; then
    fail "native data fixture invocation failed"
  fi
  grep -Eq '"service_id"[[:space:]]*:[[:space:]]*"'"$DATA_SERVICE_ID"'"' \
    "$invocation_output" || fail "native data invocation returned the wrong service"
  grep -Eq '"status"[[:space:]]*:[[:space:]]*"(succeeded|completed|done)"' \
    "$invocation_output" || fail "native data invocation did not succeed"
  grep -Eq '"source_kind"[[:space:]]*:[[:space:]]*"json"' \
    "$invocation_output" || fail "native data invocation did not use the JSON builtin"
  grep -Eq '"rows"[[:space:]]*:[[:space:]]*\[' "$invocation_output" || \
    fail "native data invocation returned no rows array"

  log "Confirmed-unpublishing the temporary data proof before service handoff..."
  compensate_transient_proof || \
    fail "temporary data proof exact unpublish and active-offer catalog restoration could not be proved"
  active_offer_id_snapshot "$final_feed_output" >"$final_offer_ids_output" || \
    fail "temporary data proof final active Offer catalog could not be read"
  preflight_offer_ids_sha256="$(file_sha256 "$preflight_offer_ids_output")"
  final_offer_ids_sha256="$(file_sha256 "$final_offer_ids_output")"
  [ "$preflight_offer_ids_sha256" = "$final_offer_ids_sha256" ] || \
    fail "temporary data proof active Offer catalog digest did not return to baseline"

  cat >"$data_proof_tmp" <<EOF
{
  "status": "ok",
  "release": "$(json_escape "$RELEASE_TAG")",
  "state_path": "$(json_escape "$DATA_DIR")",
  "service_id": "$(json_escape "$DATA_SERVICE_ID")",
  "fixture_source": "$(json_escape "$fixture_source")",
  "staged_fixture_path": "$(json_escape "$staged_fixture")",
  "staged_fixture_sha256": "$staged_fixture_sha256",
  "staged_fixture_removed": true,
  "active_offer_catalog_restored": true,
  "preflight_active_offer_ids_sha256": "$preflight_offer_ids_sha256",
  "final_active_offer_ids_sha256": "$final_offer_ids_sha256",
  "preflight_feed": $(cat "$preflight_feed_output"),
  "preflight_lifecycle_status": $(cat "$preflight_status_output"),
  "publication": $(cat "$publication_output"),
  "invocation": $(cat "$invocation_output"),
  "unpublish": $(cat "$unpublish_output"),
  "final_feed": $(cat "$final_feed_output")
}
EOF
  rm -rf "$proof_root"
  [ ! -e "$proof_root" ] || \
    fail "temporary data proof authoring directory could not be removed"
  transient_proof_root=""
  mv -f "$data_proof_tmp" "$BOOTSTRAP_DIR/data-local-proof.json"
  chmod 0600 "$BOOTSTRAP_DIR/data-local-proof.json"
  log "read-only fixture publication/invocation passed and temporary offer was removed"
}

configure_agent() {
  case "$AGENT_TARGET" in
    claude-code|codex|manual) ;;
    openclaw)
      log "OpenClaw currently requires the local OpenClaw plugin folder; bootstrap leaves MCP setup to froglet-mcp."
      printf '%s' ""
      return 0
      ;;
    *)
      fail "FROGLET_AGENT_TARGET must be claude-code, codex, openclaw, or manual"
      ;;
  esac

  if [ "$AGENT_TARGET" = "manual" ]; then
    printf '%s' ""
    return 0
  fi

  tmp_script="$BOOTSTRAP_DIR/setup-agent.sh"
  mkdir -p "$AGENT_PROJECT_DIR"
  if [ "$existing_native" = true ]; then tmp_script="$approved_setup_agent_script"; else cp "$approved_setup_agent_script" "$tmp_script"; fi
  verify_file_sha256 "$tmp_script" "$SETUP_AGENT_SHA256"
  chmod 0755 "$tmp_script"
  if [ "$AGENT_TARGET" = "claude-code" ]; then
    out_path="$AGENT_PROJECT_DIR/.mcp.json"
  else
    out_path="$AGENT_PROJECT_DIR/.codex/config.toml"
  fi
  mcp_mode="docker"
  node_bin=""
  if [ "$install_mode" = "native" ]; then
    mcp_mode="native"
    node_bin="$BIN_DIR/froglet-node"
  fi
  agent_config_receipt="$BOOTSTRAP_DIR/agent-config-receipt-$$.json"
  if ! FROGLET_AGENT_CONFIG_EXPECTED_SHA256="$AGENT_CONFIG_SHA256" \
    FROGLET_AGENT_CONFIG_RECEIPT="$agent_config_receipt" \
    FROGLET_CONFIG_MERGER="$BIN_DIR/froglet-node" \
    FROGLET_PROVIDER_URL="$PROVIDER_URL" \
    FROGLET_RUNTIME_URL="$RUNTIME_URL" \
    FROGLET_DATA_DIR="$DATA_DIR" \
    FROGLET_MCP_MODE="$mcp_mode" \
    FROGLET_NODE_BIN="$node_bin" \
    FROGLET_MCP_IMAGE="$MCP_IMAGE" \
    FROGLET_MCP_DOCKER_NETWORK="$MCP_DOCKER_NETWORK" \
    FROGLET_PROVIDER_AUTH_TOKEN_PATH="$DATA_DIR/runtime/froglet-control.token" \
    FROGLET_RUNTIME_AUTH_TOKEN_PATH="$DATA_DIR/runtime/auth.token" \
      "$tmp_script" --target "$AGENT_TARGET" --out "$out_path" >/dev/null; then
    fail "failed to write $AGENT_TARGET MCP configuration"
  fi
  printf '%s' "$out_path"
}

need_cmd curl
need_cmd bash
need_cmd mktemp
require_absolute_path FROGLET_BOOTSTRAP_DIR "$BOOTSTRAP_DIR"
require_absolute_path FROGLET_DATA_DIR "$DATA_DIR"
require_absolute_path INSTALL_DIR "$BIN_DIR"
require_absolute_path FROGLET_AGENT_PROJECT_DIR "$AGENT_PROJECT_DIR"
reject_control_chars FROGLET_BOOTSTRAP_DIR "$BOOTSTRAP_DIR"
reject_control_chars FROGLET_DATA_DIR "$DATA_DIR"
reject_control_chars INSTALL_DIR "$BIN_DIR"
reject_control_chars FROGLET_AGENT_PROJECT_DIR "$AGENT_PROJECT_DIR"
reject_control_chars FROGLET_BOOTSTRAP_DATA_FIXTURE "$DATA_FIXTURE_INPUT"
reject_control_chars FROGLET_RELAY_URL "$RELAY_URL"
reject_control_chars FROGLET_RELAY_PUBLIC_SUFFIX "$RELAY_PUBLIC_SUFFIX"
reject_control_chars COMPOSE_PROJECT_NAME "$COMPOSE_PROJECT_NAME"
reject_control_chars FROGLET_MCP_DOCKER_NETWORK "$MCP_DOCKER_NETWORK"
validate_service_id "$DATA_SERVICE_ID"
printf '%s' "$COMPOSE_PROJECT_NAME" | grep -Eq '^[a-z0-9][a-z0-9_-]*$' || \
  fail "COMPOSE_PROJECT_NAME must contain only lowercase letters, digits, underscores, and hyphens"
validate_agent_target
validate_payment_profile
validate_network_mode
validate_http_url FROGLET_MARKETPLACE_URL "$MARKETPLACE_URL"
validate_http_url FROGLET_PROVIDER_URL "$PROVIDER_URL"
validate_http_url FROGLET_RUNTIME_URL "$RUNTIME_URL"
require_msat_or_empty FROGLET_REQUESTER_SPEND_BUDGET_MSAT "$SPEND_BUDGET_MSAT"
require_msat_or_empty FROGLET_REQUESTER_MAX_DEAL_MSAT "$MAX_DEAL_MSAT"
validate_relay_url "$RELAY_URL"
validate_relay_public_suffix "$RELAY_PUBLIC_SUFFIX"
[ -z "${FROGLET_RELAY_ENABLED:-}" ] || \
  fail "FROGLET_RELAY_ENABLED is obsolete; configure the dormant relay with FROGLET_RELAY_URL plus FROGLET_RELAY_PUBLIC_SUFFIX"
if [ -n "$RELAY_URL" ] || [ -n "$RELAY_PUBLIC_SUFFIX" ]; then
  [ -n "$RELAY_URL" ] && [ -n "$RELAY_PUBLIC_SUFFIX" ] || \
    fail "FROGLET_RELAY_URL and FROGLET_RELAY_PUBLIC_SUFFIX must be configured together"
  RELAY_CONFIGURED=true
fi
case "$START_STACK" in
  0|1) ;;
  *) fail "FROGLET_BOOTSTRAP_START must be 0 or 1" ;;
esac
printf '%s' "$HEALTH_ATTEMPTS" | grep -Eq '^[1-9][0-9]*$' || \
  fail "FROGLET_HEALTH_ATTEMPTS must be a positive integer"
printf '%s' "$HEALTH_INTERVAL_SECS" | grep -Eq '^(0|[1-9][0-9]*)([.][0-9]+)?$' || \
  fail "FROGLET_HEALTH_INTERVAL_SECS must be a non-negative number"
case "$GH_ATTESTATION_MODE" in
  auto|required|off) ;;
  *) fail "FROGLET_GH_ATTESTATION_MODE must be auto, required, or off" ;;
esac

install_mode="$(select_bootstrap_mode)"
resolve_install_contract
case "$INSTALL_ACTION" in
  plan)
    print_install_plan
    rm -rf "$plan_tmp_dir"
    plan_tmp_dir=""
    bootstrap_complete=true
    exit 0
    ;;
  execute)
    require_sha256 install_approval_hash "$SUPPLIED_APPROVAL_HASH"
    [ "$SUPPLIED_APPROVAL_HASH" = "$APPROVAL_HASH" ] || \
      fail "install approval hash does not match the current immutable plan; plan again"
    ;;
  *)
    fail "usage: agent-bootstrap.sh plan | execute INSTALL_APPROVAL_HASH"
    ;;
esac

# The approval check above is the mutation boundary. Nothing before this point
# creates persistent directories, writes configuration, or changes a service.
acquire_lifecycle_lock
if [ "$existing_native" = true ]; then
  [ "$(file_sha256 "$BOOTSTRAP_DIR/native.env")" = "$existing_native_fingerprint" ] && \
    [ "$(file_sha256 "$BOOTSTRAP_DIR/current/release-manifest.json")" = "$PLAN_MANIFEST_SHA256" ] || fail "existing installation changed after approval; plan again"
elif [ "$install_mode" = native ]; then
  [ ! -e "$BOOTSTRAP_DIR" ] && [ ! -L "$BOOTSTRAP_DIR" ] && \
    [ ! -e "$BIN_DIR/froglet-node" ] && [ ! -L "$BIN_DIR/froglet-node" ] && \
    [ ! -e "$SERVICE_MANAGER_PATH" ] && [ ! -L "$SERVICE_MANAGER_PATH" ] || fail "installation destination changed after approval; preserve it and plan again"
fi
mkdir -p "$BIN_DIR"
log "selected install mode: $install_mode"

installer="$approved_installer"
# Downloads and verification must remain disposable even if setup is interrupted
# before activation. Do not leave a half-installation in the persistent root.
staging_dir="$plan_tmp_dir/staged-install"
staging_manifest="$staging_dir/release-manifest.json"
rm -rf "$staging_dir"
mkdir -p "$staging_dir"
log "Installing froglet-node from a verified Release Bundle and SHA-256 asset digest..."
RAW_BASE="$APPROVED_RAW_BASE"
chmod 0755 "$installer"
if [ "$RELEASE_TRUST" = explicit-trusted-manifest-pin ]; then
  INSTALL_DIR="$staging_dir" \
  VERSION="$INSTALL_VERSION" \
  FROGLET_INSTALL_REPO="$REPO" \
  FROGLET_RELEASE_MANIFEST_SHA256="$PLAN_MANIFEST_SHA256" \
  FROGLET_TRUSTED_MANIFEST_PIN=1 \
  FROGLET_RELEASE_MANIFEST_OUT="$staging_manifest" \
    sh "$installer"
else
  INSTALL_DIR="$staging_dir" \
  VERSION="$INSTALL_VERSION" \
  FROGLET_INSTALL_REPO="$REPO" \
  FROGLET_RELEASE_MANIFEST_OUT="$staging_manifest" \
    sh "$installer"
fi
verify_file_sha256 "$staging_manifest" "$PLAN_MANIFEST_SHA256"

RELEASE_TAG="$(manifest_value release "$staging_manifest")"
SOURCE_REVISION="$(manifest_value source_revision "$staging_manifest")"
require_source_revision "$SOURCE_REVISION"
[ "$RELEASE_TAG" = "$INSTALL_VERSION" ] || fail "installed release drifted after approval"
[ "$SOURCE_REVISION" = "$(manifest_value source_revision "$plan_manifest")" ] || \
  fail "installed source revision drifted after approval"

PROVIDER_IMAGE="${PROVIDER_IMAGE:-$(manifest_value image_provider "$staging_manifest")}"
RUNTIME_IMAGE="${RUNTIME_IMAGE:-$(manifest_value image_runtime "$staging_manifest")}"
DUAL_IMAGE="${DUAL_IMAGE:-$(manifest_value image_dual "$staging_manifest")}"
MCP_IMAGE="${MCP_IMAGE:-$(manifest_value image_mcp "$staging_manifest")}"
require_immutable_image FROGLET_PROVIDER_IMAGE "$PROVIDER_IMAGE"
require_immutable_image FROGLET_RUNTIME_IMAGE "$RUNTIME_IMAGE"
require_immutable_image FROGLET_DUAL_IMAGE "$DUAL_IMAGE"
require_immutable_image FROGLET_MCP_IMAGE "$MCP_IMAGE"

# Payment configuration is part of the activation transaction for both
# native and Docker footprints. The verified snippet is private and becomes
# the single payment source consumed by the selected service manager.
if [ "$install_mode" != native ]; then
  mkdir -p "$BOOTSTRAP_DIR"
  cp "$approved_installer" "$BOOTSTRAP_DIR/install.sh"
  configure_payment
fi

compose_started=false
compose_file=""
service_started=false
local_proof=false
data_proof=false
data_proof_path=""
lifecycle_command=""

if [ "$install_mode" = "native" ] && [ "$existing_native" = true ]; then
  cmp -s "$staging_dir/froglet-node" "$BOOTSTRAP_DIR/current/froglet-node" || fail "existing binary differs from its immutable release; keeping the existing installation unchanged"
  lifecycle_command="$SERVICE_SCRIPT"
  if [ "$START_STACK" = 1 ]; then
    FROGLET_BOOTSTRAP_DIR="$BOOTSTRAP_DIR" FROGLET_DATA_DIR="$DATA_DIR" INSTALL_DIR="$BIN_DIR" \
      FROGLET_PROVIDER_URL="$PROVIDER_URL" FROGLET_RUNTIME_URL="$RUNTIME_URL" \
      bash "$approved_service_script" verify || fail "existing service is not ready; run its doctor/status and restart if stopped; setup has not replaced it"
    service_started=true
    local_proof=true
  fi
  log "Reusing the installed release and persistent state; connecting only this agent."
elif [ "$install_mode" = "native" ]; then
  FROGLET_PROVIDER_URL="$PROVIDER_URL" FROGLET_RUNTIME_URL="$RUNTIME_URL" \
    FROGLET_NETWORK_MODE="$NETWORK_MODE" FROGLET_RELAY_URL="$RELAY_URL" \
    "$staging_dir/froglet-node" doctor --preflight --json >&2 || fail "local ports are unavailable; no service was activated"
  mkdir -p "$BOOTSTRAP_DIR"
  cp "$approved_installer" "$BOOTSTRAP_DIR/install.sh"
  cp "$approved_service_script" "$SERVICE_SCRIPT"
  verify_file_sha256 "$SERVICE_SCRIPT" "$SERVICE_SCRIPT_SHA256"
  chmod 0755 "$SERVICE_SCRIPT"
  lifecycle_command="$SERVICE_SCRIPT"
  native_activation_attempted=true
  configure_payment
  FROGLET_SERVICE_START="$START_STACK" \
  FROGLET_BOOTSTRAP_DIR="$BOOTSTRAP_DIR" \
  FROGLET_DATA_DIR="$DATA_DIR" \
  INSTALL_DIR="$BIN_DIR" \
  FROGLET_PROVIDER_URL="$PROVIDER_URL" \
  FROGLET_RUNTIME_URL="$RUNTIME_URL" \
  FROGLET_PAYMENT_ENV_FILE="$BOOTSTRAP_DIR/payment.env" \
  FROGLET_NETWORK_MODE="$NETWORK_MODE" \
  FROGLET_MARKETPLACE_URL="$MARKETPLACE_URL" \
  FROGLET_REQUESTER_SPEND_BUDGET_MSAT="$SPEND_BUDGET_MSAT" \
  FROGLET_REQUESTER_MAX_DEAL_MSAT="$MAX_DEAL_MSAT" \
  FROGLET_RELAY_URL="$RELAY_URL" \
  FROGLET_RELAY_PUBLIC_SUFFIX="$RELAY_PUBLIC_SUFFIX" \
    bash "$SERVICE_SCRIPT" activate \
      --binary "$staging_dir/froglet-node" \
      --release "$RELEASE_TAG" \
      --manifest "$staging_manifest"
  if [ "$START_STACK" = "1" ]; then
    service_started=true
    local_proof=true
  fi
else
  cp "$staging_dir/froglet-node" "$BIN_DIR/froglet-node"
  chmod 0755 "$BIN_DIR/froglet-node"
  cp "$staging_manifest" "$RELEASE_MANIFEST"
  compose_file="$BOOTSTRAP_DIR/compose.yaml"
  write_compose
  if [ "$START_STACK" = "1" ]; then
    need_cmd docker
    log "Starting the released dual-role fallback image..."
    COMPOSE_PROJECT_NAME="$COMPOSE_PROJECT_NAME" docker_compose -f "$compose_file" up -d
    docker_activation_attempted=true
    compose_started=true
    service_started=true
    wait_for_url "$PROVIDER_URL/health" provider
    wait_for_url "$RUNTIME_URL/health" runtime
    proof_file="$BOOTSTRAP_DIR/local-proof.json"
    proof_request="$BOOTSTRAP_DIR/local-proof-request.json"
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"froglet","arguments":{"action":"status"}}}' >"$proof_request"
    if ! COMPOSE_PROJECT_NAME="$COMPOSE_PROJECT_NAME" \
      docker_compose -f "$compose_file" exec -T \
        -e FROGLET_RELEASE="$RELEASE_TAG" \
        -e FROGLET_DATA_DIR=/data \
        -e FROGLET_PROVIDER_URL=http://127.0.0.1:8080 \
        -e FROGLET_RUNTIME_URL=http://127.0.0.1:8081 \
        froglet froglet-node mcp <"$proof_request" >"$proof_file"; then
      fail "dual-role fallback did not pass the native MCP local proof"
    fi
    rm -f "$proof_request"
    grep -Eq '"native_mcp"[[:space:]]*:[[:space:]]*true' "$proof_file" || \
      fail "dual-role fallback returned an unexpected local proof"
    local_proof=true
  fi
fi
if [ "$START_STACK" = "1" ] && [ "$existing_native" != true ]; then
  publish_and_prove_data_fixture "$BIN_DIR/froglet-node"
  data_proof=true
  data_proof_path="$BOOTSTRAP_DIR/data-local-proof.json"
fi
rm -rf "$staging_dir"

mcp_config_path="$(configure_agent)"

if [ "$install_mode" = "native" ]; then
  next_mcp_actions='["status", "prepare_service", "doctor", "check_updates", "open_status", "invoke_service", "marketplace_publish", "publication_status", "publication_logs", "publication_pause", "publication_resume", "publication_rollback", "publication_unpublish"]'
  next_instruction="The native service and dependency-free publication-capable MCP bridge are installed. Check the stage fields for evidence from this attempt. A command-line MCP probe does not prove agent attachment. Reconnecting an existing installation does not repeat the temporary publication proof. marketplace_publish first returns a non-mutating consent plan; call it again with the exact consent_hash only after user approval. Publication lifecycle actions operate the exact local service; rollback requires the immutable revision hash and unpublish requires confirm_service_id exactly matching service_id. Restart the agent only if it does not hot-reload the project MCP config."
else
  next_mcp_actions='["status", "publish_artifact", "invoke_service"]'
  next_instruction="The dual-role Docker fallback and MCP config are installed. Restart the agent only if it does not hot-reload the project MCP config."
fi

cat <<EOF
{
  "status": "ok",
  "bootstrap_dir": "$(json_escape "$BOOTSTRAP_DIR")",
  "data_dir": "$(json_escape "$DATA_DIR")",
  "froglet_node": "$(json_escape "$BIN_DIR/froglet-node")",
  "release": "$(json_escape "$RELEASE_TAG")",
  "source_revision": "$(json_escape "$SOURCE_REVISION")",
  "provider_image": "$(json_escape "$PROVIDER_IMAGE")",
  "runtime_image": "$(json_escape "$RUNTIME_IMAGE")",
  "dual_image": "$(json_escape "$DUAL_IMAGE")",
  "mcp_image": "$(json_escape "$MCP_IMAGE")",
  "install_mode": "$(json_escape "$install_mode")",
  "lifecycle_command": "$(json_escape "$lifecycle_command")",
  "provider_url": "$(json_escape "$PROVIDER_URL")",
  "runtime_url": "$(json_escape "$RUNTIME_URL")",
  "payment_backend": "$(json_escape "${FROGLET_PAYMENT_BACKEND:-none}")",
  "network_mode": "$(json_escape "$NETWORK_MODE")",
  "marketplace_url": "$(json_escape "$MARKETPLACE_URL")",
  "relay_url": "$(json_escape "$RELAY_URL")",
  "relay_public_suffix": "$(json_escape "$RELAY_PUBLIC_SUFFIX")",
  "relay_configured": $RELAY_CONFIGURED,
  "relay_connected": false,
  "requester_spend_budget_msat": ${SPEND_BUDGET_MSAT:-null},
  "requester_max_deal_msat": ${MAX_DEAL_MSAT:-null},
  "compose_file": "$(json_escape "$compose_file")",
  "compose_project_name": "$(json_escape "$COMPOSE_PROJECT_NAME")",
  "compose_started": $compose_started,
  "service_started": $service_started,
  "local_proof": $local_proof,
  "agent_connected": null,
  "existing_installation_reused": $existing_native,
  "stages": {"downloaded": true, "installed": true, "running": $service_started, "agent_configured": $([ "$AGENT_TARGET" != manual ] && printf true || printf false), "agent_connected": null, "local_execution_verified": $data_proof},
  "agent_connection_next_action": "Restart the agent if needed, then call its Froglet status tool; a shell probe does not prove agent attachment",
  "data_proof": $data_proof,
  "data_proof_path": "$(json_escape "$data_proof_path")",
  "data_service_id": "$(json_escape "$DATA_SERVICE_ID")",
  "agent_target": "$(json_escape "$AGENT_TARGET")",
  "mcp_docker_network": "$(json_escape "$MCP_DOCKER_NETWORK")",
  "mcp_config_path": "$(json_escape "$mcp_config_path")",
  "next_mcp_actions": $next_mcp_actions,
  "next_instruction": "$(json_escape "$next_instruction")"
}
EOF
rm -rf "$plan_tmp_dir"
plan_tmp_dir=""
bootstrap_complete=true
