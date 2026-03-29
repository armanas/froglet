# Docker

Froglet ships four image targets:

- `provider`
- `runtime`
- `discovery`
- `operator`

Image contents:

- `provider` and `operator` include `python3`
- `runtime` and `discovery` stay on the slimmer non-Python base

`python3` is required in the Docker targets that execute Python-backed projects:

- `provider` runs published Python services
- `operator` runs local project checks such as `test_project`

## Default Local Stack

```bash
docker compose up --build
```

That starts:

- discovery on `127.0.0.1:9090`
- provider on `127.0.0.1:8080`
- runtime on `127.0.0.1:8081`
- operator on `127.0.0.1:9191`

Host token path:

- `./data/runtime/froglet-control.token`

The default Compose file no longer makes that token host-readable. Set
`FROGLET_HOST_READABLE_CONTROL_TOKEN=true` only for explicit local-development
workflows that need direct host access to the provider control token.

This is the default local development topology and the one used by the OpenClaw
and MCP compose smoke coverage.

Compose-backed bot-surface smoke commands:

```bash
node integrations/openclaw/froglet/test/compose-smoke.mjs
node integrations/mcp/froglet/test/compose-smoke.mjs
```

## Single-Role Compose Files

- `compose.discovery.yaml`
- `compose.provider.yaml`
- `compose.runtime.yaml`

Examples:

```bash
docker compose -f compose.discovery.yaml up --build
docker compose -f compose.provider.yaml up --build
docker compose -f compose.runtime.yaml up --build
```

## Direct Image Builds

```bash
docker build --target provider -t froglet-provider:local .
docker build --target runtime -t froglet-runtime:local .
docker build --target discovery -t froglet-discovery:local .
docker build --target operator -t froglet-operator:local .
```

## Role Defaults

Provider image:

- public API on `:8080`
- no public runtime listener
- includes `python3` for published Python-backed services

Runtime image:

- runtime API on `:8081`
- no public provider listener

Discovery image:

- discovery API on `:9090`

Operator image:

- operator API on `:9191`
- includes `python3` for local Python project test/build flows

Use environment variables exactly as you would outside Docker to point the provider at discovery and the runtime at discovery.
