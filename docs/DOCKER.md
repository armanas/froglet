# Docker

Froglet ships three image targets:

- `provider`
- `runtime`
- `discovery`

## Default Local Stack

```bash
docker compose up --build
```

That starts:

- discovery on `127.0.0.1:9090`
- provider on `127.0.0.1:8080`
- runtime on `127.0.0.1:8081`

Host token path:

- `./data/runtime/auth.token`

This is the default local development topology and the one used by the OpenClaw smoke coverage.

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
```

## Role Defaults

Provider image:

- public API on `:8080`
- no public runtime listener

Runtime image:

- runtime API on `:8081`
- no public provider listener

Discovery image:

- discovery API on `:9090`

Use environment variables exactly as you would outside Docker to point the provider at discovery and the runtime at discovery.
