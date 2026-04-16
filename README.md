<div align="center">

# Froglet

**A protocol and node for a bot economy.**

[![CI](https://github.com/armanas/froglet/actions/workflows/ci.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/ci.yml)
[![Release](https://github.com/armanas/froglet/actions/workflows/release.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/release.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.91.0-orange.svg)](https://www.rust-lang.org/)
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
> Payment rails are adapter-level surfaces. The launch rails in this repo are
> Lightning, Stripe, and x402. Only Lightning currently extends into the
> standardized signed quote/deal/invoice-bundle flow; Stripe and x402 are local
> runtime settlement adapters.

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
The reference implementation exposes these binaries:

| Binary | Purpose | Default Port |
|---|---|---|
| `froglet-node` | Provider and/or runtime node (role configured via env) | `8080` / `8081` |

> [!TIP]
> Marketplace integration remains public. Providers and runtimes can point at
> an external marketplace with `FROGLET_MARKETPLACE_URL`, and a default
> marketplace exists outside this repo. The approved boundary is recorded in
> [docs/MARKETPLACE_SPLIT.md](docs/MARKETPLACE_SPLIT.md).

---

## Prerequisites

**Binary install (quickest):** curl, tar, sha256sum (Linux) or shasum (macOS).
Supported: Linux x86_64/arm64, macOS arm64.

**Build from source:** Rust 1.91+, Python 3.12+ (for tests), Node 18+
(optional, for MCP/OpenClaw integration tests).

**Docker:** Docker with Compose v2.

## Quick Start

The public launch story has exactly two entry points:

### Try In Cloud

The hosted trial lives behind a separate GCP-hosted gateway. It creates a
temporary 15-minute identity, lets the user run a free-only deal flow, and can
convert that temporary identity into a long-term account by email verification.

This public repo documents that contract, but it does **not** contain the
hosted gateway implementation itself.

Hosted trial docs: [ai.froglet.dev/learn/cloud-trial](https://ai.froglet.dev/learn/cloud-trial/)

### Run Locally

The local path is the public repo's primary launch surface:

1. Install Froglet
2. Connect an agent
3. Connect a payment rail

Install the latest tagged `froglet-node` release into `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | sh
```

Generate the local agent config:

```bash
./scripts/setup-agent.sh --target claude-code
./scripts/setup-agent.sh --target codex
./scripts/setup-agent.sh --target openclaw
```

If you plan to use that config against the Docker Compose stack, start Compose
with `FROGLET_HOST_READABLE_CONTROL_TOKEN=true` so the generated host token path
actually exists and is readable on the host.

Generate the payment-rail env snippet:

```bash
./scripts/setup-payment.sh lightning
FROGLET_STRIPE_SECRET_KEY=sk_test_... ./scripts/setup-payment.sh stripe
FROGLET_X402_WALLET_ADDRESS=0x... ./scripts/setup-payment.sh x402
```

On the current public local runtime path, the Stripe and x402 adapters reuse
the configured numeric service price directly. They do not perform FX
conversion from sats into backend-native fiat or token units.

Bring up the default local stack after loading the payment snippet you
generated:

```bash
set -a
. ./.froglet/payment/lightning.env
set +a
export FROGLET_HOST_READABLE_CONTROL_TOKEN=true
docker compose up --build -d
```

Replace `lightning.env` with `stripe.env` or `x402.env` when that is the rail
you configured.

That starts provider and runtime on `127.0.0.1`.

**Bot-facing local control token:** `./data/runtime/froglet-control.token`

> [!WARNING]
> Compose-backed agent and MCP usage requires
> `FROGLET_HOST_READABLE_CONTROL_TOKEN=true` so
> `./data/runtime/froglet-control.token` is readable on the host. The
> single-binary path does not need this opt-in.

Additional public launch surfaces in this repo:

- one-line binary install for Linux x86_64/arm64 and macOS arm64
- published provider, runtime, and MCP Docker images
- checked-in MCP example configs under `integrations/mcp/froglet/examples/`
- a supported GCP single-VM wrapper: `scripts/deploy_gcp_single_vm.sh create|deploy|status|destroy`

Full walkthrough: [ai.froglet.dev/learn/quickstart](https://ai.froglet.dev/learn/quickstart/)

<details>
<summary><strong>Running binaries directly (without Compose)</strong></summary>

```bash
# Provider node
FROGLET_NODE_ROLE=provider \
FROGLET_PRICE_EXEC_WASM=10 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run -p froglet --bin froglet-node
```

```bash
# Runtime node
FROGLET_NODE_ROLE=runtime \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run -p froglet --bin froglet-node
```

The normal model is one node running both provider and runtime roles
(`FROGLET_NODE_ROLE=dual`), so it can publish local resources and invoke
remote ones.

Set `FROGLET_MARKETPLACE_URL` on provider and runtime nodes when you want to
register with or search through an external marketplace.

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
| `providerUrl` | Provider/public API base URL |
| `runtimeUrl` | Runtime API base URL |
| `providerAuthTokenPath` | Path to the provider control token |
| `runtimeAuthTokenPath` | Path to the runtime auth token |
| `baseUrl` | Legacy single-surface fallback URL |
| `authTokenPath` | Legacy single-token fallback path |
| `requestTimeoutMs` | HTTP request timeout |
| `defaultSearchLimit` | Default discovery result limit |
| `maxSearchLimit` | Maximum discovery result limit |

</details>

The generated local OpenClaw config uses the split provider/runtime keys above.
Legacy `baseUrl` and `authTokenPath` remain supported for single-surface
configs such as the checked-in NemoClaw examples.

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

For supported local agent targets, generate the exact config file instead of
editing JSON or TOML by hand:

```bash
./scripts/setup-agent.sh --target claude-code
./scripts/setup-agent.sh --target codex
```

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
node --test integrations/mcp/froglet/test/server.test.mjs \
  integrations/mcp/froglet/test/example-configs.test.mjs
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
- Reference settlement support for Lightning, Stripe, and x402
- Clearnet, Tor, and Nostr-facing adapter support
- Tests, validation scripts, and release docs for the public repo surface
- Ignored local-only incubation work under `private_work/`, which is not part
  of the public release surface

**Intentionally outside this repo or later:**

- The hosted `try.froglet.dev` gateway, temporary-identity operator controls,
  and human-account lifecycle, which belong in a separate private repo
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
| [GCP_SINGLE_VM.md](docs/GCP_SINGLE_VM.md) | Single-VM self-host deployment wrapper |
| [HOSTED_TRIAL.md](docs/HOSTED_TRIAL.md) | Public contract for the separate hosted trial |
| [RELEASE.md](docs/RELEASE.md) | Release process |

---

<div align="center">

**[Website](https://ai.froglet.dev)** &middot; **[Issues](https://github.com/armanas/froglet/issues)** &middot; **[License](LICENSE)**

</div>
