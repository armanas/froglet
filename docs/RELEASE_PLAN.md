# v0.1.0 Release Plan

Status: draft launch artifact.

This plan covers the public v0.1.0 release decision for the Froglet repo. It
does not replace the kernel specification in [KERNEL.md](KERNEL.md), the
release mechanics in [RELEASE.md](RELEASE.md), or the payment coverage matrix
in [PAYMENT_MATRIX.md](PAYMENT_MATRIX.md).

## Release decision

Do not cut `v0.1.0` until every hard blocker below has direct evidence. A
generated release, successful build, or green source-level review is not enough
for launch if live hosted behavior or Claude MCP behavior is still unverified.

Current version state:

- `Cargo.toml` currently reports `0.1.0-alpha.2`.
- The final release cut still needs the version bump and matching tag decision
  recorded in the release PR.
- Git tags must keep the `v` prefix, for example `v0.1.0`.

## v0.1.0 scope

The v0.1.0 public release can claim:

- signed Froglet kernel artifacts and the reference `froglet-node`
- GitHub release assets for Linux x86_64, Linux arm64, and macOS arm64
- `SHA256SUMS` for binary asset verification
- GHCR images for provider, runtime, and MCP server roles
- local Docker Compose path for a dual-role node
- public docs at `froglet.dev`, with `docs.froglet.dev` as the docs mirror
- OpenClaw/NemoClaw plugin and MCP server integration paths
- public hosted trial on `try.froglet.dev` for a free `demo.add` flow
- public marketplace read surface if the launch smoke verifies it

## Explicit non-scope

These are not launch claims for v0.1.0:

- hosted paid rails. Hosted Lightning, hosted Stripe, and hosted x402 are
  v0.2 work. v0.1.0 may document local and self-hosted payment setup only.
- persistent hosted identities, account recovery, email claim, or hosted
  account conversion.
- production confidential or TEE execution. Confidential routes and artifacts
  may be documented as experimental only. The current attestation backend is
  mock/limited unless a real backend is proven and documented before cut.
- multi-tenant hardened hosted compute. The public trial remains a constrained,
  free-only convenience surface.
- PyPI, npm registry, Homebrew, or OS package-manager distribution unless a
  separate artifact and verification path is added before cut.

## Hard blockers

| Gate | Required evidence before tag | Evidence placeholder |
| --- | --- | --- |
| Default release gate | `./scripts/release_gate.sh` passes on the release candidate. | `<_tmp/release_gate/.../summary.tsv>` |
| Compose bot-surface smoke | `./scripts/release_gate.sh --compose` passes, covering OpenClaw and MCP compose smoke. | `<_tmp/release_gate/.../summary.tsv>` |
| Package assets | Release assets verify for Linux x86_64, Linux arm64, and macOS arm64. | `<release workflow run or local dist verification>` |
| Installer smoke | Host-compatible `scripts/smoke_install_from_assets.sh` succeeds against the candidate assets. | `<install smoke log>` |
| Claude MCP smoke | Claude Code or Claude Desktop can load the Froglet MCP config and complete the expected tool smoke. This is a hard launch blocker. | `<Claude smoke transcript/screenshots/log>` |
| Hosted trial smoke | `try.froglet.dev` mints a session and completes the documented free `demo.add` deal with a receipt. | `<curl transcript with statuses and response hash>` |
| Hosted upstream guard | Direct public session/demo writes to `ai.froglet.dev` remain outside contract and return the expected rejection. | `<curl transcript>` |
| Docs routes | `froglet.dev`, `docs.froglet.dev`, and key learn pages return expected content. | `<curl transcript>` |
| Marketplace read surface | `marketplace.froglet.dev/v1/providers` and `/v1/offers` return the expected public read shape if marketplace is included in launch copy. | `<curl transcript>` |
| Security scan | `./scripts/gitleaks_gate.sh` reports zero unallowlisted findings on release-visible refs. | `<gitleaks log>` |

## Cut runbook

1. Confirm the release tag target and update version-bearing files owned by the
   release-cut worker.
2. Move the relevant [../CHANGELOG.md](../CHANGELOG.md) entries into the final
   `v0.1.0` section.
3. Run the default release gate and attach the evidence directory to the PR.
4. Run compose smoke if any provider, runtime, OpenClaw, MCP, or install path
   changed since the last green compose run.
5. Run package and installer smoke for the release candidate assets.
6. Run the Claude MCP smoke and attach evidence. Do not launch without it.
7. Run hosted-trial, docs-route, and marketplace-route curl smoke checks against
   the live domains that the release notes mention.
8. Confirm that release notes still state hosted paid rails as v0.2 and
   confidential/TEE as experimental unless real launch evidence changed that.
9. Create and push the tag only after the hard-blocker table has no empty
   launch evidence cells.
10. After the workflow completes, verify GitHub release assets, GHCR tags,
    checksums, and install script behavior against the published release.

## Launch evidence placeholders

Fill these in the release PR or tag checklist:

- Release gate evidence: `<pending>`
- Compose smoke evidence: `<pending>`
- Package verification evidence: `<pending>`
- Installer smoke evidence: `<pending>`
- Claude MCP smoke evidence: `<pending>`
- Hosted trial evidence: `<pending>`
- Docs route evidence: `<pending>`
- Marketplace read evidence: `<pending>`
- GHCR image digest evidence: `<pending>`
- GitHub release asset evidence: `<pending>`
