# Release

This repo now has a tagged release path for the public Froglet node, tagged
Docker images in GHCR, the MCP image, and the checked-in docs deployment
configuration.

Maintained by [Armanas Povilionis-Muradian](https://armanas.dev).

## Versioning

Use semver with explicit alpha prereleases for the current train, for example:

- `0.1.0-alpha.1`
- `0.1.0-alpha.2`

The Git tag must be prefixed with `v`, for example `v0.1.0-alpha.1`.

`Cargo.toml` and the Git tag must match exactly apart from that leading `v`.
The release workflow checks this and fails if they diverge.

## Published Images

Pushing a matching tag triggers
[../.github/workflows/release.yml](../.github/workflows/release.yml), which
publishes the role-specific images:

- `ghcr.io/armanas/froglet-provider:<version>`
- `ghcr.io/armanas/froglet-provider:<sha-tag>`
- `ghcr.io/armanas/froglet-runtime:<version>`
- `ghcr.io/armanas/froglet-runtime:<sha-tag>`
- `ghcr.io/armanas/froglet-mcp:<version>`
- `ghcr.io/armanas/froglet-mcp:<sha-tag>`
- `ghcr.io/armanas/froglet-mcp:latest`

If the repository remains private, the package visibility still has to be
changed to public in GitHub package settings before anonymous pulls work.

## Published Docs

`docs-site/` is configured for Cloudflare Workers via
[`docs-site/wrangler.jsonc`](../docs-site/wrangler.jsonc). Production deploys
run either through `npm --prefix docs-site run deploy` when Cloudflare
credentials are present locally, or through Cloudflare Workers Builds with:

- Build command: `npx astro build`
- Deploy command: `npx wrangler deploy`

The repo no longer uses GitHub Pages for docs deployment. The intended public
shape is the apex `https://froglet.dev`; `docs.froglet.dev` previously mirrored
the same deployment and is no longer advertised as a separate launch surface.
The public host should only be treated as live after the Cloudflare deployment
and route checks pass (see [SUBDOMAIN_PLAN.md](SUBDOMAIN_PLAN.md)).

## Published Binaries

The same tagged workflow also publishes GitHub release assets for:

- `froglet-node-<tag>-linux-x86_64.tar.gz`
- `froglet-node-<tag>-linux-arm64.tar.gz`
- `froglet-node-<tag>-darwin-arm64.tar.gz`
- `SHA256SUMS`

The one-line installer at [../scripts/install.sh](../scripts/install.sh)
downloads from those release assets. By default it installs the latest tagged
`froglet-node` release into `~/.local/bin`. Use `VERSION=<tag>` to pin a
release and `INSTALL_DIR=/path` to override the destination.

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
- First-party hosted trial smoke. `try.froglet.dev` must mint a session and
  complete the documented free `demo.add` flow with a receipt.
- Hosted upstream guard smoke. Direct public session/demo writes to
  `ai.froglet.dev` must remain outside contract and reject as documented in
  [HOSTED_TRIAL.md](HOSTED_TRIAL.md).
- Distribution smoke for every launch channel named in the release notes. See
  [DISTRIBUTION_MATRIX.md](DISTRIBUTION_MATRIX.md).

Hosted paid rails are not part of the v0.1.0 launch gate. Hosted Lightning,
Stripe, and x402 belong to v0.2 unless a later release plan explicitly changes
that scope.

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
hosted trial for one free end-to-end deal.

## What ships

- `froglet-node` binaries for Linux x86_64, Linux arm64, and macOS arm64
- `SHA256SUMS` for release asset verification
- GHCR images:
  - `ghcr.io/armanas/froglet-provider:0.1.0`
  - `ghcr.io/armanas/froglet-runtime:0.1.0`
  - `ghcr.io/armanas/froglet-mcp:0.1.0`
- Docker Compose starter configuration
- OpenClaw/NemoClaw plugin under `integrations/openclaw/froglet/`
- MCP server under `integrations/mcp/froglet/`
- public docs at `froglet.dev`
- public status snapshot at `froglet.dev/status/`
- free hosted trial at `try.froglet.dev`

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | sh
```

To pin this release:

```bash
VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | sh
```

## Verification

Release evidence:

- default release gate: `<link-to-release-gate-summary>`
- compose OpenClaw/MCP smoke: `<link-to-compose-smoke-evidence>`
- Claude MCP smoke: `<link-to-claude-smoke-evidence>`
- hosted trial smoke: `<link-to-hosted-trial-curl-transcript>`
- release workflow: `<link-to-github-actions-run>`
- checksums: `<link-to-SHA256SUMS>`

## Hosted trial scope

The hosted trial proves one free `demo.add` discover -> deal -> result ->
receipt flow through `try.froglet.dev`. It does not prove paid rails,
persistent identity, hosted account recovery, or general hosted runtime access.

## Payment rails

Local and self-hosted payment setup is documented for Lightning, Stripe, and
x402. Hosted paid rails are v0.2 scope:

- hosted Lightning: v0.2
- hosted Stripe: v0.2
- hosted x402: v0.2

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
- tagged provider, runtime, and MCP images in GHCR
- downloadable `froglet-node` binaries
- official site at `froglet.dev` if the docs deployment is live at cut time
- public OpenClaw integration
- reference discovery
- reference operator image
- local/self-hosted payment adapters: Lightning, Stripe, and x402
- hosted paid rails deferred to v0.2
- Claude MCP smoke evidence, because it is a hard launch blocker
- confidential/TEE scope as experimental unless a real backend is proven and
  documented
- any intentionally deferred layers, especially external broker and closed higher-layer
  services
