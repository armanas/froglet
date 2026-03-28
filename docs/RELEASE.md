# Release

This repo now has a minimal tagged release path for the current Froglet node,
reference discovery, and OpenClaw alpha surface.

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
publishes the three role-specific images:

- `ghcr.io/<owner>/froglet-provider:<version>`
- `ghcr.io/<owner>/froglet-provider:<sha-tag>`
- `ghcr.io/<owner>/froglet-runtime:<version>`
- `ghcr.io/<owner>/froglet-runtime:<sha-tag>`
- `ghcr.io/<owner>/froglet-discovery:<version>`
- `ghcr.io/<owner>/froglet-discovery:<sha-tag>`

## Alpha Cut Checklist

1. Update `Cargo.toml` package version.
2. Move the relevant `Unreleased` notes in [../CHANGELOG.md](../CHANGELOG.md)
   into a concrete version section.
3. Run `./scripts/strict_checks.sh`.
4. Confirm `docker compose up --build` still starts cleanly.
5. Commit the version/changelog update.
6. Push the release tag, for example:

```bash
git tag v0.1.0-alpha.1
git push origin v0.1.0-alpha.1
```

## Release Notes Template

Use the matching changelog section as the release body. For the first alpha,
the release notes should call out:

- official Docker starter
- public OpenClaw integration
- reference discovery
- v1 Lightning reference settlement path
- any intentionally deferred layers, especially external broker and closed higher-layer
  services
