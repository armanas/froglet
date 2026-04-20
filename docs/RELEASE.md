# Release

This repo now has a tagged release path for the public Froglet node, public
Docker images, MCP image, and public docs site.

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

- `ghcr.io/<owner>/froglet-provider:<version>`
- `ghcr.io/<owner>/froglet-provider:<sha-tag>`
- `ghcr.io/<owner>/froglet-runtime:<version>`
- `ghcr.io/<owner>/froglet-runtime:<sha-tag>`
- `ghcr.io/<owner>/froglet-mcp:<version>`
- `ghcr.io/<owner>/froglet-mcp:<sha-tag>`
- `ghcr.io/<owner>/froglet-mcp:latest`

## Published Docs

The docs site deploy workflow builds `docs-site/` and publishes it to
apex `https://froglet.dev` with `https://docs.froglet.dev` mirroring the
same deployment (see [SUBDOMAIN_PLAN.md](SUBDOMAIN_PLAN.md)).

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

The public release surface covers the tracked Froglet protocol docs, the public
docs site, reference node binaries, supported integrations, and validation
assets in this repo.

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

# Release-cut gate, including packaged-asset install smoke:
./scripts/release_gate.sh \
  --compose \
  --install-smoke \
  --version v0.1.0-alpha.1 \
  --platform linux \
  --arch x86_64

```

Every step writes to `_tmp/release_gate/<ts>/<step>.log`, and the summary is
also dumped to `_tmp/release_gate/<ts>/summary.tsv` for CI ingestion.

First-party hosted smoke for `ai.froglet.dev` now lives in the private
services/operator workspace and is intentionally outside this public gate.

### Gate steps

| Step id | Status today | Validation | Underlying command | Notes |
| --- | --- | --- | --- | --- |
| `strict` | Ready | Repo validation matrix | `./scripts/strict_checks.sh` | Rust, Python, OpenClaw, MCP, release helper syntax. Also gates compose/LND/Tor integrations via env flags set by the gate. |
| `docs-build` | Ready | Docs build | `npm --prefix docs-site run build` | Pre-publish docs-site build |
| `docs-test` | Ready | Docs-site unit tests | `npm --prefix docs-site test` | Vitest suite under `docs-site/src/**/__tests__/` |
| `package` | Ready (opt-in) | Release asset packaging + verification | `scripts/package_release_assets.sh` + `scripts/verify_release_assets.sh` | Requires `--version`, `--platform`, `--arch` |
| `install-smoke` | Ready (opt-in) | Installer-path smoke from packaged assets | `scripts/smoke_install_from_assets.sh` | Implies `--package-assets` |

### Still outside the gate

- Live MCP smoke with Claude auth. Blocked on hosted stack plus valid Claude
  auth. The launch fallback date remains 2026-05-15.
- First-party hosted smoke for `froglet.dev` and `ai.froglet.dev`. That
  operator-specific check now runs from the private services/operator
  workspace.

### Cut steps

1. Update `Cargo.toml` package version.
2. Move the relevant `Unreleased` notes in [../CHANGELOG.md](../CHANGELOG.md)
   into a concrete version section.
3. Run the release gate with the release-cut flags:
   ```bash
   ./scripts/release_gate.sh \
     --compose \
     --install-smoke \
     --version v0.1.0-alpha.1 \
     --platform linux \
     --arch x86_64
   ```
4. Run the first-party hosted smoke separately from the private
   services/operator workspace when the hosted stack is part of the cut.
5. Commit the version/changelog update (attach the gate evidence directory path
   in the PR description).
6. Push the release tag, for example:

```bash
git tag v0.1.0-alpha.1
git push origin v0.1.0-alpha.1
```

## Release Notes Template

Use the matching changelog section as the release body. For the first alpha,
the release notes should call out:

- published `SHA256SUMS` for release asset verification
- published provider, runtime, and MCP images
- downloadable `froglet-node` binaries
- official site at `froglet.dev` (with `docs.froglet.dev` mirror)
- public OpenClaw integration
- reference discovery
- public operator image
- launch payment rails: Lightning, Stripe, and x402
- any intentionally deferred layers, especially external broker and closed higher-layer
  services
