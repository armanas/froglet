# Release

This repo now has a tagged release path for the public Froglet node, tagged
Docker images in GHCR, the MCP image, and the checked-in docs deployment
configuration.

New tags cut by the current workflow also publish the non-Kernel
[`froglet.release-bundle.v1`](PUBLICATION_CONTRACT.md#release-bundle-v1)
manifest that binds source, binary checksums, and immutable role-image digests.

Maintained by [Armanas Povilionis-Muradian](https://armanas.dev).

## Versioning

An uncommitted stabilization worktree is a local candidate, not a published
release. Before cutting the next release, choose an unused version, commit
the public source, and verify the immutable Release Bundle for that exact
commit. In `froglet-services`, update `.github/froglet-source.json` to that
revision and regenerate its lockfile against the same checkout. Its CI and
release jobs share this pin and both require locked dependencies. Do not
reuse the already-published `v0.4.0` tag for new bundle contents.

## Effortless publishing qualification

The current working tree is a **private candidate, not a qualified release**.
Keep existing changes uncommitted during preparation. Deployment and the final
public source pin are deferred. The command `scripts/release_gate.sh` covers
software checks; a PASS with skipped platform or external work does not satisfy
the product release gate below.

Platform and participant qualification are pending at the user's request
(2026-09-24). Do not schedule sessions or claim this gate has passed.

The supported first-use journey is free, native publishing and consumption with
Codex and Claude Code. JSON, explicitly typed CSV, SQLite selections, and small
Wasm functions are in scope. Python remains advanced Linux functionality.
Windows, managed always-on hosting, arbitrary applications, broader agent
qualification, and advanced payments are outside this release.

### Candidate implementation and local evidence

- Configuration changes merge only Froglet, preserve TOML comments, back up
  changed files, compare approved fingerprints, and compensate only matching
  writes. Reconnection reuses the installed immutable release. Unknown occupied
  installation paths fail closed and retain their contents.
- Preparation writes a selected snapshot and runs an example. Private source
  selections, fingerprints, and a recoverable preparation journal are separate
  from signed artifacts. Source edits never republish automatically.
- Native remote invocation uses the existing requester runtime and endpoint
  validation with a zero-price ceiling. Uncertain calls retain their retry key;
  subsequent calls reconcile the runtime's durable intent.
- Publication output separates local proof, public reachability, marketplace
  activation, and requester execution. The read-only local status interface has
  its own loopback listener and short-lived browser credentials, outside relay
  routes. An initialized MCP call is distinguished from a shell probe; an ended
  observed process reports disconnection without claiming current attachment.
- Sharing pages are for the first-party relay lane. Advanced hosting keeps its
  own endpoint and does not receive a first-party relay link.

Local qualification includes four native catalog/function executions and receipt
verification, deduplicated invocations, selected-data exclusion, source change
rejection, update, pause/resume, exact rollback, source deletion, and restart
identity/service continuity. Installer tests use isolated service-manager and
release fixtures; they are not real clean-machine or reboot evidence. Website
fixture states are not real public relay availability. No Kernel signing or
hashing change is part of this work.

On 2026-09-25, private digest-pinned binary installation, health, repeated
installation, and identity-retention smokes passed on fresh Linux x86_64 and
arm64 VMs without a checkout or toolchain. All 11 Linux Python isolation tests
passed on an arm64 VM with Landlock ABI v3 support. A Debian 12 arm64 VM exposed
its older ABI v2; Python fails closed there with an explicit requirement for v3.
The final uncommitted macOS arm64 candidate passed packaging and a pinned local
installer smoke. From this Mac, a separate requester runtime made a free call
through the candidate HTTPS relay and verified receipt
`ac7169e32b2356fbfc9e7e99c1c08d6f035bbf3947e1c79932c21c0e4b6ab13a`.
These checks do not prove actual Codex/Claude Code connection on clean machines,
immutable public release installation, or the five-participant acceptance gate.

On 2026-09-25, `scripts/release_gate.sh --compose --package-assets
--install-smoke --version v0.4.1-rc.1 --platform darwin --arch arm64`
passed its secret scan, strict checks (including Compose), full docs build and
tests, asset verification, and installer smoke. Evidence is in
`_tmp/release_gate/20260925T085434Z/` locally; this ignored directory is not a
published attestation. A candidate-origin docs build also confirmed the
publishing prompt points to the guide in the same build. The currently deployed
candidate site predates that prompt correction and still links to the public
host, where the guide returns 404 until cutover.

An isolated macOS agent probe on 2026-09-25 did not satisfy the agent-connection
gate: Claude Code's CLI required login, and Codex discovered the native MCP
tool but its non-interactive approval policy blocked execution. Do not count a
discovered tool, a CLI health check, or an attempted call as an agent execution.

### Required platform and external evidence

| Target | Codex on a clean machine | Claude Code on a clean machine |
|---|---|---|
| macOS Apple Silicon | Pending | Pending |
| Linux x86_64 | Pending | Pending |
| Linux arm64 | Pending | Pending |

For every cell, record OS and agent versions, independently trusted candidate
digest, actual installation approval, merged configuration diff, native service
health, an actual agent tool call, and verified catalog execution. No source
checkout, developer toolchain, or npm dependency is allowed. Exercise existing
unrelated settings, repeated install, interrupted setup, configuration drift,
occupied ports, reboot, failed upgrade, rollback, and uninstall with retained
identity and services. Refusal of an unknown partial installation is safe but
does not by itself qualify automated recovery after a hard crash.

On two independent machines, exercise trusted HTTPS, the real relay and
marketplace, exact publication approval, admission, the recipient's signed-offer
checks and free call, and receipt verification. Then test source update, changed
schema, deletion, pause/resume, rollback, unpublish, network loss, sleep/wake,
and reconnect. Confirm that stale approval, unexpected paid offers, and repeated
uncertain operations cannot produce misleading success or duplicate execution.

### Five-participant acceptance session

Recruit five first-time users with small catalogs and a separate-machine
recipient. Access to participants and test hosts remains a scheduling dependency;
no invitations have been sent by this implementation pass. Provide this task:

> Make this catalog usable by another agent. Share only the fields the recipient
> needs, inspect a local result, approve publication, and give the link to the
> recipient's agent. Then explain what is public and what happens when your
> computer sleeps.

Start timing at the first publishing prompt. Stop only after the recipient's
independent call and receipt verification. Count installation and publication
approvals within the 15 minutes. The facilitator may observe but must not repair
configuration, run commands, explain hidden steps, or supply missing manifests.
Record participant ID, platform/agent, source format, elapsed time, interventions,
result/receipt evidence, privacy explanation, sleep explanation, and every blocker.
Pass only if at least four of five finish within 15 minutes without developer
intervention and explain both consequences correctly. Fix blockers and repeat
with fresh participants when needed; views or impressions do not count as trials.

UI qualification must also include desktop/mobile rendering, keyboard operation,
screen-reader announcements, contrast, zoom, stale/offline states, copying when
clipboard permission is absent, and actual downloaded file contents. DOM roles
and screenshots alone are not a complete screen-reader qualification.

### Release sequence (deferred)

After candidate qualification: choose an unused release version; finalize the
public source commit; update the services source pin and lockfile to that exact
revision; run locked public/services checks and audits; verify repository
immutable-release configuration; produce and verify the bundle; deploy approved
relay, marketplace, and site configurations; and repeat the external journey.
Final verification must install through the actual immutable public release path,
not only candidate fixture URLs. Keep the stable claim closed until those checks
and the human gate pass. Deployment credentials remain a release dependency.

The 2026-09-24 services candidate audit reported `RUSTSEC-2026-0097` for
transitive `rand 0.10.0` and a yanked `chacha20 0.10.0`. On 2026-09-25, the
uncommitted services lockfile was provisionally updated to `rand 0.10.3` and
`chacha20 0.10.2` alongside three missing packages. `cargo audit`, locked
services tests against disposable Postgres, and locked Clippy then passed.
The services source pin still names the previous public revision; finalize it
against the release commit, regenerate the lockfile, and rerun those checks
before publication.

The supported stabilization scope is the signed agreement/receipt chain,
offline verification (including browser WASM), local node and agent flows,
and the marketplace's derived catalog. Payment adapters remain subject to
the evidence limits in [PAYMENT_MATRIX.md](PAYMENT_MATRIX.md). Local mocks
and database tests do not establish live payment, relay, cloud lifecycle,
or Linux container isolation behavior.

Broker/enterprise purchasing, batch fan-out, GPU scheduling, richer trust
ranking, identity-attestation issuance, decentralized arbitration, TEE,
and selective disclosure are outside this candidate's supported scope.
They must not be advertised as completed features. A signed receipt proves
the signer's statement and artifact linkage, not independent execution
quality or external settlement finality.

Use semver with explicit alpha prereleases for the current train, for example:

- `0.1.0-alpha.1`
- `0.1.0-alpha.2`

The Git tag must be prefixed with `v`, for example `v0.1.0-alpha.1`.

`Cargo.toml` and the Git tag must match exactly apart from that leading `v`.
The release workflow checks this and fails if they diverge.

Repository or organization **immutable releases must be enabled before a tag
is pushed**. Before creating or editing any release, the workflow performs a
read-only `GET /repos/{owner}/{repo}/immutable-releases` preflight using API
version `2026-03-10` and requires `enabled=true`; it never enables the setting
itself. It then creates a draft, uploads every binary, checksum, manifest, and
attestation asset, and publishes only after the bundle is complete. It also
requires the published release to report `immutable: true`; immutable releases
are intentionally not rebuildable or overwritable.

Historical external blocker (read-only check on 2026-07-11): the official
repository reported `{"enabled":false,"enforced_by_owner":false}`. Recheck
this setting at release time; that historical observation does not establish
its current value. A new release must fail before release mutation unless
immutable releases are enabled. No release was created or published by that
check. See GitHub's
[immutable release model](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
and [release API](https://docs.github.com/en/rest/releases/releases).

## Published Images

Pushing a matching tag triggers
[../.github/workflows/release.yml](../.github/workflows/release.yml), which
publishes the role-specific images:

- `ghcr.io/armanas/froglet-provider:<version>`
- `ghcr.io/armanas/froglet-provider:<sha-tag>`
- `ghcr.io/armanas/froglet-runtime:<version>`
- `ghcr.io/armanas/froglet-runtime:<sha-tag>`
- `ghcr.io/armanas/froglet-dual:<version>`
- `ghcr.io/armanas/froglet-dual:<sha-tag>`
- `ghcr.io/armanas/froglet-mcp:<version>`
- `ghcr.io/armanas/froglet-mcp:<sha-tag>`

The tags are discovery aliases. Agents and bootstrap scripts consume the
matching `@sha256:` references from the verified Release Bundle rather than
persisting mutable tags.

If the repository remains private, the package visibility still has to be
changed to public in GitHub package settings before anonymous pulls work.

## Published Docs

`docs-site/` is configured for Cloudflare Workers via
[`docs-site/wrangler.jsonc`](../docs-site/wrangler.jsonc). Production deploys
run either through `npm --prefix docs-site run deploy` when Cloudflare
credentials are present locally, or through Cloudflare Workers Builds with:

- Build command: `npm run build`
- Deploy command: `npx wrangler deploy`

The build requires Rust with the `wasm32-unknown-unknown` target. It compiles
the offline verifier and uses the exact `wasm-bindgen` CLI version from
`Cargo.lock` before building Astro. Running `astro build` alone is not a
complete website build. Use `npm ci` and `npm run build`; see
[`docs-site/README.md`](../docs-site/README.md) for local Worker verification.

The repo no longer uses GitHub Pages for docs deployment. The intended public
shape is the apex `https://froglet.dev`; `docs.froglet.dev` previously mirrored
the same deployment and is no longer advertised as a separate launch surface.
The public host should only be treated as live after the Cloudflare deployment
and direct route checks for `/`, `/learn/quickstart/`, and `/learn/cloud-trial/`
pass.

## Published Binaries

The same tagged workflow also publishes GitHub release assets for:

- `froglet-node-<tag>-linux-x86_64.tar.gz`
- `froglet-node-<tag>-linux-arm64.tar.gz`
- `froglet-node-<tag>-darwin-arm64.tar.gz`
- `agent-bootstrap.sh`
- `SHA256SUMS`
- `release-manifest.json` (`froglet.release-bundle.v1`)
- `release-manifest.intoto.jsonl` (offline GitHub artifact attestation bundle)
- `agent-bootstrap.intoto.jsonl` (separate offline bootstrap attestation bundle)

The one-line installer at [../scripts/install.sh](../scripts/install.sh)
downloads from those release assets. By default it installs the latest tagged
`froglet-node` release into `~/.local/bin`. Use `VERSION=<tag>` to pin a
release and `INSTALL_DIR=/path` to override the destination.

Before trusting manifest values, the dependency-free installer queries the
official GitHub Releases API for the requested tag, requires
`immutable: true`, requires exactly one manifest and bootstrap asset, and
verifies both downloads against their API `sha256:` digests. It
then verifies the platform archive against the digest in the trusted manifest.
If GitHub CLI is already installed, the installer additionally verifies the
offline artifact attestation against the expected repository, release
workflow, tag ref, and constrained source revision. A caller may instead
provide an independently trusted `FROGLET_RELEASE_MANIFEST_SHA256` pin; there
is no unverified bypass. Set `FROGLET_GH_ATTESTATION_MODE=required` when `gh`
provenance verification must be a hard gate, or `off` for a deliberately
dependency-minimal host that still enforces immutable-release digest trust. See
[`scripts/release_manifest.py`](../scripts/release_manifest.py) for the exact
closed v1 field set and validation rules.

The public first-hop block resolves immutable release metadata and verifies the
uploaded bootstrap API digest before any bootstrap bytes execute. The bootstrap
then provides a native two-call contract. `plan` writes only temporary files,
requires its own bytes to match both the manifest and GitHub asset digest, and
verifies the unique release-manifest and exact target-platform binary digests,
and binds the current bootstrap plus the source-revision install, lifecycle,
agent-setup, and payment-setup script bytes. It returns a canonical approval
hash covering the profile, persistent paths, service-manager impact, and exact
execution command. `execute` recomputes the same contract before creating a
persistent directory or changing a service and installs the exact approved
binary only when the hash matches. This removes `gh`, Python, Node.js, `jq`,
Docker, and cloud CLIs from the native lane. HTTPS/GitHub availability remains
an external dependency, but mutable branch bytes are no longer an executable
first hop.

The optional JavaScript MCP/OpenClaw compatibility surface provides a stronger
pre-execution contract for agents. `plan_install` reads GitHub release metadata
and the tag-specific bootstrap in memory, requires a published immutable
release, and returns an approval hash that binds the release tag, manifest and
bootstrap SHA-256 values, install profile, persistent paths, process-manager
impact, and exact command. `get_install_guide` returns executable commands only
when that same contract is recomputed from the exact `release_tag` and matching
`install_approval_hash`; its default command verifies the temporary bootstrap
file before passing `VERSION` and `FROGLET_RELEASE_MANIFEST_SHA256` to it.

The public release surface covered directly by the tag workflow is the tracked
protocol docs in this repo, reference node binaries, tagged container images,
supported integrations, and validation assets. The public docs host and the
first-party hosted node are separate deploy steps outside the tag workflow.

## Release Candidate Gate

This is the current release gate for the public Froglet repo. It has one
entrypoint, [`scripts/release_gate.sh`](../scripts/release_gate.sh), which
runs every line item in sequence, writes per-step evidence logs into
`_tmp/release_gate/<UTC-timestamp>/`, and prints a pass/fail summary at the
end. The same script is used both locally and in CI; a candidate is PASS when
no step is FAIL.

### Running the gate

```bash
# Minimum gate (covered end-to-end from this repo, no external deps):
./scripts/release_gate.sh

# Full local gate, including the compose-backed OpenClaw+MCP smoke:
./scripts/release_gate.sh --compose

# Cross-target package verification only (example: Linux x86_64):
./scripts/release_gate.sh \
  --package-assets \
  --version v0.1.0-alpha.1 \
  --platform linux \
  --arch x86_64

# Host-compatible package + installer smoke (example: Apple Silicon macOS):
./scripts/release_gate.sh \
  --install-smoke \
  --version v0.1.0-alpha.1 \
  --platform darwin \
  --arch arm64

```

Every step writes to `_tmp/release_gate/<ts>/<step>.log`, and the summary is
also dumped to `_tmp/release_gate/<ts>/summary.tsv` for CI ingestion.

First-party hosted smoke for `ai.froglet.dev` is intentionally outside this
scripted public-repo gate and is maintained separately from the public repo
checks. Launch still requires separate hosted evidence in the manual gates
below.

### Gate steps

| Step id | Status today | Validation | Underlying command | Notes |
| --- | --- | --- | --- | --- |
| `secrets` | Ready | Publication secret scan | `./scripts/gitleaks_gate.sh` | Current tracked tree + GitHub-visible history (`origin/main` + current public alpha tags). |
| `strict` | Ready | Repo validation matrix | `./scripts/strict_checks.sh` | Rust, Python, OpenClaw, MCP, release helper syntax. Also gates compose/LND/Tor integrations via env flags set by the gate. |
| `docs-build` | Ready | Docs build | `npm --prefix docs-site run build` | Pre-publish docs-site build |
| `docs-test` | Ready | Docs-site unit tests | `npm --prefix docs-site test` | Vitest suite under `docs-site/src/**/__tests__/` |
| `package` | Ready (opt-in) | Release asset packaging + verification | `scripts/package_release_assets.sh` + `scripts/verify_release_assets.sh` | Requires `--version`, `--platform`, `--arch` |
| `install-smoke` | Ready (opt-in) | Installer-path smoke from packaged assets | `scripts/smoke_install_from_assets.sh` | Implies `--package-assets`; packaged target must match the current host |

### Hard launch gates outside the script

The script above is necessary, but it is not sufficient for a public launch.
These checks must be green before a `v0.1.0` launch claim:

- Live Claude MCP smoke. Claude Code or Claude Desktop must load the generated
  Froglet MCP config and complete the expected tool smoke. This is a hard
  blocker, not a nice-to-have.
- First-party hosted trial smoke. `try.froglet.dev` must mint a session,
  expose the documented free demo catalog, and complete the canonical
  `demo.add` flow with a receipt.
- Hosted upstream guard smoke. Direct public session/demo writes to
  `ai.froglet.dev` must remain outside contract and reject as documented in
  [HOSTED_TRIAL.md](HOSTED_TRIAL.md).
- Distribution smoke for every launch channel named in the release notes.
  Record direct evidence links in the release PR or release notes.

The current MVP launch gate requires one live crypto rail and one live fiat
rail. Lightning and Stripe are the selected blockers; x402 is desirable but
must not block launch if its hosted proof is not ready. Do not claim any
hosted paid rail until the public transcript for that rail exists.

Confidential/TEE execution must remain framed as experimental for v0.1.0
unless a real attestation backend is proven and documented. The current launch
copy must not imply production TEE guarantees from a mock or limited backend.

### Cut steps

1. Update `Cargo.toml` package version.
2. Move the relevant `Unreleased` notes in [../CHANGELOG.md](../CHANGELOG.md)
   into a concrete version section.
3. Run the release gate with the release-cut flags. If you include
   `--install-smoke`, use the current host target so the packaged binary can
   execute locally:
   ```bash
   ./scripts/release_gate.sh \
     --install-smoke \
     --version v0.1.0-alpha.1 \
     --platform darwin \
     --arch arm64
   ```
4. Run the first-party hosted smoke separately when the hosted stack is part
   of the cut.
5. Commit the version/changelog update (attach the gate evidence directory path
   in the PR description).
6. Push the release tag, for example:

```bash
git tag v0.1.0-alpha.1
git push origin v0.1.0-alpha.1
```

## GitHub Release Body Draft

Use this as the release body for `v0.1.0` after replacing evidence
placeholders with links to the final release gate, workflow, and hosted smoke
results.

````md
# Froglet v0.1.0

Froglet v0.1.0 is the first public release of the reference Froglet node and
bot-facing integration surface. It ships the signed kernel artifacts, the
`froglet-node` binary, container images, local agent setup, and a constrained
free hosted demo catalog with one canonical end-to-end proof.

## What ships

- `froglet-node` binaries for Linux x86_64, Linux arm64, and macOS arm64
- `SHA256SUMS` for release asset verification
- GHCR images:
  - `ghcr.io/armanas/froglet-provider:0.1.0`
  - `ghcr.io/armanas/froglet-runtime:0.1.0`
  - `ghcr.io/armanas/froglet-dual:0.1.0`
  - `ghcr.io/armanas/froglet-mcp:0.1.0`
- Docker Compose starter configuration
- OpenClaw/NemoClaw plugin under `integrations/openclaw/froglet/`
- MCP server under `integrations/mcp/froglet/`
- public docs at `froglet.dev`
- free hosted trial at `try.froglet.dev`

## Install

Use the transparent dependency-free shell block in the public
[Quickstart](../docs-site/src/content/docs/learn/quickstart.mdx). It resolves
the immutable release metadata, verifies the uploaded `agent-bootstrap.sh`
asset against GitHub's API digest before execution, then runs the separate
`plan`/approved-`execute` contract. For a pinned release, set `tag` to the
desired normalized tag before the metadata request instead of resolving
`/releases/latest`; do not replace the release-asset URL with a mutable raw
branch URL.

## Verification

Release evidence:

- default release gate: `<link-to-release-gate-summary>`
- compose OpenClaw/MCP smoke: `<link-to-compose-smoke-evidence>`
- Claude MCP smoke: `<link-to-claude-smoke-evidence>`
- hosted trial smoke: `<link-to-hosted-trial-curl-transcript>`
- release workflow: `<link-to-github-actions-run>`
- checksums: `<link-to-SHA256SUMS>`

## Hosted trial scope

The hosted trial is free-only. `demo.add` is the canonical discover -> deal ->
result -> receipt proof through `try.froglet.dev`; witness/hash/notarize demos
can provide stronger evidence for URLs or content hashes. It does not prove
paid rails, persistent identity, hosted account recovery, service publication,
marketplace depth, or general hosted runtime access.

## Payment rails

Local and self-hosted payment setup is documented for Lightning, Stripe, and
x402. The MVP public paid-service claim requires hosted evidence for:

- hosted Lightning: launch blocker
- hosted Stripe: launch blocker
- hosted x402: desirable, not launch-blocking unless verified before launch

## Confidential and TEE scope

Confidential routes and artifacts are experimental in v0.1.0. The launch does
not claim production TEE guarantees unless a real backend has been separately
proven and documented; mock or limited attestation remains explicitly limited.

## Known limits

- no hosted paid settlement
- no persistent hosted user identity
- no PyPI, npm registry, Homebrew, or OS package-manager distribution
- marketplace and hosted-provider claims depend on the linked live smoke
  evidence above
````

## Release Notes Template

Use the matching changelog section as the release body. For the first alpha,
the release notes should call out:

- published `SHA256SUMS` for release asset verification
- tagged provider, runtime, dual-role fallback, and MCP images in GHCR
- downloadable `froglet-node` binaries
- official site at `froglet.dev` if the docs deployment is live at cut time
- public OpenClaw integration
- reference discovery
- reference operator image
- local/self-hosted payment adapters: Lightning, Stripe, and x402
- hosted Lightning and Stripe claims only if live transcripts exist; hosted
  x402 remains desirable but non-blocking
- Claude MCP smoke evidence, because it is a hard launch blocker
- confidential/TEE scope as experimental unless a real backend is proven and
  documented
- any intentionally deferred layers, especially external broker and closed higher-layer
  services
