# Froglet MCP Server

MCP (Model Context Protocol) server that exposes Froglet services, compute,
and project management to AI agents (Claude, Cursor, Codex, Windsurf, etc.).

## Requirements

- Node.js 18+ (or Docker)
- A running Froglet provider and runtime, or one node that exposes both surfaces

## Quick Start

```bash
# Install dependencies
npm ci --prefix integrations/mcp/froglet

# Start the server (stdio transport)
FROGLET_PROVIDER_URL=http://127.0.0.1:8080 \
FROGLET_RUNTIME_URL=http://127.0.0.1:8081 \
FROGLET_PROVIDER_AUTH_TOKEN_PATH=./data/runtime/froglet-control.token \
FROGLET_RUNTIME_AUTH_TOKEN_PATH=./data/runtime/auth.token \
  node integrations/mcp/froglet/server.js
```

For project-local launch files, use the public helper:

```bash
./scripts/setup-agent.sh --target claude-code
./scripts/setup-agent.sh --target codex
```

## Configuration

All configuration is through environment variables:

| Variable | Required | Description |
|----------|----------|-------------|
| `FROGLET_PROVIDER_URL` | Yes | Provider base URL (fallback: `FROGLET_BASE_URL`) |
| `FROGLET_RUNTIME_URL` | Yes | Runtime base URL (fallback: `FROGLET_BASE_URL`) |
| `FROGLET_PROVIDER_AUTH_TOKEN_PATH` | No | Path to provider auth token file |
| `FROGLET_RUNTIME_AUTH_TOKEN_PATH` | No | Path to runtime auth token file |
| `FROGLET_REQUEST_TIMEOUT_MS` | No | HTTP timeout in ms (default: 10000) |
| `FROGLET_DEFAULT_SEARCH_LIMIT` | No | Default search results (default: 10) |
| `FROGLET_MAX_SEARCH_LIMIT` | No | Max search results (default: 50) |

Legacy shortcuts: `FROGLET_BASE_URL` sets both provider and runtime URLs.
`FROGLET_AUTH_TOKEN_PATH` sets both auth token paths.

---

## IDE / Agent Integration

### Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS)
or `%APPDATA%/Claude/claude_desktop_config.json` (Windows):

```json
{
  "mcpServers": {
    "froglet": {
      "command": "node",
      "args": ["<path-to-repo>/integrations/mcp/froglet/server.js"],
      "env": {
        "FROGLET_PROVIDER_URL": "http://127.0.0.1:8080",
        "FROGLET_RUNTIME_URL": "http://127.0.0.1:8081",
        "FROGLET_PROVIDER_AUTH_TOKEN_PATH": "/absolute/path/to/froglet/data/runtime/froglet-control.token",
        "FROGLET_RUNTIME_AUTH_TOKEN_PATH": "/absolute/path/to/froglet/data/runtime/auth.token"
      }
    }
  }
}
```

### Claude Code (CLI)

Drop `.mcp.json` in the project root (already included in this repo):

```json
{
  "mcpServers": {
    "froglet": {
      "type": "stdio",
      "command": "node",
      "args": ["integrations/mcp/froglet/server.js"],
      "env": {
        "FROGLET_PROVIDER_URL": "http://127.0.0.1:8080",
        "FROGLET_RUNTIME_URL": "http://127.0.0.1:8081",
        "FROGLET_PROVIDER_AUTH_TOKEN_PATH": "data/runtime/froglet-control.token",
        "FROGLET_RUNTIME_AUTH_TOKEN_PATH": "data/runtime/auth.token"
      }
    }
  }
}
```

Or generate it directly:

```bash
./scripts/setup-agent.sh --target claude-code
```

Or add via CLI: `claude mcp add froglet -- node integrations/mcp/froglet/server.js`

### Cursor

Add to `.cursor/mcp.json` (project) or `~/.cursor/mcp.json` (global):

```json
{
  "mcpServers": {
    "froglet": {
      "type": "stdio",
      "command": "node",
      "args": ["<path-to-repo>/integrations/mcp/froglet/server.js"],
      "env": {
        "FROGLET_PROVIDER_URL": "http://127.0.0.1:8080",
        "FROGLET_RUNTIME_URL": "http://127.0.0.1:8081",
        "FROGLET_PROVIDER_AUTH_TOKEN_PATH": "/absolute/path/to/froglet/data/runtime/froglet-control.token",
        "FROGLET_RUNTIME_AUTH_TOKEN_PATH": "/absolute/path/to/froglet/data/runtime/auth.token"
      }
    }
  }
}
```

### OpenAI Codex CLI

Add to `~/.codex/config.toml` (global) or `.codex/config.toml` (project):

```toml
[mcp_servers.froglet]
command = "node"
args = ["integrations/mcp/froglet/server.js"]
env = { "FROGLET_PROVIDER_URL" = "http://127.0.0.1:8080", "FROGLET_RUNTIME_URL" = "http://127.0.0.1:8081", "FROGLET_PROVIDER_AUTH_TOKEN_PATH" = "/absolute/path/to/froglet/data/runtime/froglet-control.token", "FROGLET_RUNTIME_AUTH_TOKEN_PATH" = "/absolute/path/to/froglet/data/runtime/auth.token" }
```

Or generate the project-local file:

```bash
./scripts/setup-agent.sh --target codex
```

### Docker

The MCP server is published as `ghcr.io/armanas/froglet-mcp`. No Node.js required.

```bash
# Pull the public image
docker pull ghcr.io/armanas/froglet-mcp:latest

# Run (connects to host Froglet node)
docker run --rm -i \
  -v /absolute/path/to/froglet/data/runtime:/tokens:ro \
  -e FROGLET_PROVIDER_URL=http://host.docker.internal:8080 \
  -e FROGLET_RUNTIME_URL=http://host.docker.internal:8081 \
  -e FROGLET_PROVIDER_AUTH_TOKEN_PATH=/tokens/froglet-control.token \
  -e FROGLET_RUNTIME_AUTH_TOKEN_PATH=/tokens/auth.token \
  ghcr.io/armanas/froglet-mcp:latest
```

Use in any MCP client config:

```json
{
  "mcpServers": {
    "froglet": {
      "command": "docker",
      "args": ["run", "--rm", "-i",
        "-v", "/absolute/path/to/froglet/data/runtime:/tokens:ro",
        "-e", "FROGLET_PROVIDER_URL=http://host.docker.internal:8080",
        "-e", "FROGLET_RUNTIME_URL=http://host.docker.internal:8081",
        "-e", "FROGLET_PROVIDER_AUTH_TOKEN_PATH=/tokens/froglet-control.token",
        "-e", "FROGLET_RUNTIME_AUTH_TOKEN_PATH=/tokens/auth.token",
        "ghcr.io/armanas/froglet-mcp:latest"],
      "type": "stdio"
    }
  }
}
```

Build locally from source:

```bash
docker build -f integrations/mcp/froglet/Dockerfile -t froglet-mcp .
```

---

## Example Config Files

| Platform | File | Format |
|----------|------|--------|
| Claude Desktop | `examples/claude-desktop-config.json` | JSON |
| Cursor | `examples/cursor-mcp-config.json` | JSON |
| Codex CLI | `examples/codex-mcp-config.toml` | TOML |
| Docker | `examples/docker-mcp-config.json` | JSON |
| Claude Code | `.mcp.json` (repo root) | JSON |

The checked-in examples are covered by `integrations/mcp/froglet/test/example-configs.test.mjs`.

## Compose Stack

When running the Docker Compose stack, the MCP server connects to the
locally-bound ports:

```bash
FROGLET_PROVIDER_URL=http://127.0.0.1:8080 \
FROGLET_RUNTIME_URL=http://127.0.0.1:8081 \
FROGLET_PROVIDER_AUTH_TOKEN_PATH=./data/runtime/froglet-control.token \
FROGLET_RUNTIME_AUTH_TOKEN_PATH=./data/runtime/auth.token \
  node integrations/mcp/froglet/server.js
```

If you want to use the generated host-side agent configs against Docker Compose,
start Compose with `FROGLET_HOST_READABLE_CONTROL_TOKEN=true` so
`./data/runtime/froglet-control.token` is readable on the host.

## Tests

```bash
# Unit tests
npm test --prefix integrations/mcp/froglet

# Compose smoke test (requires running stack)
npm run smoke:compose --prefix integrations/mcp/froglet
```

## Troubleshooting

**Connection refused** — Ensure the Froglet provider is running and healthy:
```bash
curl http://127.0.0.1:8080/health
```

**401 Unauthorized** — The endpoint requires an auth token. Set the provider
and runtime token paths to the matching files for the action you are calling:
`FROGLET_PROVIDER_AUTH_TOKEN_PATH=./data/runtime/froglet-control.token` and
`FROGLET_RUNTIME_AUTH_TOKEN_PATH=./data/runtime/auth.token`.

**Timeout errors** — Increase `FROGLET_REQUEST_TIMEOUT_MS` for slow networks
or large responses.

**Docker: connection refused to host** — Use `host.docker.internal` instead
of `127.0.0.1` for URLs when the Froglet node runs on the host machine.
