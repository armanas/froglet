# Security Review Report

## Executive Summary

Scope reviewed: Rust services under `src/`, operator/project tooling, Python helpers under `python/`, Node integrations under `integrations/`, and a light spot-check of `private/`.

I did not find a static break in the protocol signing/hash core during this pass. The highest-risk problems are around trust boundaries outside the signed kernel: outbound provider URL handling in the operator/runtime, mount and process isolation for non-Wasm execution, and project filesystem boundary enforcement. These issues are most serious when Froglet is exposed beyond localhost, connected to untrusted discovery data, or used to run third-party Python/container workloads.

This was a static review. I did not run exploit PoCs against a live node.

## High Severity

### SEC-01: Operator and runtime trust arbitrary provider URLs; the operator also leaks the local runtime bearer token to them

- Rule ID: SSRF-URL-001
- Severity: High
- Location:
  - `src/operator.rs:1028-1038`
  - `src/operator.rs:1095-1102`
  - `src/operator.rs:1124-1204`
  - `src/operator.rs:1510-1554`
  - `src/api.rs:1266-1289`
  - `src/api.rs:1669-1773`
  - `src/discovery_server.rs:167-170`
- Evidence:
  - The operator sends `Authorization: Bearer <runtime_auth_token>` to `"{provider_url}/v1/provider/services"` and `"{provider_url}/v1/provider/services/{service_id}"`.
  - `resolve_provider_reference` returns caller-supplied `provider_url` unchanged.
  - discovery transport URLs are only trimmed, not validated against scheme, host class, or private-network targets.
- Impact:
  - Any caller with Froglet operator auth can turn the operator into an SSRF client and exfiltrate the local runtime token to an attacker-controlled endpoint.
  - Malicious or poisoned discovery entries can cause the operator to probe internal services or send authenticated requests to unintended hosts.
  - The runtime path in `src/api.rs` has the same arbitrary-URL trust pattern for provider selection, which broadens the SSRF surface even where no auth header is forwarded.
- Fix:
  - Never attach the runtime bearer token to public provider endpoints.
  - Reject caller-supplied provider URLs by default, or strictly validate them against expected provider identities.
  - Enforce `https` or onion/allowlisted transports only, and block loopback/private/link-local targets unless explicitly configured.
  - Validate discovery-registered transport URLs on ingest, not only at use time.
- Mitigation:
  - Keep the operator and runtime loopback-only.
  - Do not trust reference discovery as an unrestricted source of provider URLs.
- False positive notes:
  - If deployments guarantee all callers are local admins and all discovery data is trusted, the practical exposure is lower, but the code still creates an avoidable token-exfiltration path.

### SEC-02: Container mount authorization is not enforced on the actual container volume list

- Rule ID: EXEC-MOUNT-001
- Severity: High
- Location:
  - `src/operator.rs:1682-1683`
  - `src/operator.rs:1741-1742`
  - `src/api.rs:6542-6555`
  - `src/api.rs:6673-6714`
  - `src/api.rs:9032-9036`
- Evidence:
  - The operator derives `requested_access` as bare mount handles (`mount.handle.clone()`).
  - The runtime access check expects strings shaped like `mount.<kind>.<read|write>.<handle>`.
  - `execution_mount_context` filters only the JSON context, but `run_container_execution` mounts every `execution.mounts` binding with `-v`, regardless of `granted_access`.
- Impact:
  - A container workload can receive host volume mounts that were never actually granted by the offer/admission path.
  - The access-control model for mounts is internally inconsistent, so even intended restrictions do not protect the real container boundary.
- Fix:
  - Normalize mount capability encoding in one place and use the same format everywhere.
  - Filter the actual `-v` list by the granted access set before spawning the container.
  - Reject workloads that declare mounts without matching granted permissions.
- Mitigation:
  - Avoid container mounts for untrusted workloads until enforcement is fixed.
- False positive notes:
  - If your deployment never uses container mounts, this is dormant. The code path is still unsafe once mounts are enabled.

### SEC-03: Non-Wasm execution is host-level, and deadline enforcement does not terminate child processes

- Rule ID: EXEC-ISOLATION-001
- Severity: High
- Location:
  - `src/api.rs:6569-6654`
  - `src/api.rs:6673-6735`
  - `src/api.rs:8503-8523`
  - `src/provider_projects.rs:654-715`
- Evidence:
  - Python workloads are executed by spawning `python3 -I` and `exec(...)`-ing untrusted source on the host.
  - Container workloads are spawned as local `docker`/`podman` processes.
  - The timeout helper only wraps `spawn_blocking`; on timeout it returns an error but does not kill spawned children or their process groups.
- Impact:
  - Python workloads are not confined by the Wasm sandbox and run with host process privileges.
  - A timed-out Python or container workload can continue running after Froglet reports failure, enabling persistent CPU/disk/memory exhaustion and orphaned processes.
  - `provider_projects::test_project` has the same host-execution pattern without any timeout at all.
- Fix:
  - Treat non-Wasm runtimes as privileged features and disable them by default unless a real sandbox is present.
  - Move Python/container execution into an isolation boundary that can actually be terminated.
  - Use process-group-aware child management and kill on timeout.
- Mitigation:
  - Restrict Python/container offers to fully trusted code only.
  - Keep operator project testing unavailable to untrusted callers.
- False positive notes:
  - If the product explicitly assumes providers only run their own trusted code, the host-RCE aspect may be accepted risk, but the missing timeout kill still leaves a concrete DoS issue.

## Medium Severity

### SEC-04: Provider project file protections miss symlink leaves, allowing project-root escape through pre-existing symlinks

- Rule ID: PATH-SYMLINK-001
- Severity: Medium
- Location:
  - `src/provider_projects.rs:507-541`
  - `src/provider_projects.rs:560-568`
  - `src/provider_projects.rs:620-626`
  - `src/provider_projects.rs:1003-1031`
- Evidence:
  - `resolve_relative_path` validates path components and checks parent directories for symlinks, but it does not reject the final file if that file itself is a symlink.
  - Reads, writes, and builds then open the returned path directly with `fs::read_to_string`/`fs::write`.
- Impact:
  - If a project tree contains a symlinked file, provider project APIs can read or overwrite files outside the project root, and build/test paths can execute code sourced outside the project.
- Fix:
  - Reject symlinks on the final path as well as parent directories.
  - Prefer `symlink_metadata` plus no-follow open/write semantics where available.
  - Re-check the fully resolved path immediately before I/O.
- Mitigation:
  - Keep provider project roots on private storage not populated from untrusted repos or archives.
- False positive notes:
  - This requires a symlink to already exist in the project tree; the current HTTP API does not appear to create symlinks by itself.

### SEC-05: JS and Python clients allow bearer tokens to be sent over non-loopback plaintext HTTP

- Rule ID: TOKEN-TRANSPORT-001
- Severity: Medium
- Location:
  - `integrations/shared/froglet-lib/shared.js:28-45`
  - `integrations/shared/froglet-lib/froglet-client.js:18-28`
  - `python/froglet_client.py:473-513`
- Evidence:
  - The shared Node config accepts any `http:` or `https:` base URL.
  - Requests always attach `Authorization: Bearer <token>`.
  - The Python client does not validate the base URL at all before sending requests.
  - The separate doctor script already contains stricter logic (`https` or loopback `http`), which shows the safer intended policy exists but is not enforced in the actual clients.
- Impact:
  - Misconfigured MCP/OpenClaw/Python clients can send control or runtime bearer tokens over plaintext network links to remote hosts.
- Fix:
  - Enforce `https` or loopback-only `http` in the actual client libraries, not just the doctor utility.
  - Fail closed on insecure remote URLs unless an explicit insecure-dev override is set.
- Mitigation:
  - Use only loopback `http://127.0.0.1/...` for local development and `https://...` everywhere else.
- False positive notes:
  - This is configuration-driven; fully local loopback usage is fine.

## Residual Notes

- The reference discovery service exposes registration and lookup surfaces without visible rate limiting or abuse controls. I did not elevate this to a formal finding because that may be intentionally delegated to deployment infrastructure, but it is worth verifying at runtime.
- I did not find a static cryptographic mismatch in artifact signing, hashing, or Lightning receipt verification during this pass.
