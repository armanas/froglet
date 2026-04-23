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
> Marketplace integration is part of the public Froglet surface. Runtimes can
> point at the default public read marketplace with `FROGLET_MARKETPLACE_URL`;
> provider auto-registration requires a write-capable marketplace endpoint. See
> [docs/MARKETPLACE.md](docs/MARKETPLACE.md).

---

## Prerequisites

**Binary install (quickest):** curl, tar, sha256sum (Linux) or shasum (macOS).
Supported: Linux x86_64/arm64, macOS arm64.

**Build from source:** Rust 1.91+, Python 3.12+ (for tests), Node 18+
(optional, for MCP/OpenClaw integration tests).

**Docker:** Docker with Compose v2.

## Quick Start

Canonical onboarding lives in
[docs-site/src/content/docs/learn/index.mdx](docs-site/src/content/docs/learn/index.mdx).
Use the repo README for the product and codebase overview, the `learn/` docs
for the public launch path, and `docs/` for specs, operator notes, and
integration reference.

The public launch story still has exactly two entry points:

### 1. Try In Cloud

- Start with
  [docs-site/src/content/docs/learn/cloud-trial.mdx](docs-site/src/content/docs/learn/cloud-trial.mdx)
- Contract reference: [docs/HOSTED_TRIAL.md](docs/HOSTED_TRIAL.md)
- Session tokens on `try.froglet.dev` authorize only
  `POST /v1/runtime/deals` and `GET /v1/runtime/deals/{deal_id}`
- `try.froglet.dev` is the only public hosted-trial ingress; `ai.froglet.dev`
  does not expose session minting or hosted demo deal routes directly
- The hosted demo proves one free discover → deal → result → receipt round-trip,
  not paid rails, persistent identity, or general runtime access

### 2. Run Locally

- Start with
  [docs-site/src/content/docs/learn/quickstart.mdx](docs-site/src/content/docs/learn/quickstart.mdx)
- Then use
  [docs-site/src/content/docs/learn/agents.mdx](docs-site/src/content/docs/learn/agents.mdx)
  and
  [docs-site/src/content/docs/learn/payment-rails.mdx](docs-site/src/content/docs/learn/payment-rails.mdx)
- Self-host and operator follow-ons live in [docs/DOCKER.md](docs/DOCKER.md),
  [docs/GCP_SINGLE_VM.md](docs/GCP_SINGLE_VM.md), and
  [docs/MARKETPLACE.md](docs/MARKETPLACE.md)

Minimal full local stack from zero:

```bash
curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | sh
git clone https://github.com/armanas/froglet.git
cd froglet && ./scripts/setup-agent.sh --target claude-code
cd froglet && ./scripts/setup-payment.sh lightning
cd froglet && set -a && . ./.froglet/payment/lightning.env && export FROGLET_HOST_READABLE_CONTROL_TOKEN=true && set +a && docker compose up --build -d
```

If you only want the signed binary:

```bash
curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | sh
```

The Compose path and generated host-side agent configs depend on
`FROGLET_HOST_READABLE_CONTROL_TOKEN=true`; the quickstart page carries the
full step-by-step explanation, payment-rail variants, and direct
`froglet-node` examples.

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

Set `FROGLET_MARKETPLACE_URL` on runtime nodes to search through an external
marketplace. Set it on providers only when the target marketplace supports
write-capable provider registration.

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
- Local resource inspection and publication via `publish_artifact`
- Settlement visibility and marketplace-native wrappers
- Status and task polling
- Install guidance via `get_install_guide`
- Raw compute

<details>
<summary><strong>Important behavior notes</strong></summary>

- `summary` is metadata only; it does not generate code
- `publish_artifact` is the current local publication path
- `run_compute` is the low-level path for open-ended compute and should include
  `provider_id` or `provider_url`
- Project authoring, log tailing, and node restart are not part of the current
  public tool API

</details>

### MCP Server

External bot hosts and automation systems can use the MCP server instead of
the OpenClaw or NemoClaw plugin:

```bash
node integrations/mcp/froglet/server.js
```

It exposes the same Froglet control surface over MCP stdio.

For supported local agent targets, generate the exact config file instead of
editing JSON or TOML by hand from the cloned repo:

```bash
cd froglet && ./scripts/setup-agent.sh --target claude-code
cd froglet && ./scripts/setup-agent.sh --target codex
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
- Public-facing self-host documentation and examples

**Later or separately deployed:**

- The hosted `try.froglet.dev` gateway and its operational lifecycle
- Higher-layer marketplace ranking, reputation, and policy services
- Long-running batch orchestration, which remains out of scope for the current
  v1 runtime surface
- Native deployment adapters for AWS, GCP, OVH, and similar cloud providers
- Zip or archive packaging as a first-class execution submission format
- First-party hosted control-plane operations and runbooks

> [!WARNING]
> Execution hardening is not uniform across all runtimes.
> The strongest isolation paths are Wasm sandbox execution and confidential/TEE
> profiles; Python and OCI/container execution inherit host or container
> isolation characteristics.

---

## Documentation

| Document | Topic |
|---|---|
| [docs-site/src/content/docs/learn/index.mdx](docs-site/src/content/docs/learn/index.mdx) | Canonical onboarding index for the public launch story |
| [docs-site/src/content/docs/learn/cloud-trial.mdx](docs-site/src/content/docs/learn/cloud-trial.mdx) | Hosted trial walkthrough and contract |
| [docs-site/src/content/docs/learn/quickstart.mdx](docs-site/src/content/docs/learn/quickstart.mdx) | Local self-host quickstart |
| [docs/README.md](docs/README.md) | Reference-doc map for specs, operations, and integrations |
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | System architecture overview |
| [ADAPTERS.md](docs/ADAPTERS.md) | Payment and network adapters |
| [RUNTIME.md](docs/RUNTIME.md) | Runtime internals |
| [SERVICE_BINDING.md](docs/SERVICE_BINDING.md) | Service binding model |
| [IDENTITY_ATTESTATION.md](docs/IDENTITY_ATTESTATION.md) | Optional DNS + OAuth identity bindings for Froglet keys |
| [OPENCLAW.md](docs/OPENCLAW.md) | OpenClaw integration |
| [NEMOCLAW.md](docs/NEMOCLAW.md) | NemoClaw integration |
| [KERNEL.md](docs/KERNEL.md) | Protocol kernel spec |
| [CONFIDENTIAL.md](docs/CONFIDENTIAL.md) | Confidential execution |
| [NOSTR.md](docs/NOSTR.md) | Nostr publication adapter |
| [STORAGE_PROFILE.md](docs/STORAGE_PROFILE.md) | Storage profiles |
| [GCP_SINGLE_VM.md](docs/GCP_SINGLE_VM.md) | Single-VM self-host deployment wrapper |
| [MARKETPLACE.md](docs/MARKETPLACE.md) | Marketplace integration and the default public marketplace |
| [HOSTED_TRIAL.md](docs/HOSTED_TRIAL.md) | Public contract for the hosted trial |
| [RELEASE.md](docs/RELEASE.md) | Release process |
| [PAYMENT_MATRIX.md](docs/PAYMENT_MATRIX.md) | Supported payment rails and verification coverage |
| [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) | Community standards (Contributor Covenant 2.1) |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to contribute |

First-party hosted deployment tooling and operator runbooks are maintained
separately from the public protocol and self-host docs in this repo.

---

<div align="center">

**[Learn Index Source](docs-site/src/content/docs/learn/index.mdx)** &middot; **[Releases](https://github.com/armanas/froglet/releases)** &middot; **[Discussions](https://github.com/armanas/froglet/discussions)** &middot; **[Issues](https://github.com/armanas/froglet/issues)** &middot; **[License](LICENSE)**

</div>
