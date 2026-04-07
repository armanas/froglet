# Implementation Plan: GCP Harness Issue Triage

## Overview

This plan fixes 10 operational/runtime bugs discovered during a GCP harness run, ordered by priority (P0 → P3). The workflow follows the exploratory bugfix methodology: write tests BEFORE fixing to confirm the bug exists, write preservation tests to lock non-buggy behavior, then implement fixes and verify.

The 10 issues span: SSRF validation (Issue 1), workload kind routing (Issue 2), discovery status race (Issue 3), project fixture inconsistency (Issue 4), artifact publish API (Issue 5), soak test latency (Issue 6), clippy lint (Issue 7), LND regtest permissions (Issue 8), CSR generation (Issue 9), and harness run-ID pinning (Issue 10).

## Tasks

- [x] 1. Write bug condition exploration tests (BEFORE implementing any fixes)
  - **Property 1: Bug Condition** - SSRF Bypass for Absent Provider ID with Loopback URL
  - **CRITICAL**: This test MUST FAIL on unfixed code — failure confirms the bug exists
  - **DO NOT attempt to fix the test or the code when it fails**
  - **NOTE**: This test encodes the expected behavior — it will validate the fix when it passes after implementation
  - **GOAL**: Surface counterexamples that demonstrate Issue 1 exists
  - **Scoped PBT Approach**: Scope the property to concrete failing cases: `provider_id=None` with loopback/private URLs
  - Test that `operator_accessible_provider_url(state, "https://127.0.0.1:8443", None)` rejects with HTTP 400 (from Bug Condition in design — `provider_id.is_none_or` treats None as local)
  - Generate random loopback/private URLs (127.x.x.x, 10.x.x.x, 192.168.x.x, [::1]) with `provider_id=None` and assert rejection
  - Run test on UNFIXED code — expect FAILURE (test asserts rejection but unfixed code accepts)
  - Document counterexamples found (e.g., "`operator_accessible_provider_url(state, "https://127.0.0.1:8443", None)` returns Ok(local_url) instead of Err")
  - Mark task complete when test is written, run, and failure is documented
  - _Requirements: 1.1, 2.1_

- [x] 2. Write bug condition exploration tests — Workload Kind Routing
  - **Property 2: Bug Condition** - Workload Kind Mismatch for Non-Wasm Runtimes
  - **CRITICAL**: This test MUST FAIL on unfixed code — failure confirms the bug exists
  - **DO NOT attempt to fix the test or the code when it fails**
  - **GOAL**: Surface counterexamples that demonstrate Issue 2 exists
  - **Scoped PBT Approach**: Scope to `runtime=python` and `runtime=container` cases where `workload_kind = "compute.execution.v1"`
  - Test that `froglet_run_compute` with non-Wasm runtime selects an offer whose `offer_kind` matches `"compute.execution.v1"` (from Bug Condition in design — hardcoded `"execute.compute"` offer has `offer_kind = "compute.wasm.v1"`)
  - Run test on UNFIXED code — expect FAILURE (offer kind mismatch causes provider rejection)
  - Document counterexamples found (e.g., "runtime=python produces workload_kind=compute.execution.v1 but offer has offer_kind=compute.wasm.v1")
  - _Requirements: 1.2, 1.3, 2.2, 2.3_


- [x] 3. Write bug condition exploration tests — Discovery, Fixtures, API, Lint, CSR, Run-ID
  - **Property 3: Bug Condition** - Discovery Connected Flag Race, Project Fixtures, Artifact API, Clippy, CSR, Run-ID
  - **CRITICAL**: These tests MUST FAIL (or demonstrate buggy behavior) on unfixed code
  - **DO NOT attempt to fix the tests or the code when they fail**
  - **GOAL**: Surface counterexamples for Issues 3, 4, 5, 7, 9, 10
  - Test Issue 3: Query status immediately after spawning non-required discovery sync — expect `connected=false` even after registration succeeds (race in `mark_discovery_success` timing)
  - Test Issue 4: Create default project (no starter, no inline_source) and verify scaffold writes `source/main.py` not `source/main.wat` — confirms harness expectation mismatch
  - Test Issue 5: Deserialize `ProviderControlPublishArtifactRequest` without `service_id` field — expect deserialization error on unfixed code
  - Test Issue 7: Verify `src/api/test_support.rs` line 1 contains `#![cfg(test)]` — confirms redundant inner attribute exists
  - Test Issue 9: Inspect OpenSSL config template for `authorityKeyIdentifier` in `[v3_req]` section — confirms CSR extension bug
  - Test Issue 10: Verify `gcp_harness.sh` does NOT read `latest-run` file when `FROGLET_GCP_HARNESS_RUN_ID` is unset — confirms run-ID pinning bug
  - Run tests on UNFIXED code — expect failures/bug confirmations
  - Document counterexamples for each issue
  - _Requirements: 1.4, 1.5, 1.6, 1.7, 1.8, 1.10, 1.12, 1.13, 2.4, 2.5, 2.6, 2.7, 2.8, 2.10, 2.12, 2.13_

- [x] 4. Write preservation property tests (BEFORE implementing fixes)
  - **Property 4: Preservation** - Existing Behavior Unchanged for Non-Buggy Inputs
  - **IMPORTANT**: Follow observation-first methodology
  - **GOAL**: Lock existing correct behavior so fixes don't introduce regressions
  - Observe on UNFIXED code, then write property-based tests:
  - **SSRF Preservation (3.1, 3.2)**: For all public HTTPS URLs, `operator_accessible_provider_url` accepts and routes normally. For explicit local `provider_id` with loopback URL, routes to local provider.
  - **Wasm Compute Preservation (3.3, 3.4)**: For `runtime=wasm` direct compute, `froglet_run_compute` uses `"execute.compute"` offer with `offer_kind = "compute.wasm.v1"`. Service-addressed invocations use the service's own offer.
  - **Discovery Preservation (3.5, 3.6)**: Heartbeat failures set `connected=false`. Unconfigured discovery reports `enabled=false`.
  - **Project Preservation (3.7, 3.8)**: Explicit scaffold projects work as provided. Blank scaffold publish is rejected.
  - **Artifact Publish Preservation (3.9)**: POST with valid `service_id` continues to work.
  - **Lint Preservation (3.11)**: Properly gated test modules compile without warnings.
  - **GCP Harness Preservation (3.13, 3.14)**: Explicit `FROGLET_GCP_HARNESS_RUN_ID` takes precedence. CA-signed certs include correct v3 extensions.
  - Verify all preservation tests PASS on UNFIXED code
  - **EXPECTED OUTCOME**: Tests PASS (confirms baseline behavior to preserve)
  - Mark task complete when tests are written, run, and passing on unfixed code
  - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7, 3.8, 3.9, 3.10, 3.11, 3.12, 3.13, 3.14_

- [x] 5. P0 Fix — Issue 1: SSRF / private-network bypass

  - [x] 5.1 Implement SSRF fix in `operator_accessible_provider_url`
    - File: `src/operator.rs` (~line 447)
    - Change `provider_id.is_none_or(|value| value == state.app_state.identity.node_id())` to `provider_id.is_some_and(|value| value == state.app_state.identity.node_id())`
    - When `provider_id` is `None` and URL is loopback/private, reject with HTTP 400 instead of falling through to local provider
    - Add unit test for `provider_id=None` with loopback URL → expect rejection
    - _Bug_Condition: isBugCondition(input) where input.provider_id is None AND input.provider_url targets loopback/private_
    - _Expected_Behavior: reject with HTTP 400 and error message indicating local/private-network address_
    - _Preservation: Public HTTPS URLs accepted (3.1), explicit local provider_id routes locally (3.2)_
    - _Requirements: 1.1, 2.1, 3.1, 3.2_

  - [x] 5.2 Verify SSRF exploration test now passes
    - **Property 1: Expected Behavior** - SSRF Rejection for Absent Provider ID
    - **IMPORTANT**: Re-run the SAME test from task 1 — do NOT write a new test
    - The test from task 1 encodes the expected behavior (rejection for None provider_id with loopback URL)
    - Run bug condition exploration test from task 1
    - **EXPECTED OUTCOME**: Test PASSES (confirms SSRF bug is fixed)
    - _Requirements: 2.1_

  - [x] 5.3 Verify SSRF preservation tests still pass
    - **Property 4: Preservation** - SSRF Preservation Subset
    - **IMPORTANT**: Re-run the SAME preservation tests from task 4 — do NOT write new tests
    - Confirm public URL routing and explicit local provider_id routing still work
    - **EXPECTED OUTCOME**: Tests PASS (confirms no regressions)
    - _Requirements: 3.1, 3.2_

- [x] 6. P0 Fix — Issue 2: Workload kind mismatch

  - [x] 6.1 Investigate existing offer model and implement dynamic offer selection
    - **Exploration first**: Before adding a new builtin offer, investigate whether an execution-v1-capable offer already exists in the provider's offer set. Check `builtin_provider_offer_definitions` (src/api/mod.rs:3835, 3932, 11078) and how offer-kind normalization interacts with the quote validation path.
    - File: `src/operator.rs` (~line 2449, `froglet_run_compute`)
    - Replace hardcoded `"execute.compute"` with dynamic selection based on `spec.workload_kind()`
    - If no existing execution-v1 offer exists, add a new builtin offer — but ensure it integrates with existing offer-kind normalization and doesn't break test assumptions around `execute.compute` being wasm-only
    - _Bug_Condition: isBugCondition(input) where input.runtime IN ["python", "container"] AND workload_kind = "compute.execution.v1"_
    - _Expected_Behavior: offer_kind matches workload_kind so provider accepts the deal_
    - _Preservation: Wasm runtime uses "execute.compute" (3.3), service invocations use service offer (3.4)_
    - _Requirements: 1.2, 1.3, 2.2, 2.3, 3.3, 3.4_

  - [x] 6.2 Verify workload kind exploration test now passes
    - **Property 2: Expected Behavior** - Workload Kind Routing for Non-Wasm Runtimes
    - **IMPORTANT**: Re-run the SAME test from task 2 — do NOT write a new test
    - Run bug condition exploration test from task 2
    - **EXPECTED OUTCOME**: Test PASSES (confirms workload kind mismatch is fixed)
    - _Requirements: 2.2, 2.3_

  - [x] 6.3 Verify workload kind preservation tests still pass
    - **Property 4: Preservation** - Workload Kind Preservation Subset
    - **IMPORTANT**: Re-run the SAME preservation tests from task 4 — do NOT write new tests
    - Confirm Wasm direct compute and service-addressed invocations still work
    - **EXPECTED OUTCOME**: Tests PASS (confirms no regressions)
    - _Requirements: 3.3, 3.4_


- [x] 7. P0 Checkpoint — Verify P0 fixes
  - Run `cargo fmt --all --check`
  - Run `CARGO_INCREMENTAL=0 RUSTFLAGS="-D warnings" cargo test --all-targets`
  - Run `cargo clippy --all-targets -- -D warnings`
  - Ensure P0 fixes (Issues 1, 2) pass all Rust checks before proceeding to P1
  - Ask the user if questions arise

- [x] 8. P1 Fix — Issue 3: Discovery status false disconnection

  - [x] 8.1 Investigate discovery status lifecycle and fix connected flag
    - **Exploration first**: The initial hypothesis (moving `mark_discovery_success` in `perform_initial_sync`) is a no-op — `perform_initial_sync()` already reaches `mark_discovery_success()` through `register_node()`. The real bug is elsewhere.
    - Trace all callers of `mark_discovery_success` and all writers to the `connected` field in `src/discovery_client.rs`, `src/state.rs`, and `src/server.rs`
    - Check whether any error path (heartbeat failure, transient error) resets `connected` to `false` after a successful registration
    - Check whether the status query reads from a stale snapshot
    - Check the non-required path timing in `server.rs:440` for the actual race window
    - Implement fix based on findings (likely: prevent error paths from overwriting `connected=true` after initial success, or fix stale status reads)
    - _Bug_Condition: isBugCondition(input) where status query arrives and connected=false despite successful registration_
    - _Expected_Behavior: connected=true after successful initial registration_
    - _Preservation: Heartbeat failures still set connected=false (3.5), unconfigured discovery reports enabled=false (3.6)_
    - _Requirements: 1.4, 2.4, 3.5, 3.6_

  - [x] 8.2 Verify discovery exploration test now passes
    - **Property 3: Expected Behavior** - Discovery Connected Flag After Registration
    - **IMPORTANT**: Re-run the SAME test from task 3 (Issue 3 portion) — do NOT write a new test
    - **EXPECTED OUTCOME**: Test PASSES (confirms discovery race is fixed)
    - _Requirements: 2.4_

- [x] 9. P1 Fix — Issue 4: Project fixture inconsistency

  - [x] 9.1 Fix harness fixture to use correct project for file-read tests
    - The bug is in the harness fixture setup, not the platform's default scaffold. `build_project_id` is bootstrapped as a hidden blank project (Python scaffold), while `publish_ready_project_id` is created with `starter: "hello_world"` (Wasm scaffold). The `tool.read_file.happy` test targets `build_project_id` but expects WAT files.
    - File: `tests/e2e/gcp_harness/run-matrix.mjs` (~line 107, 132), `tests/e2e/gcp_harness/generate-scenarios.mjs` (~line 329)
    - **Option A (preferred)**: Update `tool.read_file.happy` to target `publish_ready_project_id` which already has the correct Wasm scaffold
    - **Option B**: Create `build_project_id` with an explicit starter so it has WAT content
    - **Option C**: Update expected file paths for `build_project_id` to match the Python scaffold (`source/main.py`)
    - _Bug_Condition: isBugCondition(input) where harness expects WAT files from blank bootstrap fixture_
    - _Expected_Behavior: scaffold file paths and content match harness expectations_
    - _Preservation: Explicit scaffold projects work as provided (3.7), blank scaffold publish rejected (3.8)_
    - _Requirements: 1.5, 1.6, 1.7, 2.5, 2.6, 2.7, 3.7, 3.8_

  - [x] 9.2 Verify project fixture exploration test now passes
    - **Property 3: Expected Behavior** - Project Scaffold Fixture Consistency
    - **IMPORTANT**: Re-run the SAME test from task 3 (Issue 4 portion) — do NOT write a new test
    - **EXPECTED OUTCOME**: Test PASSES (confirms fixture mismatch is fixed)
    - _Requirements: 2.5, 2.6, 2.7_

- [x] 10. P1 Fix — Issue 5: Artifact publish API shape

  - [x] 10.1 Fix agentic tool layer to include service_id and improve server error response
    - **Fix the caller, not the server**: The server correctly requires `service_id` — do NOT make it optional
    - File: `integrations/openclaw/froglet/` and/or `integrations/mcp/froglet/` (agentic tool layer)
    - Update the agentic tool schema/prompt to include `service_id` in the artifact publish payload
    - File: `src/api/mod.rs` (deserialization error handling for `/v1/froglet/artifacts/publish`)
    - Wrap serde deserialization errors into a structured JSON error response with a clear message indicating which required fields are missing
    - _Bug_Condition: isBugCondition(input) where agentic tool omits service_id field_
    - _Expected_Behavior: agentic tool includes service_id; server returns structured error if missing_
    - _Preservation: Artifact publish with valid service_id continues to work (3.9)_
    - _Requirements: 1.8, 2.8, 3.9_

  - [x] 10.2 Verify artifact publish exploration test now passes
    - **Property 3: Expected Behavior** - Artifact Publish Without service_id
    - **IMPORTANT**: Re-run the SAME test from task 3 (Issue 5 portion) — do NOT write a new test
    - **EXPECTED OUTCOME**: Test PASSES (confirms API shape is fixed)
    - _Requirements: 2.8_

- [x] 11. P1 Fix — Issue 6: Soak test latency regression

  - [x] 11.1 Profile latency bottleneck and address root cause
    - **This task is not implementation-ready** — it requires profiling before a specific fix can be determined
    - Profile the event insertion path under sustained load to identify the specific bottleneck
    - Likely candidates: SQLite write contention (single-writer), event query capacity semaphore too restrictive, per-event transaction overhead
    - Potential fixes (depends on profiling results): ensure WAL mode for events DB, batch event inserts within single transaction, adjust `try_acquire_events_query_permit` limits
    - File: `src/api/mod.rs` (event insertion path), `python/tests/test_soak.py` (line 130)
    - **Step 1**: Run soak test with profiling/tracing to identify the hotspot
    - **Step 2**: Implement targeted fix based on profiling results
    - _Bug_Condition: isBugCondition(input) where sustained load causes p99 > 2× baseline_
    - _Expected_Behavior: p99 latency ≤ 2× p99 baseline under sustained load_
    - _Preservation: Error rate threshold (< 5%) still enforced (3.10)_
    - _Requirements: 1.9, 2.9, 3.10_

  - [x] 11.2 Verify soak test passes latency threshold
    - Run `python3 -W error -m unittest discover -s python/tests -t . -v` (includes soak test)
    - Confirm p99 latency stays within 100% degradation threshold
    - **EXPECTED OUTCOME**: Soak test passes
    - _Requirements: 2.9_

- [x] 12. P1 Checkpoint — Verify P1 fixes
  - Run `cargo fmt --all --check`
  - Run `CARGO_INCREMENTAL=0 RUSTFLAGS="-D warnings" cargo test --all-targets`
  - Run `cargo clippy --all-targets -- -D warnings`
  - Run `python3 -W error -m unittest discover -s python/tests -t . -v`
  - Ensure P1 fixes (Issues 3, 4, 5, 6) pass all checks before proceeding to P2
  - Ask the user if questions arise


- [x] 13. P2 Fix — Issue 7: Duplicate test gating (clippy)

  - [x] 13.1 Remove redundant `#![cfg(test)]` from test_support.rs
    - File: `src/api/test_support.rs` (line 1)
    - Delete `#![cfg(test)]` inner attribute — the module is already gated by `#[cfg(test)] mod test_support;` in `src/api/mod.rs` line 70
    - _Bug_Condition: isBugCondition(input) where test_support.rs has redundant inner #![cfg(test)]_
    - _Expected_Behavior: cargo clippy --all-targets -- -D warnings passes without warnings from test_support.rs_
    - _Preservation: Properly gated test modules continue to compile (3.11)_
    - _Requirements: 1.10, 2.10, 3.11_

  - [x] 13.2 Verify clippy exploration test now passes
    - **Property 3: Expected Behavior** - Clippy Clean Build
    - **IMPORTANT**: Re-run `cargo clippy --all-targets -- -D warnings` to confirm no warnings from test_support.rs
    - **EXPECTED OUTCOME**: Clippy passes cleanly
    - _Requirements: 2.10_

- [x] 14. P2 Fix — Issue 8: LND regtest permissions

  - [x] 14.1 Fix host-side file ownership for LND data directories
    - File: `python/tests/test_support.py` (function `start_lnd_regtest_cluster`)
    - After `_wait_for_path` for each node's `admin_macaroon_path`, fix ownership of only the `alice/` and `bob/` host-side directories using `chown` to match the test process UID/GID
    - **Do NOT** recursively chmod the entire `.lnd` directory (makes macaroon broadly readable)
    - **Do NOT** touch the bitcoind volume (broke the container in the first rerun)
    - Guard the fix to Linux only (skip on macOS where Docker Desktop handles permissions transparently)
    - **Alternative**: Run LND containers with `--user uid:gid` matching the host test process
    - _Bug_Condition: isBugCondition(input) where host is Linux AND Docker creates root-owned dirs_
    - _Expected_Behavior: admin.macaroon files readable by test process on Linux_
    - _Preservation: macOS Docker behavior unaffected (3.12)_
    - _Requirements: 1.11, 2.11, 3.12_

  - [x] 14.2 Verify LND regtest exploration test now passes
    - **Property 3: Expected Behavior** - LND Regtest File Permissions
    - **IMPORTANT**: Re-run the SAME test from task 3 (Issue 8 portion) if applicable
    - **EXPECTED OUTCOME**: LND regtest tests pass on Linux without permission errors
    - _Requirements: 2.11_

- [x] 15. P2 Checkpoint — Verify P2 fixes
  - Run `cargo clippy --all-targets -- -D warnings` (confirms Issue 7 fix)
  - Run `python3 -W error -m unittest discover -s python/tests -t . -v` (confirms Issue 8 fix)
  - Ask the user if questions arise

- [x] 16. P3 Fix — Issue 9: GCP harness CSR generation

  - [x] 16.1 Verify CSR fix and add signing extensions if needed
    - File: `scripts/gcp_harness.sh` (function `generate_role_cert()`)
    - **Verify current state**: The CSR path bug (authorityKeyIdentifier in `[v3_req]`) was already fixed locally during the harness run. Confirm the current code no longer has `authorityKeyIdentifier` in `[v3_req]`.
    - **If already fixed**: Mark as resolved — the CA config already contains `authorityKeyIdentifier` in its own section.
    - **Only if the signed leaf certificate is missing `authorityKeyIdentifier`**: Add a `[v3_sign]` section with `authorityKeyIdentifier = keyid:always,issuer` and update the `openssl x509 -req` command to use `-extensions v3_sign`.
    - _Bug_Condition: isBugCondition(input) where authorityKeyIdentifier appears in CSR extensions_
    - _Expected_Behavior: CSR has no authorityKeyIdentifier; signed cert includes it from CA config_
    - _Preservation: CA-signed certs include correct v3 extensions (3.14)_
    - _Requirements: 1.12, 2.12, 3.14_

  - [x] 16.2 Verify CSR exploration test now passes
    - **Property 3: Expected Behavior** - CSR Without authorityKeyIdentifier
    - **IMPORTANT**: Re-run the SAME test from task 3 (Issue 9 portion) — do NOT write a new test
    - Generate a test CSR and verify `authorityKeyIdentifier` is NOT in CSR extensions
    - Verify signed certificate DOES include `authorityKeyIdentifier`
    - **EXPECTED OUTCOME**: Test PASSES
    - _Requirements: 2.12_

- [x] 17. P3 Fix — Issue 10: GCP harness run-ID pinning

  - [x] 17.1 Add scoped read-before-default logic for latest-run file
    - File: `scripts/gcp_harness.sh` (run ID initialization, ~line 30-35)
    - **Scope to continuation subcommands only**: Only read `latest-run` for continuation commands (`deploy`, `seed`, `test`, `collect`, `status`). Leave `provision` and `destroy` to always generate a fresh run ID by default.
    - This prevents `provision` and `destroy` from silently targeting the previous run
    - Explicit `FROGLET_GCP_HARNESS_RUN_ID` env var still takes precedence for all subcommands
    - _Bug_Condition: isBugCondition(input) where FROGLET_GCP_HARNESS_RUN_ID unset AND latest-run exists but not read for continuation commands_
    - _Expected_Behavior: continuation subcommands use same run directory as initial deploy; provision/destroy generate fresh IDs_
    - _Preservation: Explicit FROGLET_GCP_HARNESS_RUN_ID takes precedence (3.13)_
    - _Requirements: 1.13, 2.13, 3.13_

  - [x] 17.2 Verify run-ID exploration test now passes
    - **Property 3: Expected Behavior** - Harness Run-ID Persistence
    - **IMPORTANT**: Re-run the SAME test from task 3 (Issue 10 portion) — do NOT write a new test
    - Verify sequential subcommands without explicit env var use the same run directory
    - **EXPECTED OUTCOME**: Test PASSES
    - _Requirements: 2.13_

- [x] 18. P3 Checkpoint — Verify P3 fixes
  - Verify CSR generation produces valid certificates (no authorityKeyIdentifier in CSR, present in signed cert)
  - Verify run-ID pinning works across subcommands
  - Ask the user if questions arise

- [x] 19. Final verification — All preservation tests still pass
  - **Property 4: Preservation** - Full Preservation Suite
  - **IMPORTANT**: Re-run ALL preservation tests from task 4 — do NOT write new tests
  - Confirm all 14 preservation requirements (3.1–3.14) are satisfied after all fixes
  - **EXPECTED OUTCOME**: All preservation tests PASS (confirms no regressions across all 10 fixes)

- [x] 20. Final checkpoint — Full validation
  - Run `./scripts/strict_checks.sh` (full repo validation matrix)
  - Run `cargo fmt --all --check`
  - Run `CARGO_INCREMENTAL=0 RUSTFLAGS="-D warnings" cargo test --all-targets`
  - Run `cargo clippy --all-targets -- -D warnings`
  - Run `python3 -W error -m unittest discover -s python/tests -t . -v`
  - Run `node --check integrations/openclaw/froglet/index.js`
  - Run `node --test integrations/openclaw/froglet/test/plugin.test.js integrations/openclaw/froglet/test/config-profiles.test.mjs integrations/openclaw/froglet/test/doctor.test.mjs integrations/openclaw/froglet/test/froglet-client.test.mjs`
  - Ensure all tests pass across Rust, Python, and Node.js
  - Ask the user if questions arise

## Notes

- Issues are ordered by priority: P0 (tasks 5–7), P1 (tasks 8–12), P2 (tasks 13–15), P3 (tasks 16–18).
- Exploration tests (tasks 1–3) and preservation tests (task 4) are written BEFORE any fixes to establish the bug baseline and lock correct behavior.
- The 11 correctness properties from the design map as follows:
  - Properties 1–2 → Tasks 1, 2 (SSRF, workload kind bug conditions) and verification in tasks 5.2, 6.2
  - Properties 3–10 → Task 3 (remaining bug conditions) and verification in tasks 8.2, 9.2, 10.2, 13.2, 14.2, 16.2, 17.2
  - Property 11 → Task 4 (preservation) and verification in task 19
- **Review-driven revisions** (incorporated from design review):
  - Issue 2 (workload kind): Exploration must investigate existing offer model before inventing a new generic-compute path.
  - Issue 3 (discovery): Initial hypothesis was a no-op — `perform_initial_sync` already reaches `mark_discovery_success`. Root cause is likely in error recording or status lifecycle. Exploration-first.
  - Issue 4 (project fixtures): Bug is in harness fixture setup (`build_project_id` vs `publish_ready_project_id`), not the platform's default scaffold.
  - Issue 5 (artifact publish): Fix the caller (agentic tool layer), not the server. Do NOT make `service_id` optional.
  - Issue 6 (soak test): Not implementation-ready — requires profiling to identify the hotspot.
  - Issue 8 (LND permissions): Targeted host-side `chown` for `alice/` and `bob/` only. Do NOT recursive chmod `.lnd` or touch bitcoind volume.
  - Issue 9 (CSR): Already partially fixed during the run. Verify current state before adding signing extensions.
  - Issue 10 (run-ID): Scope `latest-run` reading to continuation subcommands only (`deploy`, `seed`, `test`, `collect`, `status`), not `provision`/`destroy`.
- Each priority tier has its own checkpoint to catch regressions early before proceeding.
- Validation commands are from AGENTS.md.
