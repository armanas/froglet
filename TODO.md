# Froglet TODO / Roadmap Notes

This file preserves roadmap order references used by code comments, tests, and
design stubs. It is not the kernel specification. `docs/KERNEL.md` remains the
authoritative source for canonical artifact payloads, hashing, signing bytes,
state transitions, and settlement bindings.

## Current checkpoint — 28 September 2026

Product development is paused while the next direction is decided. Preserve the
existing work; this checkpoint does not authorize a relaunch or new features.

| Area | State | Where to resume |
|---|---|---|
| Public website | Temporary “Coming back soon” page is live. The redesigned full site is preserved. | [Deployment and exact rollback](docs-site/README.md#temporary-pause-2026-09-28). `npm run deploy` keeps maintenance; `npm run deploy:site` explicitly ends it. |
| File downloads (F1) | Implemented and exercised locally, capped at 8 MiB; not released or deployed. | [Implementation checkpoint and evidence](docs/FILE_AND_COMPUTE_SCOPE.md#10-implementation-checkpoint--2026-09-28). Node, share UI and companion relay must be qualified together. |
| CPU containers (C1) | Recovery/quarantine foundations exist. General jobs are incomplete. | Controlled pulls with hard disk limits, batch adapter, status/cancel/results and Linux isolation/deadline qualification. |
| GPU (G1) | Execution disabled in the current worker; not a rental capability. | Finish C1 first, then qualify actual NVIDIA hardware and exclusive-device cleanup. |
| Payments | Deferred by the operator. | No new rails, purchases, paid flow, or automatic allowance refill. |

When development resumes:

1. Choose the next user-facing outcome. The smallest prepared candidate is F1
   download-only sharing; do not bundle CPU/GPU or payment work into its release.
2. Use matching `froglet` and `froglet-services` checkpoints. The companion
   relay carries the download headers/8 MiB transfer coverage; its OCI worker
   contains the persisted quarantine work. Those changes are not deployed.
3. Rerun the relevant checks, then qualify the selected outcome end to end in an
   isolated environment. Keep the website pause until a relaunch is requested.

Validation entrypoints: `./scripts/strict_checks.sh`, `npm test --prefix docs-site`,
`npm run build --prefix docs-site`, and the companion relay/OCI-worker tests.
Historical test evidence is recorded in the scope document; hardware-gated tests
must not be counted as passing when skipped. Generated builds, test ledgers,
screenshots and raw logs remain ignored; do not discard them as part of Git cleanup.

### Saved checkpoints

Both repositories use the local branch `arma/paused-development-checkpoint`.
These commits were not pushed or deployed during cleanup. Keep the main repo
at this branch's tip to include the restart notes as well as the work below.

| Repository | Commit | Preserved work |
|---|---|---|
| `froglet` | `3d1012e` | Website redesign, illustrations and reviewed documentation |
| `froglet` | `b44a0ec` | Live maintenance source, rollback instructions and deployment defaults |
| `froglet` | `c1f057c` | Unreleased F1 file sharing and C1 safety foundations |
| `froglet-services` | `ca1c4d1` | Bounded relay file transport |
| `froglet-services` | `62d2345` | Worker recovery/quarantine, including the relay commit |

Cleanup verification, 28 September 2026:

- Main strict matrix passed: 936 Rust tests (8 ignored), Clippy, native/wasm
  verifier builds, frozen Kernel fixture check, secret scan, all 185 Node
  integration tests, 145 Python runtime/setup tests, 221 Python verifier tests,
  and 90 conformance checks. Log: `_tmp/repo-cleanup/strict-checks.log`.
- Site: 199 tests and the 37-page build passed. Default deployment dry run
  bundled only the maintenance worker. Logs: `_tmp/repo-cleanup/site-*.log`.
- Companion relay/OCI worker: 34 tests passed, actual-container test ignored;
  formatting, Clippy and scans of the changed service directories passed.
  Logs: `/private/tmp/froglet-cleanup-{companion,worker,relay}-*.log`.

This is source/test checkpointing, not new hardware or production qualification.
The existing rendered-site and local transfer evidence remains in the dated
review and scope documents. No source, test ledger, runtime data or evidence
was deleted; generated files and local caches stay outside Git.

## Order 44 - Batch Fan-Out

Status: not implemented; boundary is enforced in agent-facing truth strings
(`integrations/shared/froglet-lib/tool-dispatch.js` `plan_use_case` batch
profile, `docs/HOSTED_TRIAL.md`, `docs/llms/try.froglet.dev.txt`).

Multi-item batch fan-out, retries, and paid async settlement across the
runtime deal flow. Until this lands, batch workloads use the existing async
task-status primitives one task at a time, and agent surfaces must not claim
batch support.

## Order 45 - GPU Scheduling and Routing

Status: disabled in the current isolated-worker path; not release-qualified.

Earlier single-node Docker/T4 smoke evidence belongs to the previous execution
path. It does not qualify the current worker for GPU rental or isolation.
The current node refuses GPU execution and the worker reports it unsupported. Follow the staged C1/G1 gates
in [the file and compute scope](docs/FILE_AND_COMPUTE_SCOPE.md), including actual
device allocation, finite occupancy, cancellation, cleanup and safe reuse.

## Order 70 - Strict Egress Pin Propagation

Status: implemented with regression coverage in
`integrations/shared/froglet-lib/test/egress-mode.test.mjs`.

Track strict egress behavior through the shared Froglet JavaScript client:
`frogletRequest`, `frogletRequestWithStatus`, and `frogletPublicRequest` must
resolve operator pins when `FROGLET_EGRESS_MODE=strict`, cache the resolved pin,
and honor caller-supplied pins over cached lookups.

Remaining work: keep this coverage aligned as additional agent integrations
delegate requests through `integrations/shared/froglet-lib/`.

## Order 81 - Identity Attestation Issuing Service

Status: protocol payload and validator exist in
`froglet-protocol/src/protocol/identity_attestation.rs`; service issuance
flows remain to be built.

Implement the marketplace attestation service that issues DNS and OAuth/OIDC
identity attestations:

- DNS flow: create a challenge, verify the DNS evidence, issue the signed
  `identity_attestation/v1` artifact, and schedule re-verification.
- OAuth/OIDC flow: verify provider identity evidence, bind it to the Froglet
  subject key, issue the signed artifact, and schedule re-verification.
- Revocation and expiry handling: consumers must reject expired attestations;
  failed re-verification should invalidate the attestation before expiry.

Out of scope for this order: W3C Verifiable Credentials and proof-of-personhood.

## Arbiter Mechanism Design

Status: stubbed in `docs/ARBITER.md`.

The arbiter design still needs data-backed economic parameters before it should
be promoted from stub to implementation spec. Open parameters include deposit
tiers, stake floors, fee split, appeal multiplier, adjudicator throughput, and
grief-filing cost assumptions.

Implementation should update `docs/ARBITER.md` with concrete values and tests
once those parameters are chosen.
