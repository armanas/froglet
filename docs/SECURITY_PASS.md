# Pre-Launch Security Pass

**Date:** 2026-04-18
**Updated:** 2026-04-22
**Scope:** One-shot pre-launch review covering
dependency audits, full-history secret scanning, and a threat-model sketch for
the public hosted node (`ai.froglet.dev`). The 2026-04-22 update adds the
publication scrub for the current tracked tree and GitHub-visible refs only
(`origin/main` plus `v0.1.0-alpha.{0,1,2}`); local-only refs such as
`fleet-local-history` are intentionally out of scope for the GitHub scrub.
This is not a standing security program; see [SECURITY.md](../SECURITY.md)
for the vulnerability-report channel.
**Result:** PASS — dependency audits remain clean, and the publication secret
scan reports 0 unallowlisted findings on both the current tracked tree and the
GitHub-visible history after the remediations and narrow commit-scoped
allowlists recorded below.

Evidence artifacts under `_tmp/security_pass/20260418T212849Z/` (local; not
committed): raw `cargo audit`, `pip-audit`, `npm audit` JSON+text, and the
per-fix logs. Publication-scrub evidence for the fresh gitleaks pass lives
under `_tmp/gitleaks_gate/20260422T105843Z/` (local; not committed).

## 1. Dependency Audits

### Rust (`cargo audit`)

Workspace: root `Cargo.toml` + `froglet-protocol/` sub-crate. 449 crates scanned
against the RustSec advisory-db (1,049 advisories loaded).

| ID | Crate | Severity | Status | Action |
| --- | --- | --- | --- | --- |
| RUSTSEC-2026-0098 | `rustls-webpki` 0.103.10 | Vuln | **FIXED** | `cargo update -p rustls-webpki` → 0.103.12. Name constraints for URI names were incorrectly accepted in the vulnerable version. |
| RUSTSEC-2026-0099 | `rustls-webpki` 0.103.10 | Vuln | **FIXED** | Same bump. Name constraints accepted for certificates asserting a wildcard name. |
| RUSTSEC-2026-0097 | `rand` 0.8.5 + 0.9.2 | Unsound (warning) | **ACCEPTED** | The advisory only fires when the caller installs a custom logger that calls `rand::rng()` during its own log-emission path. Froglet installs no such logger (`tracing_subscriber::fmt` default). Transitive paths (quinn-proto, proptest) are dev-only or network-internal. No remediation required. |
| Yanked | `gimli` 0.33.1 | Yanked (warning) | **ACCEPTED** | Transitive only through `wasmtime 42.0.2`. Waiting on upstream wasmtime to bump; not re-pinnable without forking wasmtime's pin. |

Post-fix: `cargo audit` reports **0 vulnerabilities, 3 warnings** (the two
accepted entries above, counted once per crate version).

### Python (`pip-audit`)

Dependencies: [`python/requirements.txt`](../python/requirements.txt).

| ID | Package | Severity | Status | Action |
| --- | --- | --- | --- | --- |
| GHSA-r6ph-v2qm-q3c2 | `cryptography` 45.0.7 | Vuln | **FIXED** | Bumped pin to `cryptography>=46.0.7,<47`. |
| GHSA-m959-cc7f-wv43 | `cryptography` 45.0.7 | Vuln | **FIXED** | Same bump. |
| GHSA-p423-j2cm-9vmq | `cryptography` 45.0.7 | Vuln | **FIXED** | Same bump. |

No direct `import cryptography` in this repo's Python tree — used only
transitively via TLS-speaking deps — so the bump is API-compatible by
inspection. The Python security/privacy/hardening test suite
(`python.tests.test_security` + `test_privacy` + `test_hardening`, 16 tests)
still passes after installing 46.0.7.

Post-fix: `pip-audit -r python/requirements.txt` → **No known vulnerabilities
found**.

### Node (`npm audit`)

Four Node packages in the public tree:

| Package | Pre-fix | Action | Post-fix |
| --- | --- | --- | --- |
| `integrations/mcp/froglet` | 3 vulns (2 mod, 1 high) in `hono`, `@hono/node-server`, `path-to-regexp` | `npm audit fix` (3 packages changed, no `--force` needed) | 0 |
| `docs-site` | 1 high in `vite` (path-traversal + `server.fs.deny` bypass + arbitrary file read) | `npm audit fix` (1 package changed) | 0 |
| `integrations/openclaw/froglet` | No lockfile (published as an npm package) | N/A by design | N/A |
| `integrations/shared/froglet-lib` | No lockfile (published as an npm package) | N/A by design | N/A |

Accepted: the two missing-lockfile packages are intentionally shipped without
committed lockfiles because they are published to npm and downstream consumers
resolve their own transitive trees. Their runtime dependency surfaces are
narrow (stdlib plus `@modelcontextprotocol/sdk` on the MCP side) and are
covered transitively when the top-level `integrations/mcp/froglet` audit runs.

## 2. Secret Scan (Publication Scrub)

Tool: [`scripts/gitleaks_gate.sh`](../scripts/gitleaks_gate.sh). The gate now
runs two distinct scans:

1. **Current tracked tree** — a tracked-files-only snapshot of the working
   tree, so untracked local-only files do not become publication blockers.
2. **GitHub-visible history** — `origin/main` plus the currently published
   tags `v0.1.0-alpha.0`, `v0.1.0-alpha.1`, and `v0.1.0-alpha.2`.

Fresh result on 2026-04-22:

| Scan | Scope | Result |
| --- | --- | --- |
| Current tracked tree | Current working tree, excluding untracked local-only files | **0 findings** |
| GitHub-visible history | `origin/main` + `v0.1.0-alpha.{0,1,2}` | **0 unallowlisted findings** |

The only historical matches on GitHub-visible refs were verified false
positives:

| Rule | Historical location | Commits | Verdict |
| --- | --- | --- | --- |
| `stripe-access-token` | `src/settlement/stripe.rs`, historical `TODO.md`, historical `docs/SECURITY_PASS.md` | `26050948`, `970655fe`, `b8986f70` | Test/documentation placeholders, not real Stripe credentials |
| `generic-api-key` | `test_support.py` | `d3ee6404` | Cashu parser test fixture against a public test mint, not a redeemable secret |

These are now suppressed via narrow commit-scoped allowlists in
[.gitleaks.toml](../.gitleaks.toml). No history rewrite is justified because
no verified real secret was found on a GitHub-visible ref.

## 3. Threat Model Sketch — Hosted `ai.froglet.dev`

This sketch covers the first public hosted Froglet environment. It is scoped
to the launch surface; self-hosted deployments are the user's own trust
boundary. Anything marked **P** is a precondition the hosted-environment
services work (TODO Orders 53–64) must satisfy before the threat model holds
at launch.

### 3.1 Assets (what an attacker wants)

1. The node identity private key (signs offers, receipts, settlement records).
2. The Lightning node macaroons and hot-wallet balance.
3. The Stripe restricted API key and webhook signing secret.
4. The marketplace Postgres read/write credentials.
5. Arbitrary code execution on the node host.
6. Data from other callers' compute runs (information leakage).

### 3.2 Trust boundaries

```
Internet
   │
   ▼
[TLS edge — Caddy or Cloudflare]        ◄── P: first-party HTTPS edge
   │
   ├─ /health, /v1/node/capabilities, /v1/node/identity, /v1/openapi.yaml
   │     → unauthenticated (intentional metadata)
   │
   ├─ public_router (provider role)     ◄── rate-limited (P: edge rate limiting)
   │     events query / provider / publish
   │
   ├─ runtime_router (runtime role)     ◄── require_runtime_auth bearer
   │     execute_wasm / jobs / runtime_routes
   │
   ├─ provider-control endpoints        ◄── require_provider_control_auth bearer
   │
   └─ webhook receivers                 ◄── Stripe signature verify (P: planned webhook receiver)
          │
          ▼
     [workload sandbox]
          │
          ├─ Python: landlock + seccomp (src/python_sandbox.rs)
          └─ Container: docker w/ --network none
```

The combined `router()` (everything merged, unauth) is `#[deprecated]` outside
of tests (`src/api/mod.rs:256-260`). Production deploys must wire
`public_router()` + `runtime_router()` separately; the deploy automation
and operator guide need to enforce this.

### 3.3 Top risks and existing mitigations

| # | Threat | Mitigation today | Gap / depends on |
| --- | --- | --- | --- |
| T1 | LLM-controlled `provider_url` → SSRF / DNS rebind | IP-pinned outbound via `pinnedJsonRequest` in `integrations/shared/froglet-lib/url-safety.js`; `.onion` hostnames rejected from this path (handled by the Rust runtime only) | Operator-configured URLs still use stock `fetch`. Extension gated behind `FROGLET_EGRESS_MODE=strict` remains planned work. |
| T2 | Tenant code escapes the Python sandbox | Linux landlock + seccomp installed via `Command::pre_exec` in `src/python_sandbox.rs` | Landlock is kernel-version-gated. Multi-tenant hosted operation should add microVM isolation before scaling beyond single-operator. |
| T3 | Container escape from `run_container` | `docker run --network none` + mounted tempdir only | Same microVM argument as T2 for genuine multi-tenancy. |
| T4 | Forged offers claiming a descriptor they did not sign | `validate_offer_artifact` checks `offer.signer == offer.payload.provider_id` | Cross-binding `descriptor_hash → descriptor.signer` lookup is planned at the service layer. Low impact; the fixture attacker would still fail at payment. |
| T5 | Stolen node identity key | Identity key is on-disk, operator-protected. No HSM path today. | A documented key-rotation runbook is a launch prerequisite. HSM / KMS is deferred (post-MVP). |
| T6 | Stolen Stripe or Lightning secrets | Secrets injected via env at deploy time; not checked into source (verified by gitleaks §2). | Hosted paid-rail secrets are not part of the v0.1.0 `try.froglet.dev` demo.add trial. Hosted LND, webhook receiver, and rotation procedures are v0.2 hosted paid-rail prerequisites. |
| T7 | Public endpoint abused as compute vector | `ConcurrencyLimitLayer(16)` on provider_routes; request body cap `MAX_BODY_BYTES` | Per-IP rate limiting at the edge is a launch prerequisite. The in-process limit caps concurrency, not calls/sec. |
| T8 | Unauthenticated write via the deprecated combined `router()` | `#[deprecated]` attribute fires outside `cfg(test)`; comment in source warns explicitly | Deploy automation and the operator guide must pick the split routers explicitly; add a smoke check that confirms `/execute_wasm` requires auth in prod. |
| T9 | Channel-state loss on the hosted LND node | SCB plan documented | v0.2 hosted Lightning prerequisite; not applicable to the v0.1.0 free hosted trial. |
| T10 | Forged Stripe webhook → double-settlement | Signature verify + idempotent event-id dedup remain planned work | v0.2 hosted Stripe prerequisite; not applicable to the v0.1.0 free hosted trial. |

### 3.4 Residual risks accepted for launch

- **Tor onion endpoint** — currently promised but not provisioned. Resolution
  is either shipping the hidden service or softening the README. Must be
  closed before the launch post.
- **Tenant isolation beyond landlock+seccomp** — adequate for a
  single-operator launch. The hosted service explicitly is not advertised as
  multi-tenant-hardened until the microVM isolation tier lands.
- **Attestation backend** — only `nvidia.mock.v1` ships today. Any marketing
  claim of "confidential execution" at launch would be misleading; docs must
  keep confidential mode framed as experimental until a real backend lands.

## 4. Incidental Findings

### Pre-existing broken build on `main` (fixed in this pass)

Running `cargo check --all-targets` against `main` (before the rustls-webpki
bump) reproducibly failed with `missing field postgres_mounts` in four test
files. The Postgres-mounts commit (`3c3f0c4`) missed them. This also broke
`./scripts/strict_checks.sh` and therefore the release gate shipped in Order
28. Fixed inline by adding `postgres_mounts: std::collections::BTreeMap::new()`
to the four `NodeConfig` literals in `tests/payments_and_discovery.rs:117`,
`tests/builtin_service_dispatch.rs:144`, `tests/lnd_rest_settlement.rs:507`,
`tests/runtime_routes.rs:203`. Verified with `cargo check --all-targets` clean.

This is worth recording here because it is exactly the kind of silent drift a
pre-launch pass is meant to catch — a red release gate on `main` is a launch
blocker, not a test-hygiene nit.

### Pre-existing rustfmt drift on `main` (fixed in this pass)

`cargo fmt --all --check` (the first step of `scripts/strict_checks.sh`) was
also failing on `main` for cosmetic drift in [src/api/mod.rs](../src/api/mod.rs)
and [src/python_sandbox.rs](../src/python_sandbox.rs). Fixed in place with
`cargo fmt --all`. Zero behavior change, verified by `git diff --stat` showing
only line-shape reformatting. This was the other half of the silent
release-gate break.

## 5. Follow-ups Beyond This Pass

None are gating for launch. Recorded here so they are not lost:

- Consider `cargo deny` in addition to `cargo audit` for advisory-level
  license and source-of-origin checks. Not required today; keeps drift
  visible if the crate tree expands.
- Keep `scripts/gitleaks_gate.sh` aligned with the public-tag set whenever a
  new GitHub-visible release tag is cut.
- Periodic re-run: this doc is dated. Re-run the whole pass quarterly or
  before any subsequent `v0.*` release.
