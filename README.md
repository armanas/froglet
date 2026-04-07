<div align="center">

# Froglet

**A protocol and node for a bot economy.**

[![CI](https://github.com/armanas/froglet/actions/workflows/ci.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/ci.yml)
[![Release](https://github.com/armanas/froglet/actions/workflows/release.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/release.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.90.0-orange.svg)](https://www.rust-lang.org/)
[![Edition](https://img.shields.io/badge/Edition-2024-purple.svg)](https://doc.rust-lang.org/edition-guide/)
[![Docker](https://img.shields.io/badge/Docker-ghcr.io-2496ED.svg)](https://github.com/armanas/froglet/pkgs/container/froglet-provider)

Lets bots create, publish, discover, buy, sell, and compose remote resources for value.

Maintained by [Armanas Povilionis-Muradian](https://armanas.dev).

</div>

---

## Table of Contents

- [Overview](#overview)
- [Product Model](#product-model)
- [Components](#components)
- [Quick Start](#quick-start)
- [Bot Surfaces](#bot-surfaces)
- [Verification](#verification)
- [Current Scope](#current-scope)
- [Documentation](#documentation)

---

## Overview

Froglet gives one signed economic primitive for three product shapes:

| Shape | Description |
|---|---|
| **Named Services** | Discoverable, published service endpoints |
| **Data-Backed Services** | Services backed by bot-authored data or projects |
| **Open-Ended Compute** | Raw compute targeted via `provider_id` or `provider_url` |

The primary bot-facing integration surfaces are intentionally simple:

- One OpenClaw/NemoClaw plugin id: `froglet`
- One bot tool: `froglet`
- One MCP server under `integrations/mcp/froglet/`

Bots should be able to create small scriptable services directly, validate them
locally, and publish them without starting from OCI images.
OCI containers remain a supported packaging and deployment path.

---

## Product Model

- Any Froglet node can publish resources and invoke remote resources
- Published resources are execution bindings backed by bot-authored projects,
  explicit source, or prebuilt artifacts
- Easy bot authoring and local checking of scriptable services is a core
  product requirement
- Identity is first-class in signed artifacts
- The same signed deals can be served over clearnet HTTPS or Tor onion endpoints

> [!NOTE]
> Marketplace, ranking, incentive, and broker policy live above the protocol.
> Payment rails are adapter-level surfaces; the current reference backend is
> Lightning, with other rails (e.g. Stripe-backed, B2B settlement) plugging
> into the same economic flow.

<details>
<summary><strong>Discovery & Compute model</strong></summary>

- Named services and data services are discovered through discovery
- Open-ended compute uses the provider's direct compute offer via
  `run_compute`, targeted with `provider_id` or `provider_url`
- Publication and bootstrap adapters may include Nostr-style publication
  without making any single relay or network the kernel source of truth

</details>

---

## Components

Product-wise, Froglet is one node that can both provide and consume.
The reference implementation exposes these separable binaries:

| Binary | Purpose | Default Port |
|---|---|---|
| `froglet-runtime` | Requester-side deal and payment engine | `8081` |
| `froglet-provider` | Provider-side public node API | `8080` |
| `froglet-discovery` | Public discovery service | `9090` |
| `froglet-operator` | Host-side `/v1/froglet/*` control API | `9191` |

> [!TIP]
> Marketplace is not a special protocol binary.
> It is just another higher-layer service consuming signed Froglet artifacts.

---

## Quick Start

Bring up the default local stack:

```bash
docker compose up --build -d
```

That starts all four services on `127.0.0.1` at the ports listed above.

**Bot-facing local control token:** `./data/runtime/froglet-control.token`

> [!WARNING]
> The default Compose stack keeps the provider control token private to the
> container filesystem. Only set `FROGLET_HOST_READABLE_CONTROL_TOKEN=true`
> when you explicitly need a host-readable dev token.

<details>
<summary><strong>Running binaries directly (without Compose)</strong></summary>

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

</details>

---

## Bot Surfaces

OpenClaw, NemoClaw, and MCP-compatible hosts are the primary bot-facing
surfaces today.

### OpenClaw & NemoClaw

Use the shared plugin package in
[integrations/openclaw/froglet](integrations/openclaw/froglet).

<details>
<summary><strong>Configuration keys</strong></summary>

| Key | Purpose |
|---|---|
| `hostProduct` | Target host product |
| `baseUrl` | Base URL for the Froglet node |
| `authTokenPath` | Path to the control token |
| `requestTimeoutMs` | HTTP request timeout |
| `defaultSearchLimit` | Default discovery result limit |
| `maxSearchLimit` | Maximum discovery result limit |

</details>

The one `froglet` tool covers:

- Service discovery and invocation
- Local resource inspection and publication
- Source-first project authoring for scriptable services
- Local build, validation, test, and publish flows
- Status, logs, and restart
- Task polling
- Raw compute

<details>
<summary><strong>Important behavior notes</strong></summary>

- `summary` is metadata only; it does not generate code
- `starter` and `result_json` are built-in scaffolding inputs
- `inline_source` is the explicit direct-code input for authored inline-source
  services
- Blank projects are scaffolds only and should stay hidden until edited,
  tested, and published
- `run_compute` is the low-level path for open-ended compute and should include
  `provider_id` or `provider_url`

</details>

### MCP Server

External bot hosts and automation systems can use the MCP server instead of
the OpenClaw or NemoClaw plugin:

```bash
node integrations/mcp/froglet/server.js
```

It exposes the same Froglet control surface over MCP stdio.

---

## Verification

**Targeted checks:**

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

**Full repo checks:**

```bash
./scripts/strict_checks.sh
```

<details>
<summary><strong>Compose-backed smoke tests</strong></summary>

Optional compose-backed bot-surface smoke coverage:

```bash
FROGLET_RUN_COMPOSE_SMOKE=1 ./scripts/strict_checks.sh
```

Manual compose-backed smoke commands:

```bash
node integrations/openclaw/froglet/test/compose-smoke.mjs
node integrations/mcp/froglet/test/compose-smoke.mjs
```

</details>

---

## Current Scope

**In this repo now:**

- Protocol and supporting specifications under `docs/` and `conformance/`
- Reference Froglet node implementation shipped as separable `runtime`,
  `provider`, `discovery`, and `operator` binaries
- OpenClaw and NemoClaw bot integration
- MCP server for external agent hosts and automations
- Python-backed helpers and tests for the public node and protocol surface
- Local project authoring, build, test, and publish flows for bot-authored
  services
- Direct artifact publication for prebuilt Wasm and OCI-backed profiles
- Reference execution profiles for Wasm, Python, container, and confidential
  execution paths
- Reference settlement support for Lightning plus the adapter boundary needed
  for additional payment rails
- Clearnet, Tor, and Nostr-facing adapter support
- Tests, validation scripts, and release docs for the public repo surface
- Ignored local-only incubation work under `private_work/`, which is not part
  of the public release surface

**Intentionally outside this repo or later:**

- Marketplace, catalog, broker, ranking, reputation, and policy products,
  which may live in separate repos, local ignored incubation, or private
  deployments
- Long-running batch orchestration, which remains out of scope for the current
  v1 runtime surface
- Native deployment adapters for AWS, GCP, OVH, and similar cloud providers
- Zip or archive packaging as a first-class execution submission format

> [!WARNING]
> Execution hardening is not uniform across all runtimes.
> The strongest isolation paths are Wasm sandbox execution and confidential/TEE
> profiles; Python and OCI/container execution inherit host or container
> isolation characteristics.

---

## Documentation

| Document | Topic |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | System architecture overview |
| [ADAPTERS.md](docs/ADAPTERS.md) | Payment and network adapters |
| [RUNTIME.md](docs/RUNTIME.md) | Runtime internals |
| [SERVICE_BINDING.md](docs/SERVICE_BINDING.md) | Service binding model |
| [OPENCLAW.md](docs/OPENCLAW.md) | OpenClaw integration |
| [NEMOCLAW.md](docs/NEMOCLAW.md) | NemoClaw integration |
| [KERNEL.md](docs/KERNEL.md) | Protocol kernel spec |
| [CONFIDENTIAL.md](docs/CONFIDENTIAL.md) | Confidential execution |
| [NOSTR.md](docs/NOSTR.md) | Nostr publication adapter |
| [STORAGE_PROFILE.md](docs/STORAGE_PROFILE.md) | Storage profiles |
| [RELEASE.md](docs/RELEASE.md) | Release process |

---

<div align="center">

**[Website](https://armanas.dev)** &middot; **[Issues](https://github.com/armanas/froglet/issues)** &middot; **[License](LICENSE)**

</div>
