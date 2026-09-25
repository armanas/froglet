# Agent-first publication and cloud-portable operations

Status: implementation and evidence audit; the default public path is **not
yet production-proven**.

Last audited: 2026-07-11

2026-09-24 follow-up: the native preparation, selected snapshots, configuration
merge, remote invocation, sharing, diagnosis, source checks, and local read-only
status candidate is implemented. The original audit below is historical evidence;
its network observations are not claims about today's deployment. Current scope,
remaining platform/external/human gates, and release sequencing are recorded in
[RELEASE.md](RELEASE.md#effortless-publishing-qualification). Managed hosting in
the historical roadmap is outside the current effortless-publishing release.

Scope: `froglet` and the hosted market stack in `froglet-services`.

This document is the execution record for the two-approval publication goal.
It is intentionally stricter than a roadmap: every requirement is paired with
the evidence that proves it, and local implementation is kept separate from a
live external proof. It does not change [`KERNEL.md`](KERNEL.md). Publication,
reachability, deployment, identity custody, marketplace admission, and
commerce remain application or operator layers above the Kernel.

## Completion truth

The implemented local lanes and their whole-repository gates now pass, but the
north star is not externally production-proven. In particular:

- `hosting.managed` now connects publication to the sibling operator through
  shared versioned contracts, deterministic private OCI packaging, paired
  Provider/Nostr identity transfer, durable compare-and-swap orchestration,
  status-based crash reconciliation, pre-approved compensation, exact remote
  canary, and normalized registration evidence; a credentialed disposable-host
  proof is still required;
- the current worktree has not been cut as an immutable public release and run
  through the no-clone Ubuntu/macOS matrix;
- `relay.froglet.dev` did not resolve during the 2026-07-11 external check, so
  a clean host cannot currently complete the default relay path;
- the AWS Lightsail and generic SSH + OCI adapters have local contract proofs,
  but still require separately authorized disposable live canaries; and
- paid publication remains outside the default path and cannot be called live
  until rail-side staging evidence satisfies Phase 5.

The public marketplace health endpoint was reachable during the same check.
That proves only the marketplace service was up, not that this unreleased
worktree, relay ingress, or an exact Publication Revision was live.

## North star and approvals

On a clean supported macOS or Ubuntu host, a user should be able to ask a
supported agent to publish data or a function and approve only:

1. **Installation**: the exact immutable-release-digest-verified bundle and
   bootstrap, installed binary, separately verifiable provenance attestations,
   persistent state path, and process-manager change.
2. **Publication**: the exact consent hash and Publication Revision, package
   and schema digests, exposed capabilities, access policy, price and rail,
   endpoint, relay trust disclosure, and expected recurring cost.

The second call must rebuild the package once, reject a changed consent hash,
verify locally before exposure, activate only the exact revision, and return:

- the public HTTPS URL;
- the exact marketplace offer URL when listing was requested;
- provider identity plus immutable revision and offer hashes;
- local, marketplace-owned, and requester-side canary evidence as distinct
  claims; and
- native agent actions for status, bounded logs, pause, resume, rollback, and
  confirmed unpublish.

Free relay publication on the user's existing machine remains the default.
Managed Deployment is optional independent uptime, not a prerequisite for
publication.

## Architecture decisions now encoded

| Concern | Contract decision |
|---|---|
| Kernel | No Publication, relay, deployment, DNS, release, or custody field enters canonical Kernel bytes or state transitions. |
| Publication | `PublicationIntent` is the single request shape used by manifests, CLI, native MCP, JavaScript compatibility adapters, the publish engine, and provider control. |
| Approval | `froglet.publication-consent.v2` binds the exact canonical provider request, package/build digests, schemas, capabilities, fixture-input hash, daemon identity for every public hosting adapter, transport endpoint when knowable before preparation, and commerce disclosure. |
| Revisions | Verified publications store immutable signed revisions and mutate only a selected/active pointer. |
| Reachability | Relay, self-hosted HTTPS, Tor, and local-only hosting are adapters; relay is the public default only when the daemon reports an exact `up` identity-derived HTTPS URL. |
| Installation | Native checksum-verified binary plus launchd/systemd is preferred. A digest-pinned dual-role OCI image is the fallback, not a hidden host dependency. |
| Agent surface | `froglet-node mcp` is the no-Node.js clean-host bridge. The JavaScript MCP/plugin remains a wider compatibility surface, not a second publication-policy implementation. |
| Data | Native JSON, CSV, and SQLite publication uses explicit schemas and read-only builtins. CSV package identity binds bytes and schema. |
| Python | Python publication uses a resolver-free, hash-pinned pure-wheel bundle and exact CPython compatibility; raw inline source is compatibility-only. |
| OCI execution | The Node calls an authenticated provider-neutral worker. Only the dedicated worker talks to a rootless Docker/Podman engine. |
| Managed Deployment | Portable desired state contains OCI digests, resources, ports, health, ingress, volumes, and logical references. Native cloud IDs stay in adapters. |
| Managed Publication | A provider-neutral target registry selects an external operator adapter. Consent binds exact package, operator plans, endpoint, identity, and cost; the Node owns durable activation/reconciliation/compensation and registry-neutral image upload. |
| Release images | Build once in canonical GHCR, record the digest, and copy that exact manifest to ECR or another mirror. Never rebuild per registry. |
| DNS | Domain policy calls a DNS record seam. `manual.v1` and `cloudflare.v1` are adapters; Cloudflare is not marketplace architecture. |
| Identity | Local identities support encrypted backup, fail-closed restore, signed rotation continuity, and operator custody adapters without making a cloud account the identity authority. |
| Commerce | Free is the default. Direct provider rails, hosted credits, and third-party checkout/payout are separate product/legal relationships. |

AWS is therefore a supported adapter, not the product architecture. Removing
the Lightsail adapter must require no Kernel, manifest, publication,
marketplace admission, canary, or normalized lifecycle contract change.

## Canonical execution order

```text
read manifest
  -> build immutable package and build evidence
  -> preflight daemon identity and requested transport (read-only)
  -> return consent v2 (no publish, hosting, or registration mutation)
  -> user approves exact consent hash
  -> rebuild once and compare exact consent
  -> daemon local verification and signed revision persistence
  -> prepare or confirm the approved public endpoint
  -> submit pending-validation exact tuple
  -> marketplace-owned canary and atomic activation
  -> poll exact offer hash
  -> independent requester-side canary
  -> report healthy
```

Failure at any step must preserve the strongest truthful earlier state. A
local fixture does not prove reachability; reachability does not prove a
listing; a listing does not prove a requester invocation; none proves payment.

## Requirement-by-requirement evidence matrix

Status meanings:

- **verified locally**: focused current-worktree evidence exists; this does not
  substitute for the final full-suite or external gate.
- **implemented; external proof pending**: source and whole-repository local
  validation pass, but a separately authorized external gate remains.
- **external proof pending**: credentials, public infrastructure, a release
  tag, or a disposable host is required; unit/fake-runner tests cannot close it.
- **not complete**: a required implementation or external service is absent.

### Phase 0 — Truthful canonical publication and release inputs

| Requirement | Authoritative evidence | Status |
|---|---|---|
| One lossless publication request | `froglet-protocol/src/publication.rs`, `froglet-publish-engine`, provider API, CLI/native MCP, and JS compatibility tests exercise the same v1 shape. Current focused results: protocol 114/114, publish engine 55/55 plus warning-denied check/clippy, and combined Node integration 123/123. | verified locally, including the final whole-repository rerun |
| Exact consent binds build and disclosure | Publish-engine tests change a locked wheel and CSV schema, reject stale approval, and keep private bytes out of the consent summary. | verified locally |
| Build/local verify precedes exposure | `changed_package_rejects_stale_approval_before_daemon_publish` and `approved_publish_reuses_exact_build_and_does_not_prepare_hosting_on_local_failure` assert ordering and no hosting mutation on local failure. | verified locally |
| Exact offer, revision, and canary | Provider/API tests bind revision-to-offer, preserve unrelated active offers, and sign a fresh exact canary. Marketplace Postgres tests cover exact-candidate activation. | verified locally |
| Immutable release inputs | `release-manifest.json`, the immutable `agent-bootstrap.sh` release asset, their GitHub API asset digests, optional separate attestation checks, and binary digest validation are covered by installer/release tests. The workflow preflights repository immutable-release support before creating or editing a release. The final current-tree Darwin arm64 release binary was packaged, checksum-verified, installed from the generated local Release Bundle, started, and health-probed on 2026-07-11; its extracted binary digest exactly matched the built binary (`a94baa2a098dcc095ae5a13b0a54eb939e0897553a3a2a27231cf3af54b97121`), and its native MCP advertised all 13 expected actions. | verified locally; live read-only check on 2026-07-11 returned `enabled=false`, `enforced_by_owner=false`, so the first immutable tag is externally blocked |
| Build once, mirror by digest | `froglet-services/.github/workflows/release.yml`, `ops/mirror_oci_by_digest.sh`, and `froglet.hosted-oci-release.v1` record GHCR source and same-digest ECR mirrors. | implemented; live registry release pending |
| Vendor-free desired state | Managed Deployment types and fixtures contain no AWS region, ARN, Lightsail name, ECR repository, Cloudflare zone, or provider SDK type. | verified locally |
| Kernel compatibility | `docs/KERNEL.md` and `conformance/kernel_v1.json` are outside this change; Kernel-vector tests remain a required final gate. | full repository and Kernel-vector gates passed; both canonical files remain unchanged |

### Phase 1 — Native clean-host install and local proof

| Requirement | Authoritative evidence | Status |
|---|---|---|
| Exact install approval before execution | JavaScript `plan_install` remains an optional compatibility path. The zero-dependency default resolves an immutable GitHub release and verifies the uploaded `agent-bootstrap.sh` API digest before executing it. `agent-bootstrap.sh plan` writes only temporary files, verifies its own manifest-bound bytes plus the manifest and target-platform binary assets, and binds all material profile, path, manager, and command values into `install_approval_hash`. `execute` re-resolves and exact-matches before the first persistent write. | verified locally; live v0.4.0 fails closed because its required release assets are absent and the release is mutable; repository immutable releases reported `enabled=false`, `enforced_by_owner=false` on 2026-07-11, so the first immutable release proof is externally blocked |
| Checksum-verified no-clone native install | `scripts/install.sh`, `scripts/agent-bootstrap.sh`, and `scripts/froglet-service.sh` install the exact approved versioned asset and configure launchd/systemd without Node.js, Python, Docker, or `jq` on the native lane. Caller-supplied manifest/script mirrors require the explicit `FROGLET_TRUSTED_MANIFEST_PIN=1` fixture/trust path; there is no silent pin bypass. GitHub artifact attestations are published and separately verifiable, not silently claimed by the dependency-free digest path. | verified locally; released-host proof pending |
| Health and local proof before success | Native lifecycle waits for both health endpoints and performs an MCP status proof without seeding user services. Before the transient read-only fixture, bootstrap captures the complete active Offer-ID catalog and requires HTTP-404 lifecycle absence for the exact proof ID, then records a private durable cleanup intent before publishing. Success requires invoke, exact confirmed-unpublish, restoration of that catalog, and removal of the authoring directory/marker. Exact Offer hashes intentionally rotate when Descriptor sequence changes, so they remain lifecycle/lease evidence rather than a false cleanup-equality invariant. Failure compensates while the service is live; an unproved compensation preserves recovery markers and remains a failed install. | verified locally, including built-in catalog preservation, collision, and post-publication/post-invocation failure injection; released-host proof pending |
| Native agent bridge | `froglet-node mcp` provides status, invoke, local proof, two-step publication, lifecycle actions, and durable managed-operation status/confirmed reconciliation/confirmed compensation over stdio. | verified locally |
| Read-only dependency-free data | Native JSON/CSV/SQLite builders and `data_query` service execute locally; CSV requires an explicit typed/indexed schema. | verified locally |
| Restart, upgrade, rollback, uninstall | The service helper uses immutable release directories, current/previous pointers, health-gated rollback, and OS process supervision. | script tests passed; reboot and failed-upgrade host canaries pending |
| Identity custody warning/gate | Public consent carries the daemon-observed structured backup state and exact bundle digest, without disclosing local paths or custody identifiers. The provider recomputes that binding immediately before mutation, so replacement, corruption, identity rotation, or status drift requires a new approval. Private local proof records backup as not required. | verified locally |
| Ubuntu and macOS no-clone matrix | Current worktree must be released and exercised on clean current Ubuntu LTS and supported macOS, including reboot. | external proof pending |
| OCI fallback parity | Release Bundle pins the dual image and bootstrap can use it only when native service management is unavailable. | local fixtures pass; anonymous pull and live fallback proof pending |

### Phase 2 — Default relay publication and marketplace activation

| Requirement | Authoritative evidence | Status |
|---|---|---|
| Relay bootstrap is explicit and dormant | Bootstrap plans and persists `FROGLET_RELAY_URL=wss://relay.froglet.dev/v1/tunnel` plus `FROGLET_RELAY_PUBLIC_SUFFIX=relay.froglet.dev` by default. This derives an endpoint but opens no WSS without an exact durable grant; setting both empty opts out. WSS is required except literal loopback WS. | verified locally |
| Approval requires ready exact endpoint | Public planning reads node capabilities before consent. Relay requires enabled, a lifecycle status of `reserved`, `starting`, `up`, or `down`, provider identity, and a credential-free HTTPS root origin; only exact approval activates its durable grant. Tor requires enabled, provider identity, and an exact credential-free `http://<56-char-v3>.onion` origin; self-hosted requires a credential-free public HTTPS root origin. Prepared endpoints must equal the approved endpoint. | verified locally |
| Marketplace-owned exact canary | Exact `offer_hash` + signed revision + private hash-bound input enters `pending_validation`; only exact feed/artifact/canary validation activates it. Partial tuples fail closed. | verified with isolated and real-Postgres tests |
| Independent requester canary | The publish engine resolves public DNS, rejects private/ambiguous addresses, pins the resolved endpoint, invokes the exact revision, and verifies provider signature/challenge/result. | verified locally |
| Renewable listing health | Feed `active_offer_hashes` and indexer leases hide stale, disconnected, paused, or unpublished exact offers without inventing Kernel tombstones. | verified with real-Postgres cardinality/availability tests |
| Trust disclosure | Consent says relay terminates TLS, sees plaintext, and applies operator quotas. | verified locally |
| Clean-host NAT-to-public proof | `relay.froglet.dev` did not resolve on 2026-07-11. No current clean-host external invocation can close this gate. | not complete |
| Disconnect/reconnect continuity | Requires the live relay, wildcard DNS/TLS, marketplace lease expiry, and same-identity reconnect canary. | external proof pending |

### Phase 3 — Lifecycle and real deployment portability

| Requirement | Authoritative evidence | Status |
|---|---|---|
| Immutable revision lifecycle | SQLite stores immutable revisions and separate lifecycle pointers with `active`, `paused`, and `unpublished` states; rollback activates only a validated exact prior revision. | verified locally |
| Operator API surface | Provider control and native MCP expose publication status, bounded logs, pause, resume, exact rollback, confirmed unpublish, plus managed-operation status and exact-confirmed reconcile/compensate actions. OpenAPI documents the HTTP wire contract. | verified locally |
| Provider-neutral operator | `froglet-services/services/operator` owns `plan`, `provision`, `deploy`, `status`, `history`, `logs`, `rollback`, and confirmed `destroy` with normalized JSON. All-target tests passed 27/27, warning-denied check/clippy passed, and operator/ops helper tests passed 37/37. | verified locally |
| Mutation-free unsupported capability | Coordinator validates desired state against adapter capabilities before any adapter command is rendered or invoked. | verified with fake-runner contract tests |
| Lightsail compatibility adapter | Adapter wraps the existing deployment script/template and preserves port 3010 `/healthz` for the arbiter instead of pretending it is Froglet port 8080. | local template/command tests pass; live canary pending |
| Generic SSH + OCI adapter | Same portable subset renders a non-interactive SSH deployment with digest pins, resource limits, optional persistent volume, HTTPS health, history, and rollback. | local fake-runner tests pass; disposable-host canary pending |
| Publication-to-operator orchestration | `froglet.managed-publication.*.v1` binds the deterministic private OCI layout, exact target/profile/operator plans, paired identity, endpoint-specific signed Descriptor/Offer/Revision, canary, compensation, and registration. Desired state crosses the operator over bounded stdin; external phases are durably recorded and interrupted mutations require read-only status reconciliation. | verified locally with fake operator, fake registry, fresh same-identity target import, feed, signed canary, crash recovery, stale-CAS, and compensation tests; disposable-host proof pending |
| DNS seam | Domain claims use `manual.v1` or `cloudflare.v1`; credentials and record API details remain adapter-private. | verified locally; live record mutation pending |
| Same Release Bundle on both adapters | Both adapters must deploy the same recorded digest and pass the same external smoke. | external proof pending |
| Deleting AWS changes no product contract | Architectural dependency direction and vendor-free type tests support this claim; an actual deletion/compile experiment is still the hard proof. | verified by the SSH-only build and deletion/compile proof |

### Phase 4 — Useful data, WASM, locked Python, and isolated OCI

| Requirement | Authoritative evidence | Status |
|---|---|---|
| Indexed data lane | JSON/SQLite remain content-bound; CSV ingestion validates ordered types, nullability, bounded indexes, and query behavior before signing. | verified locally |
| Hermetic WASM source lane | Embedded WAT builder records its pinned compiler dependency and invokes no external toolchain. | verified locally |
| Reproducible locked Python | Canonical bundle includes source, exact CPython compatibility, lock, parser identity, and hash-pinned pure wheels; tests reproduce from local artifacts and reject native/traversal/symlink cases. | verified locally; empty-cache clean-host proof pending |
| Isolated OCI worker seam | Node sends a digest-only, bounded, capability-reduced authenticated HTTP request; the default client is disabled and no engine socket enters the Node. The extracted non-Kernel wire contract passed 3/3 and Node conversion/client tests passed 5/5. | verified locally |
| Worker hardening | Reference worker requires rootless Docker/Podman, `--pull=never`, read-only/non-root/cap-drop/no-new-privileges execution, bounded resources/output, private logical bindings, and default-deny network. Worker tests passed 15/15 and warning-denied check/clippy passed; dependency-tree inspection proves no full `froglet` or Wasmtime normal/build dependency. The one opt-in live test was correctly ignored. | verified locally; live rootless container proof pending |
| Externally listed examples | One expanded data, one WASM, one locked Python, and one OCI example must each carry provenance, local evidence, marketplace evidence, and requester evidence. | external proof pending |

### Phase 5 — Identity continuity and optional commerce

| Requirement | Authoritative evidence | Status |
|---|---|---|
| Encrypted backup and fail-closed restore | Custody tests and a CLI round trip restore both identities into a second directory with private modes; tampering/wrong keys/existing destinations fail closed. | verified locally; separate-host proof pending |
| Signed rotation continuity | Old Froglet and Nostr identities authorize the new identities; independent verification retains historical artifact validity. | verified locally |
| Pluggable custody | Encrypted file and operator-command custody exist. OS keychain is explicitly reported unsupported rather than silently claimed; vendor KMS/HSM deployments remain optional adapters. | partial; platform/KMS live proof pending |
| Free remains default | No payment account or rail is enabled by the default publication request. | verified locally |
| Paid approval disclosure | Consent binds amount, currency, direct-provider rail, fees, payout party, and refund-policy limits before publication. | verified locally; live rail evidence pending |
| Paid rail truth | Sandbox and live-staging must reconcile signed Froglet state, duplicate events, refunds, restart recovery, and rail state. Unit tests alone cannot close this row. | external proof pending |

## Managed Deployment and v3 Fly migration

New authoring uses `froglet-service/v4` and a neutral managed target/profile.
The v4 publication contract does not select AWS, GCP, Fly, Kubernetes, or a
registry. Operator configuration maps target/profile to an adapter.

`froglet-service/v3` remains readable. Its `hosting.fly` fields are a deprecated
compatibility input only: new authoring does not emit them, v4 rejects them,
and migration chooses `hosting.default = "managed"` with provider selection in
operator-owned adapter config. Adding another cloud must not add another
manifest enum variant.

## Dependency budget

| Category | Default-path rule |
|---|---|
| Required | Supported OS, outbound HTTPS/WSS, local disk, and an agent allowed to request shell approval |
| Bundled | Node/runtime/CLI, native MCP bridge, Wasmtime, release verifier, and launchd/systemd integration |
| Not required | Docker/Podman, Node.js, Python, `jq`, Tor, cloud CLI, DNS account, payment account, or inbound port |
| Optional workload | CPython for locked Python; an isolated rootless worker for arbitrary OCI |
| Optional hosted | Marketplace/relay convenience services and Managed Deployment adapters |

## Cross-repository ownership

| Concern | `froglet` | `froglet-services` |
|---|---|---|
| Kernel and signed evidence | authoritative | consumes and verifies |
| Publication/consent/revision | authoritative | admission consumes exact tuple |
| Packaging and Node execution | authoritative | runs released artifacts and worker service |
| Relay | public contract and node adapter | hosted relay and operator policy |
| Marketplace | client/registration contract | admission, projection, health, and abuse policy |
| Managed Deployment | neutral manifest/contract boundary | deep operator and all infrastructure adapters |
| Registry/DNS/secrets/observability | no provider requirement | separate adapter implementations and runbooks |
| Commerce | signed settlement stays authoritative | checkout, payout, compliance, and reconciliation above Kernel |

## Remaining completion gates

The objective can be called complete only after all of these are recorded
against one immutable release:

1. **Passed locally on 2026-07-11:** full `froglet` and `froglet-services`
   formatting, check, lint, unit, integration, JavaScript, Python, and
   real-Postgres matrices;
2. the Release Bundle is tagged, immutable, attested, anonymously fetchable,
   and its canonical/mirror digest map verifies live; the services release
   must pin the intended immutable public Froglet revision rather than its
   current older source revision;
3. no-clone current Ubuntu and macOS native installs prove local data
   invocation, reboot identity continuity, upgrade rollback, and uninstall;
4. the live relay has DNS/TLS/readiness and a behind-NAT clean host completes
   consent, exact activation, requester canary, disconnect expiry, and
   same-identity reconnect;
5. the same Release Bundle completes authorized disposable Lightsail and
   generic SSH + OCI lifecycle/canary runs; and
6. expanded workload, custody-on-second-host, and any paid claim satisfy their
   phase-specific external evidence cells.

Provisioning public DNS, deploying paid infrastructure, cutting a release, or
using cloud/payment credentials are external state changes. They are not
silently inferred from implementation work and require the operator's explicit
authorization.
