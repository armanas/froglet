#!/bin/sh
set -eu

REPO="${FROGLET_INSTALL_REPO:-armanas/froglet}"
RAW_BASE="${FROGLET_RAW_BASE:-https://raw.githubusercontent.com/$REPO/main}"
BOOTSTRAP_DIR="${FROGLET_BOOTSTRAP_DIR:-$HOME/.froglet/agent}"
DATA_DIR="${FROGLET_DATA_DIR:-$HOME/.froglet/data}"
BIN_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
IMAGE_TAG="${FROGLET_IMAGE_TAG:-${VERSION:-}}"
PROVIDER_IMAGE="${FROGLET_PROVIDER_IMAGE:-}"
RUNTIME_IMAGE="${FROGLET_RUNTIME_IMAGE:-}"
MCP_IMAGE="${FROGLET_MCP_IMAGE:-}"
AGENT_TARGET="${FROGLET_AGENT_TARGET:-claude-code}"
PROVIDER_URL="${FROGLET_PROVIDER_URL:-http://127.0.0.1:8080}"
RUNTIME_URL="${FROGLET_RUNTIME_URL:-http://127.0.0.1:8081}"
NETWORK_MODE="${FROGLET_NETWORK_MODE:-clearnet}"
MARKETPLACE_URL="${FROGLET_MARKETPLACE_URL:-https://marketplace.froglet.dev}"
START_STACK="${FROGLET_BOOTSTRAP_START:-1}"
COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-${FROGLET_COMPOSE_PROJECT_NAME:-froglet_agent}}"
MCP_DOCKER_NETWORK="${FROGLET_MCP_DOCKER_NETWORK:-${COMPOSE_PROJECT_NAME}_default}"
export COMPOSE_PROJECT_NAME

log() {
  printf '%s\n' "$*" >&2
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
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

resolve_latest_image_tag() {
  resolved_url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null || true)"
  tag="${resolved_url##*/}"
  if [ -n "$tag" ] && [ "$tag" != "$resolved_url" ]; then
    printf '%s' "${tag#v}"
  else
    printf '%s' "latest"
  fi
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
    curl -fsSL "$RAW_BASE/scripts/setup-payment.sh" -o "$tmp_payment_script"
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
    printf '%s\n' "FROGLET_PAYMENT_BACKEND=none" >"$BOOTSTRAP_DIR/payment.env"
  fi
}

write_compose() {
  mkdir -p "$BOOTSTRAP_DIR" "$DATA_DIR"
  cat >"$BOOTSTRAP_DIR/compose.yaml" <<EOF
services:
  provider:
    image: $(yaml_quote FROGLET_PROVIDER_IMAGE "$PROVIDER_IMAGE")
    env_file:
      - payment.env
    environment:
      FROGLET_DATA_ROOT: /data
      FROGLET_DB_PATH: /data/provider.node.db
      FROGLET_NETWORK_MODE: $(yaml_quote FROGLET_NETWORK_MODE "$NETWORK_MODE")
      FROGLET_PUBLIC_BASE_URL: http://provider:8080
      FROGLET_HOST_READABLE_CONTROL_TOKEN: "true"
    ports:
      - "127.0.0.1:8080:8080"
    volumes:
      - $(yaml_quote FROGLET_DATA_DIR "$DATA_DIR:/data")
    healthcheck:
      test: ["CMD-SHELL", "curl -fsS http://127.0.0.1:8080/health >/dev/null"]
      interval: 10s
      timeout: 3s
      retries: 10
      start_period: 5s

  runtime:
    image: $(yaml_quote FROGLET_RUNTIME_IMAGE "$RUNTIME_IMAGE")
    depends_on:
      provider:
        condition: service_healthy
    env_file:
      - payment.env
    environment:
      FROGLET_DATA_ROOT: /data
      FROGLET_DB_PATH: /data/runtime.node.db
      FROGLET_RUNTIME_PROVIDER_BASE_URL: http://provider:8080
      FROGLET_MARKETPLACE_URL: $(yaml_quote FROGLET_MARKETPLACE_URL "$MARKETPLACE_URL")
      FROGLET_NETWORK_MODE: $(yaml_quote FROGLET_NETWORK_MODE "$NETWORK_MODE")
      FROGLET_HOST_READABLE_CONTROL_TOKEN: "true"
    ports:
      - "127.0.0.1:8081:8081"
    volumes:
      - $(yaml_quote FROGLET_DATA_DIR "$DATA_DIR:/data")
    healthcheck:
      test: ["CMD-SHELL", "curl -fsS http://127.0.0.1:8081/health >/dev/null"]
      interval: 10s
      timeout: 3s
      retries: 10
      start_period: 5s
EOF
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
  mkdir -p "$BOOTSTRAP_DIR/mcp"
  curl -fsSL "$RAW_BASE/scripts/setup-agent.sh" -o "$tmp_script"
  chmod 0755 "$tmp_script"
  if [ "$AGENT_TARGET" = "claude-code" ]; then
    out_path="$BOOTSTRAP_DIR/mcp/.mcp.json"
  else
    out_path="$BOOTSTRAP_DIR/mcp/config.toml"
  fi
  FROGLET_PROVIDER_URL="$PROVIDER_URL" \
  FROGLET_RUNTIME_URL="$RUNTIME_URL" \
  FROGLET_MCP_IMAGE="$MCP_IMAGE" \
  FROGLET_MCP_DOCKER_NETWORK="$MCP_DOCKER_NETWORK" \
  FROGLET_PROVIDER_AUTH_TOKEN_PATH="$DATA_DIR/runtime/froglet-control.token" \
  FROGLET_RUNTIME_AUTH_TOKEN_PATH="$DATA_DIR/runtime/auth.token" \
    "$tmp_script" --target "$AGENT_TARGET" --out "$out_path" >/dev/null
  printf '%s' "$out_path"
}

need_cmd curl

if [ -z "$IMAGE_TAG" ]; then
  IMAGE_TAG="$(resolve_latest_image_tag)"
else
  IMAGE_TAG="${IMAGE_TAG#v}"
fi
PROVIDER_IMAGE="${PROVIDER_IMAGE:-ghcr.io/armanas/froglet-provider:latest}"
RUNTIME_IMAGE="${RUNTIME_IMAGE:-ghcr.io/armanas/froglet-runtime:latest}"
MCP_IMAGE="${MCP_IMAGE:-ghcr.io/armanas/froglet-mcp:latest}"

mkdir -p "$BIN_DIR" "$BOOTSTRAP_DIR"

log "Installing froglet-node from signed release assets..."
curl -fsSL "$RAW_BASE/scripts/install.sh" | INSTALL_DIR="$BIN_DIR" sh

compose_started=false
compose_file=""
if [ "$START_STACK" = "1" ]; then
  need_cmd docker
  configure_payment
  write_compose
  compose_file="$BOOTSTRAP_DIR/compose.yaml"
  log "Starting Froglet provider/runtime from published images..."
  COMPOSE_PROJECT_NAME="$COMPOSE_PROJECT_NAME" docker_compose -f "$compose_file" up -d
  compose_started=true
fi

mcp_config_path="$(configure_agent)"

cat <<EOF
{
  "status": "ok",
  "bootstrap_dir": "$(json_escape "$BOOTSTRAP_DIR")",
  "data_dir": "$(json_escape "$DATA_DIR")",
  "froglet_node": "$(json_escape "$BIN_DIR/froglet-node")",
  "provider_url": "$(json_escape "$PROVIDER_URL")",
  "runtime_url": "$(json_escape "$RUNTIME_URL")",
  "network_mode": "$(json_escape "$NETWORK_MODE")",
  "compose_file": "$(json_escape "$compose_file")",
  "compose_project_name": "$(json_escape "$COMPOSE_PROJECT_NAME")",
  "compose_started": $compose_started,
  "agent_target": "$(json_escape "$AGENT_TARGET")",
  "mcp_docker_network": "$(json_escape "$MCP_DOCKER_NETWORK")",
  "mcp_config_path": "$(json_escape "$mcp_config_path")",
  "next_mcp_actions": ["status", "publish_artifact"],
  "next_instruction": "Restart or point your agent at the MCP config path, call froglet status, then publish a demo service with template demo.add."
}
EOF
