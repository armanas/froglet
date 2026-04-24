# v0.1.0 Release Evidence

Status: post-tag, pre-publication evidence record.

This document records what has direct evidence for the public v0.1.0 release.
It does not replace the kernel specification in [KERNEL.md](KERNEL.md), the
release mechanics in [RELEASE.md](RELEASE.md), or the payment coverage matrix
in [PAYMENT_MATRIX.md](PAYMENT_MATRIX.md).

## Release state

- GitHub release: <https://github.com/armanas/froglet/releases/tag/v0.1.0>
- Release status observed with `gh release view v0.1.0 --repo armanas/froglet`:
  published, non-draft, non-prerelease, `publishedAt=2026-04-24T00:15:55Z`.
- Local package versions observed in `Cargo.toml` and
  `froglet-protocol/Cargo.toml`: `0.1.0`.
- GitHub-reported release assets:
  - `froglet-node-v0.1.0-linux-x86_64.tar.gz`
    (`sha256:c1c8734eeaac044949032610eaa838c98aca1854761ad2935c7e5e51fec0d845`)
  - `froglet-node-v0.1.0-linux-arm64.tar.gz`
    (`sha256:3c89e3cfce132d98fc8f845c8cbd3f45ba47b1eb40965273ab0bd733ab0d3c5f`)
  - `froglet-node-v0.1.0-darwin-arm64.tar.gz`
    (`sha256:d0ecf17422217bb1640fdd79592d8c6f3a62015ae8f3f315559d152679fbf6da`)
  - `SHA256SUMS`
    (`sha256:92fbd0693bac9bd3b05d780afa595b386991436bb04d312e053b3b45a6b66174`)

## Evidence summary

| Surface | Evidence | Status | Supported claim |
| --- | --- | --- | --- |
| Release gate | `/Users/armanas/Projects/github.com/armanas/froglet/_tmp/release_gate/20260424T000837Z/summary.tsv` | PASS for `secrets`, `strict`, `docs-build`, and `docs-test`; `package` and `install-smoke` are SKIP | Repo checks, secret scan, docs build, and docs tests passed for the release gate evidence run. |
| GitHub release assets | `gh release view v0.1.0 --repo armanas/froglet --json tagName,url,assets,isDraft,isPrerelease,publishedAt,name` | Present on the published release | The v0.1.0 release page includes Linux x86_64, Linux arm64, macOS arm64, and `SHA256SUMS` assets. |
| Hosted verifier | `/Users/armanas/Projects/github.com/armanas/froglet-services/_tmp/post_deploy_verify/20260423T233450Z/summary.tsv` | PASS | Deployed node and marketplace services were running active revisions, with rollback refs present. |
| Hosted trial smoke | `/Users/armanas/Projects/github.com/armanas/froglet-services/_tmp/post_deploy_verify/20260423T233450Z/hosted_smoke.log` | PASS | `try.froglet.dev` minted a session and completed the free `demo.add` round trip. This proves only the free hosted trial flow. |
| Hosted upstream guard | Same hosted smoke log | PASS | Public trial routes were hidden on `ai.froglet.dev`; unauthenticated `try.froglet.dev/v1/feed` returned 401. |
| Hosted read surfaces | Same hosted smoke log | PASS | `docs.froglet.dev`, `ai.froglet.dev` health/capabilities/identity/OpenAPI, and `marketplace.froglet.dev` health/providers/offers responded in the smoke. |
| Claude MCP smoke | Same hosted smoke log | PASS | Claude MCP smoke was recorded against `/Users/armanas/Projects/github.com/armanas/froglet/.mcp.json`. |
| Live route spot-check | `curl -fsS -o /dev/null -w ...` from 2026-04-24 | PASS | `froglet.dev/`, `docs.froglet.dev/learn/quickstart/`, `ai.froglet.dev/health`, `try.froglet.dev/llms.txt`, and `marketplace.froglet.dev/healthz` returned 200. |
| Hosted demo spot-check | Manual curl run from 2026-04-24 | PASS | `try.froglet.dev` returned a succeeded `demo.add` deal with `sum=12` and a receipt. |
| Hosted version spot-check | `curl https://ai.froglet.dev/v1/node/capabilities` from 2026-04-24 | BLOCKER | The live hosted node reports `version=0.1.0-alpha.2`, while the public release is `v0.1.0`. Do not publish broad launch posts until the hosted node is redeployed to `v0.1.0` or all public copy explicitly states the hosted trial version. |
| Release asset checksum spot-check | `gh release download v0.1.0 --repo armanas/froglet` followed by `shasum -a 256 -c SHA256SUMS` from 2026-04-24 | PASS | All three published binary archives matched the published `SHA256SUMS`. |
| GHCR image manifest spot-check | `docker buildx imagetools inspect` from 2026-04-24 | PASS | Provider, runtime, and MCP images exist at `:0.1.0`; digests are recorded in [DISTRIBUTION_MATRIX.md](DISTRIBUTION_MATRIX.md). |
| Docker Compose CI job | <https://github.com/armanas/froglet/actions/runs/24872056345/job/72820478876> | PASS | The scheduled CI Docker Compose job validated config, built images, started provider/runtime, and completed OpenClaw plus MCP compose smoke. |

## v0.1.0 publication claims

The v0.1.0 public release can claim:

- the published GitHub release at
  <https://github.com/armanas/froglet/releases/tag/v0.1.0>
- binary release assets for Linux x86_64, Linux arm64, and macOS arm64
- a published `SHA256SUMS` asset for the release
- release-gate evidence for secret scan, strict repo checks, docs build, and
  docs tests
- public hosted trial on `try.froglet.dev` for the free `demo.add` flow only
- public hosted read surfaces for provider metadata/OpenAPI and marketplace
  providers/offers, as covered by the hosted smoke
- 200 responses from the primary live endpoints listed in the evidence table
- a hosted `demo.add` spot-check returning `sum=12` and a receipt
- OpenClaw/NemoClaw and MCP integration paths as checked by the repo strict
  gate and the recorded Claude MCP smoke
- GHCR provider, runtime, and MCP images at `:0.1.0`, with manifest digests
  recorded in [DISTRIBUTION_MATRIX.md](DISTRIBUTION_MATRIX.md)

## Explicit non-scope

These are not v0.1.0 publication claims:

- Hosted paid rails. Hosted Lightning, hosted Stripe, and hosted x402 are
  deferred to v0.2. v0.1.0 may document local and self-hosted payment setup.
- Any hosted paid execution flow. The hosted trial evidence proves only the
  free `demo.add` path.
- Persistent hosted identities, account recovery, email claim, or hosted
  account conversion.
- Production confidential or TEE execution. Confidential routes and artifacts
  may be documented as experimental only. The current v0.1.0 evidence does not
  prove a production TEE attestation backend.
- Multi-tenant hardened hosted compute. The public trial remains a constrained,
  free-only convenience surface.
- PyPI, npm registry, Homebrew, or OS package-manager distribution.

## Evidence gaps before broader publication copy

- Hosted deployment version drift is a hard publication blocker:
  `ai.froglet.dev/v1/node/capabilities` currently reports `0.1.0-alpha.2`,
  not `0.1.0`. Either redeploy the hosted node to the public release version
  before publication, or change all launch copy to say the hosted trial is still
  running `0.1.0-alpha.2`.
- Package and installer smoke were not run in the recorded release gate:
  `package=SKIP`, `install-smoke=SKIP`.
- The latest scheduled CI run failed only in the cloud-backed GCP Test Rig
  authentication step because GCP secrets were not configured for that run.
  Source and docs jobs passed. The launch-prep branch must make that job skip
  cleanly when GCP secrets are absent, and publication should wait for the
  follow-up CI run to be green.
- A public status page URL is not recorded here yet. Do not publish to HN,
  Reddit, X, or LinkedIn until the status page or equivalent public monitoring
  URL exists and is added to [LAUNCH_COPY.md](LAUNCH_COPY.md).
- The final LLM prompt must be tested through at least one LLM host after the
  docs/site prompt changes land. Record the host, date, and result before
  publication.

## LLM prompt test

Claude Code `2.1.119` ran the final hosted-demo prompt on 2026-04-24 with
shell access limited to `curl`, `jq`, `sleep`, and `printf`. It successfully
fetched `/llms.txt`, minted a session, discovered `demo.add`, created a deal,
observed `sum=12`, found a receipt, and produced a useful proved/not-proved
assessment. It also identified the hosted version drift above, so the prompt is
useful but publication remains blocked until that drift is resolved or disclosed.
