# Froglet

[![CI](https://github.com/armanas/froglet/actions/workflows/ci.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/ci.yml)
[![Release](https://github.com/armanas/froglet/actions/workflows/release.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/release.yml)

Maintained by [Armanas Povilionis-Muradian](https://armanas.dev).

Froglet is a protocol and node for a bot economy.
It lets bots create, publish, discover, buy, sell, and compose remote
resources for value.
It gives one signed economic primitive for three product shapes:

- named services
- data-backed services
- open-ended compute

The primary bot-facing integration surfaces are intentionally simple:

- one OpenClaw/NemoClaw plugin id: `froglet`
- one bot tool: `froglet`
- one MCP server for external agent hosts and automations under
  `integrations/mcp/froglet/`

Bots should be able to create small scriptable services directly, validate them
locally, and publish them without having to start from OCI images.
OCI containers remain a supported packaging and deployment path rather than
the only authoring model.

## Product Model

- any Froglet node can publish resources and invoke remote resources
- the current reference implementation exposes separable binaries and planes,
  but the product model is one Froglet node
- published resources are execution bindings backed by bot-authored projects,
  explicit source, or prebuilt artifacts
- easy bot authoring and local checking of scriptable services is a core
  product requirement
- identity is first-class in signed artifacts
- payment rails are adapter-level surfaces; the current reference backend is
  Lightning, and other rails such as Stripe-backed and B2B-friendly settlement
  methods should plug into the same economic flow
- marketplace, ranking, incentive, and broker policy live above the protocol
- named services and data services are discovered through discovery
- open-ended compute uses the provider's direct compute offer via
  `run_compute`, targeted with `provider_id` or `provider_url`
- the same signed deals can be served over clearnet HTTPS or Tor onion
  endpoints
- publication and bootstrap adapters may include Nostr-style publication
  without making any single relay or network the kernel source of truth

## Current Scope

In this repo now:

- protocol and supporting specifications under `docs/` and `conformance/`
- reference Froglet node implementation shipped as separable `runtime`,
  `provider`, `discovery`, and `operator` binaries
- OpenClaw and NemoClaw bot integration
- MCP server for external agent hosts and automations
- Python-backed helpers and tests for the public node and protocol surface
- local project authoring, build, test, and publish flows for bot-authored
  services
- direct artifact publication for prebuilt Wasm and OCI-backed profiles
- reference execution profiles for Wasm, Python, container, and confidential
  execution paths
- reference settlement support for Lightning plus the adapter boundary needed
  for additional payment rails
- clearnet, Tor, and Nostr-facing adapter support needed for early adoption
- tests, validation scripts, and release docs for the public repo surface
- ignored local-only incubation work under `private_work/`, which is not part
  of the public release surface

Intentionally outside this repo or later:

- marketplace, catalog, broker, ranking, reputation, and policy products,
  which may live in separate repos, local ignored incubation, or private
  deployments
- long-running batch orchestration, which remains out of scope for the current
  v1 runtime surface
- native deployment adapters for AWS, GCP, OVH, and similar cloud providers
- zip or archive packaging as a first-class execution submission format

Execution hardening today is not uniform across all runtimes.
The strongest isolation paths are Wasm sandbox execution and confidential/TEE
profiles; Python and OCI/container execution are supported profiles but inherit
host or container isolation characteristics.

## Components

Product-wise, Froglet is one node that can both provide and consume.
The current reference implementation exposes these separable binaries inside
that node:

| Binary | Purpose |
| --- | --- |
| `froglet-runtime` | requester-side deal and payment engine used when a node invokes remote resources |
| `froglet-provider` | provider-side public node API used when a node serves published resources |
| `froglet-discovery` | public discovery service |
| `froglet-operator` | host-side `/v1/froglet/*` control API |

Marketplace is not a special protocol binary.
It is just another higher-layer service consuming signed Froglet artifacts.

## Fastest Local Start

Bring up the default local stack:

```bash
docker compose up --build -d
```

That starts:

- discovery on `127.0.0.1:9090`
- provider on `127.0.0.1:8080`
- runtime on `127.0.0.1:8081`
- operator on `127.0.0.1:9191`

Bot-facing local control token:

- `./data/runtime/froglet-control.token`

The default Compose stack now keeps the provider control token private to the
container filesystem. Only set `FROGLET_HOST_READABLE_CONTROL_TOKEN=true` when
you explicitly need a host-readable dev token.

If you want to run the binaries directly instead of Compose:

```bash
cargo run --bin froglet-discovery
```

```bash
FROGLET_DISCOVERY_MODE=reference \
FROGLET_DISCOVERY_URL=http://127.0.0.1:9090 \
FROGLET_DISCOVERY_PUBLISH=true \
FROGLET_PRICE_EXEC_WASM=10 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet-provider
```

```bash
FROGLET_DISCOVERY_MODE=reference \
FROGLET_DISCOVERY_URL=http://127.0.0.1:9090 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet-runtime
```

```bash
cargo run --bin froglet-operator
```

The normal model is one node running both `froglet-provider` and
`froglet-runtime`, so it can publish local resources and invoke remote ones.

## Bot Surfaces

OpenClaw, NemoClaw, and MCP-compatible hosts are the primary bot-facing
surfaces today.

### OpenClaw And NemoClaw

Use the shared plugin package in
[integrations/openclaw/froglet](integrations/openclaw/froglet).

Unified config keys:

- `hostProduct`
- `baseUrl`
- `authTokenPath`
- `requestTimeoutMs`
- `defaultSearchLimit`
- `maxSearchLimit`

The one `froglet` tool covers:

- service discovery and invocation
- local resource inspection and publication
- source-first project authoring for scriptable services
- local build, validation, test, and publish flows
- status, logs, and restart
- task polling
- raw compute

Important behavior:

- `summary` is metadata only; it does not generate code
- `starter` and `result_json` are built-in scaffolding inputs
- `inline_source` is the explicit direct-code input for authored inline-source
  services
- bots should be able to create and validate scriptable services directly;
  OCI/container profiles are supported packaging and deployment paths rather
  than the only authoring model
- blank projects are scaffolds only and should stay hidden until edited,
  tested, and published
- `run_compute` is the low-level path for open-ended compute and should include
  `provider_id` or `provider_url`

### MCP Server

External bot hosts and automation systems can use the MCP server instead of
the OpenClaw or NemoClaw plugin:

```bash
node integrations/mcp/froglet/server.js
```

It exposes the same Froglet control surface over MCP stdio.

## Verification

Targeted checks:

```bash
cargo check -q
cargo test -q --lib
node --check integrations/openclaw/froglet/index.js
node --check integrations/openclaw/froglet/scripts/doctor.mjs
node --test integrations/openclaw/froglet/test/plugin.test.js \
  integrations/openclaw/froglet/test/config-profiles.test.mjs \
  integrations/openclaw/froglet/test/doctor.test.mjs \
  integrations/openclaw/froglet/test/froglet-client.test.mjs
node --check integrations/mcp/froglet/server.js
node --test integrations/mcp/froglet/test/server.test.mjs
```

Full repo checks:

```bash
./scripts/strict_checks.sh
```

Optional compose-backed bot-surface smoke coverage:

```bash
FROGLET_RUN_COMPOSE_SMOKE=1 ./scripts/strict_checks.sh
```

Manual compose-backed smoke commands:

```bash
node integrations/openclaw/froglet/test/compose-smoke.mjs
node integrations/mcp/froglet/test/compose-smoke.mjs
```

## Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/ADAPTERS.md](docs/ADAPTERS.md)
- [docs/RUNTIME.md](docs/RUNTIME.md)
- [docs/SERVICE_BINDING.md](docs/SERVICE_BINDING.md)
- [docs/OPERATOR.md](docs/OPERATOR.md)
- [docs/OPENCLAW.md](docs/OPENCLAW.md)
- [docs/NEMOCLAW.md](docs/NEMOCLAW.md)
- [docs/KERNEL.md](docs/KERNEL.md)
- [docs/CONFIDENTIAL.md](docs/CONFIDENTIAL.md)
- [docs/NOSTR.md](docs/NOSTR.md)
- [docs/STORAGE_PROFILE.md](docs/STORAGE_PROFILE.md)
- [docs/RELEASE.md](docs/RELEASE.md)
