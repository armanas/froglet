# Froglet

[![CI](https://github.com/armanas/froglet/actions/workflows/ci.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/ci.yml)
[![Release](https://github.com/armanas/froglet/actions/workflows/release.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/release.yml)

Froglet is a signed-deal execution protocol with a simple bot-facing model:

- one OpenClaw/NemoClaw plugin id: `froglet`
- one bot tool: `froglet`
- named services plus expert raw compute
- services are just code/projects, not templates
- any Froglet node can publish services and invoke services

## Core Binaries

| Binary | Purpose |
| --- | --- |
| `froglet-runtime` | deal and payment engine used when a node invokes remote services |
| `froglet-provider` | public service API used when a node serves local services |
| `froglet-discovery` | public discovery service |
| `froglet-operator` | host-side `/v1/froglet/*` control API |

Marketplace is no longer a special product binary. It is just Froglet services
published by a Froglet node.

## Quick Start

Start discovery:

```bash
cargo run --bin froglet-discovery
```

Start the public node API:

```bash
FROGLET_DISCOVERY_MODE=reference \
FROGLET_DISCOVERY_URL=http://127.0.0.1:9090 \
FROGLET_DISCOVERY_PUBLISH=true \
FROGLET_PRICE_EXEC_WASM=10 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet-provider
```

Start the deal/payment runtime:

```bash
FROGLET_DISCOVERY_MODE=reference \
FROGLET_DISCOVERY_URL=http://127.0.0.1:9090 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet-runtime
```

The same node can run both `froglet-provider` and `froglet-runtime`. That is
the normal model: one Froglet node can publish local services and invoke remote
services.

Start the local control API:

```bash
cargo run --bin froglet-operator
```

## OpenClaw And NemoClaw

Use the shared plugin package in
[integrations/openclaw/froglet](/Users/armanas/Projects/github.com/armanas/froglet/integrations/openclaw/froglet).

The plugin config is now unified:

- `hostProduct`
- `baseUrl`
- `authTokenPath`
- `requestTimeoutMs`
- `defaultSearchLimit`
- `maxSearchLimit`

The tool surface is unified too. The one `froglet` tool supports actions for:

- service discovery and invocation
- local service inspection and publication
- project authoring
- build/test/publish
- status/logs/restart
- task polling
- raw compute

See:

- [docs/OPENCLAW.md](docs/OPENCLAW.md)
- [docs/NEMOCLAW.md](docs/NEMOCLAW.md)
- [docs/OPERATOR.md](docs/OPERATOR.md)

## Verification

```bash
cargo check -q
cargo test -q --lib
node --check integrations/openclaw/froglet/index.js
node --check integrations/openclaw/froglet/scripts/doctor.mjs
node --test integrations/openclaw/froglet/test/plugin.test.js \
  integrations/openclaw/froglet/test/config-profiles.test.mjs \
  integrations/openclaw/froglet/test/doctor.test.mjs
```
