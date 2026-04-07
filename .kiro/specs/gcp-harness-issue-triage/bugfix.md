# Bugfix Requirements Document

## Introduction

This spec covers 10 operational and runtime bugs discovered during a GCP harness run. These are distinct from the protocol contract hardening work in `production-contract-hardening` — they address SSRF enforcement, workload kind routing, discovery status reporting, project lifecycle fixtures, artifact publish API shape, latency regression, lint gating, LND regtest permissions, GCP harness CSR generation, and harness run-ID pinning.

Priorities: P0 (2 issues), P1 (4 issues), P2 (2 issues), P3 (2 issues).

## Bug Analysis

### Current Behavior (Defect)

**P0 — SSRF / private-network bypass on direct compute (Issue 1)**

1.1 WHEN a `run_compute` or `invoke_service` request supplies a `provider_url` targeting a loopback or private-network address (e.g. `https://127.0.0.1:8080`) AND the `provider_id` is either absent or matches the local node identity THEN the system accepts the task and starts execution instead of rejecting the request, because `operator_accessible_provider_url` falls through to the local provider base URL for self-referencing loopback addresses

**P0 — Workload kind mismatch on direct compute (Issue 2)**

1.2 WHEN the operator builds an execution via `build_execution_from_compute_request` for an inline Wasm module THEN the resulting `WorkloadSpec` has `workload_kind = "compute.wasm.v1"` but the offer used by `froglet_run_compute` is the builtin `"execute.compute"` offer whose `offer_kind = "compute.wasm.v1"`, while the provider quote path validates `offer.payload.offer_kind == workload_kind` — this works for Wasm-to-Wasm but fails when the operator sends a `compute.execution.v1` workload (Python inline, OCI container) against the `"execute.compute"` offer whose `offer_kind` is `"compute.wasm.v1"`

1.3 WHEN an agentic tool issues a direct compute request with `runtime=python` or `runtime=container` THEN the upstream provider rejects the deal with `"offer does not match workload kind"` because the operator hardcodes offer_id `"execute.compute"` which maps to `offer_kind = "compute.wasm.v1"` but the workload spec has `workload_kind = "compute.execution.v1"`

**P1 — Discovery status reports false disconnection (Issue 3)**

1.4 WHEN the marketplace operator queries froglet status AND the reference discovery heartbeat loop has successfully registered and is receiving heartbeats THEN the system reports `reference_discovery.connected = false` even though the discovery search contains all expected nodes, because the `connected` flag is only set to `true` inside `mark_discovery_success` which requires a successful HTTP round-trip, but a transient error or race between initial registration and the first status query can leave `connected` at its default `false`

**P1 — Marketplace project build/publish fixture inconsistency (Issue 4)**

1.5 WHEN the harness runs `tool.read_file.happy` against a freshly created project THEN the system fails because `source/main.wat` is missing from the default project scaffold

1.6 WHEN the harness runs `tool.publish_project.happy` against a blank scaffold project THEN the system rejects the publish with "blank scaffold" because the project was not properly seeded with source content before publish

1.7 WHEN the harness runs `tool.test_project.happy` THEN the system fails because the expected test output is missing from the project fixture

**P1 — Artifact publish API rejects agentic payload shape (Issue 5)**

1.8 WHEN an agentic tool (OpenClaw exploratory flow) sends a POST to `/v1/froglet/artifacts/publish` THEN the system returns a deserialization error for missing `service_id` field, because `ProviderControlPublishArtifactRequest` requires `service_id` as a mandatory field but the agentic tooling does not include it in the payload

**P1 — Soak test latency regression (Issue 6)**

1.9 WHEN the local soak test runs sustained publish/query load THEN the p99 latency increases from ~56.4ms baseline to ~305.5ms (+442%), exceeding the 100% degradation threshold in `test_soak.py` line 130

**P2 — Duplicate test gating breaks clippy -D warnings (Issue 7)**

1.10 WHEN running `cargo clippy --all-targets -- -D warnings` THEN the build fails because `src/api/test_support.rs` line 1 has `#![cfg(test)]` (inner attribute) while `src/api/mod.rs` line 70 already gates the module with `#[cfg(test)] mod test_support;`, resulting in a duplicate/redundant test gate that clippy flags

**P2 — LND regtest host-permission sensitivity (Issue 8)**

1.11 WHEN the LND regtest harness runs on a Linux host with Docker-mounted LND state THEN the settlement regression tests fail spuriously because container-created directories (`alice/`, `bob/`) are owned by `root:root`, making `admin.macaroon` unreadable by the test process

**P3 — GCP harness CSR generation bug (Issue 9)**

1.12 WHEN `generate_role_cert()` in `scripts/gcp_harness.sh` generates a CSR for a fresh provision THEN the provision fails because the OpenSSL config previously included `authorityKeyIdentifier` in the `[v3_req]` extensions section, which is invalid for CSR generation (it belongs only in CA-signed certificate extensions)

**P3 — GCP harness run-ID not pinned across subcommands (Issue 10)**

1.13 WHEN a user runs `gcp_harness.sh deploy` followed by `gcp_harness.sh seed` or later phases without exporting `FROGLET_GCP_HARNESS_RUN_ID` THEN each subcommand generates a new timestamp-based run ID via `date -u +%Y%m%d%H%M%S`, silently creating a new empty run directory instead of operating on the existing deployment artifacts

### Expected Behavior (Correct)

**P0 — SSRF / private-network bypass on direct compute (Issue 1)**

2.1 WHEN a `run_compute` or `invoke_service` request supplies a `provider_url` targeting a loopback or private-network address AND the `provider_id` does not match the local node identity (or is absent with no local-node inference) THEN the system SHALL reject the request with HTTP 400 and an error message indicating the URL targets a local or private-network address

**P0 — Workload kind mismatch on direct compute (Issue 2)**

2.2 WHEN the operator builds a direct compute execution with any supported runtime (Wasm, Python, Container) THEN the system SHALL select an offer whose `offer_kind` matches the resulting `workload_kind` of the `WorkloadSpec`, or use a runtime-any offer that accepts `compute.execution.v1`

2.3 WHEN an agentic tool issues a direct compute request with `runtime=python` or `runtime=container` THEN the system SHALL route the request through an offer with `offer_kind = "compute.execution.v1"` so the provider accepts the deal

**P1 — Discovery status reports false disconnection (Issue 3)**

2.4 WHEN the reference discovery heartbeat loop has completed at least one successful registration or heartbeat THEN the system SHALL report `reference_discovery.connected = true` in the froglet status response, and SHALL NOT report `false` due to initialization race conditions

**P1 — Marketplace project build/publish fixture inconsistency (Issue 4)**

2.5 WHEN the harness runs `tool.read_file.happy` against a freshly created project THEN the system SHALL return the expected source file content from the default project scaffold

2.6 WHEN the harness runs `tool.publish_project.happy` THEN the system SHALL successfully publish the project because the scaffold contains valid source content

2.7 WHEN the harness runs `tool.test_project.happy` THEN the system SHALL return the expected test output

**P1 — Artifact publish API rejects agentic payload shape (Issue 5)**

2.8 WHEN an agentic tool sends a POST to `/v1/froglet/artifacts/publish` without a `service_id` field THEN the system SHALL either derive `service_id` from the payload context or return a clear validation error indicating the required field, rather than a raw deserialization error

**P1 — Soak test latency regression (Issue 6)**

2.9 WHEN the local soak test runs sustained publish/query load THEN the p99 latency SHALL remain within the 100% degradation threshold (p99 final ≤ 2× p99 baseline)

**P2 — Duplicate test gating breaks clippy -D warnings (Issue 7)**

2.10 WHEN running `cargo clippy --all-targets -- -D warnings` THEN the build SHALL pass without warnings from `src/api/test_support.rs`, because the file SHALL NOT contain a redundant `#![cfg(test)]` inner attribute when the module is already gated by `#[cfg(test)]` in `mod.rs`

**P2 — LND regtest host-permission sensitivity (Issue 8)**

2.11 WHEN the LND regtest harness runs on a Linux host with Docker-mounted LND state THEN the harness SHALL ensure correct file ownership on `alice/` and `bob/` directories before reading `admin.macaroon`, so tests do not fail spuriously due to permission errors

**P3 — GCP harness CSR generation bug (Issue 9)**

2.12 WHEN `generate_role_cert()` generates a CSR THEN the OpenSSL config SHALL NOT include `authorityKeyIdentifier` in the CSR extensions section; that extension SHALL only appear in the CA-signing step

**P3 — GCP harness run-ID not pinned across subcommands (Issue 10)**

2.13 WHEN a user runs sequential `gcp_harness.sh` subcommands (deploy, seed, test, collect) without explicitly setting `FROGLET_GCP_HARNESS_RUN_ID` THEN the system SHALL read the run ID from the `latest-run` file if it exists, so that subsequent subcommands operate on the same run directory as the initial deploy

### Unchanged Behavior (Regression Prevention)

**SSRF validation**

3.1 WHEN a `run_compute` or `invoke_service` request supplies a `provider_url` targeting a public HTTPS endpoint THEN the system SHALL CONTINUE TO accept and route the request normally

3.2 WHEN a `run_compute` request supplies no `provider_url` and uses `provider_id` matching the local node THEN the system SHALL CONTINUE TO route to the local provider base URL

**Workload kind routing**

3.3 WHEN the operator builds a direct compute execution with `runtime=wasm` and an inline module THEN the system SHALL CONTINUE TO route through the `"execute.compute"` offer with `offer_kind = "compute.wasm.v1"`

3.4 WHEN a service-addressed invocation is used (via `invoke_service` with a `service_id`) THEN the system SHALL CONTINUE TO use the service's own offer and offer_kind for deal creation

**Discovery status**

3.5 WHEN the reference discovery heartbeat fails THEN the system SHALL CONTINUE TO report `connected = false` and populate `last_error`

3.6 WHEN reference discovery is not configured THEN the system SHALL CONTINUE TO report `enabled = false`

**Project lifecycle**

3.7 WHEN a project is created with an explicit scaffold containing source files THEN the system SHALL CONTINUE TO accept and store the scaffold as provided

3.8 WHEN a project publish is attempted on a genuinely blank scaffold (no source written) THEN the system SHALL CONTINUE TO reject the publish

**Artifact publish API**

3.9 WHEN a provider control client sends a POST to `/v1/froglet/artifacts/publish` with a valid `service_id` field THEN the system SHALL CONTINUE TO accept and process the artifact publication normally

**Soak test**

3.10 WHEN the soak test runs with a healthy provider THEN the system SHALL CONTINUE TO enforce the error rate threshold (< 5% errors)

**Clippy / lint**

3.11 WHEN running `cargo clippy --all-targets -- -D warnings` after the fix THEN all existing test modules that are properly gated SHALL CONTINUE TO compile and pass

**LND regtest**

3.12 WHEN the LND regtest harness runs on macOS or other hosts where Docker volume permissions are not an issue THEN the system SHALL CONTINUE TO run tests without any ownership correction overhead

**GCP harness**

3.13 WHEN a user explicitly sets `FROGLET_GCP_HARNESS_RUN_ID` before running a subcommand THEN the system SHALL CONTINUE TO use the explicitly provided run ID, not the latest-run file

3.14 WHEN `generate_role_cert()` signs the certificate with the CA THEN the signed certificate SHALL CONTINUE TO include the correct v3 extensions (SAN, keyUsage, extendedKeyUsage) from the config
