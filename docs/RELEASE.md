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

The docs site deploy workflow builds `docs-site/` and publishes it to:

- `https://ai.froglet.dev`

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
assets in this repo. Ignored local-only incubation under `private_work/` is not
part of the release surface.

## Release Candidate Gate

This is the current release gate for the public Froglet repo. The goal is to
separate checks that are already implemented here from hosted checks that still
need the first public environment and external credentials.

### Implemented in this repo

| Status | Validation | How to run | Notes |
| --- | --- | --- | --- |
| Ready | Repo validation matrix | `./scripts/strict_checks.sh` | Rust, Python, OpenClaw, MCP, release helper syntax |
| Ready | Docs build | `npm --prefix docs-site run build` | Pre-publish docs-site build |
| Ready | Release asset structure | `scripts/package_release_assets.sh` + `scripts/verify_release_assets.sh` | Confirms packaged `froglet-node` assets and checksums |
| Ready | Installer-path smoke | `scripts/smoke_install_from_assets.sh --assets-dir <dir> --version <tag>` | Verifies install script plus launched node health |
| Ready | Compose bot-surface smoke | `FROGLET_RUN_COMPOSE_SMOKE=1 ./scripts/strict_checks.sh` | Optional local Docker check for OpenClaw and MCP |

### Pending hosted checks

| Status | Validation | Current entrypoint | Why still pending |
| --- | --- | --- | --- |
| Pending | Hosted docs URL smoke | `./scripts/hosted_smoke.sh` | Needs the published docs URL |
| Pending | Hosted provider and runtime health smoke | `./scripts/hosted_smoke.sh` | Needs the first public Froglet environment |
| Pending | Live MCP smoke with Claude auth | none yet in this repo | Blocked on hosted stack plus valid Claude auth; tracked by `Order: 11` |

### Cut Steps

1. Update `Cargo.toml` package version.
2. Move the relevant `Unreleased` notes in [../CHANGELOG.md](../CHANGELOG.md)
   into a concrete version section.
3. Run `./scripts/strict_checks.sh`.
4. Run `npm --prefix docs-site run build`.
5. Package and verify the release assets.
6. Run the installer-path smoke against those assets.
7. Run `./scripts/hosted_smoke.sh` once the public URLs exist.
8. Commit the version/changelog update.
9. Push the release tag, for example:

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
- official docs site at `ai.froglet.dev`
- public OpenClaw integration
- reference discovery
- public operator image
- launch payment rails: Lightning, Stripe, and x402
- any intentionally deferred layers, especially external broker and closed higher-layer
  services
