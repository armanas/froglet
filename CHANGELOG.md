# Changelog

All notable changes to this repo should be recorded here.

The format follows Keep a Changelog and the release line currently targets the
`0.1.x` alpha series.

## [Unreleased]

### Added

- OCI Wasm workload kind (`compute.wasm.oci.v1`) allowing Wasm modules to be
  referenced by OCI image (`oci_reference` + `oci_digest`) instead of inline hex
  bytes; supports `ghcr.io` and Docker Hub registries with anonymous pulls
- `OciWasmSubmission` and `OciWasmWorkload` structs in `src/wasm.rs`
- `oci-registry-client` dependency for OCI manifest and blob fetching
- OCI Wasm deal execution path with digest verification and sandbox execution
- official Docker assets for `froglet` and `marketplace`, including a starter
  `compose.yaml`
- public OpenClaw plugin with marketplace discovery and provider-surface tools
- checked-in OpenClaw starter config example
- GitHub Actions CI for strict checks and Docker starter validation
- GitHub Actions release workflow for tagged GHCR image publication

### Fixed

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
