#!/usr/bin/env bash
set -euo pipefail

# Detect whether we're running from a cloned repo (node-based MCP path
# available) or piped via `curl | sh -s -- --target ...` with only the
# froglet-node binary installed. In repo mode we keep the existing
# node-based MCP invocation; in binary-only mode we emit a Docker-based
# config that pulls ghcr.io/armanas/froglet-mcp.
repo_root=""
candidate_root=""
if [[ -n "${BASH_SOURCE[0]:-}" && -f "${BASH_SOURCE[0]}" ]]; then
  if candidate_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." 2>/dev/null && pwd)"; then
    if [[ -f "$candidate_root/integrations/mcp/froglet/server.js" ]]; then
      repo_root="$candidate_root"
    fi
  fi
fi
# Fallback: check the current working directory (useful if the script was
# downloaded into a repo root manually).
if [[ -z "$repo_root" && -f "$PWD/integrations/mcp/froglet/server.js" ]]; then
  repo_root="$PWD"
fi

mcp_image="${FROGLET_MCP_IMAGE:-ghcr.io/armanas/froglet-mcp:latest}"

provider_url="${FROGLET_PROVIDER_URL:-http://127.0.0.1:8080}"
runtime_url="${FROGLET_RUNTIME_URL:-http://127.0.0.1:8081}"
# Token paths default to the repo-relative `data/runtime/` tree when run
# from inside a cloned repo. In binary-only mode we fall back to
# `~/.froglet/runtime/` since the repo path doesn't exist.
default_token_dir="${repo_root:+$repo_root/data/runtime}"
default_token_dir="${default_token_dir:-$HOME/.froglet/runtime}"
provider_token_path="${FROGLET_PROVIDER_AUTH_TOKEN_PATH:-$default_token_dir/froglet-control.token}"
runtime_token_path="${FROGLET_RUNTIME_AUTH_TOKEN_PATH:-$default_token_dir/auth.token}"
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

# Output path defaults to the current working directory when not in a repo.
# Repo mode keeps the original repo-root anchoring so existing contributors
# don't see behavior change.
default_out_dir="${repo_root:-$PWD}"

server_path=""
openclaw_plugin_path=""
if [[ -n "$repo_root" ]]; then
  server_path="$repo_root/integrations/mcp/froglet/server.js"
  openclaw_plugin_path="$repo_root/integrations/openclaw/froglet"
fi

case "$target" in
  claude-code)
    out_path="${out_path:-$default_out_dir/.mcp.json}"
    mkdir -p "$(dirname "$out_path")"
    if [[ -n "$repo_root" ]]; then
      # Repo mode: use local node server.js
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
    else
      # Binary-only mode: use the published MCP Docker image. Docker MCP
      # mode intentionally omits token paths — host file mounts would be
      # brittle across users. Provider/runtime URLs point at the local
      # docker compose stack via host.docker.internal by default; override
      # with FROGLET_PROVIDER_URL / FROGLET_RUNTIME_URL for hosted use.
      docker_provider_url="${provider_url/127.0.0.1/host.docker.internal}"
      docker_provider_url="${docker_provider_url/localhost/host.docker.internal}"
      docker_runtime_url="${runtime_url/127.0.0.1/host.docker.internal}"
      docker_runtime_url="${docker_runtime_url/localhost/host.docker.internal}"
      cat >"$out_path" <<EOF
{
  "mcpServers": {
    "froglet": {
      "type": "stdio",
      "command": "docker",
      "args": [
        "run", "--rm", "-i",
        "-e", "FROGLET_PROVIDER_URL",
        "-e", "FROGLET_RUNTIME_URL",
        "-e", "FROGLET_REQUEST_TIMEOUT_MS",
        "-e", "FROGLET_DEFAULT_SEARCH_LIMIT",
        "-e", "FROGLET_MAX_SEARCH_LIMIT",
        "$mcp_image"
      ],
      "env": {
        "FROGLET_PROVIDER_URL": "$docker_provider_url",
        "FROGLET_RUNTIME_URL": "$docker_runtime_url",
        "FROGLET_REQUEST_TIMEOUT_MS": "$request_timeout_ms",
        "FROGLET_DEFAULT_SEARCH_LIMIT": "$default_search_limit",
        "FROGLET_MAX_SEARCH_LIMIT": "$max_search_limit"
      }
    }
  }
}
EOF
    fi
    printf 'Wrote Claude Code MCP config to %s\n' "$out_path"
    if [[ -n "$repo_root" ]]; then
      printf 'Activation: restart Claude Code in this repo so it reloads %s\n' "$out_path"
    else
      printf 'Activation: move this .mcp.json to your project dir; Claude Code picks it up on next start\n'
      printf 'Note: Docker MCP mode requires docker; host.docker.internal is used for provider/runtime URLs\n'
    fi
    ;;
  codex)
    out_path="${out_path:-$default_out_dir/.codex/config.toml}"
    mkdir -p "$(dirname "$out_path")"
    if [[ -n "$repo_root" ]]; then
      cat >"$out_path" <<EOF
[mcp_servers.froglet]
command = "node"
args = ["$server_path"]
env = { FROGLET_PROVIDER_URL = "$provider_url", FROGLET_RUNTIME_URL = "$runtime_url", FROGLET_PROVIDER_AUTH_TOKEN_PATH = "$provider_token_path", FROGLET_RUNTIME_AUTH_TOKEN_PATH = "$runtime_token_path", FROGLET_REQUEST_TIMEOUT_MS = "$request_timeout_ms", FROGLET_DEFAULT_SEARCH_LIMIT = "$default_search_limit", FROGLET_MAX_SEARCH_LIMIT = "$max_search_limit" }
EOF
    else
      docker_provider_url="${provider_url/127.0.0.1/host.docker.internal}"
      docker_provider_url="${docker_provider_url/localhost/host.docker.internal}"
      docker_runtime_url="${runtime_url/127.0.0.1/host.docker.internal}"
      docker_runtime_url="${docker_runtime_url/localhost/host.docker.internal}"
      cat >"$out_path" <<EOF
[mcp_servers.froglet]
command = "docker"
args = ["run", "--rm", "-i", "-e", "FROGLET_PROVIDER_URL", "-e", "FROGLET_RUNTIME_URL", "-e", "FROGLET_REQUEST_TIMEOUT_MS", "-e", "FROGLET_DEFAULT_SEARCH_LIMIT", "-e", "FROGLET_MAX_SEARCH_LIMIT", "$mcp_image"]
env = { FROGLET_PROVIDER_URL = "$docker_provider_url", FROGLET_RUNTIME_URL = "$docker_runtime_url", FROGLET_REQUEST_TIMEOUT_MS = "$request_timeout_ms", FROGLET_DEFAULT_SEARCH_LIMIT = "$default_search_limit", FROGLET_MAX_SEARCH_LIMIT = "$max_search_limit" }
EOF
    fi
    printf 'Wrote Codex MCP config to %s\n' "$out_path"
    if [[ -n "$repo_root" ]]; then
      printf 'Activation: start Codex from %s so it picks up the project config\n' "$repo_root"
    else
      printf 'Activation: place this config.toml at .codex/config.toml in your project dir\n'
    fi
    ;;
  openclaw)
    if [[ -z "$repo_root" ]]; then
      fail "openclaw target requires the froglet repo cloned. The OpenClaw plugin is a local folder (integrations/openclaw/froglet). Clone https://github.com/armanas/froglet and re-run scripts/setup-agent.sh from the repo root, or use --target claude-code for a Docker-based MCP config."
    fi
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
