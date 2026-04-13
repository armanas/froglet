# Release

This repo now has a tagged release path for the current Froglet node,
marketplace binary, Docker images, MCP image, and public docs site.

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
- `ghcr.io/<owner>/froglet-marketplace:<version>`
- `ghcr.io/<owner>/froglet-marketplace:<sha-tag>`
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
- `froglet-marketplace-<tag>-linux-x86_64.tar.gz`
- `froglet-marketplace-<tag>-linux-arm64.tar.gz`
- `froglet-marketplace-<tag>-darwin-arm64.tar.gz`
- `SHA256SUMS`

The one-line installer at [../scripts/install.sh](../scripts/install.sh)
downloads from those release assets. By default it installs the latest tagged
`froglet-node` release into `~/.local/bin`. Use `VERSION=<tag>` to pin a
release, `INSTALL_DIR=/path` to override the destination, and
`INSTALL_MARKETPLACE=1` to install `froglet-marketplace` too.

The public release surface covers the tracked Froglet protocol docs, the public
docs site, reference node binaries, supported integrations, and validation
assets in this repo. Ignored local-only incubation under `private_work/` is not
part of the release surface.

## Alpha Cut Checklist

1. Update `Cargo.toml` package version.
2. Move the relevant `Unreleased` notes in [../CHANGELOG.md](../CHANGELOG.md)
   into a concrete version section.
3. Run `./scripts/strict_checks.sh`.
4. Confirm the packaged binary smoke still passes through the installer path.
5. Confirm the published docs site still builds from `docs-site/`.
6. Confirm `docker compose up --build` still starts cleanly.
7. Commit the version/changelog update.
8. Push the release tag, for example:

```bash
git tag v0.1.0-alpha.1
git push origin v0.1.0-alpha.1
```

## Release Notes Template

Use the matching changelog section as the release body. For the first alpha,
the release notes should call out:

- downloadable `froglet-node` and `froglet-marketplace` binaries
- published `SHA256SUMS` for release asset verification
- published provider, runtime, marketplace, and MCP images
- official docs site at `ai.froglet.dev`
- public OpenClaw integration
- reference discovery
- public operator image
- launch payment rails: Lightning, Stripe, and x402
- any intentionally deferred layers, especially external broker and closed higher-layer
  services
