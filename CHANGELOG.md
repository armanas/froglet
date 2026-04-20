# Changelog

All notable changes to this repo should be recorded here.

The format follows Keep a Changelog and the release line currently targets the
`0.1.x` alpha series.

## [Unreleased]

### Added

- Public boundary notes clarifying that first-party hosted deployment,
  monitoring, and rotation runbooks now live in the private services/operator
  workspace.

### Changed

- Moved first-party hosted deployment tooling, Lightsail specs, operator
  runbooks, and the working launch backlog out of this public repo and into
  the private `froglet-services` workspace.

## [0.1.0-alpha.0] - 2026-04-19

### Added

- `scripts/release_gate.sh` — single release-candidate entrypoint combining
  strict checks, docs-site build, docs-site tests, and optional packaging /
  install-smoke / hosted cells, with per-step evidence logs and a
  `summary.tsv`
- `docs/SECURITY_PASS.md` — pre-launch security pass with cargo / pip / npm
  audit remediations, full-history gitleaks scan (0 real leaks), and threat
  model for `ai.froglet.dev`
- `docs/PAYMENT_MATRIX.md` — supported payment rails × verification modes
  matrix with per-cell status and re-run commands
- `docs/IDENTITY_ATTESTATION.md` — normative spec for DNS + OAuth/OIDC
  identity bindings for Froglet keys
- `docs/ARBITER.md` — design stub for the marketplace-layer claims-court
  service
- `froglet-protocol::protocol::identity_attestation` — `IdentityAttestation`
  credential type, validator, and 8 roundtrip tests
- `scripts/cloudflare_dns.sh` — Cloudflare v4 DNS helper (verify / zone /
  list / create / delete / upsert); reads token from macOS Keychain, never
  echoed
- `scripts/deploy_aws.sh` + `ops/lightsail/*.json` — AWS Lightsail
  Container Service deploy helper (verify / status / create / deploy /
  logs / endpoint / destroy); AWS keys read from macOS Keychain per
  invocation, never environment-persisted. First deployment live at
  `ai.froglet.dev` (nginx placeholder pending the first Froglet image tag)
- `FROGLET_EGRESS_MODE=strict` — opt-in propagation of the
  same DNS-pinning + SSRF validator used for LLM-controlled URLs to the
  operator-configured `FROGLET_PROVIDER_URL` / `FROGLET_RUNTIME_URL`
  surfaces in the Node MCP and OpenClaw integrations. Off by default;
  local/dev URLs stay on stock `fetch`
- Order-28-style content-shape assertions in `scripts/hosted_smoke.sh`:
  `/health` JSON envelope, `/v1/node/capabilities` key presence,
  `/v1/node/identity` minimum shape, `/v1/openapi.yaml` prefix, docs
  `text/html` + body marker
- `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), `.github/ISSUE_TEMPLATE/`
  (bug + feature YAML forms + Discussions/security contact links),
  `.github/pull_request_template.md` mirroring the release gate (TODO
  scaffolding; Discussions toggle still pending)
- OCI Wasm workload kind (`compute.wasm.oci.v1`) allowing Wasm modules to be
  referenced by OCI image (`oci_reference` + `oci_digest`) instead of inline hex
  bytes; supports `ghcr.io` and Docker Hub registries with anonymous pulls
- `OciWasmSubmission` and `OciWasmWorkload` structs in `src/wasm.rs`
- `oci-registry-client` dependency for OCI manifest and blob fetching
- OCI Wasm deal execution path with digest verification and sandbox execution
- official Docker assets for the Froglet node, including a starter
  `compose.yaml`
- public OpenClaw plugin with Froglet discovery and provider-surface tools
- checked-in OpenClaw starter config example
- GitHub Actions CI for strict checks and Docker starter validation
- GitHub Actions release workflow for tagged GHCR image publication

### Fixed

- `rustls-webpki` bumped 0.103.10 → 0.103.12 (RUSTSEC-2026-0098 + -0099)
- `cryptography` (Python) bumped 45 → 46.0.7 (3 GHSAs)
- `npm audit fix` in `integrations/mcp/froglet` (hono, @hono/node-server,
  path-to-regexp) and in `docs-site` (vite)
- Added `postgres_mounts` field to four test NodeConfig literals to
  restore `cargo check --all-targets` on `main`
- replaced `todo!()` panic in free OCI Wasm job path with full implementation
- added 50 MB size cap on OCI module downloads to prevent memory exhaustion
- fixed OCI reference parsing to handle `@sha256:` digest syntax alongside `:tag`
- replaced hardcoded registry allowlist with generic `https://{host}` fallback
  for unknown OCI-compliant registries
- extracted shared `fetch_oci_wasm_module` helper to deduplicate OCI pull logic

### Changed

- added `FROGLET_PUBLIC_BASE_URL` so containerized nodes can advertise a
  host-reachable clearnet URL
- tightened OpenClaw output defaults so raw JSON is opt-in via `include_raw`
- expanded OpenClaw tests to cover missing config, 404, invalid JSON, and
  timeout failure paths

### Fixed

- cleaned the warnings-denied Rust build path by removing stale `NodeConfig`
  initializer gaps and the unused-events-query warning

## Alpha Cut Notes

When cutting the first alpha:

1. move the current `Unreleased` notes into `0.1.0-alpha.1`
2. set `Cargo.toml` package version to `0.1.0-alpha.1`
3. push tag `v0.1.0-alpha.1`
4. let `.github/workflows/release.yml` publish the matching GHCR images
