#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

provider_url="${FROGLET_PROVIDER_URL:-http://127.0.0.1:8080}"
runtime_url="${FROGLET_RUNTIME_URL:-http://127.0.0.1:8081}"
provider_token_path="${FROGLET_PROVIDER_AUTH_TOKEN_PATH:-$repo_root/data/runtime/froglet-control.token}"
runtime_token_path="${FROGLET_RUNTIME_AUTH_TOKEN_PATH:-$repo_root/data/runtime/auth.token}"
request_timeout_ms="${FROGLET_REQUEST_TIMEOUT_MS:-10000}"
default_search_limit="${FROGLET_DEFAULT_SEARCH_LIMIT:-10}"
max_search_limit="${FROGLET_MAX_SEARCH_LIMIT:-50}"
target=""
out_path=""

usage() {
  cat <<'EOF'
Usage:
  scripts/setup-agent.sh --target claude-code|codex|openclaw [--out PATH]

Generates an agent config file that points at the local Froglet provider and
runtime by default.
EOF
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || fail "--target requires a value"
      target="$2"
      shift 2
      ;;
    --out)
      [[ $# -ge 2 ]] || fail "--out requires a value"
      out_path="$2"
      shift 2
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

[[ -n "$target" ]] || fail "--target is required"

server_path="$repo_root/integrations/mcp/froglet/server.js"
openclaw_plugin_path="$repo_root/integrations/openclaw/froglet"

case "$target" in
  claude-code)
    out_path="${out_path:-$repo_root/.mcp.json}"
    mkdir -p "$(dirname "$out_path")"
    cat >"$out_path" <<EOF
{
  "mcpServers": {
    "froglet": {
      "type": "stdio",
      "command": "node",
      "args": ["$server_path"],
      "env": {
        "FROGLET_PROVIDER_URL": "$provider_url",
        "FROGLET_RUNTIME_URL": "$runtime_url",
        "FROGLET_PROVIDER_AUTH_TOKEN_PATH": "$provider_token_path",
        "FROGLET_RUNTIME_AUTH_TOKEN_PATH": "$runtime_token_path",
        "FROGLET_REQUEST_TIMEOUT_MS": "$request_timeout_ms",
        "FROGLET_DEFAULT_SEARCH_LIMIT": "$default_search_limit",
        "FROGLET_MAX_SEARCH_LIMIT": "$max_search_limit"
      }
    }
  }
}
EOF
    printf 'Wrote Claude Code MCP config to %s\n' "$out_path"
    printf 'Activation: restart Claude Code in this repo so it reloads %s\n' "$out_path"
    ;;
  codex)
    out_path="${out_path:-$repo_root/.codex/config.toml}"
    mkdir -p "$(dirname "$out_path")"
    cat >"$out_path" <<EOF
[mcp_servers.froglet]
command = "node"
args = ["$server_path"]
env = { FROGLET_PROVIDER_URL = "$provider_url", FROGLET_RUNTIME_URL = "$runtime_url", FROGLET_PROVIDER_AUTH_TOKEN_PATH = "$provider_token_path", FROGLET_RUNTIME_AUTH_TOKEN_PATH = "$runtime_token_path", FROGLET_REQUEST_TIMEOUT_MS = "$request_timeout_ms", FROGLET_DEFAULT_SEARCH_LIMIT = "$default_search_limit", FROGLET_MAX_SEARCH_LIMIT = "$max_search_limit" }
EOF
    printf 'Wrote Codex MCP config to %s\n' "$out_path"
    printf 'Activation: start Codex from %s so it picks up the project config\n' "$repo_root"
    ;;
  openclaw)
    out_path="${out_path:-$repo_root/.froglet/openclaw.config.json}"
    mkdir -p "$(dirname "$out_path")"
    cat >"$out_path" <<EOF
{
  "plugins": {
    "load": {
      "paths": [
        "$openclaw_plugin_path"
      ]
    },
    "entries": {
      "froglet": {
        "enabled": true,
        "config": {
          "hostProduct": "openclaw",
          "providerUrl": "$provider_url",
          "runtimeUrl": "$runtime_url",
          "providerAuthTokenPath": "$provider_token_path",
          "runtimeAuthTokenPath": "$runtime_token_path",
          "requestTimeoutMs": $request_timeout_ms,
          "defaultSearchLimit": $default_search_limit,
          "maxSearchLimit": $max_search_limit
        }
      }
    }
  },
  "agents": {
    "list": [
      {
        "id": "main",
        "tools": {
          "allow": [
            "froglet"
          ]
        }
      }
    ]
  }
}
EOF
    printf 'Wrote OpenClaw config to %s\n' "$out_path"
    printf 'Verification: node %s --config %s --target openclaw\n' \
      "$repo_root/integrations/openclaw/froglet/scripts/doctor.mjs" \
      "$out_path"
    ;;
  *)
    fail "unsupported target: $target"
    ;;
esac

if [[ "$provider_token_path" == "$repo_root/data/runtime/froglet-control.token" ]]; then
  printf 'Compose-backed usage: start docker compose with FROGLET_HOST_READABLE_CONTROL_TOKEN=true so %s is readable on the host.\n' "$provider_token_path"
fi
