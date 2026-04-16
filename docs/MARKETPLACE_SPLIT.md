# Marketplace Split Decision

Status: approved for `Order: 01` and `Order: 02`

This document is boundary-only. It defines what moves to `../froglet-services`, what stays public in `froglet`, which public interfaces remain stable for now, and which cleanup is required in `../froglet-services`. It does not redesign the marketplace contract.

## Decision

- `froglet` remains the public repository for the kernel/protocol, the node/runtime/provider implementation, the public feed and artifact APIs, the public deal and settlement semantics, and the public agent surfaces (`MCP`, `OpenClaw`, `NemoClaw`).
- `froglet` continues to support marketplace integration as a public capability. Marketplace remains a valid Froglet node role/capability; what moves out is the bundled first-party marketplace service implementation.
- The first-party marketplace implementation moves to `../froglet-services`, which is already structured as the private services workspace for the market-read plane and later commercial services.

## Moves To `../froglet-services`

- The `froglet-marketplace/` crate and the `froglet-marketplace` binary.
- Any `froglet` release, install, container, compose, or deploy surface whose job is to build, package, publish, or run `froglet-marketplace` as part of the public repo. Current known surfaces include:
  - `Cargo.toml`
  - `Dockerfile`
  - `compose.yaml`
  - `scripts/install.sh`
  - `scripts/package_release_assets.sh`
  - `scripts/verify_release_assets.sh`
  - `scripts/docker-entrypoint.sh`
  - `scripts/gcp_harness.sh`
- Public-repo docs and docs-site content that present `froglet-marketplace` as a bundled first-party service implementation, or that explain how the default marketplace is built, packaged, deployed, or developed, rather than keeping marketplace references high-level. Current known surfaces include:
  - `README.md`
  - `docs/DOCKER.md`
  - `docs/RELEASE.md`
  - `docs/CONFIGURATION.md`
  - `docs/README.md`
  - `docs/ARCHITECTURE.md`
  - `docs-site/src/content/docs/architecture/crates.md`
  - `docs-site/src/content/docs/architecture/files.md`
  - `docs-site/src/content/docs/marketplace/overview.md`
- Tests and helpers in `froglet` that exist only to install, package, or run the bundled `froglet-marketplace` binary move with that implementation or get rewritten after the extraction.
- Hosted and end-to-end harnesses that run the real first-party marketplace implementation also move to `../froglet-services`, even when they exercise public node behavior at the same time. Current known surfaces include:
  - `scripts/gcp_harness.sh`
  - `tests/e2e/gcp_harness/`
  - the GCP harness section in `tests/README.md`

## Stays Public In `froglet`

- `froglet-protocol/` and `docs/KERNEL.md`.
- The public node/runtime/provider implementation under `src/` and `src/bin/`.
- Public Froglet HTTP surfaces, especially `/v1/feed` and `/v1/artifacts/:hash`, which remain canonical inputs for external services including `../froglet-services`.
- Public agent surfaces and shared client code:
  - `integrations/mcp/froglet/`
  - `integrations/openclaw/froglet/`
  - `integrations/shared/froglet-lib/`
  - `python/`
- Public docs that describe protocol, node/runtime behavior, and marketplace only at a high level: Froglet can integrate with marketplaces, and a default marketplace exists. Public docs should not describe how the first-party marketplace is implemented, developed, packaged, or deployed.
- Contract tests that verify how a public Froglet node talks to an external marketplace stay in `froglet` when they use fixtures, stubs, or mock marketplace endpoints rather than the private first-party marketplace implementation.

## Stable Interfaces After The Split

- Marketplace remains part of the public Froglet model as an external node/integration role; only the first-party implementation moves.
- `FROGLET_MARKETPLACE_URL` remains supported in `froglet`.
- Provider registration and runtime discovery against an external marketplace remain supported in `froglet`.
- Public Froglet feed and artifact endpoints remain the ingest boundary for `../froglet-services`.
- Current kernel, quote, deal, receipt, and settlement semantics do not change in this task.
- No marketplace API redesign is decided here. The current read-only `services/marketplace-api` REST surface in `../froglet-services` is not treated by this document as the replacement for Froglet's current marketplace integration contract.

## `../froglet-services` Cleanup Required

- Remove the empty `packages/shared-models/` scaffold.
- Keep the repo narrative scoped to the current runnable slice:
  - `packages/froglet-adapter`
  - `packages/marketplace-types`
  - `services/indexer`
  - `services/marketplace-api`
  - `migrations`
  - `ops`
- Keep `services/broker/`, `services/operator/`, and `packages/trust/` explicitly labeled as later-phase placeholders, not active product surfaces.
- Keep `packages/froglet-adapter` as the only crate that depends on `../froglet` for now.
- Do not document the current `services/marketplace-api` read API as if it already replaces Froglet's existing marketplace-facing behavior.

## Non-Goals

- No code moves in this step.
- No contract redesign between `froglet` and `../froglet-services` in this step.
- No release-surface cleanup in this step; this document is the input to that follow-up work.
