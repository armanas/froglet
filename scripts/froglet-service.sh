#!/usr/bin/env bash
set -euo pipefail

# Native Froglet lifecycle adapter. The node binary is versioned separately
# from its persistent state, so upgrades can switch one symlink and roll back
# without moving identity, databases, or runtime tokens.

REPO="${FROGLET_INSTALL_REPO:-armanas/froglet}"
FROGLET_HOME="${FROGLET_HOME:-$HOME/.froglet}"
BOOTSTRAP_DIR="${FROGLET_BOOTSTRAP_DIR:-$FROGLET_HOME/agent}"
DATA_DIR="${FROGLET_DATA_DIR:-$FROGLET_HOME/data}"
DATA_DIR_WAS_SET="${FROGLET_DATA_DIR+x}"
BIN_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
RELEASES_DIR="$BOOTSTRAP_DIR/releases"
CURRENT_LINK="$BOOTSTRAP_DIR/current"
PREVIOUS_LINK="$BOOTSTRAP_DIR/previous"
ENV_FILE="$BOOTSTRAP_DIR/native.env"
PAYMENT_ENV_FILE="$BOOTSTRAP_DIR/payment.env"
LAUNCHER="$BOOTSTRAP_DIR/run-native.sh"
RELEASE_FILE="$BOOTSTRAP_DIR/current-release"
PROOF_FILE="$BOOTSTRAP_DIR/local-proof.json"
INSTALLER="${FROGLET_INSTALLER_PATH:-$BOOTSTRAP_DIR/install.sh}"
PROVIDER_URL="${FROGLET_PROVIDER_URL:-http://127.0.0.1:8080}"
RUNTIME_URL="${FROGLET_RUNTIME_URL:-http://127.0.0.1:8081}"
PROVIDER_URL_WAS_SET="${FROGLET_PROVIDER_URL+x}"
RUNTIME_URL_WAS_SET="${FROGLET_RUNTIME_URL+x}"
PAYMENT_ENV_INPUT="${FROGLET_PAYMENT_ENV_FILE:-}"
NETWORK_MODE_WAS_SET="${FROGLET_NETWORK_MODE+x}"
MARKETPLACE_URL_WAS_SET="${FROGLET_MARKETPLACE_URL+x}"
SPEND_BUDGET_WAS_SET="${FROGLET_REQUESTER_SPEND_BUDGET_MSAT+x}"
MAX_DEAL_WAS_SET="${FROGLET_REQUESTER_MAX_DEAL_MSAT+x}"
RELAY_URL_WAS_SET="${FROGLET_RELAY_URL+x}"
RELAY_PUBLIC_SUFFIX_WAS_SET="${FROGLET_RELAY_PUBLIC_SUFFIX+x}"
NETWORK_MODE="${FROGLET_NETWORK_MODE:-clearnet}"
MARKETPLACE_URL="${FROGLET_MARKETPLACE_URL:-https://marketplace.froglet.dev}"
SPEND_BUDGET_MSAT="${FROGLET_REQUESTER_SPEND_BUDGET_MSAT:-}"
MAX_DEAL_MSAT="${FROGLET_REQUESTER_MAX_DEAL_MSAT:-}"
RELAY_URL="${FROGLET_RELAY_URL:-}"
RELAY_PUBLIC_SUFFIX="${FROGLET_RELAY_PUBLIC_SUFFIX:-}"
START_SERVICE="${FROGLET_SERVICE_START:-1}"
HEALTH_ATTEMPTS="${FROGLET_HEALTH_ATTEMPTS:-60}"
HEALTH_INTERVAL="${FROGLET_HEALTH_INTERVAL_SECS:-1}"
SERVICE_LABEL="dev.froglet.node"
LIFECYCLE_LOCK_DIR="$BOOTSTRAP_DIR.lifecycle.lock"
LIFECYCLE_LOCK_TOKEN="${FROGLET_LIFECYCLE_LOCK_TOKEN:-service-$$}"
lifecycle_lock_owned=false

log() {
  printf '[froglet-service] %s\n' "$*" >&2
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage:
  froglet-service.sh activate --binary PATH --release TAG --manifest PATH
  froglet-service.sh restart
  froglet-service.sh status
  froglet-service.sh verify
  froglet-service.sh upgrade [TAG]
  froglet-service.sh rollback
  froglet-service.sh uninstall [--purge-data]

Environment overrides:
  FROGLET_SERVICE_MANAGER=systemd|launchd
  FROGLET_SERVICE_START=0       write installation without starting it
  FROGLET_HEALTH_ATTEMPTS=N     health/proof retry count
  FROGLET_PAYMENT_ENV_FILE=PATH verified payment env to install on activation
EOF
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

write_lifecycle_lock_owner() {
  local started
  chmod 0700 "$LIFECYCLE_LOCK_DIR" || fail "could not secure lifecycle lock directory"
  started="$(date +%s)"
  [[ "$started" =~ ^[0-9]+$ ]] || fail "could not determine lifecycle lock timestamp"
  if ! (
    umask 077
    printf 'pid=%s\nstarted=%s\ntoken=%s\n' \
      "$$" "$started" "$LIFECYCLE_LOCK_TOKEN" \
      >"$LIFECYCLE_LOCK_DIR/owner"
  ); then
    fail "could not write lifecycle lock owner metadata"
  fi
}

release_lifecycle_lock() {
  [[ "$lifecycle_lock_owned" == "true" ]] || return 0
  lifecycle_lock_owned=false
  [[ -d "$LIFECYCLE_LOCK_DIR" ]] || return 0
  local owner_token
  owner_token="$(sed -n 's/^token=//p' "$LIFECYCLE_LOCK_DIR/owner" 2>/dev/null || true)"
  [[ "$owner_token" == "$LIFECYCLE_LOCK_TOKEN" ]] || return 0
  rm -f "$LIFECYCLE_LOCK_DIR/owner"
  rmdir "$LIFECYCLE_LOCK_DIR" 2>/dev/null || true
}

acquire_lifecycle_lock() {
  local owner_file owner_pid owner_started owner_token now age
  mkdir -p "$BOOTSTRAP_DIR"
  if mkdir "$LIFECYCLE_LOCK_DIR" 2>/dev/null; then
    lifecycle_lock_owned=true
    write_lifecycle_lock_owner
    return 0
  fi

  owner_file="$LIFECYCLE_LOCK_DIR/owner"
  [[ -f "$owner_file" ]] || \
    fail "another Froglet install is acquiring the lifecycle lock; retry later"
  owner_pid="$(sed -n 's/^pid=//p' "$owner_file")"
  owner_started="$(sed -n 's/^started=//p' "$owner_file")"
  owner_token="$(sed -n 's/^token=//p' "$owner_file")"

  # A bootstrap owns the outer transaction and invokes this script as its
  # direct child. Re-enter only for that exact parent/token pair.
  if [[ -n "${FROGLET_LIFECYCLE_LOCK_TOKEN:-}" \
    && "$owner_pid" == "$PPID" \
    && "$owner_token" == "$FROGLET_LIFECYCLE_LOCK_TOKEN" ]]; then
    return 0
  fi

  [[ "$owner_pid" =~ ^[0-9]+$ ]] || \
    fail "lifecycle lock has invalid owner metadata; inspect $LIFECYCLE_LOCK_DIR"
  [[ "$owner_started" =~ ^[0-9]+$ ]] || \
    fail "lifecycle lock has invalid timestamp metadata; inspect $LIFECYCLE_LOCK_DIR"
  if kill -0 "$owner_pid" 2>/dev/null; then
    fail "another Froglet lifecycle mutation is active (pid $owner_pid)"
  fi

  now="$(date +%s)"
  [[ "$now" =~ ^[0-9]+$ ]] || fail "could not determine lifecycle lock age"
  age=$((now - owner_started))
  ((age >= 900)) || \
    fail "a recent stale Froglet lifecycle lock exists; retry later or inspect $LIFECYCLE_LOCK_DIR"
  log "reclaiming stale Froglet lifecycle lock from pid $owner_pid"
  local stale_dir moved_owner expected_owner
  stale_dir="$LIFECYCLE_LOCK_DIR.stale.$$"
  mv "$LIFECYCLE_LOCK_DIR" "$stale_dir" 2>/dev/null || \
    fail "another Froglet lifecycle mutation won the stale-lock race"
  moved_owner="$(cat "$stale_dir/owner" 2>/dev/null || true)"
  expected_owner="$(printf 'pid=%s\nstarted=%s\ntoken=%s' \
    "$owner_pid" "$owner_started" "$owner_token")"
  if [[ "$moved_owner" != "$expected_owner" ]]; then
    mv "$stale_dir" "$LIFECYCLE_LOCK_DIR" 2>/dev/null || true
    fail "lifecycle lock owner changed during stale-lock reclamation"
  fi
  if ! mkdir "$LIFECYCLE_LOCK_DIR" 2>/dev/null; then
    rm -f "$stale_dir/owner"
    rmdir "$stale_dir" 2>/dev/null || true
    fail "another Froglet lifecycle mutation won the lock race"
  fi
  lifecycle_lock_owned=true
  rm -f "$stale_dir/owner"
  rmdir "$stale_dir" 2>/dev/null || \
    fail "stale lifecycle lock contained unexpected files: $stale_dir"
  write_lifecycle_lock_owner
}

trap release_lifecycle_lock EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

require_absolute_path() {
  case "$2" in
    /*) ;;
    *) fail "$1 must be an absolute path: $2" ;;
  esac
}

reject_control_chars() {
  local name="$1"
  local value="$2"
  local newline=$'\n'
  case "$value" in
    *"$newline"*) fail "$name must not contain control characters" ;;
  esac
  if LC_ALL=C printf '%s' "$value" | grep '[[:cntrl:]]' >/dev/null 2>&1; then
    fail "$name must not contain control characters"
  fi
}

shell_quote_value() {
  local name="$1"
  local value="$2"
  reject_control_chars "$name" "$value"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//\$/\\\$}"
  value="${value//\`/\\\`}"
  printf '"%s"' "$value"
}

validate_relay_url() {
  local value="$1"
  local authority port
  [[ -n "$value" ]] || return 0
  case "$value" in
    *'"'*|*'\'*|*'$'*|*'`'*) fail "FROGLET_RELAY_URL contains an unsupported character" ;;
  esac
  case "$value" in
    wss://*)
      authority="${value#wss://}"
      authority="${authority%%/*}"
      [[ -n "$authority" ]] || fail "FROGLET_RELAY_URL must include a host"
      [[ "$authority" != *@* ]] || fail "FROGLET_RELAY_URL must not contain credentials"
      ;;
    ws://*)
      authority="${value#ws://}"
      authority="${authority%%/*}"
      case "$authority" in
        127.0.0.1|localhost|'[::1]') ;;
        127.0.0.1:*|localhost:*|'[::1]':*)
          port="${authority##*:}"
          [[ -n "$port" && "$port" != *[!0-9]* ]] || \
            fail "loopback FROGLET_RELAY_URL has an invalid port"
          ;;
        *) fail "FROGLET_RELAY_URL must use wss:// unless it points to loopback ws://" ;;
      esac
      ;;
    *) fail "FROGLET_RELAY_URL must use wss:// unless it points to loopback ws://" ;;
  esac
}

validate_relay_public_suffix() {
  local value="$1"
  [[ -z "$value" ]] && return 0
  [[ "$value" =~ ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$ ]] || \
    fail "FROGLET_RELAY_PUBLIC_SUFFIX must be a lowercase DNS suffix"
}

validate_network_mode() {
  case "$NETWORK_MODE" in
    clearnet|tor|dual) ;;
    *) fail "FROGLET_NETWORK_MODE must be clearnet, tor, or dual" ;;
  esac
}

validate_http_url() {
  local name="$1"
  local value="$2"
  local authority
  reject_control_chars "$name" "$value"
  case "$value" in
    *'"'*|*'\'*|*'$'*|*'`'*) fail "$name contains an unsupported character" ;;
    http://*) authority="${value#http://}" ;;
    https://*) authority="${value#https://}" ;;
    *) fail "$name must use http:// or https://" ;;
  esac
  authority="${authority%%/*}"
  [[ -n "$authority" ]] || fail "$name must include a host"
  [[ "$authority" != *@* ]] || fail "$name must not contain credentials"
}

native_listener() {
  local name="$1" value="$2" port host
  case "$value" in
    http://127.0.0.1:*) host=127.0.0.1; port="${value#http://127.0.0.1:}" ;;
    http://\[::1\]:*) host='[::1]'; port="${value#http://\[::1\]:}" ;;
    *) fail "$name must be an http:// loopback origin with an explicit port" ;;
  esac
  [[ "$port" =~ ^[0-9]+$ ]] && (( 10#$port > 0 && 10#$port < 65536 )) || fail "$name has an invalid port"
  printf '%s:%s' "$host" "$port"
}

require_msat_or_empty() {
  local name="$1"
  local value="$2"
  case "$value" in
    '') ;;
    0|0[0-9]*|*[!0-9]*) fail "$name must be a positive integer (millisatoshis)" ;;
  esac
}

persisted_value() {
  local name="$1"
  local line
  line="$(grep -E -m 1 "^${name}=\"[^\"\\\\]*\"$" "$ENV_FILE" || true)"
  [[ -n "$line" ]] || return 1
  line="${line#*=\"}"
  printf '%s' "${line%\"}"
}

load_persisted_config() {
  local value
  [[ -f "$ENV_FILE" ]] || return 0
  if [[ -z "$DATA_DIR_WAS_SET" ]] && value="$(persisted_value FROGLET_DATA_DIR)"; then
    DATA_DIR="$value"
  fi
  if [[ -z "$PROVIDER_URL_WAS_SET" ]] && value="$(persisted_value FROGLET_PUBLIC_BASE_URL)"; then
    PROVIDER_URL="$value"
  fi
  if [[ -z "$RUNTIME_URL_WAS_SET" ]] && value="$(persisted_value FROGLET_RUNTIME_LISTEN_ADDR)"; then
    RUNTIME_URL="http://$value"
  fi
  if [[ -z "$NETWORK_MODE_WAS_SET" ]] && value="$(persisted_value FROGLET_NETWORK_MODE)"; then
    NETWORK_MODE="$value"
  fi
  if [[ -z "$MARKETPLACE_URL_WAS_SET" ]] && value="$(persisted_value FROGLET_MARKETPLACE_URL)"; then
    MARKETPLACE_URL="$value"
  fi
  if [[ -z "$SPEND_BUDGET_WAS_SET" ]] && value="$(persisted_value FROGLET_REQUESTER_SPEND_BUDGET_MSAT)"; then
    SPEND_BUDGET_MSAT="$value"
  fi
  if [[ -z "$MAX_DEAL_WAS_SET" ]] && value="$(persisted_value FROGLET_REQUESTER_MAX_DEAL_MSAT)"; then
    MAX_DEAL_MSAT="$value"
  fi
  if [[ -z "$RELAY_URL_WAS_SET" ]]; then
    if value="$(persisted_value FROGLET_RELAY_URL)"; then
      RELAY_URL="$value"
    fi
  fi
  if [[ -z "$RELAY_PUBLIC_SUFFIX_WAS_SET" ]]; then
    if value="$(persisted_value FROGLET_RELAY_PUBLIC_SUFFIX)"; then
      RELAY_PUBLIC_SUFFIX="$value"
    fi
  fi
}

payment_variable_allowed() {
  case "$1" in
    FROGLET_PAYMENT_BACKEND|FROGLET_LIGHTNING_MODE|FROGLET_LIGHTNING_REST_URL|\
    FROGLET_LIGHTNING_MACAROON_PATH|FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS|\
    FROGLET_LIGHTNING_TLS_CERT_PATH|FROGLET_LIGHTNING_PHOENIXD_URL|\
    FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD|\
    FROGLET_LIGHTNING_PHOENIXD_REQUEST_TIMEOUT_SECS|\
    FROGLET_LIGHTNING_PHOENIXD_MAINNET_CONFIRM|FROGLET_STRIPE_SECRET_KEY|\
    FROGLET_STRIPE_API_VERSION|FROGLET_STRIPE_WEBHOOK_SECRET|\
    FROGLET_X402_WALLET_ADDRESS|FROGLET_X402_NETWORK|\
    FROGLET_X402_FACILITATOR_URL) return 0 ;;
    *) return 1 ;;
  esac
}

validate_escaped_shell_word() {
  local name="$1"
  local value="$2"
  local character
  [[ -n "$value" ]] || fail "$name must not be empty"
  while [[ -n "$value" ]]; do
    character="${value:0:1}"
    if [[ "$character" == '\' ]]; then
      [[ ${#value} -ge 2 ]] || fail "$name has an incomplete escape"
      value="${value:2}"
      continue
    fi
    case "$character" in
      [A-Za-z0-9_.,/:@%+=-]|'#') value="${value:1}" ;;
      *) fail "$name contains an unescaped shell metacharacter" ;;
    esac
  done
}

validate_payment_environment() {
  local path="$1"
  local line name value seen_names backend mode permissions
  [[ -f "$path" && ! -L "$path" ]] || fail "payment environment must be a regular file"
  [[ -O "$path" ]] || fail "payment environment must be owned by the current user"
  permissions="$(stat -f '%Lp' "$path" 2>/dev/null || stat -c '%a' "$path" 2>/dev/null)" || \
    fail "could not inspect payment environment permissions"
  [[ "$permissions" =~ ^[0-7]{3,4}$ ]] || fail "could not validate payment environment permissions"
  (( (8#$permissions & 077) == 0 )) || fail "payment environment must not be readable by group or other users"
  backend=""
  mode=""
  seen_names=$'\n'
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -n "$line" ]] || continue
    reject_control_chars "payment environment line" "$line"
    [[ "$line" == *=* ]] || fail "payment environment contains a malformed line"
    name="${line%%=*}"
    value="${line#*=}"
    [[ "$name" =~ ^[A-Z_][A-Z0-9_]*$ ]] || fail "payment environment contains an invalid variable name"
    payment_variable_allowed "$name" || fail "payment environment contains unsupported variable: $name"
    [[ "$seen_names" != *$'\n'"$name"$'\n'* ]] || fail "payment environment repeats variable: $name"
    seen_names+="$name"$'\n'
    validate_escaped_shell_word "$name" "$value"
    case "$name" in
      FROGLET_PAYMENT_BACKEND) backend="$value" ;;
      FROGLET_LIGHTNING_MODE) mode="$value" ;;
    esac
  done <"$path"
  case "$backend" in
    none)
      [[ "$seen_names" == $'\nFROGLET_PAYMENT_BACKEND\n' ]] || \
        fail "none payment environment must not contain rail credentials"
      ;;
    lightning)
      case "$mode" in
        mock) ;;
        lnd_rest)
          [[ "$seen_names" == *$'\nFROGLET_LIGHTNING_REST_URL\n'* ]] || \
            fail "lnd_rest payment environment is missing FROGLET_LIGHTNING_REST_URL"
          [[ "$seen_names" == *$'\nFROGLET_LIGHTNING_MACAROON_PATH\n'* ]] || \
            fail "lnd_rest payment environment is missing FROGLET_LIGHTNING_MACAROON_PATH"
          ;;
        phoenixd)
          [[ "$seen_names" == *$'\nFROGLET_LIGHTNING_PHOENIXD_URL\n'* ]] || \
            fail "phoenixd payment environment is missing FROGLET_LIGHTNING_PHOENIXD_URL"
          [[ "$seen_names" == *$'\nFROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD\n'* ]] || \
            fail "phoenixd payment environment is missing FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD"
          ;;
        *) fail "lightning payment environment must select mock, lnd_rest, or phoenixd" ;;
      esac
      ;;
    stripe)
      [[ "$seen_names" == *$'\nFROGLET_STRIPE_SECRET_KEY\n'* ]] || \
        fail "stripe payment environment is missing FROGLET_STRIPE_SECRET_KEY"
      ;;
    x402)
      [[ "$seen_names" == *$'\nFROGLET_X402_WALLET_ADDRESS\n'* ]] || \
        fail "x402 payment environment is missing FROGLET_X402_WALLET_ADDRESS"
      ;;
    *) fail "payment environment must contain one supported FROGLET_PAYMENT_BACKEND" ;;
  esac
  bash -n "$path" >/dev/null 2>&1 || fail "payment environment is not valid shell assignment syntax"
}

install_payment_environment() {
  local source="$1"
  local temporary
  mkdir -p "$BOOTSTRAP_DIR"
  if [[ -z "$source" ]]; then
    if [[ -f "$PAYMENT_ENV_FILE" ]]; then
      validate_payment_environment "$PAYMENT_ENV_FILE"
      chmod 0600 "$PAYMENT_ENV_FILE"
      return 0
    fi
    umask 077
    printf '%s\n' 'FROGLET_PAYMENT_BACKEND=none' >"$PAYMENT_ENV_FILE"
    chmod 0600 "$PAYMENT_ENV_FILE"
    return 0
  fi
  require_absolute_path FROGLET_PAYMENT_ENV_FILE "$source"
  validate_payment_environment "$source"
  if [[ "$source" == "$PAYMENT_ENV_FILE" ]]; then
    chmod 0600 "$PAYMENT_ENV_FILE"
    return 0
  fi
  temporary="${PAYMENT_ENV_FILE}.tmp.$$"
  umask 077
  cp "$source" "$temporary"
  chmod 0600 "$temporary"
  validate_payment_environment "$temporary"
  mv -f "$temporary" "$PAYMENT_ENV_FILE"
}

xml_escape() {
  local name="$1"
  local value="$2"
  reject_control_chars "$name" "$value"
  printf '%s' "$value" | sed \
    -e 's/&/\&amp;/g' \
    -e 's/</\&lt;/g' \
    -e 's/>/\&gt;/g' \
    -e 's/"/\&quot;/g' \
    -e "s/'/\&apos;/g"
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
  [[ "$count" == "1" ]] || fail "manifest must contain exactly one string field: $key"
  printf '%s' "$values"
}

normalize_release() {
  local release="$1"
  case "$release" in
    v*) printf '%s' "$release" ;;
    *) printf 'v%s' "$release" ;;
  esac
}

validate_release() {
  [[ "$1" =~ ^v[0-9A-Za-z][0-9A-Za-z.+-]*$ ]] || fail "invalid release tag: $1"
}

validate_lifecycle_settings() {
  [[ "$START_SERVICE" == "0" || "$START_SERVICE" == "1" ]] || \
    fail "FROGLET_SERVICE_START must be 0 or 1"
  [[ "$HEALTH_ATTEMPTS" =~ ^[1-9][0-9]*$ ]] || \
    fail "FROGLET_HEALTH_ATTEMPTS must be a positive integer"
  [[ "$HEALTH_INTERVAL" =~ ^[0-9]+([.][0-9]+)?$ ]] || \
    fail "FROGLET_HEALTH_INTERVAL_SECS must be a non-negative number"
}

manager() {
  if [[ -n "${FROGLET_SERVICE_MANAGER:-}" ]]; then
    case "$FROGLET_SERVICE_MANAGER" in
      systemd|launchd) printf '%s' "$FROGLET_SERVICE_MANAGER"; return ;;
      *) fail "FROGLET_SERVICE_MANAGER must be systemd or launchd" ;;
    esac
  fi
  case "$(uname -s 2>/dev/null || true)" in
    Linux)
      command -v systemctl >/dev/null 2>&1 || fail "systemd user services are unavailable"
      printf 'systemd'
      ;;
    Darwin)
      command -v launchctl >/dev/null 2>&1 || fail "launchd is unavailable"
      printf 'launchd'
      ;;
    *) fail "native lifecycle supports Ubuntu/systemd and macOS/launchd" ;;
  esac
}

systemd_unit_path() {
  printf '%s/systemd/user/froglet.service' "${XDG_CONFIG_HOME:-$HOME/.config}"
}

launchd_plist_path() {
  printf '%s/Library/LaunchAgents/%s.plist' "$HOME" "$SERVICE_LABEL"
}

write_environment() {
  local release="$1"
  local temporary="${ENV_FILE}.tmp.$$"
  native_listener FROGLET_PROVIDER_URL "$PROVIDER_URL" >/dev/null
  native_listener FROGLET_RUNTIME_URL "$RUNTIME_URL" >/dev/null
  mkdir -p "$BOOTSTRAP_DIR" "$DATA_DIR"
  umask 077
  {
    printf 'FROGLET_RELEASE=%s\n' "$(shell_quote_value FROGLET_RELEASE "$release")"
    printf 'FROGLET_NODE_ROLE=%s\n' '"dual"'
    printf 'FROGLET_DATA_DIR=%s\n' "$(shell_quote_value FROGLET_DATA_DIR "$DATA_DIR")"
    printf 'FROGLET_IDENTITY_AUTO_GENERATE=%s\n' '"true"'
    printf 'FROGLET_NETWORK_MODE=%s\n' "$(shell_quote_value FROGLET_NETWORK_MODE "$NETWORK_MODE")"
    printf 'FROGLET_MARKETPLACE_URL=%s\n' "$(shell_quote_value FROGLET_MARKETPLACE_URL "$MARKETPLACE_URL")"
    printf 'FROGLET_REQUESTER_SPEND_BUDGET_MSAT=%s\n' "$(shell_quote_value FROGLET_REQUESTER_SPEND_BUDGET_MSAT "$SPEND_BUDGET_MSAT")"
    printf 'FROGLET_REQUESTER_MAX_DEAL_MSAT=%s\n' "$(shell_quote_value FROGLET_REQUESTER_MAX_DEAL_MSAT "$MAX_DEAL_MSAT")"
    printf 'FROGLET_LISTEN_ADDR=%s\n' "$(shell_quote_value FROGLET_LISTEN_ADDR "$(native_listener FROGLET_PROVIDER_URL "$PROVIDER_URL")")"
    printf 'FROGLET_RUNTIME_LISTEN_ADDR=%s\n' "$(shell_quote_value FROGLET_RUNTIME_LISTEN_ADDR "$(native_listener FROGLET_RUNTIME_URL "$RUNTIME_URL")")"
    printf 'FROGLET_PUBLIC_BASE_URL=%s\n' "$(shell_quote_value FROGLET_PUBLIC_BASE_URL "$PROVIDER_URL")"
    printf 'FROGLET_RUNTIME_PROVIDER_BASE_URL=%s\n' "$(shell_quote_value FROGLET_RUNTIME_PROVIDER_BASE_URL "$PROVIDER_URL")"
    printf 'FROGLET_RELAY_URL=%s\n' "$(shell_quote_value FROGLET_RELAY_URL "$RELAY_URL")"
    printf 'FROGLET_RELAY_PUBLIC_SUFFIX=%s\n' "$(shell_quote_value FROGLET_RELAY_PUBLIC_SUFFIX "$RELAY_PUBLIC_SUFFIX")"
    printf 'FROGLET_HOST_READABLE_CONTROL_TOKEN=%s\n' '"true"'
  } >"$temporary"
  chmod 0600 "$temporary"
  mv -f "$temporary" "$ENV_FILE"
}

write_launcher() {
  local env_path payment_path binary_path temporary
  env_path="$(shell_quote_value native_environment "$ENV_FILE")"
  payment_path="$(shell_quote_value payment_environment "$PAYMENT_ENV_FILE")"
  binary_path="$(shell_quote_value froglet_binary "$CURRENT_LINK/froglet-node")"
  temporary="${LAUNCHER}.tmp.$$"
  umask 077
  cat >"$temporary" <<EOF
#!/usr/bin/env bash
set -euo pipefail
set -a
. $env_path
. $payment_path
set +a
exec $binary_path "\$@"
EOF
  chmod 0700 "$temporary"
  mv -f "$temporary" "$LAUNCHER"
}

write_systemd_unit() {
  local unit_path launcher_path
  unit_path="$(systemd_unit_path)"
  launcher_path="$(shell_quote_value systemd_launcher "$LAUNCHER")"
  mkdir -p "$(dirname "$unit_path")"
  cat >"$unit_path" <<EOF
[Unit]
Description=Froglet local provider and requester node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$launcher_path
Restart=on-failure
RestartSec=2
TimeoutStopSec=20

[Install]
WantedBy=default.target
EOF
}

write_launchd_plist() {
  local plist_path launcher_path log_dir
  plist_path="$(launchd_plist_path)"
  launcher_path="$(xml_escape launcher "$LAUNCHER")"
  log_dir="$BOOTSTRAP_DIR/logs"
  mkdir -p "$(dirname "$plist_path")" "$log_dir"
  cat >"$plist_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$SERVICE_LABEL</string>
  <key>ProgramArguments</key>
  <array><string>$launcher_path</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>$(xml_escape log "$log_dir/stdout.log")</string>
  <key>StandardErrorPath</key><string>$(xml_escape log "$log_dir/stderr.log")</string>
</dict>
</plist>
EOF
  chmod 0600 "$plist_path"
}

write_service_definition() {
  write_launcher
  case "$(manager)" in
    systemd) write_systemd_unit ;;
    launchd) write_launchd_plist ;;
  esac
}

manager_start() {
  case "$(manager)" in
    systemd)
      systemctl --user daemon-reload
      systemctl --user enable --now froglet.service
      if command -v loginctl >/dev/null 2>&1; then
        if ! loginctl enable-linger "${USER:-$(id -un)}" >/dev/null 2>&1; then
          log "warning: could not enable systemd linger; restart is verified for login sessions, reboot-before-login is not yet proven"
        fi
      else
        log "warning: loginctl is unavailable; reboot-before-login is not enabled"
      fi
      ;;
    launchd)
      local domain
      domain="gui/$(id -u)"
      launchctl bootout "$domain/$SERVICE_LABEL" >/dev/null 2>&1 || true
      launchctl bootstrap "$domain" "$(launchd_plist_path)"
      launchctl enable "$domain/$SERVICE_LABEL"
      launchctl kickstart -k "$domain/$SERVICE_LABEL"
      ;;
  esac
}

manager_restart() {
  case "$(manager)" in
    systemd)
      systemctl --user daemon-reload
      systemctl --user restart froglet.service
      ;;
    launchd)
      # `kickstart` alone keeps the already-loaded plist, including its old
      # release environment. Reload the definition before starting the new
      # binary so upgrade and rollback metadata follow the active symlink.
      local domain
      domain="gui/$(id -u)"
      launchctl bootout "$domain/$SERVICE_LABEL" >/dev/null 2>&1 || true
      launchctl bootstrap "$domain" "$(launchd_plist_path)"
      launchctl enable "$domain/$SERVICE_LABEL"
      launchctl kickstart -k "$domain/$SERVICE_LABEL"
      ;;
  esac
}

manager_stop() {
  case "$(manager)" in
    systemd)
      systemctl --user disable --now froglet.service >/dev/null 2>&1 || true
      systemctl --user daemon-reload >/dev/null 2>&1 || true
      ;;
    launchd)
      launchctl bootout "gui/$(id -u)/$SERVICE_LABEL" >/dev/null 2>&1 || true
      ;;
  esac
}

manager_status() {
  case "$(manager)" in
    systemd) systemctl --user is-active froglet.service ;;
    launchd) launchctl print "gui/$(id -u)/$SERVICE_LABEL" >/dev/null && printf 'active\n' ;;
  esac
}

current_release() {
  [[ -f "$RELEASE_FILE" ]] || fail "Froglet native service is not installed"
  local release
  release="$(sed -n '1p' "$RELEASE_FILE")"
  validate_release "$release"
  printf '%s' "$release"
}

atomic_link() {
  local target="$1"
  local link="$2"
  local temporary="${link}.new.$$"
  rm -f "$temporary"
  ln -s "$target" "$temporary"
  # Rename is the actual activation transaction. BSD `mv -h` and GNU
  # `mv -T` both replace the symlink itself instead of following a
  # destination that points at a directory.
  if mv -fT "$temporary" "$link" 2>/dev/null; then
    return 0
  fi
  if ! mv -fh "$temporary" "$link"; then
    rm -f "$temporary"
    return 1
  fi
}

wait_for_health() {
  need_cmd curl
  local attempt
  for ((attempt = 1; attempt <= HEALTH_ATTEMPTS; attempt++)); do
    if curl -fsS --max-time 5 "$PROVIDER_URL/health" >/dev/null 2>&1 \
      && curl -fsS --max-time 5 "$RUNTIME_URL/health" >/dev/null 2>&1; then
      log "provider and runtime are healthy"
      return 0
    fi
    sleep "$HEALTH_INTERVAL"
  done
  return 1
}

run_local_proof() {
  local proof_tmp="${PROOF_FILE}.tmp.$$"
  local request='{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"froglet","arguments":{"action":"status"}}}'
  if ! printf '%s\n' "$request" | \
    FROGLET_RELEASE="$(current_release)" \
    FROGLET_DATA_DIR="$DATA_DIR" \
    FROGLET_PROVIDER_URL="$PROVIDER_URL" \
    FROGLET_RUNTIME_URL="$RUNTIME_URL" \
      "$CURRENT_LINK/froglet-node" mcp >"$proof_tmp"; then
    rm -f "$proof_tmp"
    return 1
  fi
  if ! grep -Eq '"status"[[:space:]]*:[[:space:]]*"ok"' "$proof_tmp" \
    || ! grep -Eq '"native_mcp"[[:space:]]*:[[:space:]]*true' "$proof_tmp" \
    || ! grep -Eq '"isError"[[:space:]]*:[[:space:]]*false' "$proof_tmp"; then
    rm -f "$proof_tmp"
    return 1
  fi
  mv -f "$proof_tmp" "$PROOF_FILE"
  chmod 0600 "$PROOF_FILE"
  log "native MCP command-line probe and local health passed; agent attachment still requires a call from the agent"
}

wait_ready() {
  wait_for_health || return 1
  run_local_proof || return 1
}

warn_identity_backup_required() {
  log "identity backup is not created automatically: before public publication, run 'froglet-node identity backup' and store its encrypted bundle and recovery key in separate secure locations"
}

activate_release() {
  local binary="$1"
  local release="$2"
  local manifest="$3"
  local target old_target
  validate_release "$release"
  [[ -x "$binary" ]] || fail "release binary is not executable: $binary"
  [[ -f "$manifest" ]] || fail "release manifest is missing: $manifest"
  [[ "$(manifest_value release "$manifest")" == "$release" ]] || \
    fail "release manifest does not match $release"

  target="$RELEASES_DIR/$release"
  mkdir -p "$target" "$BIN_DIR" "$DATA_DIR"
  if [[ -e "$target/froglet-node" ]]; then
    cmp -s "$binary" "$target/froglet-node" || \
      fail "immutable release target already exists with different bytes: $target/froglet-node"
    cmp -s "$manifest" "$target/release-manifest.json" || \
      fail "immutable release target already exists with a different release manifest: $target/release-manifest.json"
  else
    cp "$binary" "$target/froglet-node"
    chmod 0755 "$target/froglet-node"
    cp "$manifest" "$target/release-manifest.json"
    chmod 0644 "$target/release-manifest.json"
  fi

  old_target=""
  if [[ -L "$CURRENT_LINK" ]]; then
    old_target="$(readlink "$CURRENT_LINK")"
  fi
  if [[ -n "$old_target" && "$old_target" != "$target" ]]; then
    atomic_link "$old_target" "$PREVIOUS_LINK"
  fi
  atomic_link "$target" "$CURRENT_LINK"
  atomic_link "$CURRENT_LINK/froglet-node" "$BIN_DIR/froglet-node"
  printf '%s\n' "$release" >"$RELEASE_FILE"
  chmod 0644 "$RELEASE_FILE"
  cp "$target/release-manifest.json" "$BOOTSTRAP_DIR/release-manifest.json"
  write_environment "$release"
  write_service_definition

  if [[ "$START_SERVICE" == "1" ]]; then
    if [[ -n "$old_target" ]]; then
      manager_restart
    else
      manager_start
    fi
    if ! wait_ready; then
      return 1
    fi
    warn_identity_backup_required
  fi
}

rollback_to() {
  local target="$1"
  [[ -x "$target/froglet-node" ]] || fail "rollback target is invalid: $target"
  local release
  release="$(basename "$target")"
  validate_release "$release"
  atomic_link "$target" "$CURRENT_LINK"
  atomic_link "$CURRENT_LINK/froglet-node" "$BIN_DIR/froglet-node"
  printf '%s\n' "$release" >"$RELEASE_FILE"
  cp "$target/release-manifest.json" "$BOOTSTRAP_DIR/release-manifest.json"
  write_environment "$release"
  write_service_definition
  manager_restart
  wait_ready || fail "rollback release $release did not recover"
  log "rolled back to $release"
}

upgrade() {
  local requested="${1:-}"
  local old_target stage release target
  [[ -x "$INSTALLER" ]] || fail "verified installer is missing: $INSTALLER"
  [[ -L "$CURRENT_LINK" ]] || fail "Froglet native service is not installed"
  old_target="$(readlink "$CURRENT_LINK")"
  mkdir -p "$RELEASES_DIR"
  stage="$(mktemp -d "$RELEASES_DIR/.upgrade.XXXXXX")"

  log "downloading and verifying upgrade ${requested:-latest}"
  if ! INSTALL_DIR="$stage" \
    VERSION="$requested" \
    FROGLET_INSTALL_REPO="$REPO" \
    FROGLET_RELEASE_MANIFEST_OUT="$stage/release-manifest.json" \
    sh "$INSTALLER"; then
    rm -rf "$stage"
    fail "upgrade download or Release Bundle verification failed; current release was not changed"
  fi
  release="$(manifest_value release "$stage/release-manifest.json")"
  validate_release "$release"
  target="$RELEASES_DIR/$release"

  if [[ "$target" == "$old_target" ]]; then
    rm -rf "$stage"
    log "$release is already active; restarting and re-running local proof"
    manager_restart
    wait_ready || fail "active release $release failed health/proof after restart"
    return 0
  fi

  if activate_release "$stage/froglet-node" "$release" "$stage/release-manifest.json"; then
    rm -rf "$stage"
    log "upgrade succeeded: $release"
    return 0
  fi

  log "upgrade to $release failed health/proof; restoring $(basename "$old_target")"
  rm -rf "$target" "$stage"
  rollback_to "$old_target"
  fail "upgrade failed and was rolled back to $(basename "$old_target")"
}

remove_service_definition() {
  case "$(manager)" in
    systemd) rm -f "$(systemd_unit_path)" ;;
    launchd) rm -f "$(launchd_plist_path)" ;;
  esac
}

command="${1:-}"
[[ -n "$command" ]] || { usage >&2; exit 2; }
shift
require_absolute_path FROGLET_BOOTSTRAP_DIR "$BOOTSTRAP_DIR"
require_absolute_path FROGLET_DATA_DIR "$DATA_DIR"
require_absolute_path INSTALL_DIR "$BIN_DIR"
reject_control_chars FROGLET_BOOTSTRAP_DIR "$BOOTSTRAP_DIR"
reject_control_chars FROGLET_DATA_DIR "$DATA_DIR"
reject_control_chars INSTALL_DIR "$BIN_DIR"
reject_control_chars FROGLET_LIFECYCLE_LOCK_TOKEN "$LIFECYCLE_LOCK_TOKEN"
[[ "$LIFECYCLE_LOCK_TOKEN" =~ ^[A-Za-z0-9._:-]{1,200}$ ]] || \
  fail "FROGLET_LIFECYCLE_LOCK_TOKEN contains unsupported characters"
load_persisted_config
validate_network_mode
validate_http_url FROGLET_MARKETPLACE_URL "$MARKETPLACE_URL"
require_msat_or_empty FROGLET_REQUESTER_SPEND_BUDGET_MSAT "$SPEND_BUDGET_MSAT"
require_msat_or_empty FROGLET_REQUESTER_MAX_DEAL_MSAT "$MAX_DEAL_MSAT"
reject_control_chars FROGLET_RELAY_URL "$RELAY_URL"
reject_control_chars FROGLET_RELAY_PUBLIC_SUFFIX "$RELAY_PUBLIC_SUFFIX"
validate_relay_url "$RELAY_URL"
validate_relay_public_suffix "$RELAY_PUBLIC_SUFFIX"
[[ -z "${FROGLET_RELAY_ENABLED:-}" ]] || \
  fail "FROGLET_RELAY_ENABLED is obsolete; configure FROGLET_RELAY_URL plus FROGLET_RELAY_PUBLIC_SUFFIX"
if [[ -n "$RELAY_URL" || -n "$RELAY_PUBLIC_SUFFIX" ]]; then
  [[ -n "$RELAY_URL" && -n "$RELAY_PUBLIC_SUFFIX" ]] || \
    fail "FROGLET_RELAY_URL and FROGLET_RELAY_PUBLIC_SUFFIX must be configured together"
fi
validate_lifecycle_settings

case "$command" in
  activate|restart|upgrade|rollback|uninstall) acquire_lifecycle_lock ;;
esac

case "$command" in
  activate)
    binary=""
    release=""
    manifest=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --binary) [[ $# -ge 2 ]] || fail "--binary requires a path"; binary="$2"; shift 2 ;;
        --release) [[ $# -ge 2 ]] || fail "--release requires a tag"; release="$(normalize_release "$2")"; shift 2 ;;
        --manifest) [[ $# -ge 2 ]] || fail "--manifest requires a path"; manifest="$2"; shift 2 ;;
        *) fail "unknown activate argument: $1" ;;
      esac
    done
    [[ -n "$binary" && -n "$release" && -n "$manifest" ]] || fail "activate requires --binary, --release, and --manifest"
    install_payment_environment "$PAYMENT_ENV_INPUT"
    if ! activate_release "$binary" "$release" "$manifest"; then
      fail "release $release did not pass health and local proof"
    fi
    ;;
  restart)
    [[ $# -eq 0 ]] || fail "restart takes no arguments"
    validate_payment_environment "$PAYMENT_ENV_FILE"
    write_service_definition
    manager_restart
    wait_ready || fail "Froglet did not recover after restart"
    ;;
  verify)
    [[ $# -eq 0 ]] || fail "verify takes no arguments"
    manager_status >/dev/null || fail "Froglet service is not active"
    wait_ready || fail "existing Froglet service did not pass health and command-line MCP checks"
    ;;
  status)
    [[ $# -eq 0 ]] || fail "status takes no arguments"
    service_status="$(manager_status)" || fail "Froglet service is not active"
    printf 'status=%s\n' "$service_status"
    printf 'release=%s\n' "$(current_release)"
    printf 'state_path=%s\n' "$DATA_DIR"
    printf 'binary=%s\n' "$CURRENT_LINK/froglet-node"
    [[ -f "$PROOF_FILE" ]] && printf 'local_proof=%s\n' "$PROOF_FILE"
    ;;
  upgrade)
    [[ $# -le 1 ]] || fail "upgrade accepts at most one release tag"
    validate_payment_environment "$PAYMENT_ENV_FILE"
    upgrade "${1:-}"
    ;;
  rollback)
    [[ $# -eq 0 ]] || fail "rollback takes no arguments"
    validate_payment_environment "$PAYMENT_ENV_FILE"
    [[ -L "$PREVIOUS_LINK" ]] || fail "no previous release is available"
    rollback_to "$(readlink "$PREVIOUS_LINK")"
    ;;
  uninstall)
    purge=0
    if [[ "${1:-}" == "--purge-data" ]]; then
      purge=1
      shift
    fi
    [[ $# -eq 0 ]] || fail "uninstall accepts only --purge-data"
    manager_stop
    remove_service_definition
    rm -f "$BIN_DIR/froglet-node"
    rm -rf "$BOOTSTRAP_DIR"
    if [[ "$purge" == "1" ]]; then
      rm -rf "$DATA_DIR"
      log "uninstalled Froglet and removed persistent state"
    else
      log "uninstalled Froglet; persistent identity/state preserved at $DATA_DIR"
    fi
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    usage >&2
    fail "unknown command: $command"
    ;;
esac
