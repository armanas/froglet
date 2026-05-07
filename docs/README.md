# Docs

Maintained by [Armanas Povilionis-Muradian](https://armanas.dev).

## Start Here

Canonical onboarding lives in
[../docs-site/src/content/docs/learn/index.mdx](../docs-site/src/content/docs/learn/index.mdx).
Use the repo docs with this split:

- [../README.md](../README.md): repo overview, component map, verification, and release surfaces
- [../docs-site/src/content/docs/learn/index.mdx](../docs-site/src/content/docs/learn/index.mdx): canonical public onboarding path
- `docs/`: specifications, operator references, and integration details once you know which path you need

Public launch entry points:

- [../docs-site/src/content/docs/learn/cloud-trial.mdx](../docs-site/src/content/docs/learn/cloud-trial.mdx): hosted trial walkthrough
- [../docs-site/src/content/docs/learn/quickstart.mdx](../docs-site/src/content/docs/learn/quickstart.mdx): local self-host quickstart
- [../docs-site/src/content/docs/learn/agents.mdx](../docs-site/src/content/docs/learn/agents.mdx): agent setup
- [../docs-site/src/content/docs/learn/plugin-distribution.mdx](../docs-site/src/content/docs/learn/plugin-distribution.mdx): plugin and registry distribution
- [../docs-site/src/content/docs/learn/payment-rails.mdx](../docs-site/src/content/docs/learn/payment-rails.mdx): Lightning, Stripe, and x402 local/self-hosted setup

## Specifications

- [KERNEL.md](KERNEL.md): normative kernel specification — signed artifacts, settlement, state machines
- [SERVICE_BINDING.md](SERVICE_BINDING.md): service-binding contract — service_id, offer_kind, product shapes

## Architecture

- [ARCHITECTURE.md](ARCHITECTURE.md): system layering — kernel, adapters, runtime, services
- [MARKETPLACE.md](MARKETPLACE.md): marketplace integration and the default public marketplace
- [ARBITER.md](ARBITER.md): MVP complaint and marketplace enforcement boundary

## Companion Docs

- [ADAPTERS.md](ADAPTERS.md): transport and execution adapter boundaries
- [CONFIDENTIAL.md](CONFIDENTIAL.md): TEE and encrypted execution extension
- [NOSTR.md](NOSTR.md): Nostr linked identity publication
- [RUNTIME.md](RUNTIME.md): bot-facing localhost runtime surface
- [STORAGE_PROFILE.md](STORAGE_PROFILE.md): storage and archival profiles

## Operations

- [DOCKER.md](DOCKER.md): local compose and container deployment
- [GCP_SINGLE_VM.md](GCP_SINGLE_VM.md): single-VM self-host deployment wrapper
- [HOSTED_TRIAL.md](HOSTED_TRIAL.md): public contract for the hosted trial
- [NAME_COHERENCE.md](NAME_COHERENCE.md): lightweight launch name and registry-risk note
- [RELEASE.md](RELEASE.md): release process and published image contract

## Integrations

- [PLUGIN_DISTRIBUTION.md](PLUGIN_DISTRIBUTION.md): MCP registry and agent-plugin distribution order
- [OPENCLAW.md](OPENCLAW.md): OpenClaw (Claude) integration
- [NEMOCLAW.md](NEMOCLAW.md): NemoClaw (local agent) integration
- [ROLE_TOOL_ARCHITECTURE.md](ROLE_TOOL_ARCHITECTURE.md): role and tool architecture
