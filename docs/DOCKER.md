# Docker

Froglet ships these public image targets:

- `provider` — provider-mode froglet-node (includes `python3`)
- `runtime` — runtime-mode froglet-node
- `dual` — both provider and runtime in one container
- `marketplace` — froglet-marketplace (Postgres-backed search/registration)
- `froglet-mcp` — MCP server image published to GHCR

## Default Local Stack

```bash
docker compose up --build
```

That starts:

- postgres on `127.0.0.1:5432`
- marketplace on `127.0.0.1:8090`
- provider on `127.0.0.1:8080`
- runtime on `127.0.0.1:8081`

Host token path:

- `./data/runtime/froglet-control.token`

The default Compose file does not make that token host-readable. Set
`FROGLET_HOST_READABLE_CONTROL_TOKEN=true` whenever a host-side agent or MCP
client needs direct access to `./data/runtime/froglet-control.token`.

This is the default local development topology and the one used by the OpenClaw
and MCP compose smoke coverage.

Compose-backed bot-surface smoke commands:

```bash
node integrations/openclaw/froglet/test/compose-smoke.mjs
node integrations/mcp/froglet/test/compose-smoke.mjs
```

## Single-Role Compose Files

- `compose.provider.yaml`
- `compose.runtime.yaml`

Examples:

```bash
docker compose -f compose.provider.yaml up --build
docker compose -f compose.runtime.yaml up --build
```

## Direct Image Builds

```bash
docker build --target provider -t froglet-provider:local .
docker build --target runtime -t froglet-runtime:local .
docker build --target dual -t froglet-dual:local .
docker build --target marketplace -t froglet-marketplace:local .
docker build -f integrations/mcp/froglet/Dockerfile -t froglet-mcp:local .
```

## Published Images

The tagged release workflow publishes:

- `ghcr.io/<owner>/froglet-provider:<version>`
- `ghcr.io/<owner>/froglet-runtime:<version>`
- `ghcr.io/<owner>/froglet-marketplace:<version>`
- `ghcr.io/<owner>/froglet-mcp:<version>`

## Role Defaults

Provider image:

- public API on `:8080`
- no public runtime listener
- includes `python3` for published Python-backed services

Runtime image:

- runtime API on `:8081`
- no public provider listener

Marketplace image:

- provider API on `:8080`
- requires `MARKETPLACE_DATABASE_URL` and optionally `MARKETPLACE_FEED_SOURCES`

Use `FROGLET_MARKETPLACE_URL` on provider and runtime nodes to point them at the marketplace for discovery and registration.
