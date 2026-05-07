# Froglet MCP Server

MCP (Model Context Protocol) server that exposes Froglet services, compute,
and project management to AI agents (Claude, Cursor, Codex, Windsurf, etc.).

## Requirements

- Node.js 18+ (or Docker)
- A running Froglet provider/runtime is required for provider, runtime,
  marketplace, payment, and publication actions
- Use the public `llms.txt` HTTP flow when you only want the no-install hosted
  proof

## Quick Start

### No-clone local node

For a user who wants a local Froglet node plus MCP config:

```bash
curl -fsSL https://froglet.dev/agent | bash
```

That bootstrap installs the signed `froglet-node`, starts provider/runtime from
published GHCR images matching the latest release, and writes MCP config backed
by the published `froglet-mcp` image. After health passes, use MCP actions for service
publication, invocation, public registration, and payment setup.

### Local npm profile

```bash
npx froglet-mcp
```

The npm package defaults to `FROGLET_PROFILE=local` with provider/runtime URLs
pointing at `http://127.0.0.1:8080` and `http://127.0.0.1:8081`. Agents should
call `status` first. If the local node or token files are missing, call
`plan_install` and then `get_install_guide` before running host-shell setup
commands. After local health is verified, call `plan_use_case` before
implementing consumer, provider, evidence, payments, batch, or GPU workflows.

### Local source checkout

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

Source checkout is contributor mode. For project-local launch files in a clone,
use the helper:

```bash
cd froglet && ./scripts/setup-agent.sh --target claude-code
cd froglet && ./scripts/setup-agent.sh --target codex
```

Agents should call the Froglet `plan_install` action before local setup when the
user has not specified the target agent, install footprint, role, payment rail,
network mode, marketplace URL, or first use case. If `payment_rail` is omitted,
the tool returns `decision_required`; recommend `none` for the first local demo.
After the profile is confirmed, `get_install_guide` returns the exact
host-shell commands.
After health checks pass, `plan_use_case` returns a bounded first-workflow plan
and names unsupported edges before execution. In particular, true batch
fan-out, GPU scheduling/provider selection, marketplace GPU routing, and
production capacity management are still separate implementation work.
Self-hosted GPU capability metadata, generic-compute offer metadata, Docker
`--gpus all` gating, no-CPU-fallback errors, and one GCP T4 container workload
with a signed receipt are verified for explicitly configured GPU providers.

### Explicit local npm profile

```bash
FROGLET_PROFILE=local \
FROGLET_PROVIDER_URL=http://127.0.0.1:8080 \
FROGLET_RUNTIME_URL=http://127.0.0.1:8081 \
FROGLET_PROVIDER_AUTH_TOKEN_PATH=/absolute/path/to/froglet/data/runtime/froglet-control.token \
FROGLET_RUNTIME_AUTH_TOKEN_PATH=/absolute/path/to/froglet/data/runtime/auth.token \
  npx froglet-mcp
```

## Configuration

All configuration is through environment variables:

| Variable | Required | Description |
|----------|----------|-------------|
| `FROGLET_PROFILE` | No | `local` by default |
| `FROGLET_PROVIDER_URL` | No | Provider base URL (fallback: `FROGLET_BASE_URL`; default: `http://127.0.0.1:8080`) |
| `FROGLET_RUNTIME_URL` | No | Runtime base URL (fallback: `FROGLET_BASE_URL`; default: `http://127.0.0.1:8081`) |
| `FROGLET_MARKETPLACE_URL` | No | Marketplace API base URL for `marketplace_register` (default: `https://marketplace.froglet.dev`) |
| `FROGLET_MARKETPLACE_ARBITER_URL` | No | Marketplace arbiter base URL for complaint filing/reads (default: `https://arbiter.froglet.dev`) |
| `FROGLET_PROVIDER_AUTH_TOKEN_PATH` | No | Path to provider auth token file |
| `FROGLET_RUNTIME_AUTH_TOKEN_PATH` | No | Path to runtime auth token file |
| `FROGLET_REQUEST_TIMEOUT_MS` | No | HTTP timeout in ms (default: 10000) |
| `FROGLET_DEFAULT_SEARCH_LIMIT` | No | Default search results (default: 10) |
| `FROGLET_MAX_SEARCH_LIMIT` | No | Max search results (default: 50) |
| `FROGLET_EGRESS_MODE` | No | `strict` applies the same DNS-pinning + SSRF validation used for LLM-controlled URLs to operator-configured `FROGLET_PROVIDER_URL` / `FROGLET_RUNTIME_URL`. Use when the operator host sits behind public DNS and you want uniform rebind-resistance. Lenient mode (the default) keeps operator-configured local/dev HTTP topologies working, including loopback and Docker host bridges such as `http://host.docker.internal:8080`. |

Legacy shortcuts: `FROGLET_BASE_URL` sets both provider and runtime URLs.
`FROGLET_AUTH_TOKEN_PATH` sets both auth token paths.

Actions that hit provider/runtime APIs require the matching token path at call
time. `plan_install`, `get_install_guide`, and `plan_use_case` do not require
local token files.
The hosted demo is intentionally not an MCP action; use
`https://froglet.dev/llms.txt` for the no-install proof.

Marketplace registration helpers:

- `marketplace_register` posts a public HTTPS or Tor provider URL to
  `/v1/registrations`.
- `registration_transport=tor` is required for onion registration.
- `marketplace_domain_claim` and `marketplace_domain_complete` claim a
  Froglet-managed `*.providers.froglet.dev` hostname before HTTPS registration.
- General `provider_url` overrides still reject onion URLs outside the
  controlled registration path.

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
cd froglet && ./scripts/setup-agent.sh --target claude-code
```

Or add via CLI: `claude mcp add froglet -- node integrations/mcp/froglet/server.js`

### Cursor

This repo includes a project config at `.cursor/mcp.json` that runs the
published package with local defaults. Add the same shape to
`~/.cursor/mcp.json` if you want a global config:

```json
{
  "mcpServers": {
    "froglet": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "froglet-mcp"],
      "env": {
        "FROGLET_PROFILE": "local",
        "FROGLET_PROVIDER_URL": "http://127.0.0.1:8080",
        "FROGLET_RUNTIME_URL": "http://127.0.0.1:8081",
        "FROGLET_PROVIDER_AUTH_TOKEN_PATH": "data/runtime/froglet-control.token",
        "FROGLET_RUNTIME_AUTH_TOKEN_PATH": "data/runtime/auth.token"
      }
    }
  }
}
```

Cursor host verification requires Cursor or `cursor-agent` to be installed. If
neither is available, use the MCP server tests and direct stdio smoke as the
available non-host evidence.

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
cd froglet && ./scripts/setup-agent.sh --target codex
```

### Docker

The MCP server is published as `ghcr.io/armanas/froglet-mcp`. No Node.js required.

```bash
# Pull the public image
docker pull ghcr.io/armanas/froglet-mcp:latest

# Run (connects to a Froglet node reachable from inside the container)
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

## Publishing

The npm package is defined at the repository root so the tarball can include
both `integrations/mcp/froglet` and `integrations/shared/froglet-lib`.

```bash
npm run check:mcp
npm run test:mcp
npm pack --dry-run
npm publish --provenance=false
```

Do not publish `integrations/mcp/froglet/package.json` directly; it is a
repo-local development package and cannot include the shared library by itself.
Local manual publishes must disable provenance. Use CI/OIDC for a later
provenance-enabled publish flow.

### MCP Registry

The official MCP Registry entry is driven by the repo-root `server.json`.
Publish the matching npm version first because the registry validates the
package `mcpName` against `server.json`.

```bash
npm publish --provenance=false --otp <npm-otp>
mcp-publisher login github
mcp-publisher publish
curl "https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.armanas/froglet"
```

Expected proof: the registry search returns `io.github.armanas/froglet` with
package identifier `froglet-mcp`.

If you want to use the generated host-side agent configs against Docker Compose,
start Compose with `FROGLET_HOST_READABLE_CONTROL_TOKEN=true` so
`./data/runtime/froglet-control.token` is readable on the host.
The checked-in Compose stack also points the runtime at the default public read
marketplace, so `discover_services` works without running a local marketplace.

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
Those operator-configured Docker bridge URLs are accepted in the MCP server's
default lenient mode.
