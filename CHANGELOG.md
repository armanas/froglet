# Changelog

All notable changes to this repo should be recorded here.

The format follows Keep a Changelog. The current release line is `0.2.x`,
which adds agent-grade publish (one MCP call from intent to live offer) on
top of the `0.1.x` protocol core.

## [Unreleased]

### Added

- **Requester spend caps (buyer safety).** New node-local spend policy bounds
  what a node will pay when it creates deals as a buyer, across every
  settlement rail: `FROGLET_REQUESTER_SPEND_BUDGET_MSAT` (cumulative budget,
  persistently tracked in a spend ledger) and `FROGLET_REQUESTER_MAX_DEAL_MSAT`
  (per-deal cap). Enforcement happens before any money moves; refusals are
  402s with stable codes (`spend_budget_unconfigured`, `spend_cap_exceeded`,
  `spend_budget_exceeded`). New endpoints: `GET /v1/runtime/spend`,
  `POST /v1/runtime/spend/reset`. The runtime now also re-checks the signed
  quote against the caller's `max_price_sats` locally instead of trusting the
  provider to enforce it.

### Changed

- **BREAKING (behavioral, fail-closed): paid deals are refused until
  `FROGLET_REQUESTER_SPEND_BUDGET_MSAT` is configured.** Buyer nodes that pay
  for services must now set an explicit budget; the node logs a startup
  warning when a buyer wallet is configured without one. Free deals are
  unaffected.

## [0.4.0] - 2026-05-30

### Added

- **phoenixd Lightning backend — the easy self-custodial rail.**
  `FROGLET_LIGHTNING_MODE=phoenixd` points Froglet at an ACINQ
  [phoenixd](https://phoenix.acinq.co/server) node: a single self-custodial
  binary with automatic liquidity (pay-to-open / splicing), HTTP Basic auth,
  and no channel management. Run it, paste a URL + password, start earning.
- **New settlement method `lightning.prepaid.v1`** (additive; existing methods
  and signing bytes unchanged). phoenixd cannot do hold-invoice escrow, so the
  prepaid model is used: the provider mints an ordinary invoice at deal
  creation, the buyer pays it upfront, and the provider confirms payment before
  executing. The signed receipt carries the payment **preimage** as a
  cryptographic proof of payment (`sha256(preimage) == payment_hash`) — strictly
  stronger than Stripe's attested model.
  - Trade-off vs the LND hold-invoice tier: **no escrow**. The buyer pays
    upfront; on execution failure there is no automatic refund — the signed
    `failed` receipt (`settlement_state=settled`, `execution_state=failed`) is
    the buyer's cryptographic evidence. Use `FROGLET_LIGHTNING_MODE=lnd_rest`
    for pay-on-success escrow.
- Buyer-side prepaid payments: set `FROGLET_LIGHTNING_BUYER_PHOENIXD_URL` +
  `FROGLET_LIGHTNING_BUYER_PHOENIXD_HTTP_PASSWORD` so a node can pay providers'
  prepaid invoices from its own phoenixd. Both sides running phoenixd is a fully
  self-custodial, agent-to-agent Lightning exchange.
- `scripts/setup-payment.sh lightning --mode phoenixd` writes the env snippet
  and probes `GET /getinfo`. Non-loopback (real-funds) phoenixd URLs require an
  explicit `FROGLET_LIGHTNING_PHOENIXD_MAINNET_CONFIRM=1` opt-in.

### Verification

- New end-to-end test `phoenixd_prepaid_full_paid_deal_produces_settled_receipt`
  drives the full HTTP provider+buyer flow against a mock phoenixd and asserts a
  kernel-valid `lightning.prepaid.v1` receipt whose preimage hashes to the
  payment hash. Nine new kernel tests cover the method, including the
  mismatched-preimage rejection and the failed-but-paid (settled) receipt.

## [0.3.0] - 2026-05-30

### Added

- Paid services on the publish path. `settlement.method` now accepts
  `"lightning"` and `"stripe"` in addition to `"none"` (the hosted trial stays
  free). Price a service with `[price] sats = N` plus `currency = "sat"`
  (satoshis, settled over Lightning) or `currency = "usd"` (US cents, settled
  via Stripe).
- Stripe settlement rail using Shared Payment Tokens + manual-capture
  PaymentIntents: buyer-side SPT minting and seller-side reserve → capture →
  release, producing a signed `stripe_mpp.v1` receipt. Stripe settlement proof
  is attested (PaymentIntent reference); Lightning remains the
  preimage-verifiable rail.
- `LightningWallet` trait abstracting the settlement backend (LND REST behind
  it; behavior unchanged).

### Security

- SPT validation now fails closed: a Shared Payment Token missing
  `expires_at`, `currency`, or `maximum_amount` is rejected instead of
  silently passing (previously a payment-bypass risk).
- Stripe `secret_key` / `webhook_secret` are redacted in `Debug` output.

### Notes

- Stripe shared-payment is a preview API; the SPT field shapes are validated
  against a mock and must be confirmed against live Stripe before production
  use (see `src/settlement/stripe.rs::mint_spt`).
- Capture-then-persist is not atomic: a DB failure after a successful Stripe
  capture is logged CRITICAL for manual reconciliation; a durable
  reconciliation queue is a follow-up.

## [0.2.1] - 2026-05-15

Same surface as 0.2.0. Three corrections that made 0.2.0 imperfect:

### Fixed

- npm audit at the repo root and inside `integrations/mcp/froglet`
  both now report 0 vulnerabilities. 0.2.0 was published with two
  unpatched advisories in the MCP integration lockfile (high
  `fast-uri`, moderate `hono`) plus four advisories at the root
  (high `fast-uri`, moderate `hono` / `ip-address` / `express-rate-limit`),
  all transitive through `@modelcontextprotocol/sdk`. `npm audit fix`
  resolves both. The advisories are not reachable from
  `marketplace_publish` or any other Froglet code path, but the clean
  audit posture matters for downstream consumers and for the
  `npx -y froglet-mcp` install story.
- All 16 Froglet version sources now agree at 0.2.1. 0.2.0 shipped
  the publish-engine + CLI + MCP work correctly, but four metadata
  files (`server.json`, `plugins/froglet/.codex-plugin/plugin.json`,
  `plugins/froglet/.claude-plugin/plugin.json`,
  `.claude-plugin/marketplace.json`) were left at 0.1.5. The
  docs-site `plugin-distribution.test.ts` caught it and the fix
  landed on main but did not make it into the 0.2.0 tag.
- The 0.2.0 git tag points at commit `201db5e`, which has CI
  failures on the docs-site test and the npm audit gate. 0.2.1
  points at a tip with all gates green so the tagged commit and the
  published Docker images / binary tarballs match a CI-green state.

### Changed

- No feature changes. The publish engine, CLI subcommands, MCP
  tool, manifest v3, Phase 4 harness, and cleanup script ship at
  byte-identical behavior to 0.2.0; the only differences are in
  metadata files, lockfile dependency-resolution, and the
  `Cargo.toml` / `package.json` version strings.

### Migration

If you pulled `ghcr.io/armanas/froglet-mcp:0.2.0` or installed
`froglet-mcp@0.2.0` from npm (it's not yet published there), repull
or reinstall at 0.2.1. The 0.2.0 Docker images remain available
under their original tags; 0.2.1 supersedes them as the recommended
version.

## [0.2.0] - 2026-05-15

The headline of this release is **agent-grade publish**: an LLM with the
Froglet MCP can turn a one-sentence user intent into a live marketplace
offer in a single `marketplace_publish` tool call. Both the CLI and the
MCP wrap the same Rust publish engine, so the two surfaces can't diverge.

### Added

- `froglet-publish-engine` Rust crate — orchestrates
  build → host → sign → register against the running `froglet-node`
  daemon. Three hosting backends ship in this release: Local (loopback
  only), Tor (auto-onion via the existing hidden-service helper), and
  SelfHosted (operator-supplied URL). Managed and Fly backends are
  deferred to 0.3.
- `froglet-node` CLI subcommands: `init`, `build`, `publish`, `invoke`,
  `whoami`. `init` scaffolds a project (`froglet.toml`,
  `froglet-service.toml`, `handler.py`, `.gitignore`); `publish` reads
  the manifests and calls the engine.
- `marketplace_publish` MCP action in
  `integrations/mcp/froglet/lib/tools.js`. Shells out to
  `froglet-node publish --json` so MCP and CLI can never drift. Input
  mirrors the manifest; output returns `provider_id`, `public_url`,
  `marketplace_offer_url`, `offer_hash`, `invoke_command`.
- Manifest v3: project-level `froglet.toml` + per-service
  `froglet-service.toml`, parsed by `froglet-protocol::manifest` with
  `deny_unknown_fields` and 20 unit tests. v2 manifests still load with
  a deprecation warning rather than an error.
- Phase 4 LLM acceptance matrix harness (`tests/llm_acceptance/`):
  5 prompts × 2 Claude models × 3 hosting backends = 30 cells,
  dual pass-bar (≥27 count AND ≥90% rate). Per-cell JSON transcripts +
  summary.tsv. Stable failure categories (`tool-not-called`,
  `tool-misuse`, `engine-error`, `marketplace-lag`, `llm-loop`).
- 25 regression tests in `tests/llm_acceptance/test_validators.py`
  binding the harness contract — every defect a reviewer caught in the
  initial pass now has a test that would have caught it.
- `scripts/llm_acceptance_cleanup.py` (Phase 4.6) — extracts
  `provider_id`s from a matrix run's cell JSONs and emits idempotent
  SQL suspending each test provider via `provider_enforcements`.
- `ops/hosted_smoke.sh` — single entrypoint to verify hosted surfaces
  (`marketplace.froglet.dev` /healthz + `/v1/providers`,
  `arbiter.froglet.dev` /v1/feed) are reachable and well-shaped.
- Docs: `docs/MANIFEST.md` (v3 spec, both files);
  `docs/PROVIDER_ONBOARDING.md` rewritten to lead with
  `marketplace_publish` / `froglet-node publish`, with the four
  DNS-less paths demoted from "user paths" to "backend implementation
  details."
- Launch packet (`_tmp/launch_packet/{show_hn.md,blog_post.md,faq.md}`)
  rewritten around the one-MCP-call narrative with real evidence
  references.

### Changed

- Bumped workspace versions: `froglet` 0.1.1 → 0.2.0; `froglet-protocol`
  and `froglet-publish-engine` 0.1.0 → 0.2.0; `@froglet/mcp-server`
  0.1.5 → 0.2.0.
- `froglet-node`'s `args[1]` dispatch refactored into a small subcommand
  enum + `CliHandler` trait. No new dependency — does not pull in
  `clap` — but the dispatcher now produces stable exit codes via
  `CliError::exit_code()`.
- `arbiter.froglet.dev` is now a real Froglet provider (it appears in
  `/v1/feed`) rather than a legacy axum-only service. The previous
  `/healthz` route no longer exists; smoke scripts should check
  `/v1/feed` instead.

## [0.1.0-alpha.2] - 2026-05-10 (rolled up from prior Unreleased section)

### Added

- Docs-site and hosted-trial privacy posture note: v0.1.0 has no account,
  email, analytics cookie, or conversion tracking in the public hosted trial
  contract.
- DCO contribution policy, with manual maintainer enforcement for v0.1.0
  unless a DCO bot is installed.

### Changed

- Public claim language now states that `try.froglet.dev` exposes a free
  hosted demo catalog: `demo.add` is the canonical round-trip proof, while
  witness/hash/notarize flows are optional follow-up evidence and Lightning,
  Stripe, and x402 remain local/self-hosted launch adapters.
- `docs/RELEASE.md` now includes a v0.1.0 GitHub release body draft and
  separates scripted release gates from hard manual launch gates such as
  Claude MCP smoke and hosted-trial verification.
- `README.md` hosted-trial paragraph rewritten to match the MVP scope: a
  first-party hosted gateway in front of the reference node, a shared
  session-token pool (authentication only, not per-session identity), five free
  demo services, and explicit removal of the email-claim / account-conversion
  path.
- Docs site `Try In Cloud` page (`docs-site/src/content/docs/learn/cloud-trial.mdx`)
  rewritten to describe the shared session-pool model and remove the stale
  `POST /api/sessions/claim` / `verify` / `resume` endpoints and stale
  hosted-provider implementation details.
- Docs site landing (`docs-site/src/pages/index.astro`) hosted-trial card
  updated to match the session-pool model and drop the GCP wording.
- Docs site learning index (`docs-site/src/content/docs/learn/index.mdx`)
  updates its roadmap copy and stat strip to reference the shared session
  pool and 15-minute session TTL instead of "temporary identity".

### Removed

- Pre-publication launch plans, distribution matrices, historical playground
  plans, first-party hosted runbook stubs, and one-off security evidence docs
  that belonged in private operator/release records rather than the public
  open-source repo.

## [0.1.0-alpha.1] - 2026-04-20

### Added

- Public boundary notes clarifying that first-party hosted deployment,
  monitoring, and rotation runbooks now live in the private services/operator
  workspace.
- `docs-site/wrangler.jsonc` plus `docs-site` Wrangler scripts for the public
  Cloudflare Workers docs deployment path

### Changed

- Moved first-party hosted deployment tooling, hosted-provider specs, operator
  runbooks, and the working launch backlog out of this public repo and into
  a private operator workspace.
- standardized the public docs deployment path on Cloudflare Workers and
  removed the stale GitHub Pages workflow
- enabled GitHub Discussions and linked it from the public README

### Fixed

- release workflow `gh release view/create/download/upload` calls now pass
  `--repo "$GITHUB_REPOSITORY"` so GitHub Actions does not depend on local
  `.git` context when generating notes or publishing release assets

## [0.1.0-alpha.0] - 2026-04-19

### Added

- `scripts/release_gate.sh` — single release-candidate entrypoint combining
  strict checks, docs-site build, docs-site tests, and optional packaging /
  install-smoke / hosted cells, with per-step evidence logs and a
  `summary.tsv`
- `docs/PAYMENT_MATRIX.md` — supported payment rails × verification modes
  matrix with per-cell status and re-run commands
- `docs/IDENTITY_ATTESTATION.md` — normative spec for DNS + OAuth/OIDC
  identity bindings for Froglet keys
- `docs/ARBITER.md` — design stub for the marketplace-layer claims-court
  service
- `froglet-protocol::protocol::identity_attestation` — `IdentityAttestation`
  credential type, validator, and 8 roundtrip tests
- `scripts/cloudflare_dns.sh` — Cloudflare v4 DNS helper (verify / zone /
  list / create / delete / upsert); reads token from macOS Keychain, never
  echoed
- `FROGLET_EGRESS_MODE=strict` — opt-in propagation of the
  same DNS-pinning + SSRF validator used for LLM-controlled URLs to the
  operator-configured `FROGLET_PROVIDER_URL` / `FROGLET_RUNTIME_URL`
  surfaces in the Node MCP and OpenClaw integrations. Off by default;
  local/dev URLs stay on stock `fetch`
- Order-28-style content-shape assertions in `scripts/hosted_smoke.sh`:
  `/health` JSON envelope, `/v1/node/capabilities` key presence,
  `/v1/node/identity` minimum shape, `/v1/openapi.yaml` prefix, docs
  `text/html` + body marker
- `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), `.github/ISSUE_TEMPLATE/`
  (bug + feature YAML forms + Discussions/security contact links),
  `.github/pull_request_template.md` mirroring the release gate (TODO
  scaffolding; Discussions toggle still pending)
- OCI Wasm workload kind (`compute.wasm.oci.v1`) allowing Wasm modules to be
  referenced by OCI image (`oci_reference` + `oci_digest`) instead of inline hex
  bytes; supports `ghcr.io` and Docker Hub registries with anonymous pulls
- `OciWasmSubmission` and `OciWasmWorkload` structs in `src/wasm.rs`
- `oci-registry-client` dependency for OCI manifest and blob fetching
- OCI Wasm deal execution path with digest verification and sandbox execution
- official Docker assets for the Froglet node, including a starter
  `compose.yaml`
- public OpenClaw plugin with Froglet discovery and provider-surface tools
- checked-in OpenClaw starter config example
- GitHub Actions CI for strict checks and Docker starter validation
- GitHub Actions release workflow for tagged GHCR image publication

### Fixed

- `rustls-webpki` bumped 0.103.10 → 0.103.12 (RUSTSEC-2026-0098 + -0099)
- `cryptography` (Python) bumped 45 → 46.0.7 (3 GHSAs)
- `npm audit fix` in `integrations/mcp/froglet` (hono, @hono/node-server,
  path-to-regexp) and in `docs-site` (vite)
- Added `postgres_mounts` field to four test NodeConfig literals to
  restore `cargo check --all-targets` on `main`
- replaced `todo!()` panic in free OCI Wasm job path with full implementation
- added 50 MB size cap on OCI module downloads to prevent memory exhaustion
- fixed OCI reference parsing to handle `@sha256:` digest syntax alongside `:tag`
- replaced hardcoded registry allowlist with generic `https://{host}` fallback
  for unknown OCI-compliant registries
- extracted shared `fetch_oci_wasm_module` helper to deduplicate OCI pull logic

### Changed

- added `FROGLET_PUBLIC_BASE_URL` so containerized nodes can advertise a
  host-reachable clearnet URL
- tightened OpenClaw output defaults so raw JSON is opt-in via `include_raw`
- expanded OpenClaw tests to cover missing config, 404, invalid JSON, and
  timeout failure paths

### Fixed

- cleaned the warnings-denied Rust build path by removing stale `NodeConfig`
  initializer gaps and the unused-events-query warning

## Alpha Cut Notes

When cutting the first alpha:

1. move the current `Unreleased` notes into `0.1.0-alpha.1`
2. set `Cargo.toml` package version to `0.1.0-alpha.1`
3. push tag `v0.1.0-alpha.1`
4. let `.github/workflows/release.yml` publish the matching GHCR images
