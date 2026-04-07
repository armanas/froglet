# GCP Harness Issue Triage Bugfix Design

## Overview

This design addresses 10 operational/runtime bugs discovered during a GCP harness run. The bugs span SSRF validation gaps, workload kind routing mismatches, discovery status race conditions, project fixture inconsistencies, API shape mismatches, latency regression, lint gating, Docker permissions, CSR generation, and harness run-ID pinning. Each bug has a distinct root cause in a specific code path; fixes are minimal and targeted.

## Glossary

- **Bug_Condition (C)**: The set of inputs/states that trigger each specific bug
- **Property (P)**: The desired correct behavior when the bug condition holds
- **Preservation**: Existing behavior that must remain unchanged by each fix
- **operator_accessible_provider_url**: Function in `src/operator.rs` that resolves provider URLs for operator-initiated requests, including SSRF validation
- **build_execution_from_compute_request**: Function in `src/operator.rs` that constructs a `WorkloadSpec` from a `RunComputeRequest`
- **froglet_run_compute**: Handler in `src/operator.rs` that hardcodes `offer_id = "execute.compute"` for all direct compute requests
- **mark_discovery_success**: Function in `src/discovery_client.rs` that sets `connected = true` on the `ReferenceDiscoveryStatus`
- **ProviderControlPublishArtifactRequest**: Struct in `src/api/types.rs` requiring `service_id: String` (non-optional)
- **generate_role_cert()**: Function in `scripts/gcp_harness.sh` that generates TLS certificates for GCP harness roles

## Bug Details

### Bug Condition

The 10 bugs manifest across different subsystems. The composite bug condition is:

```
FUNCTION isBugCondition(input)
  INPUT: input of type HarnessOperation
  OUTPUT: boolean

  RETURN
    (input.type == "run_compute" OR input.type == "invoke_service")
      AND input.provider_url targets loopback/private AND input.provider_id is absent
      AND system falls through to local provider (Issue 1)
    OR
    (input.type == "run_compute")
      AND input.runtime IN ["python", "container"]
      AND system hardcodes offer_id "execute.compute" with offer_kind "compute.wasm.v1"
      AND workload_kind == "compute.execution.v1" (Issue 2)
    OR
    (input.type == "status_query")
      AND reference_discovery is configured AND initial registration succeeded
      AND status query arrives before mark_discovery_success completes (Issue 3)
    OR
    (input.type == "project_lifecycle")
      AND scaffold is default (no starter, no inline_source, no result_json)
      AND harness expects source/main.wat but scaffold writes source/main.py (Issue 4)
    OR
    (input.type == "artifact_publish")
      AND payload omits service_id field
      AND ProviderControlPublishArtifactRequest requires service_id: String (Issue 5)
    OR
    (input.type == "soak_test")
      AND sustained load causes p99 latency to exceed 2x baseline (Issue 6)
    OR
    (input.type == "clippy")
      AND src/api/test_support.rs has #![cfg(test)] inner attribute
      AND src/api/mod.rs already gates with #[cfg(test)] mod test_support (Issue 7)
    OR
    (input.type == "lnd_regtest")
      AND host is Linux AND Docker volumes create root-owned dirs (Issue 8)
    OR
    (input.type == "csr_generation")
      AND OpenSSL config includes authorityKeyIdentifier in v3_req (Issue 9)
    OR
    (input.type == "harness_subcommand")
      AND FROGLET_GCP_HARNESS_RUN_ID is not exported
      AND latest-run file exists but is not read (Issue 10)
END FUNCTION
```

### Examples

- **Issue 1**: `run_compute(provider_url="https://127.0.0.1:8080", provider_id=None)` → system accepts and routes to local provider instead of rejecting (SSRF bypass for absent provider_id with loopback URL)
- **Issue 2**: `run_compute(runtime="python", inline_source="def handler(e,c): return 42")` → provider rejects with "offer does not match workload kind" because offer_kind is "compute.wasm.v1" but workload_kind is "compute.execution.v1"
- **Issue 3**: Operator status query immediately after startup → `reference_discovery.connected = false` even though registration succeeded, because `mark_discovery_success` hasn't been called yet in the non-required path
- **Issue 4**: `tool.read_file.happy` on default scaffold → fails because scaffold writes `source/main.py` (Python) but harness expects `source/main.wat` (Wasm)
- **Issue 5**: Agentic POST to `/v1/froglet/artifacts/publish` without `service_id` → deserialization error "missing field `service_id`"
- **Issue 7**: `cargo clippy --all-targets -- -D warnings` → fails on redundant `#![cfg(test)]` in `src/api/test_support.rs`
- **Issue 9**: `generate_role_cert("froglet-discovery")` → CSR includes `authorityKeyIdentifier` which is invalid for CSR extensions
- **Issue 10**: `gcp_harness.sh deploy` then `gcp_harness.sh seed` without export → seed creates new run directory instead of using deploy's directory

## Expected Behavior

### Preservation Requirements

**Unchanged Behaviors:**
- Public HTTPS provider URLs must continue to be accepted and routed normally (3.1)
- Local provider_id with loopback URL must continue to route to local provider (3.2)
- Wasm runtime direct compute must continue to use "execute.compute" offer with "compute.wasm.v1" (3.3)
- Service-addressed invocations must continue to use the service's own offer_id (3.4)
- Discovery heartbeat failures must continue to set connected=false (3.5)
- Unconfigured discovery must continue to report enabled=false (3.6)
- Explicit scaffold projects must continue to work as provided (3.7)
- Blank scaffold publish rejection must continue to work (3.8)
- Artifact publish with valid service_id must continue to work (3.9)
- Soak test error rate threshold must continue to be enforced (3.10)
- Properly gated test modules must continue to compile (3.11)
- macOS Docker volume permissions must not be affected (3.12)
- Explicit FROGLET_GCP_HARNESS_RUN_ID must continue to take precedence (3.13)
- CA-signed certificates must continue to include correct v3 extensions (3.14)

**Scope:**
All inputs that do NOT match the specific bug conditions above should be completely unaffected by these fixes. The fixes are minimal and targeted to the specific code paths identified.

## Hypothesized Root Cause

### Issue 1: SSRF / private-network bypass (P0)

**Root Cause**: In `operator_accessible_provider_url` (src/operator.rs:433), when `provider_id` is `None`, the condition `provider_id.is_none_or(|value| value == state.app_state.identity.node_id())` evaluates to `true` because `is_none_or` returns `true` for `None`. This means any loopback/private URL with no `provider_id` falls through to `provider_base_url(state)` — effectively routing to the local provider without any SSRF rejection.

The fix should treat `provider_id = None` with a loopback URL as an error, not as a local-provider shortcut. Only an explicit `provider_id` matching the local node identity should be allowed to use loopback URLs.

### Issue 2: Workload kind mismatch (P0)

**Root Cause**: `froglet_run_compute` (src/operator.rs:2449) hardcodes `"execute.compute"` as the offer_id for all direct compute requests. The builtin `"execute.compute"` offer is defined in `builtin_provider_offer_definitions` (src/api/mod.rs:4020) with `offer_kind = wasm::WORKLOAD_KIND_COMPUTE_WASM_V1` ("compute.wasm.v1"). When `build_execution_from_compute_request` produces a Python or Container workload, the `WorkloadSpec` has `workload_kind = "compute.execution.v1"`. The provider's quote path validates `offer.payload.offer_kind == workload_kind`, which fails.

**Design decision needed**: The hardcoded `"execute.compute"` in the operator is the real bug, but the fix must account for the existing offer-kind normalization and test assumptions around `execute.compute` being wasm-only. Before jumping to a new `"execute.compute.generic"` offer ID, the design must first determine whether: (a) the operator should select an existing execution-v1-capable offer that the provider already advertises, or (b) the provider should advertise a new builtin offer under the current model. The exploration phase (task 2) should investigate which execution-v1 offers already exist in the provider's offer set and how the offer-kind normalization works, then decide the minimal fix path.

### Issue 3: Discovery status false disconnection (P1)

**Root Cause (revised)**: The initial hypothesis that `perform_initial_sync` doesn't reach `mark_discovery_success` is incorrect — `perform_initial_sync()` already calls `register_node()` → `register_node_inner()` → `mark_discovery_success()` on success. Moving flag-setting around inside `perform_initial_sync()` is effectively a no-op.

If status still shows `connected=false` after later successful registrations, the bug is more likely in one of these areas:
1. **Error recording overwriting success**: A later error in the heartbeat loop or a transient failure could reset `connected` to `false` after it was set to `true`, and the status query happens to land in that window.
2. **Status lifecycle**: The `ReferenceDiscoveryStatus` struct may be read from a stale snapshot or the `connected` flag may be reset by a path other than `mark_discovery_success`.
3. **Non-required path timing**: In the non-required path (server.rs:440), the spawned task may not have started executing by the time the first status query arrives — but this is an inherent async startup race, not a missing `mark_discovery_success` call.

The exploration phase (task 3) should investigate the actual status lifecycle: trace all callers of `mark_discovery_success` and all writers to the `connected` field, check whether any error path resets `connected` after a successful registration, and determine the actual race window. The fix depends on what the exploration finds.

### Issue 4: Project build/publish fixture inconsistency (P1)

**Root Cause (revised)**: The bug is in the harness fixture setup, not the platform's default scaffold behavior. The harness uses two project IDs: `build_project_id` (bootstrapped as a hidden blank project) and `publish_ready_project_id` (created with `starter: "hello_world"`). The failure is the harness expecting WAT files from `build_project_id` (the blank bootstrap fixture), not from the publish-ready project. The platform's default scaffold correctly writes `source/main.py` for blank projects.

The fix is in the harness test fixtures (`tests/e2e/gcp_harness/run-matrix.mjs` and `generate-scenarios.mjs`): either (a) create `build_project_id` with an explicit starter so it has WAT content, or (b) update the `tool.read_file.happy` test to target `publish_ready_project_id` which already has the correct scaffold, or (c) update the expected file paths for `build_project_id` to match the Python scaffold it actually produces.

### Issue 5: Artifact publish API rejects agentic payload (P1)

**Root Cause**: `ProviderControlPublishArtifactRequest` (src/api/types.rs:464) declares `service_id: String` as a required field (no `#[serde(default)]`). When the agentic tooling sends a POST without `service_id`, serde deserialization fails with "missing field `service_id`".

**Design decision (revised)**: Making `service_id` optional broadens a stable API contract to compensate for a bad caller. The server currently keys normalization off `service_id`, and the harness already supplies it everywhere. The failing case came from the agentic/tool layer omitting a required field.

The correct fix is in the caller, not the server:
1. **Primary fix**: Update the agentic tool schema/prompt (in `integrations/openclaw/froglet/` and/or `integrations/mcp/froglet/`) to include `service_id` in the artifact publish payload.
2. **Secondary improvement**: Wrap the server's deserialization error into a structured JSON error response with a clear message indicating `service_id` is required, rather than returning a raw serde error. This improves the developer experience without weakening the API contract.

### Issue 6: Soak test latency regression (P1)

**Root Cause**: The p99 latency increase from ~56.4ms to ~305.5ms (+442%) suggests a resource contention issue under sustained load. Likely causes include: SQLite write contention (single-writer), event query capacity limits, or lack of connection pooling. The soak test (python/tests/test_soak.py) publishes events at moderate concurrency (10 workers) and checks that p99 doesn't increase more than 100% from baseline.

This requires profiling to identify the specific bottleneck. Potential fixes include: adding WAL mode to SQLite, batching event inserts, or adjusting the events query capacity semaphore.

### Issue 7: Duplicate test gating (P2)

**Root Cause**: `src/api/test_support.rs` line 1 has `#![cfg(test)]` (inner attribute), while `src/api/mod.rs` line 70 gates the module with `#[cfg(test)] mod test_support;`. The inner attribute is redundant and clippy flags it.

The fix is to remove `#![cfg(test)]` from `src/api/test_support.rs`.

### Issue 8: LND regtest host-permission sensitivity (P2)

**Root Cause**: The LND Docker container runs as root and creates directories (`alice/data/chain/bitcoin/regtest/`, `bob/data/chain/bitcoin/regtest/`) owned by `root:root` inside the mounted volume. On Linux, the test process (running as a non-root user) cannot read `admin.macaroon` from these root-owned directories. On macOS, Docker Desktop's file sharing layer transparently handles permissions.

**Design decision (revised)**: A recursive `chmod 755 /root/.lnd` is unsafe — it makes `admin.macaroon` broadly readable and risks breaking the bitcoind volume (which caused a failure in the first rerun). The fix should be targeted:
1. **Preferred**: Fix host-side ownership of `alice/` and `bob/` directories only (not the entire `.lnd` tree), using `chown` to match the test process UID/GID. This is scoped to the mounted volume directories the test process needs to read.
2. **Alternative**: Run the LND containers with a compatible `--user uid:gid` matching the host test process, so created files are owned correctly from the start.
3. **Do NOT** touch the bitcoind volume or recursively chmod the entire `.lnd` directory.

### Issue 9: GCP harness CSR generation (P3)

**Root Cause (revised)**: The bug was in the leaf CSR path — `authorityKeyIdentifier` was previously included in `[v3_req]` which is used for CSR generation, and that's invalid for CSRs. The current code has already been partially fixed during the harness run: `[v3_req]` no longer contains `authorityKeyIdentifier`. The CA config already contains `authorityKeyIdentifier` in its own section.

**Remaining work**: The observed failure was the CSR path, which is now fixed. Adding a separate `[v3_sign]` section for the signing step may be a nice-to-have for correctness (ensuring the signed leaf certificate includes `authorityKeyIdentifier`), but it is not justified by the observed failure. The exploration phase should verify the current state of the config and confirm the CSR path no longer fails. If the signed certificate already gets `authorityKeyIdentifier` from the CA config, no further changes are needed.

### Issue 10: GCP harness run-ID not pinned (P3)

**Root Cause**: In `scripts/gcp_harness.sh`, the run ID is set via `run_id_default="$(date -u +%Y%m%d%H%M%S)"` and `FROGLET_GCP_HARNESS_RUN_ID` defaults to this timestamp. The script writes the run ID to `latest-run` file (`printf '%s\n' "$FROGLET_GCP_HARNESS_RUN_ID" >"$latest_run_file"`), but it does NOT read from `latest-run` when `FROGLET_GCP_HARNESS_RUN_ID` is unset. Each subcommand invocation generates a new timestamp.

**Design decision (revised)**: Reading `latest-run` as the global default for ALL subcommands is too broad — it would make `provision` and `destroy` silently target the previous run unless the caller overrides. The fix should scope `latest-run` reading to continuation subcommands only (`deploy`, `seed`, `test`, `collect`, `status`) while leaving `provision` and `destroy` to always generate a fresh run ID by default. This prevents trading one footgun for another.

## Correctness Properties

Property 1: Bug Condition - SSRF Rejection for Absent Provider ID with Loopback URL

_For any_ `run_compute` or `invoke_service` request where the `provider_url` targets a loopback or private-network address AND `provider_id` is absent (None), the fixed `operator_accessible_provider_url` function SHALL reject the request with HTTP 400 and an error message indicating the URL targets a local or private-network address.

**Validates: Requirements 2.1**

Property 2: Bug Condition - Workload Kind Routing for Non-Wasm Runtimes

_For any_ `run_compute` request where the runtime is "python" or "container" and the resulting `WorkloadSpec` has `workload_kind = "compute.execution.v1"`, the fixed `froglet_run_compute` function SHALL select an offer whose `offer_kind` matches `"compute.execution.v1"` so the provider accepts the deal.

**Validates: Requirements 2.2, 2.3**

Property 3: Bug Condition - Discovery Connected Flag After Successful Registration

_For any_ status query that occurs after the reference discovery heartbeat loop has completed at least one successful registration, the fixed system SHALL report `reference_discovery.connected = true`.

**Validates: Requirements 2.4**

Property 4: Bug Condition - Project Scaffold Fixture Consistency

_For any_ harness test that creates a default project and reads files from it, the fixed system SHALL produce a scaffold whose file paths and content match what the harness expects.

**Validates: Requirements 2.5, 2.6, 2.7**

Property 5: Bug Condition - Artifact Publish Without service_id

_For any_ POST to `/v1/froglet/artifacts/publish` that omits the `service_id` field, the fixed system SHALL either derive `service_id` from the payload context or return a clear validation error, rather than a raw deserialization error.

**Validates: Requirements 2.8**

Property 6: Bug Condition - Soak Test Latency Stability

_For any_ sustained publish/query load run, the fixed system SHALL maintain p99 latency within the 100% degradation threshold (p99 final ≤ 2× p99 baseline).

**Validates: Requirements 2.9**

Property 7: Bug Condition - Clippy Clean Build

_For any_ `cargo clippy --all-targets -- -D warnings` invocation, the fixed codebase SHALL pass without warnings from `src/api/test_support.rs`.

**Validates: Requirements 2.10**

Property 8: Bug Condition - LND Regtest File Permissions

_For any_ LND regtest harness run on Linux with Docker-mounted state, the fixed harness SHALL ensure `admin.macaroon` files are readable by the test process.

**Validates: Requirements 2.11**

Property 9: Bug Condition - CSR Generation Without authorityKeyIdentifier

_For any_ CSR generated by `generate_role_cert()`, the fixed OpenSSL config SHALL NOT include `authorityKeyIdentifier` in the CSR extensions section.

**Validates: Requirements 2.12**

Property 10: Bug Condition - Harness Run-ID Persistence

_For any_ sequence of `gcp_harness.sh` subcommands without explicit `FROGLET_GCP_HARNESS_RUN_ID`, the fixed script SHALL read the run ID from the `latest-run` file so subsequent subcommands operate on the same run directory.

**Validates: Requirements 2.13**

Property 11: Preservation - Existing Behavior Unchanged

_For any_ input where none of the 10 bug conditions hold, the fixed functions SHALL produce the same result as the original functions, preserving all existing functionality including public URL routing, Wasm direct compute, discovery error reporting, explicit scaffold projects, valid artifact publish, soak error thresholds, test module compilation, macOS Docker behavior, explicit run-ID override, and CA certificate extensions.

**Validates: Requirements 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7, 3.8, 3.9, 3.10, 3.11, 3.12, 3.13, 3.14**

## Fix Implementation

### Changes Required

Assuming our root cause analysis is correct:

### Issue 1: SSRF / private-network bypass (P0)

**File**: `src/operator.rs`
**Function**: `operator_accessible_provider_url`

**Specific Changes**:
1. **Tighten None provider_id handling**: Change the loopback fallback condition from `provider_id.is_none_or(...)` to `provider_id.is_some_and(|value| value == state.app_state.identity.node_id())`. When `provider_id` is `None` and the URL is loopback/private, reject with HTTP 400 instead of falling through to the local provider.

```rust
// Before (line ~447):
if provider_id.is_none_or(|value| value == state.app_state.identity.node_id()) {
    return provider_base_url(state);
}

// After:
if provider_id.is_some_and(|value| value == state.app_state.identity.node_id()) {
    return provider_base_url(state);
}
```

2. **Update existing test**: The test `operator_accessible_provider_url_rejects_https_loopback_for_remote_provider` already tests rejection with a non-local provider_id. Add a new test for `provider_id = None` with loopback URL.

### Issue 2: Workload kind mismatch (P0)

**File**: `src/operator.rs`
**Function**: `froglet_run_compute`

**Specific Changes**:
1. **Investigate existing offer model first**: Before adding a new builtin offer, the exploration phase must determine whether an execution-v1-capable offer already exists in the provider's offer set, and how the offer-kind normalization in `builtin_provider_offer_definitions` (src/api/mod.rs:3835, 3932, 11078) interacts with the quote validation path.
2. **Dynamic offer_id selection**: After `build_execution_from_compute_request` returns the `WorkloadSpec`, inspect its `workload_kind()` to select the correct offer_id. The exact offer_id depends on what the exploration finds — it may be an existing offer or a new one.
3. **If a new builtin offer is needed**: Add it to `builtin_provider_offer_definitions` with `offer_kind = "compute.execution.v1"`, but ensure it integrates with the existing offer-kind normalization and doesn't break test assumptions around `execute.compute` being wasm-only.

### Issue 3: Discovery status false disconnection (P1)

**File**: `src/discovery_client.rs`, `src/server.rs`, `src/state.rs`

**Specific Changes**:
1. **Exploration-first**: The initial hypothesis (moving `mark_discovery_success` around in `perform_initial_sync`) was a no-op. The exploration phase must:
   - Trace all callers of `mark_discovery_success` and all writers to the `connected` field
   - Check whether any error path (heartbeat failure, transient error) resets `connected` to `false` after a successful registration
   - Determine whether the status query reads from a stale snapshot
   - Check the non-required path timing in `server.rs:440` to see if the spawned task has a meaningful race window
2. **Fix depends on exploration findings**: If the bug is an error path overwriting success, the fix is to not reset `connected` on transient errors after initial success. If it's a stale snapshot, the fix is in the status read path. If it's purely the async startup race, the fix may be to await initial sync before accepting requests in the non-required path.

### Issue 4: Project build/publish fixture inconsistency (P1)

**File**: `tests/e2e/gcp_harness/run-matrix.mjs`, `tests/e2e/gcp_harness/generate-scenarios.mjs`

**Specific Changes**:
1. **Fix the harness fixture, not the platform**: The `build_project_id` is bootstrapped as a hidden blank project (which produces a Python scaffold), while `publish_ready_project_id` is created with `starter: "hello_world"` (which produces a Wasm scaffold). The `tool.read_file.happy` test targets `build_project_id` but expects WAT files.
2. **Option A (preferred)**: Update `tool.read_file.happy` to target `publish_ready_project_id` which already has the correct Wasm scaffold.
3. **Option B**: Create `build_project_id` with an explicit starter so it has WAT content.
4. **Option C**: Update the expected file paths for `build_project_id` to match the Python scaffold it actually produces (`source/main.py` instead of `source/main.wat`).

### Issue 5: Artifact publish API shape (P1)

**File**: `integrations/openclaw/froglet/` and/or `integrations/mcp/froglet/` (agentic tool layer)

**Specific Changes**:
1. **Fix the caller, not the server**: Update the agentic tool schema/prompt to include `service_id` in the artifact publish payload. The server's `ProviderControlPublishArtifactRequest` correctly requires `service_id` — the harness already supplies it everywhere.
2. **Do NOT make `service_id` optional**: The server keys normalization off `service_id`. Weakening the API contract to compensate for a bad caller is the wrong fix.

**File**: `src/api/mod.rs` (deserialization error handling)

3. **Improve error response**: Wrap serde deserialization errors for `/v1/froglet/artifacts/publish` into a structured JSON error response with a clear message indicating which required fields are missing, rather than returning a raw serde error string.

### Issue 6: Soak test latency regression (P1)

**File**: `src/api/mod.rs` (event insertion path)

**Specific Changes**:
1. **Profile and identify bottleneck**: The most likely cause is SQLite write contention. Ensure WAL mode is enabled for the events database.
2. **Batch event inserts**: If the bottleneck is per-event transaction overhead, batch inserts within a single transaction.
3. **Review events query capacity semaphore**: The `try_acquire_events_query_permit` may be too restrictive under sustained load.

### Issue 7: Duplicate test gating (P2)

**File**: `src/api/test_support.rs`

**Specific Changes**:
1. **Remove redundant inner attribute**: Delete `#![cfg(test)]` from line 1 of `src/api/test_support.rs`.

### Issue 8: LND regtest host-permission sensitivity (P2)

**File**: `python/tests/test_support.py`
**Function**: `start_lnd_regtest_cluster`

**Specific Changes**:
1. **Targeted host-side ownership fix**: After `_wait_for_path` for each node's `admin_macaroon_path`, fix ownership of only the `alice/` and `bob/` host-side directories (not the entire `.lnd` tree) using `chown` to match the test process UID/GID.
2. **Do NOT recursively chmod the entire `.lnd` directory** — this makes `admin.macaroon` broadly readable and risks breaking the bitcoind volume.
3. **Do NOT touch the bitcoind volume** — this broke the container in the first rerun.
4. **Guard to Linux only** — skip on macOS where Docker Desktop handles permissions transparently.

```python
# After _wait_for_path for macaroon — targeted fix:
import platform
if platform.system() == "Linux":
    for node_key in ["alice", "bob"]:
        host_dir = os.path.join(cluster.data_dir, node_key)
        subprocess.run(["sudo", "chown", "-R", f"{os.getuid()}:{os.getgid()}", host_dir], check=True)
```

**Alternative**: Run the LND containers with `--user uid:gid` matching the host test process, so created files are owned correctly from the start.

### Issue 9: GCP harness CSR generation (P3)

**File**: `scripts/gcp_harness.sh`
**Function**: `generate_role_cert()`

**Specific Changes**:
1. **Verify current state**: The CSR path bug (authorityKeyIdentifier in `[v3_req]`) was already fixed locally during the harness run. The exploration phase should confirm the current code no longer has `authorityKeyIdentifier` in `[v3_req]`.
2. **If already fixed**: Mark as resolved. The CA config already contains `authorityKeyIdentifier` in its own section, so the signed certificate should already include it.
3. **If the signed leaf certificate is missing `authorityKeyIdentifier`**: Only then add a `[v3_sign]` section. But this is not justified by the observed failure (which was the CSR path, not the signed cert).

### Issue 10: GCP harness run-ID not pinned (P3)

**File**: `scripts/gcp_harness.sh`

**Specific Changes**:
1. **Scope latest-run reading to continuation subcommands only**: Instead of making `latest-run` the global default for all subcommands, only read it for continuation commands (`deploy`, `seed`, `test`, `collect`, `status`). Leave `provision` and `destroy` to always generate a fresh run ID by default.

```bash
# Continuation subcommands read latest-run:
continuation_cmds="deploy|seed|test|collect|status"
if [[ -z "${FROGLET_GCP_HARNESS_RUN_ID:-}" && -f "$latest_run_file" && "$subcommand" =~ ^($continuation_cmds)$ ]]; then
  run_id_default="$(cat "$latest_run_file")"
else
  run_id_default="$(date -u +%Y%m%d%H%M%S)"
fi
```

2. **Explicit `FROGLET_GCP_HARNESS_RUN_ID` still takes precedence** for all subcommands.

## Testing Strategy

### Validation Approach

The testing strategy follows a two-phase approach: first, surface counterexamples that demonstrate the bug on unfixed code, then verify the fix works correctly and preserves existing behavior.

### Exploratory Bug Condition Checking

**Goal**: Surface counterexamples that demonstrate the bugs BEFORE implementing fixes. Confirm or refute the root cause analysis. If we refute, we will need to re-hypothesize.

**Test Plan**: Write targeted tests for each issue that exercise the specific bug condition on unfixed code.

**Test Cases**:
1. **SSRF None provider_id test**: Call `operator_accessible_provider_url` with `provider_id=None` and a loopback URL — expect it to incorrectly succeed (will pass on unfixed code, demonstrating the bug)
2. **Workload kind mismatch test**: Call `froglet_run_compute` with `runtime=python` — expect provider rejection with "offer does not match workload kind" (will fail on unfixed code)
3. **Discovery connected race test**: Query status immediately after spawning the non-required discovery sync — expect `connected=false` even after registration succeeds (will demonstrate the race on unfixed code)
4. **Clippy test**: Run `cargo clippy --all-targets -- -D warnings` — expect failure on `test_support.rs` (will fail on unfixed code)
5. **CSR authorityKeyIdentifier test**: Generate a CSR with the current config and inspect extensions — verify whether `authorityKeyIdentifier` is present
6. **Run-ID pinning test**: Run two subcommands without export and verify they use different run directories (will demonstrate the bug on unfixed code)

**Expected Counterexamples**:
- Issue 1: `operator_accessible_provider_url(state, "https://127.0.0.1:8443", None)` returns `Ok(local_provider_url)` instead of `Err`
- Issue 2: Provider returns 400 with "offer does not match workload kind"
- Issue 7: Clippy emits warning about redundant `#![cfg(test)]`

### Fix Checking

**Goal**: Verify that for all inputs where the bug condition holds, the fixed function produces the expected behavior.

**Pseudocode:**
```
FOR ALL input WHERE isBugCondition(input) DO
  result := fixedFunction(input)
  ASSERT expectedBehavior(result)
END FOR
```

### Preservation Checking

**Goal**: Verify that for all inputs where the bug condition does NOT hold, the fixed function produces the same result as the original function.

**Pseudocode:**
```
FOR ALL input WHERE NOT isBugCondition(input) DO
  ASSERT originalFunction(input) = fixedFunction(input)
END FOR
```

**Testing Approach**: Property-based testing is recommended for preservation checking because:
- It generates many test cases automatically across the input domain
- It catches edge cases that manual unit tests might miss
- It provides strong guarantees that behavior is unchanged for all non-buggy inputs

**Test Plan**: Observe behavior on UNFIXED code first for non-bug inputs, then write property-based tests capturing that behavior.

**Test Cases**:
1. **Public URL Preservation**: Verify that public HTTPS provider URLs continue to be accepted and routed normally after the SSRF fix
2. **Local Provider ID Preservation**: Verify that explicit local provider_id with loopback URL continues to route to local provider
3. **Wasm Compute Preservation**: Verify that Wasm runtime direct compute continues to use "execute.compute" offer
4. **Discovery Error Preservation**: Verify that discovery heartbeat failures continue to set connected=false
5. **Scaffold Preservation**: Verify that explicit scaffold projects continue to work as provided
6. **Artifact Publish Preservation**: Verify that artifact publish with valid service_id continues to work

### Unit Tests

- Test `operator_accessible_provider_url` with None provider_id and loopback URL → expect rejection
- Test `operator_accessible_provider_url` with local provider_id and loopback URL → expect success (preservation)
- Test `operator_accessible_provider_url` with public URL → expect success (preservation)
- Test `froglet_run_compute` with runtime=python → expect correct offer_id selection
- Test `froglet_run_compute` with runtime=wasm → expect "execute.compute" (preservation)
- Test `ProviderControlPublishArtifactRequest` deserialization without service_id → expect success
- Test `ProviderControlPublishArtifactRequest` deserialization with service_id → expect success (preservation)
- Test `#![cfg(test)]` removal does not break test_support compilation

### Property-Based Tests

- Generate random provider URLs (public, private, loopback, onion) with random provider_id (None, local, remote) and verify SSRF classification is correct
- Generate random RunComputeRequest payloads with various runtimes and verify offer_id selection matches workload_kind
- Generate random artifact publish payloads with and without service_id and verify correct handling

### Integration Tests

- Full GCP harness deploy → seed → run-matrix flow with pinned run-ID
- LND regtest flow on Linux with Docker permission fix
- Soak test with latency monitoring after performance fix
- Clippy clean build after test_support.rs fix
