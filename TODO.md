# Froglet TODO

As of 2026-04-16, this file tracks the release backlog for taking Froglet from verified local/distributed testing to a public hosted launch.

## Legend

- ✅ verified complete
- 🟡 in progress / partially verified / blocked on external dependency
- ⬜ not started or not yet verified
- 🚀 current MVP release scope
- 🔭 beyond current MVP release scope
- 🤖 entirely LLM-doable once existing repo/cloud access is available
- 🤝 mixed: LLM can implement and verify most of the work, but a human must provide credentials, DNS, billing, approvals, posting authority, or hardware
- 🧑 manual-heavy: mostly product, business, partner, legal, publishing, or community work
- `Order: NN` indicates dependency-based execution order; sections stay grouped by theme, so numbers are not strictly ascending within every section

## Current Verified State

- ✅ 🚀 🤖 GCP distributed sweep completed end to end.
- ✅ 🚀 🤖 Repo-wide strict checks completed.
- ✅ 🚀 🤖 Local compose smoke completed.
- ✅ 🚀 🤖 Local LND regtest completed.
- ✅ 🚀 🤖 Live Codex CLI MCP smoke completed.
- ✅ 🚀 🤖 Docs repository and docs-site content audit completed.
- 🟡 🚀 🤝 Live Claude MCP host verification is the last major host-validation gap in current MVP testing, and it is presently blocked by Claude auth rather than by a confirmed Froglet runtime defect.

## Current MVP Release Scope

MVP means Froglet can be announced publicly and used by early adopters without hand-holding for the main path. That requires: a public docs site, one stable hosted Froglet environment, repeatable deploy and smoke verification, published Docker artifacts, verified Codex/Claude/Docker MCP paths, a clean public repo boundary, at least one real hosted payment path plus Lightning coverage, and a launch packet that can support a zero-cost release push.

Items marked 🔭 are intentionally outside that first public release bar. They are still useful, but they should not delay the first hosted release unless they become hard dependencies during rollout.

## Beyond MVP

Beyond MVP currently includes: Cursor host validation, AWS automation, PayPal, GPU work, broader data integrations, long-task batch support, Matrix and Telegram distribution, arXiv, and wider cross-posting channels that do not materially change whether the first public release is usable.

## Post-Launch v0.2 Scope

Within the 🔭 bucket, these items are the intended scope of an explicit v0.2 release in the first two to three months after initial launch, as opposed to deferred indefinitely. Separating them from the "someday" bucket makes the near-term roadmap visible.

- Long-task batch support (Order 44)
- Cursor MCP host validation (Order 41)
- PayPal support (Order 43)
- Dev.to technical deep dive (Order 36)
- One additional external data integration (subset of Order 46)

Items that remain 🔭 without a near-term target:

- GPU support (Order 45) — waits on a real workload driving the requirement
- Matrix integration (Order 47) and Telegram integration (Order 48) — only if user demand materializes
- arXiv submission (Order 40) — only if there is a paper-worthy artifact
- AWS deploys (Order 42) — only after GCP is proven and there is user demand for AWS-native deployment

## External Constraints Informing This Backlog

- Astro now steers new Cloudflare deployments toward Workers rather than legacy Pages-first assumptions, so docs hosting should default to Workers/Workers Builds for new setup: <https://docs.astro.build/en/guides/deploy/cloudflare/>
- Claude Code supports project-shared MCP config via `.mcp.json`, which matches the current Froglet setup: <https://code.claude.com/docs/en/mcp>
- Cursor exposes MCP setup in its docs under `/docs/mcp`, including project-scoped configuration patterns, so Cursor validation should be tested against a checked-in project config instead of one-off local state: <https://cursor.com/docs/mcp>
- Google Cloud’s container-on-VM creation path is deprecated; GCP automation should use startup scripts or `cloud-init`, or move to a heavier orchestrator only if the footprint justifies it: <https://cloud.google.com/compute/docs/containers/deploying-containers>
- AWS App Runner is no longer open to new customers starting 2026-04-30, so AWS automation should not default to App Runner for greenfield release work: <https://docs.aws.amazon.com/apprunner/latest/dg/what-is-apprunner.html>
- Stripe recommends isolated sandboxes for testing without real money movement; hosted Stripe verification should be written around sandbox fixtures, not ad hoc live-card poking: <https://docs.stripe.com/sandboxes>
- PayPal sandbox requires developer accounts plus sandbox accounts, so PayPal work is inherently mixed rather than fully automated by an LLM alone: <https://developer.paypal.com/tools/sandbox/>
- Hacker News explicitly warns against using HN primarily for promotion or asking for votes, so the Show HN post has to stand on technical merit and discussion value: <https://news.ycombinator.com/newsguidelines.html>
- Reddit expects each community’s own rules to be followed and treats repeated self-promotion badly; release posts must be tailored subreddit by subreddit: <https://support.reddithelp.com/hc/en-us/articles/205926439-Reddiquette>
- Lobsters is narrowly computing-focused and explicitly limits self-promo as a rough fraction of participation, so it only fits if the post is technical and discussion-worthy: <https://lobste.rs/about>
- Product Hunt remains free to use, but it prohibits company accounts and vote begging, so it is a distribution option rather than a guaranteed launch core: <https://www.producthunt.com/launch>
- arXiv submission is free, but it is for refereeable scientific work and may require endorsement and moderation, so it only makes sense if Froglet is accompanied by a real paper, benchmark note, or protocol write-up: <https://info.arxiv.org/help/submit/index.html>

## Recommended Release Order

1. Lock the public/private repo boundary and extract marketplace work that should not stay in the public repo.
2. Clear the "Froglet" name for public use and purchase the primary domain and subdomain baseline.
3. Publish Docker artifacts and stand up the hosted environment services (TLS edge, Lightning, Postgres, webhooks, rate limiting, logs, status page).
4. Stand up the hosted docs site and the hosted Froglet environment behind the new domain.
5. Make hosted verification repeatable: health, MCP, payment, and rollback checks.
6. Close the remaining MVP host-validation gap on Claude, with a dated fallback so it cannot block launch indefinitely.
7. Complete launch hygiene: security pass, community scaffolding, license and contribution policy, privacy posture, feedback loop.
8. Prepare the release packet: changelog, screenshots, demo flow, pricing/payment notes, launch FAQs.
9. Launch from owned channels first, then distribute to community channels that fit the audience and rules, driven by one distribution matrix rather than ad hoc per-channel posting.

## Public Boundary And Repo Hygiene

### ✅ 🚀 🧑 Approve the marketplace extraction boundary
Order: 02

Specification: Review the draft marketplace split proposal, resolve any open questions, and ratify exactly what leaves the public repo and what remains public. This should name the directories, services, secrets, deployment assumptions, and business logic that belong to the marketplace versus the core Froglet runtime, MCP surface, SDKs, and docs. The output should be the short written boundary decision that future cleanup work can implement against.

Definition of done: There is a checked-in approved decision document or issue comment that answers three questions with no ambiguity: what moves out, what stays in, and what public interfaces must remain stable after the split.

Execution: 🧑 Manual-heavy. An LLM can draft and revise the split proposal, but a human has to approve the product and ownership boundary.

### ✅ 🚀 🤝 Execute marketplace extraction from the public repo
Order: 03

Specification: Move marketplace-only code, secrets assumptions, deployment scripts, and documentation out of the public repository while keeping public Froglet builds, tests, and docs working. The split should preserve any required public contracts through adapters or documented APIs rather than hidden imports or private-path dependencies.

Definition of done: Public Froglet builds and tests pass after the extraction, no public code depends on private marketplace paths, and the new repository boundary is documented in the root docs.

Execution: 🤝 Mixed. The LLM can perform the code move and fixups, but a human may need to create the destination repo, approve private ownership, and decide migration timing.

### ✅ 🚀 🤖 Clean the repository after the split
Order: 04

Specification: Remove dead docs references, stale scripts, private-only examples, orphaned env samples, and misleading quickstarts left behind by the marketplace extraction. The goal is that a new reader sees only supported public release surfaces.

Definition of done: `rg` and docs review show no stale marketplace references in the public repo outside explicitly historical notes, and strict checks still pass.

Execution: 🤖 Entirely LLM-doable.

### ✅ 🚀 🤖 Re-run the validation matrix after the split
Order: 07

Specification: After repo cleanup, re-run the smallest useful checks first and then the full validation matrix, including strict checks, compose smoke, MCP tests, and any docs-site checks affected by moved files.

Definition of done: The same release gate used before launch passes against the post-split tree, with failures either fixed or explicitly classified as external.

Execution: 🤖 Entirely LLM-doable, assuming the same local/cloud credentials already used in prior validation remain available.

## Domain And Naming

### 🟡 🚀 🧑 Basic name and registry coherence check for "Froglet"
Order: 50

Specification: Froglet is an open source protocol name, not a startup brand, so this is a lightweight check rather than a full trademark clearance. Do a basic web search plus a USPTO TESS search for obvious conflicts in the software or infrastructure space (the goal is only to avoid stepping on a clearly-conflicting existing name), and verify the Docker org, npm scope, PyPI name, and GitHub org are coherent so the protocol does not collide with itself across registries. If usage later picks up and a commercial entity is formed, that entity will use a different name, so brand clearance is explicitly out of scope here.

Definition of done: There is a short written note confirming no obvious name conflicts in the relevant package registries and no flagrant trademark collision in the software space, and the relevant package and registry names are either owned or confirmed available.

Status (2026-04-17): registry + software-space check complete in [docs/NAME_COHERENCE.md](docs/NAME_COHERENCE.md). crates.io / npm unscoped / RubyGems / Packagist / Snap are free and should be locked. PyPI `froglet` and npm `@froglet` scope are held by unrelated projects; use `froglet-protocol` on PyPI and stay unscoped on npm. GitHub user + Docker Hub user are squatter-held but we ship under `armanas/*` and `ghcr.io/armanas/*` so these do not block launch. The manual USPTO TESS software-class trademark check is still pending.

Execution: 🧑 Manual-heavy. The LLM can run the public searches and draft the note, but the decision to proceed is manual.

### 🟡 🚀 🤝 Purchase the primary domain and set the DNS and email baseline
Order: 51

Specification: Purchase the primary Froglet domain (candidates: `froglet.sh`, `froglet.dev`, `froglet.io`, `froglet.app`) after the trademark sweep clears. Pick a registrar that also handles DNS cleanly (Cloudflare Registrar, Porkbun, or Namecheap). Configure DMARC, SPF, and DKIM for the email domain before the launch post mentions any contact address, so outbound mail is not silently filtered.

Definition of done: The domain is owned, DNS is authoritative at the chosen provider, and email authentication records exist for the launch email domain.

Status (2026-04-17): `froglet.dev` purchased. Registrar + DNS authority pending (Cloudflare Registrar is the current-best default per the external-constraints note). Email authentication records not yet configured — tracked in [docs/SUBDOMAIN_PLAN.md](docs/SUBDOMAIN_PLAN.md). Cloud-provider decision (which governs outbound-email provider) still in flight.

Execution: 🤝 Mixed. The LLM can document the registrar choice and produce the DNS and email records, but a human must own the purchase and the payment method.

### ✅ 🚀 🤝 Allocate the subdomain plan
Order: 52

Specification: Decide and document the subdomain layout for the launch surface. Froglet is a protocol, not a startup, so the apex is the open source project landing page rather than a company marketing site. Baseline plan:

- `froglet.X` — protocol landing page (what it is, link to GitHub, docs, and the hosted instance)
- `docs.froglet.X` — docs site (Astro Starlight from the public repo)
- `ai.froglet.X` — the hosted Froglet provider environment (the reference protocol instance)
- `marketplace.froglet.X` — the marketplace read API served from `froglet-services`
- `status.froglet.X` — public status page

All subdomains should be provisioned with valid TLS before launch day.

Definition of done: The subdomain map is written down, DNS records exist for each planned subdomain, TLS is valid on each, and every subdomain either serves its intended content or returns an explicit placeholder page.

Status (2026-04-17): plan written in [docs/SUBDOMAIN_PLAN.md](docs/SUBDOMAIN_PLAN.md) against the purchased `froglet.dev` domain. Single-deployment model adopted: `docs-site/` serves the apex landing page at `/` and the docs under `/learn/*`, with `docs.froglet.dev` mirroring via Cloudflare Workers hostname routing. `ai.froglet.dev`, `marketplace.froglet.dev`, and `status.froglet.dev` are defined but not yet provisioned. README + docs-site URL references consolidated to match the plan.

Execution: 🤝 Mixed. The LLM can configure DNS and deploy scripts, but a human must own the registrar and cloud accounts.

## Hosting And Deploy

### ⬜ 🚀 🤝 Host the documentation website
Order: 18

Specification: Deploy `docs-site/` as the public documentation site on a stable free or low-friction host, with Cloudflare Workers/Workers Builds as the default path for a new Astro deployment. The setup should build from version control, publish the generated site, support preview deploys, and attach a custom domain with TLS once the domain decision is made.

Definition of done: A production docs URL serves the home page plus key learning pages, static assets load correctly, TLS is valid, preview deploys exist for changes, and a scripted smoke check verifies route health after deployment.

Execution: 🤝 Mixed. The LLM can add deploy config and smoke checks, but a human must provide the hosting account, custom domain, and DNS authority.

### ⬜ 🚀 🤝 Host Froglet itself for the public hosted version
Order: 19

Specification: Stand up one public hosted Froglet environment on GCP with stable ingress, health checks, logs, restart behavior, and a documented operator path for rotating tokens and rolling updates. This should optimize for a single reliable release environment, not for multi-cloud breadth.

Definition of done: A public base URL exists, the standard health endpoints pass, at least one end-to-end user flow works through the public ingress, logs are inspectable, and rollback instructions are tested.

Execution: 🤝 Mixed. The LLM can automate and verify most of the stack, but a human must own the GCP project, billing, DNS, and public cutover.

### ⬜ 🚀 🤝 Establish the stable cloud-instance baseline
Order: 14

Specification: Define the baseline machine shape, disk, regions, image strategy, restart policy, secrets injection, and backup expectations for the first hosted environment. This should be lean enough to stay cheap and boring, while still being reproducible.

Definition of done: There is one documented baseline instance profile that can be recreated from scratch and that is the reference for all deploy automation and runbooks.

Execution: 🤝 Mixed. The LLM can produce the scripts and docs, but a human has to accept the cost and region choices.

### ⬜ 🚀 🤝 Automate GCP deploys
Order: 15

Specification: Build a repeatable GCP deploy path that publishes images, provisions or updates the instance, applies startup-script or `cloud-init` boot logic, injects required secrets, and runs post-deploy smoke checks. The automation should avoid deprecated `create-with-container` style flows.

Definition of done: A single documented command or CI job can deploy a fresh environment or update the existing one, and the post-deploy smoke suite confirms health, MCP reachability, and the core flow.

Related: Order 26 (operator deployment guide) should be derived from these scripts, not written as a parallel document.

Execution: 🤝 Mixed. The LLM can write and test the deploy scripts, but a human must provide GCP credentials, service accounts, and any final production approvals.

### ⬜ 🔭 🤝 Automate AWS deploys
Order: 42

Specification: Add a second deploy target only after GCP is stable. For new AWS work, default to ECS/Fargate or EC2-based automation rather than App Runner. The goal is portability and operator confidence, not cloud symmetry for its own sake.

Definition of done: There is one reproducible AWS deployment path with environment bootstrap, secrets wiring, health checks, and rollback guidance, and it is documented as a secondary target rather than the primary launch path.

Execution: 🤝 Mixed. The LLM can build the automation, but a human must provide AWS account access, IAM approval, and cost ownership.

### 🟡 🚀 🤖 Add hosted verification scripts for docs and Froglet
Order: 16

Specification: Create repeatable post-deploy smoke checks that hit docs routes, health endpoints, one MCP flow, and one public runtime flow. The scripts should fail loudly, produce machine-readable output, and be runnable both locally and in CI.

Definition of done: A single verification entrypoint can be run after deploy and clearly reports pass/fail for docs, health, runtime, and MCP coverage.

Status (2026-04-19): [scripts/hosted_smoke.sh](scripts/hosted_smoke.sh) now performs content-shape assertions, not just HTTP-200 reachability. Checks: docs URL (text/html + body contains "Froglet" — catches parked-page regressions), `/health` (JSON with `status=="ok"` and `service=="froglet"`), `/v1/node/capabilities` (JSON with `api_version=="v1"` + non-empty `identity.node_id` + non-empty `version`), `/v1/node/identity` (`node_id` + `public_key` of min length 32), `/v1/openapi.yaml` (body starts with `openapi:` prefix). Network errors surface as explicit FAIL, not silent 000. Live-MCP cell stays PENDING until Order 11 and hosted project config land. Remaining 🟡: the `one MCP flow` piece of the spec stays as a pending row; closes fully when Order 11 lands.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Add monitoring, alerting, and rollback runbooks
Order: 17

Specification: Define the minimum hosted operations layer: logs, uptime checks, alert routing, deployment history, and rollback procedures. This should stay intentionally lightweight, but it has to exist before public launch.

Definition of done: An operator can detect a broken deploy, identify the failing component, and roll back to the previous known-good state without improvising.

Execution: 🤝 Mixed. The LLM can prepare runbooks and configs, but a human must connect real alert destinations and decide the on-call path.

## Hosted Environment Services

These are the concrete services the hosted Froglet environment depends on. They are called out separately because "host Froglet" above conflates a dozen operational concerns into one bullet. Each item here is its own surface with its own failure mode, and each one has bitten hosted services at launch before.

### ⬜ 🚀 🤝 Reverse proxy and TLS automation
Order: 53

Specification: Stand up the public HTTPS edge for the hosted Froglet environment. Caddy or Cloudflare in front of the node, with automatic certificate issuance and renewal, HTTP-to-HTTPS redirect, and HSTS. The edge must handle the clearnet entry point and any webhook receivers on the same hostname tree.

Definition of done: Public HTTPS terminates correctly for all launch subdomains, certificates auto-renew, and a documented runbook covers cert failure recovery.

Execution: 🤝 Mixed. The LLM can produce the config and renewal checks, but a human must own DNS and the hosting account.

### ⬜ 🚀 🤝 Hosted Lightning node beyond regtest
Order: 54

Specification: Decide between self-hosting LND or using Voltage or Greenlight, then stand up the chosen path with real channel liquidity appropriate to expected volume. The node must be reachable from the public Froglet provider and from external peers. This is distinct from Order 22, which verifies the signed flow against the hosted node; this item is about the node existing at all.

Definition of done: A real Lightning node is running, has inbound and outbound capacity, is monitored, and is wired into the Froglet provider's settlement adapter with documented credentials injection.

Execution: 🤝 Mixed. The code is ready; the node choice, funding decision, and key custody are manual.

### ⬜ 🚀 🤖 LND channel-state backup automation
Order: 55

Specification: Automate static channel backup export and off-site replication. Losing channel state on a live Lightning node means losing money with no recovery path. Backups must be encrypted, replicated to at least one off-host destination, and restoration must be tested before launch.

Definition of done: Channel backups rotate automatically, off-site copies exist, a test restore has been performed successfully on a non-production node, and the runbook documents the recovery procedure.

Execution: 🤖 Entirely LLM-doable given access to the hosting and storage accounts.

### ⬜ 🚀 🤝 Postgres hosting decision and setup
Order: 56

Specification: Decide between a managed Postgres (Cloud SQL, Neon, Supabase) or Postgres on the same VM for the marketplace indexer and read API state. The decision affects cost, backup posture, and connection pooling. Provision the chosen option, load the schema, and wire the indexer and API to it.

Definition of done: Postgres is reachable from the indexer and API, automated backups exist, and the migration path from empty to populated state is scripted.

Execution: 🤝 Mixed. The LLM can automate provisioning and schema, but a human must accept the cost and hosting choice.

### ⬜ 🚀 🤖 Stripe webhook receiver
Order: 57

Specification: Expose a public HTTPS endpoint that receives Stripe webhook events, verifies the signature, deduplicates by event id, and updates settlement state idempotently. The endpoint must be reachable only via HTTPS with a valid Stripe signing secret and must survive Stripe retry semantics without double-settling.

Definition of done: Stripe delivers sandbox events to the hosted endpoint, signature verification rejects forged events, idempotent retries do not double-settle, and failures surface in logs and alerts.

Execution: 🤖 Entirely LLM-doable once the Stripe account and signing secret are available.

### ⬜ 🚀 🤖 Rate limiting and abuse prevention at the edge
Order: 58

Specification: A public compute endpoint without rate limiting becomes an abuse vector within hours of being discoverable. Add per-IP and per-identity rate limits at the edge, with a documented policy covering limits, burst, lockout behavior, and exceptions for signed internal traffic.

Definition of done: The rate limit policy is written, enforced at the edge, observable via logs or metrics, and easy to tune without a code deploy.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Tor hidden service decision and setup
Order: 59

Specification: The README currently claims signed deals can be served over clearnet or Tor onion endpoints. Decide whether the first hosted release ships with an onion endpoint or whether that sentence is softened until a follow-up release. If shipping, configure a Tor hidden service, verify the node serves the same signed artifacts on both transports, and document the setup. If not shipping, update the README and docs so no public material overpromises.

Definition of done: Either an onion endpoint is live and documented, or the README and docs no longer promise Tor support at launch. The decision is reflected consistently in all public materials.

Execution: 🤝 Mixed. The LLM can configure the hidden service and run verification, but a human must approve operating an onion endpoint and any associated policy.

### ⬜ 🚀 🤖 Log aggregation for the hosted environment
Order: 60

Specification: Ship hosted-node and related service logs to a single queryable location. A minimal setup is acceptable (journald plus a simple tail-to-file ingest, or free-tier Loki or Grafana Cloud), but the operator must be able to answer "what happened at 03:17 UTC" without SSHing into the box.

Definition of done: Logs from the node, reverse proxy, indexer, and webhook receiver land in one queryable place with a documented retention window and access procedure.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Operator admin surface
Order: 61

Specification: A minimal authenticated surface for the operator to view node health, in-flight deals, settlement queue state, and invoke a kill switch or paused-mode toggle. This does not need to be a full dashboard; it needs to be reachable, authenticated, and sufficient for incident response without editing config files or restarting services.

Definition of done: The operator can see the state they need and take the actions they need during an incident, using only the admin surface.

Execution: 🤝 Mixed. The LLM can build it; a human must decide the authn path and any exposure boundary.

### ⬜ 🚀 🤖 Key and identity rotation runbook
Order: 62

Specification: Document and rehearse the procedure for rotating the node's identity key, Stripe signing secret, Lightning macaroons, and any MCP tokens. A compromise response where the operator has no documented rotation procedure is a multi-hour outage.

Definition of done: There is a single runbook covering each secret's rotation steps, the rehearsal has been performed on a non-production instance, and the runbook links to the monitoring expectations during rotation.

Execution: 🤖 Entirely LLM-doable given access to the hosted environment and its secrets.

### ⬜ 🚀 🤝 Public status page
Order: 63

Specification: Expected baseline for any public hosted service. Use a free tier (UptimeRobot, BetterStack free, or self-hosted Uptime Kuma) that can monitor the docs site, the hosted node, and the marketplace read API, and publish a public URL the launch post can link to.

Definition of done: A public status URL exists, is monitoring the launch-critical endpoints, and is linked from the launch page and docs.

Execution: 🤝 Mixed. The LLM can configure the checks; a human chooses the provider and owns the account.

### ⬜ 🚀 🤖 Deploy the marketplace read API and indexer
Order: 64

Specification: The read side of the marketplace (indexer plus API) is already built in `froglet-services`. Deploy the publicly-appropriate subset behind `marketplace.froglet.X` with Postgres wired in, health checks, and the same smoke suite used for the core node. Confirm which endpoints are public at launch versus held for later, and document that boundary.

Definition of done: The read API responds on its public URL, the indexer is tailing the feed, the projected endpoints return real data, and the public surface matches the documented public contract with no private-only routes exposed.

Execution: 🤖 Entirely LLM-doable. Boundary decisions about which routes are public are manual, but the surface itself is small.

## Packaging And Agent Surfaces

### 🟡 🚀 🤖 Publish and verify Docker images
Order: 08

Specification: Produce release-quality Docker images for the supported Froglet surfaces, with pinned versions, documented tags, and smoke verification against pulled images rather than only local builds. Current evidence shows the local compose stack works, but public release tags still need a clean publish-and-pull verification loop.

Definition of done: Release tags are published, fresh pulls work on a clean machine, startup docs match reality, and the post-pull smoke suite passes without depending on local build artifacts.

Execution: 🤖 for build/test logic; effectively 🤝 for the final publish step if registry credentials or org permissions are manual. Treat the implementation as LLM-doable and the publish permission as external.

### 🟡 🚀 🤖 Verify the Docker-backed MCP path
Order: 09

Specification: Validate the checked-in Docker MCP usage path against the release image, not only against local source. The smoke should cover `status`, `discover_services`, and one `run_compute` plus completion wait, matching the host-level behavior expected from external agent users.

Definition of done: The documented Docker MCP command works from a clean environment with mounted tokens and returns the expected tool behavior through one full compute cycle.

Execution: 🤖 Entirely LLM-doable if the release image is already published and any required tokens are available in the environment.

### ✅ 🚀 🤖 Verify Codex MCP live
Order: 10

Specification: The Codex path has already been exercised against the local compose stack using the project’s MCP configuration and a real tool round-trip.

Definition of done: Already satisfied. Keep this in the release gate so it stays green as the stack evolves.

Execution: 🤖 Entirely LLM-doable.

### 🟡 🚀 🤝 Verify Claude MCP live
Order: 12

Specification: Run one live Claude Code MCP smoke against the checked-in project config with the local or hosted stack up, using the proven envelope of `status`, `discover_services`, and one compute round-trip. The only acceptable blockers are clearly classified setup issues such as missing auth or a genuine tool/runtime defect.

Definition of done: Claude executes the full smoke successfully, or the failure is pinned precisely to auth, config, or runtime behavior with evidence.

Execution: 🤝 Mixed. The LLM can run and classify the test, but a human must supply working Claude auth and any account-level approvals.

### ⬜ 🔭 🤝 Verify Cursor MCP live
Order: 41

Specification: Add a project-scoped Cursor MCP smoke using `.cursor/mcp.json`, then run the same `status` and compute envelope through a real Cursor or `cursor-agent` session. This is useful coverage, but it should not block the first hosted release unless Cursor becomes a primary launch claim.

Definition of done: Cursor can load the project MCP config and complete one full smoke flow, or the failure is classified as host configuration, auth, or Froglet runtime behavior.

Execution: 🤝 Mixed. The LLM can wire the config and run the smoke, but a human likely needs Cursor auth or an interactive host environment.

### ⬜ 🚀 🤝 Verify NemoClaw support end to end
Order: 13

Specification: Exercise the OpenClaw/NemoClaw integration path against a real environment that matches how NemoClaw users will install and invoke Froglet. This needs to cover config loading, tool behavior, and at least one happy-path execution rather than just unit coverage.

Definition of done: NemoClaw-facing install docs are accurate, one real invocation succeeds end to end, and any NemoClaw-specific config deltas are documented.

Execution: 🤝 Mixed. The LLM can do the implementation and test harnessing, but a human may need access to the actual NemoClaw host/runtime environment.

## Payments And Runtime

### ✅ 🚀 🤖 Verify Lightning locally with LND regtest
Order: 21

Specification: The local Lightning regtest path has already been exercised and should remain part of the release gate because it proves the core payment rail under deterministic conditions.

Definition of done: Already satisfied locally. Keep it green in validation.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Verify Lightning in the hosted path beyond regtest
Order: 22

Specification: Add one hosted or externally reachable Lightning verification path that proves the hosted deployment can complete the intended settlement flow and failure handling outside the purely local regtest harness.

Definition of done: A documented hosted Lightning test passes, including success, timeout or cancellation behavior, and operator observability for the result.

Execution: 🤝 Mixed. The LLM can drive the test harness, but a human may need live node access or hosted routing credentials.

### ⬜ 🚀 🤝 Verify Stripe hosted behavior
Order: 23

Specification: Test the hosted Stripe path in Stripe sandboxes, including config loading, payment intent or checkout creation, callback or webhook handling, success and failure paths, and restart behavior after interrupted state transitions.

Definition of done: The hosted Stripe flow completes successfully in sandbox mode, known failure modes are exercised, webhook handling is stable, and the operator docs say exactly how to configure and re-run it.

Execution: 🤝 Mixed. The LLM can implement and validate the flow, but a human must supply Stripe sandbox ownership, webhook endpoints, and secrets.

### ⬜ 🚀 🤝 Verify x402 hosted behavior
Order: 24

Specification: Exercise the hosted x402 path through the public deployment, confirming pricing metadata, payment challenge/response behavior, free-path handling where applicable, and the operator experience when the rail is misconfigured.

Definition of done: A public x402 smoke passes from the external edge, and the failure messages are specific enough to diagnose auth, pricing, or settlement issues without reading source.

Execution: 🤝 Mixed. The LLM can write and run the hosted checks, but a human may need external credentials or gateway ownership.

### ⬜ 🔭 🤝 Add PayPal support
Order: 43

Specification: Add PayPal only after the initial release rails are stable. This should include a clear adapter boundary, sandbox-based verification, webhook handling where required, documentation, and tests that match the rest of the payment matrix.

Definition of done: PayPal sandbox flows pass end to end, the feature is documented as supported, and payment-rail selection logic handles PayPal cleanly alongside existing rails.

Execution: 🤝 Mixed. The LLM can build it, but a human must provide PayPal developer access and sandbox accounts.

### ✅ 🚀 🤝 Expand the payment verification matrix
Order: 25

Specification: Turn payments into an explicit matrix instead of scattered one-off tests. The matrix should cover local/regtest, hosted sandbox, failure injection, restart recovery, and observability expectations per supported rail.

Definition of done: There is a documented table of supported payment rails and a repeatable test for each promised mode, with unsupported cells called out explicitly.

Status (2026-04-19): matrix written in [docs/PAYMENT_MATRIX.md](docs/PAYMENT_MATRIX.md). Four rails (`None`, `Lightning::Mock`, `Lightning::LndRest`, `X402`, `Stripe`) × seven verification columns (unit, local integration, hosted sandbox, hosted live, failure injection, restart recovery, observability). Every cell maps to either a `release_gate.sh` flag or an explicit TODO order blocker ([22](todo.md) Lightning hosted, [23](todo.md) Stripe hosted, [24](todo.md) x402 hosted, [54](todo.md) hosted LND, [57](todo.md) Stripe webhook). Known gaps — multi-rail fallback, chaos testing, load testing — called out explicitly in §5 so deferrals are visible.

Execution: 🤝 Mixed. The LLM can define and automate the matrix, but real provider accounts are manual.

### ⬜ 🔭 🤖 Add long-task batch support
Order: 44

Specification: Support multi-task or queued long-running execution where the caller can submit batches, observe progress, and retrieve results predictably. This should build on existing task semantics rather than invent a second execution model.

Definition of done: Batch submission, progress reporting, and completion retrieval are implemented, tested, documented, and exercised in one realistic scenario.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🔭 🤝 Add GPU support
Order: 45

Specification: Introduce a clearly bounded GPU execution path for workloads that need accelerated inference or compute, including scheduling constraints, instance selection, dependency management, and clear fallback behavior when GPUs are unavailable.

Definition of done: At least one GPU-backed workload runs successfully in a supported environment, the deployment story is documented, and non-GPU environments fail gracefully.

Execution: 🤝 Mixed. The code path is LLM-doable; the hardware and cloud quota are manual.

### ⬜ 🔭 🤝 Add integrations to external data sources
Order: 46

Specification: Define a first set of data integrations that materially improve Froglet workflows, such as object storage, databases, or external APIs. Each integration needs a strict credential boundary, tests, and docs, not just a one-off connector.

Definition of done: The chosen integrations have stable interfaces, credential handling, tests, and examples, and they are clearly marked supported in docs.

Execution: 🤝 Mixed. The LLM can implement the adapters, but a human must decide which systems are strategically worth supporting and provide credentials for real verification.

### ⬜ 🔭 🤝 Add Matrix integration
Order: 47

Specification: Support Matrix as an inbound or notification channel only if it improves the hosted product story. The work should include auth, room selection, rate limiting, and moderation expectations.

Definition of done: A real Matrix room can receive or trigger the intended Froglet behavior, and the integration is documented and testable.

Execution: 🤝 Mixed. The LLM can build it; a human must provide homeserver and room access.

### ⬜ 🔭 🤝 Add Telegram integration
Order: 48

Specification: Support Telegram as an inbound or notification channel with a clear bot setup story, message parsing rules, and a constrained action set that is safe for remote triggers.

Definition of done: A Telegram bot can complete the intended workflow in a real chat, and the operator docs cover bot creation, secrets, and failure handling.

Execution: 🤝 Mixed. The LLM can implement it; a human must create and manage the bot credentials.

## Docs, Website, And Release Packaging

### ✅ 🚀 🤖 Audit the docs and docs website for correctness
Order: 06

Specification: The existing docs and Astro docs-site have already been reviewed and corrected where they diverged from the current token split and MCP usage expectations.

Definition of done: Already satisfied for the current tree. Keep docs checks in the release gate so drift is caught before launch.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Finish the public protocol landing page and resolve launch blockers
Order: 27

Specification: The site in the `froglet-website` repo is the public landing page for the Froglet protocol, not a startup marketing site. There is no company yet; a commercial entity may be formed later under a different name if and when usage picks up. The site's job is therefore narrow: explain what the protocol is, link to GitHub and docs, and point to the hosted reference instance at `ai.froglet.X` and the marketplace read API at `marketplace.froglet.X`. It is already well underway and is closer to "ship" than to "build," but significant current content assumes a startup framing that needs to come out before launch.

Remaining work (concrete blocker list):

- Replace the hardcoded `"#"` GitHub URL in `src/lib/constants.ts` with the real public repo URL.
- Remove or hide the footer Discord and Twitter placeholder `href="#"` links. There are no official community channels yet; either create them or hide the icons entirely rather than ship broken links.
- Remove the visible "PLACEHOLDER · CONNECT MARKETPLACE API TO GO LIVE" text in the MarketplaceTraction section; either wire it to the real read API (Order 64) or replace with a static screenshot.
- Implement actual docs page routing so `/docs` links go somewhere useful (most likely, redirect to `docs.froglet.X` once Order 18 lands).
- Remove or repurpose the "Pro" and "Enterprise" pricing tiers. There is no SaaS product being sold; the site should not imply one. Either drop the pricing page entirely, or reframe as "hosted access" with clear "reference instance, no SLA, no commercial entity behind it" language. The decision to reintroduce pricing waits until a commercial entity exists under a different name.
- Drop `sales@` contact addresses; there is no sales function. Keep only a `hello@` or `contact@` address, and verify it receives mail before publishing it; coordinate with Order 51.
- Stand up a deploy pipeline (the repo has `fly.toml` but no CI deploy); pick one path and document it.
- Make the hosted-versus-self-hosted distinction explicit: self-hosted Froglet is the default; `ai.froglet.X` is a convenience reference instance, not a commercial offering.

Definition of done: There is a public protocol landing page with working navigation, all blocker items above resolved, framing that does not imply a company that does not exist, clear calls to GitHub, docs, the hosted reference instance, and the marketplace read API, and an automated or documented deploy path.

Execution: 🤝 Mixed. The LLM can implement the fixes and deploy config, but a human should approve positioning and any claims about hosted availability or support.

### ⬜ 🚀 🤖 Add an operator deployment and verification guide
Order: 26

Specification: Write one operator-focused guide that covers image selection, tokens, compose or cloud deployment, hosted smoke checks, payment verification, and rollback. This should be the document followed during release week. The guide should be derived from the Order 15 deploy automation (quoting scripts and expected output) rather than written as an independent parallel document, so the two cannot drift.

Definition of done: A new operator can deploy and verify the stack from the guide without relying on tribal knowledge or old chat logs, and the guide cites the deploy automation as its source of truth.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Create the full release plan
Order: 29

Specification: Build a dated release plan covering code freeze, release candidate validation, image publishing, docs cutover, hosted cutover, launch content prep, launch day posting order, and post-launch monitoring. The plan should separate blockers from stretch goals.

Definition of done: There is one release plan with owners, order, required inputs, go/no-go criteria, and a launch-day checklist.

Execution: 🤝 Mixed. The LLM can draft the plan, but a human has to approve dates, owners, and public commitments.

### ✅ 🚀 🤖 Create the release-candidate gate
Order: 28

Specification: Turn the pre-launch bar into a named release gate that combines strict checks, docs-site build/tests, compose smoke, MCP smokes, and hosted smoke scripts into one checklist or automation entrypoint.

Definition of done: A candidate release can be marked pass or fail from one place, and every line item has an evidence artifact or log.

Status (2026-04-18): single entrypoint landed at [scripts/release_gate.sh](scripts/release_gate.sh). Steps: `strict`, `docs-build`, `docs-test`, optional `package`/`install-smoke`, optional `hosted`. Per-step logs under `_tmp/release_gate/<ts>/<step>.log` plus a `summary.tsv`. Exit codes: 0 PASS, 1 FAIL, 2 PENDING-under-`--strict`. Documented in [docs/RELEASE.md](docs/RELEASE.md#release-candidate-gate); the live-Claude smoke (Order 11) stays explicitly outside the gate until the hosted environment and Claude auth land.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Publish the GitHub release and changelog
Order: 30

Specification: Prepare a real release note set that explains what Froglet is, what changed, how to run it, what is hosted versus self-hosted, and what remains intentionally out of scope. This is the authoritative launch artifact that other channels should point back to.

Definition of done: A tagged release exists with a changelog, install paths, image references, docs links, and known limitations, and it matches the actual shipped artifacts.

Execution: 🤝 Mixed. The LLM can draft and assemble the release, but a human typically owns the final publish action and version choice.

## Launch Hygiene

These items are cheap individually, easy to forget, and highly visible when missing. They are not covered by the sections above.

### ✅ 🚀 🤖 Pre-launch security pass
Order: 65

Specification: One-shot review that combines dependency audits, secret scanning across the whole git history of the public repo, and a short threat model sketch for the public hosted node. The point is not to stand up a security program; it is to catch the high-probability misses (committed tokens, vulnerable transitive dependencies, unauthenticated internal endpoints) before the launch post hits aggregators.

Definition of done: A written security-pass note exists covering dep audit output, secret scan output across history, and a short threat model for the hosted surface. Any findings are either fixed or explicitly accepted with a documented reason.

Status (2026-04-18): written up in [docs/SECURITY_PASS.md](docs/SECURITY_PASS.md). Fixes landed inline: `rustls-webpki` 0.103.10→0.103.12 (RUSTSEC-2026-0098/0099); `cryptography` 45→46.0.7 (3 GHSAs); `npm audit fix` in `integrations/mcp/froglet` (hono + path-to-regexp) and `docs-site` (vite). Post-fix status: 0 vulns across cargo/pip/npm; remaining cargo warnings (`rand` unsound-with-custom-logger, `gimli` yanked-transitive-via-wasmtime) accepted with documented reason. Secret scan across all 71 commits in full history → 7 findings, 100% verified false positives on test fixtures (cashu public test-mint token + literal `"sk_test_placeholder"` in Rust unit tests); zero real leaks. Threat model for `ai.froglet.dev` enumerates 10 top risks with existing mitigations and points at the Order 53–75 dependencies. Incidental fix: `postgres_mounts` field missing from 4 test NodeConfig literals on `main` (predates this pass; broke `cargo check --all-targets` and therefore the Order 28 release gate) — repaired in [tests/payments_and_discovery.rs](tests/payments_and_discovery.rs), [tests/builtin_service_dispatch.rs](tests/builtin_service_dispatch.rs), [tests/lnd_rest_settlement.rs](tests/lnd_rest_settlement.rs), [tests/runtime_routes.rs](tests/runtime_routes.rs).

Execution: 🤖 Entirely LLM-doable. Remediation may require human approval if it changes public behavior.

### ⬜ 🚀 🤖 Add CODE_OF_CONDUCT, issue and PR templates, and enable Discussions
Order: 66

Specification: Standard public repo scaffolding. Add a CODE_OF_CONDUCT.md (Contributor Covenant or similar), bug and feature issue templates, a PR template that mirrors the release gate checklist, and enable GitHub Discussions for non-issue conversation.

Definition of done: All four items are present in the public repo, referenced from the README, and the templates render correctly when opening a new issue or PR.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🧑 License and contribution policy decision
Order: 67

Specification: The repo is Apache-2.0. Decide whether external contributions require a CLA, a DCO sign-off, or nothing beyond the LICENSE. Document the decision in CONTRIBUTING.md and install any required bot or workflow. This has legal consequences and should not be decided by automation.

Definition of done: CONTRIBUTING.md states the contribution policy, any required bots are installed, and the first outside PR has a documented path through the process.

Execution: 🧑 Manual-heavy. The LLM can draft the policy once a direction is chosen.

### ⬜ 🚀 🤝 Privacy and cookie posture on the landing page and docs site
Order: 68

Specification: If the protocol landing page or docs site adds analytics, session replay, or any third-party script, they need a privacy policy, a cookie notice where required, and a documented data-retention position. A zero-analytics launch is a valid choice and skips most of this, but the decision has to be explicit, not accidental. Since there is no commercial entity behind the site, the privacy policy should be written as an open source project notice rather than a company policy.

Definition of done: Either the sites run with no trackable third-party scripts and that is documented, or a privacy policy and cookie notice exist and match the actual third parties in use.

Execution: 🤝 Mixed. The LLM can draft the policy; a human accepts the legal posture.

### ⬜ 🚀 🤖 Post-launch feedback loop
Order: 69

Specification: Define how the project will learn from the first four weeks after launch. Minimum set: analytics choice (or explicit decision to skip), a single inbox or channel for feedback, and a weekly triage ritual that reviews issues, discussions, and direct feedback. Without this, early signal gets lost.

Definition of done: The analytics choice is implemented or explicitly skipped, the feedback channel is documented in the README and launch post, and the first triage run is scheduled on the calendar.

Execution: 🤖 Entirely LLM-doable for implementation; the decision to skip analytics is manual.

## Security Hardening Follow-ups

These items are defense-in-depth extensions beyond the closed-out findings from the 2026-04-16 security review. None is a known exploitable vulnerability; each narrows a residual risk or restores a capability that was intentionally scoped out of the initial fix.

### ✅ 🔭 🤖 Extend IP pinning to operator-configured URL paths (FROGLET_EGRESS_MODE=strict)
Order: 70

Specification: The Node MCP / OpenClaw integration already IP-pins outbound requests on the LLM-controlled `request.provider_url` path via the `pinnedJsonRequest` helper added in `integrations/shared/froglet-lib/url-safety.js`. Operator-configured `runtimeUrl` and `providerUrl` still go through stock `fetch`, which re-resolves DNS per request. An operator whose DNS resolver is compromised could therefore be rebound. Extend pinning to those paths behind an explicit opt-in `FROGLET_EGRESS_MODE=strict` environment flag so enterprise and high-assurance deployments get uniform pinning without forcing the custom dispatcher on everyone. Thread the same `pin` option through `frogletRequest` and `frogletRequestWithStatus` in `integrations/shared/froglet-lib/froglet-client.js`, validate the operator-configured URLs at config-load time using the same `validateProviderUrl` helper, and document the flag in the integrations README.

Definition of done: When `FROGLET_EGRESS_MODE=strict` is set, every outbound HTTP request issued by the Node integrations goes through a DNS-pinned dispatcher; when unset, behavior is unchanged. A test fixture confirms the pinned path is used in strict mode and stock `fetch` is used otherwise.

Status (2026-04-19): `isStrictEgressMode()` + `resolveOperatorPin()` added to [integrations/shared/froglet-lib/froglet-client.js](integrations/shared/froglet-lib/froglet-client.js); `pin` now threads through `frogletRequest`, `frogletRequestWithStatus`, and `frogletPublicRequest` with caller-supplied pin taking precedence over opportunistic strict-mode resolution. Per-process pin cache keyed on normalized base URL keeps validation one-shot. 6 tests in [integrations/shared/froglet-lib/test/egress-mode.test.mjs](integrations/shared/froglet-lib/test/egress-mode.test.mjs) cover: env-var parsing, lenient-mode passthrough, strict-mode null-URL handling, loopback / non-https rejection under strict, failure-not-cached semantics. Flag documented in [integrations/mcp/froglet/README.md](integrations/mcp/froglet/README.md). Test file + source added to `scripts/strict_checks.sh` so regressions break the release gate.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🔭 🤖 Cryptographic offer-to-descriptor binding at the service layer
Order: 71

Specification: `validate_offer_artifact` in `froglet-protocol` enforces `offer.signer == offer.payload.provider_id` but does not verify that `offer.payload.descriptor_hash` refers to a descriptor actually signed by the same provider. This is consistent with the rest of the kernel, which stays storage-free, so the check cannot live there. Add it at the service layer in `froglet-services/services/marketplace-node`: before accepting an offer, look up the referenced descriptor by hash in Postgres and confirm the stored descriptor's `signer` matches the offer's `signer`. Reject otherwise. This closes a low-impact integrity gap where an attacker who controls provider A could attach a valid offer referencing a `descriptor_hash` that was never actually signed by A.

Definition of done: The marketplace register handler and the indexer offer-projection path both run the descriptor-lookup binding check; tests cover the accept case, reject case when the descriptor is absent, and reject case when the descriptor exists but has a different signer.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🔭 🤝 Restore Tor `.onion` routing via the LLM-controlled `provider_url` surface
Order: 72

Specification: The LLM-controlled `provider_url` on the Node integration currently rejects `.onion` hostnames with a clear error pointing the caller at the Rust runtime, which handles Tor natively. That preserves correctness at the cost of one capability: callers cannot point `get_service` or `invoke_service` directly at a Tor onion provider via the tool-argument override. Add it back by introducing a `torSocksDispatcher(torSocksUrl)` helper in `integrations/shared/froglet-lib/url-safety.js` that wraps the established `socks-proxy-agent` package (or an equivalent audited SOCKS5 client), read `FROGLET_TOR_SOCKS_URL` from the environment, and replace the `.onion` rejection branch with dispatcher construction. Adding a production dependency requires human review.

Definition of done: `.onion` hostnames on the LLM-controlled path are routed via a SOCKS5 dispatcher when `FROGLET_TOR_SOCKS_URL` is set, rejected with a clear error when unset, and tests cover both. The new dependency is called out in the commit body for reviewer attention.

Execution: 🤝 Mixed. The LLM can implement and test; a human should review the new production dependency before it lands.

### ⬜ 🔭 🤖 Container-wrap Python as an alternative sandbox mode
Order: 73

Specification: The Python sandbox that ships with Froglet today uses Linux landlock + seccomp installed via `Command::pre_exec` in [`src/python_sandbox.rs`](src/python_sandbox.rs). That works uniformly whether Froglet runs on host or inside Docker. For operators who prefer container-level isolation (e.g., who already run every workload in a container) add an alternative mode gated by `FROGLET_PYTHON_SANDBOX=container`. In that mode `run_python_execution` routes through the existing `run_container_execution` path with a Python base image (`python:3.12-slim` or similar) pinned by sha256, `--network none`, and the invocation tempdir volume-mounted. Does not replace the landlock path as default; ships alongside.

Definition of done: setting the env var switches Python execution through the container runner; existing landlock path stays the default; both modes have integration tests; docs/RUNTIME.md documents the trade-off (needs docker socket access on the host if Froglet itself runs in Docker — see `docs/MOUNTS.md` commentary about docker-in-docker constraints).

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🔭 🤝 Replace the `nvidia.mock.v1` TEE attestation backend with a real one
Order: 74

Specification: `src/confidential.rs:37` currently ships only `ATTESTATION_BACKEND_NVIDIA_MOCK_V1` as a real-looking placeholder for TEE/confidential-execution attestation. Every confidential flow ends at a mocked attestation, which is fine for CI and integration tests but not for any deployment that actually relies on confidential guarantees. Replace with at least one real backend: AWS Nitro Enclaves (via `nsm-api`), AMD SEV-SNP, Intel TDX, or real NVIDIA H100 via NVIDIA's NRAS. Pick by target hardware; keep the mock available under a clearly-named `_test_only` flag so CI does not break.

Definition of done: at least one real attestation backend is wired into `issue_attestation` / `verify_attestation` in [`src/confidential.rs`](src/confidential.rs); the existing tests still pass against the mock when an explicit `FROGLET_ATTESTATION_BACKEND=mock` is set; an end-to-end test on the chosen real backend runs in a suitable hardware CI job (may be skipped locally).

Execution: 🤝 Mixed. Vendor SDKs, credentials, and hardware access are human prerequisites; the wiring itself is LLM-doable.

### ⬜ 🔭 🤝 Firecracker / gVisor microVM isolation tier for hosted.froglet.dev
Order: 75

Specification: The landlock + seccomp sandbox (Order-73 companion) is the right answer for single-tenant self-hosts. For a future managed hosted Froglet instance that serves multiple tenants from one node, add a microVM isolation tier using either AWS Firecracker or Google gVisor. This runs each Python (and Container) workload in its own kernel-level isolation boundary, closes kernel-level side-channel risks landlock alone cannot address, and is the industry-standard choice for genuinely-multi-tenant code execution (Lambda, Cloud Run). Gate behind `FROGLET_EXECUTION_ISOLATION=microvm` with Firecracker as the default choice.

Definition of done: `run_python_execution` and `run_container_execution` can both be routed through a microVM runner; a hosted-ready deployment recipe exists; documentation explains the trade-off (cost, latency, operational complexity) vs. the landlock default.

Execution: 🤝 Mixed. Meaningful operational research plus code. Not urgent until the hosted product is live.

### ⬜ 🔭 🤝 LLM-guided local install flow from a hosted Froglet instance
Order: 76

Specification: The long-term product flow is "user visits `ai.froglet.dev` → their LLM (Claude Code / Codex / etc.) is connected to the hosted instance → user asks to install Froglet locally → the LLM uses its own shell-execution capability to run the 4-line quickstart." No new Froglet-side MCP action is required; what's needed is (a) a reliable copy-paste quickstart (shipped as Order-71 / item C above) and (b) the hosted instance emitting clear "install me locally" tool-output text when a user asks, so the LLM reaches for the right commands. Write the hosted-instance UX copy, the docs section addressed to an LLM reader, and a test that the copy-paste block from `README.md` actually runs clean on a fresh host.

Definition of done: documented flow from hosted Froglet to a running local stack without human intervention beyond approving the LLM's shell commands; end-to-end test runs the quickstart block in a disposable VM; hosted instance produces recognisable tool output.

Execution: 🤝 Mixed. Needs hosted instance control (live UX); the docs + local test are LLM-doable.

### ⬜ 🔭 🤖 Additional mount connectors beyond Postgres (SQLite, S3, KV)
Order: 77

Specification: The Postgres mount landed in the same cycle as this TODO entry ([`docs/MOUNTS.md`](docs/MOUNTS.md), [`src/api/mod.rs::collect_postgres_mount_env`](src/api/mod.rs)). Extend the pattern to SQLite (local file path mounts, read-only + read-write variants), S3-compatible object stores (DSN-style config, credentials passed as separate env vars), and a generic key-value handle for Redis / DynamoDB / etc. Each kind follows the same shape: operator config via `FROGLET_MOUNT_<kind>_<handle>`, capability gating via `mount.<kind>.<read|write>.<handle>`, env-var injection into the workload, and sandbox network-allowlist toggling when the kind needs outbound TCP.

Definition of done: at least one more mount kind past Postgres shipped end-to-end with tests; `docs/MOUNTS.md` updated with the new kinds and their DSN/env-var shape.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🔭 🤖 Third-party marketplace deploy recipe
Order: 78

Specification: `froglet-services/services/marketplace-node` is architecturally a standalone Froglet service — it registers the marketplace Builtin handlers and runs the normal provider/runtime server. But there's no documented recipe for a third party to run their own marketplace instance. Write `ops/compose.third-party.yaml` (or equivalent) + `docs/THIRD_PARTY_MARKETPLACE.md` that explains: which env vars the operator must set, which config (stake parameters, fee schedule, identity) is theirs to choose vs. must stay compatible with the Froglet marketplace contract, and how to verify that a self-run marketplace accepts deals from a vanilla Froglet node pointed at it via `FROGLET_MARKETPLACE_URL`.

Definition of done: docs exist; smoke test runs a self-hosted marketplace against a vanilla Froglet and verifies the end-to-end flow.

Execution: 🤖 LLM-doable docs + ops. Lives primarily in `froglet-services` repo.

### ⬜ 🔭 🤖 Marketplace builtin-service wrappers as first-class MCP actions
Order: 79

Specification: Today `marketplace.register`, `marketplace.search`, `marketplace.provider`, `marketplace.receipts`, `marketplace.stake`, and `marketplace.topup` are reachable via the generic `invoke_service` MCP action, but require the LLM to know the exact service_id string and the precise argument shape for each one. Add ergonomic wrapper actions — `marketplace_register`, `marketplace_search`, `marketplace_stake`, `marketplace_topup` — that accept friendlier parameter shapes and construct the `invoke_service` call underneath. Parallels the shape of the settlement-visibility wrappers added in Order-equivalent (this cycle's item B).

Definition of done: 4+ new MCP actions land in `integrations/mcp/froglet/lib/tools.js` with corresponding handlers and tests; MCP tool description explains when to use each; the `invoke_service` escape hatch stays available.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🔭 🤖 Marketplace arbiter / claims-court service
Order: 80

Specification: Build a first-class enforcement service on the marketplace so cheating can be adjudicated and punished without baking policy into the kernel. The service lives alongside `marketplace-node` in `froglet-services/services/marketplace-arbiter` and introduces four handler groups:

- **Complaint filing.** `marketplace.file_complaint` accepts a signed grievance referencing a `deal_id`, the signed `Receipt` (or absence thereof), optional evidence blobs (signed-artifact references, content hashes), and a **refundable filing deposit** in sats. Deposit is refunded in full if the complaint is upheld, forfeited to the respondent or the arbiter-pool if dismissed. Deposit sizing scales with the disputed value so grief-filing is not viable.
- **Adjudicator registration.** `marketplace.register_adjudicator` lets any identity post an **adjudicator stake** and opt in to dispute panels. Stake is slashable for (a) liveness failures (no verdict within the adjudication TTL), (b) verdicts overturned on appeal, (c) adjudicating a case where a conflict-of-interest declaration is missing or false. Above a configurable value threshold, adjudicator eligibility requires at least one verified identity attestation (see Order 81), which is the sybil-resistance lever.
- **Adjudication.** `marketplace.adjudicate` issues a signed verdict from the adjudicator's identity key. Above the de-minimis threshold, panels are drawn by random selection from the eligible-adjudicator pool, weighted by stake, with a majority decision. Below the threshold, a single adjudicator verdict is acceptable to keep small-dispute latency low.
- **Appeal and slashing.** `marketplace.appeal` escalates a disputed verdict to a larger panel at a higher deposit tier. Upheld verdicts trigger slashing via the existing `marketplace.stake` primitive; slashed funds flow to the complainant (reimbursement) plus a portion to the arbiter pool.

Kernel impact: none. Every new artifact is a signed payload indexed by the marketplace indexer; the `marketplace.stake` slashing primitive already exists. The kernel stays storage-free.

Hard problem this does not fully solve: capture of the adjudicator pool by a well-capitalized sybil attacker below the attestation threshold. Documented explicitly — the deposit + stake + random-panel + appeal stack raises the cost but does not reduce it to zero. Cross-reference: Order 81 (identity attestation) is what lifts the ceiling for high-value disputes.

Definition of done: the four handler groups ship with Postgres-backed projections in `marketplace-arbiter`, MCP wrappers for `file_complaint` / `register_adjudicator` / `adjudicate` / `appeal` land in this repo's `integrations/mcp/froglet/lib/tools.js`, a full dispute lifecycle test (happy path + dismissed complaint + overturned appeal + adjudicator liveness slash) runs in CI, and [docs/ARBITER.md](docs/ARBITER.md) documents the mechanism design, deposit-sizing table, and threat model.

Execution: 🤖 Entirely LLM-doable. Economic parameters (deposit tiers, stake floors, fee split) are explicit inputs — a human picks them, the LLM implements against them.

### 🟡 🔭 🤖 Identity attestation service handlers (DNS + OAuth/OIDC)
Order: 81

Specification: Add optional, opt-in identity attestation handlers so a provider (or requester or adjudicator) can bind their Froglet identity key to a real-world identifier. Two attestation kinds, both LLM-doable, both with concrete flows already specified in [docs/IDENTITY_ATTESTATION.md](docs/IDENTITY_ATTESTATION.md):

- **DNS attestation.** The subject signs a bind statement with their Froglet key, publishes it in a `_froglet.<domain>` TXT record, and calls `marketplace.attest_dns`. The marketplace resolver fetches the record over DNS-over-HTTPS (to avoid operator-local resolver manipulation), verifies the signature and the key match, and issues a marketplace-signed `IdentityAttestation` credential valid for 180 days. Re-verification runs on expiry; transferred or dropped domains invalidate the credential automatically.
- **OAuth / OIDC attestation.** The subject signs the same bind statement, posts it at a URL whose authorship OAuth can prove (a GitHub gist, a profile README, a repo file, a release), then calls `marketplace.attest_oauth` with the URL plus an OAuth authorization code for the matching provider. The marketplace exchanges the code, verifies the authenticated user owns the URL, verifies the posted signature matches the Froglet pubkey, and issues the attestation. GitHub is the first target; Google OIDC, GitLab, and Gitea follow the same pattern without protocol changes.

The protocol stays identity-agnostic — attestations are a marketplace-layer projection, never mandatory, never gating deal execution at the kernel level. Consumers filter by attestation kind in `marketplace.search` results; the arbiter service (Order 80) requires attestations for adjudicator eligibility at high-value tiers.

Definition of done: two handlers land in `marketplace-node` (or a new `marketplace-attestation` service if the DNS/OAuth surface grows too wide), the `IdentityAttestation` credential type is defined in `froglet-protocol` with roundtrip tests, MCP wrappers `marketplace_attest_dns` and `marketplace_attest_oauth` land in `integrations/mcp/froglet/lib/tools.js`, `marketplace.search` and `marketplace.provider` projections expose any attestations a provider holds, and the doc in this repo documents the full flow.

Status (2026-04-19, partial): protocol-crate scaffolding landed in [froglet-protocol/src/protocol/identity_attestation.rs](froglet-protocol/src/protocol/identity_attestation.rs). Types: `IdentityAttestationPayload`, `IdentityAttestationKind { Dns, Oauth }`, `IdentityAttestationClaim` (tagged union), `IdentityAttestationEvidenceRef` (tagged union). Validator `validate_identity_attestation_artifact` enforces `signer == payload.issuer`, non-empty fields, and agreement between `attestation_kind` / `attestation_claim` / `evidence_ref` variants. 8 tests green: DNS + OAuth sign→verify→validate roundtrips, signer/issuer mismatch rejection, kind/claim mismatch rejection, empty-field rejection, wrong-artifact-type rejection, serde roundtrips for both variants. Remaining 🟡: marketplace service handlers (`marketplace.attest_dns`, `marketplace.attest_oauth`), MCP wrappers, and `marketplace.search` projection — those live in `froglet-services` and wait on that repo. GitHub OAuth app registration (human step, B6) is the external prerequisite for OAuth handler verification.

Execution: 🤖 Entirely LLM-doable. OAuth flow needs a GitHub OAuth app to exist (human action: register one, share client id + secret with the marketplace service); everything else is pure code + DoH resolution.

## Zero-Cost Launch Channels

### ⬜ 🚀 🧑 Build the launch distribution matrix
Order: 31a

Specification: Before publishing any individual launch post, produce one distribution plan that lists every target channel in priority order, the tailored copy for each, the post timing, and the per-channel rules check (HN guidelines, Reddit subreddit rules, Lobsters self-promo ratio, Product Hunt vote-solicitation rules, each community's norms). The individual per-channel tasks below (blog, HN, Reddit, X/LinkedIn, communities, Dev.to, Product Hunt, Indie Hackers, Lobsters, arXiv) execute against this plan rather than being planned ad hoc.

Definition of done: A single distribution matrix document exists, lists each target channel with tailored copy, timing, and rules check, and every individual channel post below references it as its source.

Execution: 🧑 Manual-heavy. The LLM can produce the draft matrix and per-channel copy variants; a human owns final channel selection and posting identity.

### ⬜ 🚀 🧑 Publish the launch post on your own blog
Order: 31

Specification: Write the canonical launch narrative on a property you control. It should explain the problem, the product, why now, what is open, how hosted access works, what early users should try first, and what kind of feedback is wanted.

Definition of done: The blog post is live, links to the hosted product, docs, and GitHub release, and can be reused as the source of truth for all other launch copy.

Execution: 🧑 Manual-heavy. The LLM can draft the post, but a human has to choose tone, publish on the owned platform, and stand behind the claims.

### ⬜ 🚀 🧑 Post a Show HN launch
Order: 32

Specification: Write a technical Show HN submission that stands on its own merits, uses a factual title, links to the primary source, and invites technical discussion rather than promotional voting. The post should focus on what is novel and what hackers can try immediately.

Definition of done: The HN post is live with a factual title, working link, and prepared answers for the first wave of technical questions and criticism.

Execution: 🧑 Manual-heavy. The LLM can draft the post and likely comments, but a human must submit and respond as a real participant.

### ⬜ 🚀 🧑 Publish targeted Reddit launch posts
Order: 33

Specification: Prepare subreddit-specific posts for a short list of communities where Froglet is genuinely relevant. Each post should match the subreddit’s norms, avoid generic cross-posting language, and point to the most appropriate artifact for that audience.

Definition of done: A reviewed list of target subreddits exists, each with custom copy and rule checks, and the posts are published without vote solicitation or obvious spam patterns.

Execution: 🧑 Manual-heavy. The LLM can draft the variants and subreddit matrix, but a human must choose communities, post under a real account, and handle moderation feedback.

### ⬜ 🔭 🧑 Launch on Product Hunt
Order: 37

Specification: Prepare a Product Hunt listing only after the core launch artifacts are ready. The listing should use a maker account, clear screenshots, a strong tagline, a short demo-oriented description, and a support plan for launch-day comments.

Definition of done: The Product Hunt page is published with complete content, working links, launch-day comment coverage, and no vote-begging language.

Execution: 🧑 Manual-heavy. The LLM can draft assets and copy, but a human must publish from a personal maker account.

### ⬜ 🔭 🧑 Post on Indie Hackers
Order: 38

Specification: Write an Indie Hackers launch thread or build-in-public post that emphasizes lessons learned, open problems, user feedback, and launch metrics rather than a dry product announcement. This works best as a founder narrative, not as copied release notes.

Definition of done: An Indie Hackers post is live, linked back to the canonical launch assets, and written in a way that fits IH discussion norms.

Execution: 🧑 Manual-heavy. The LLM can draft the narrative, but a human has to publish and engage authentically.

### ⬜ 🔭 🧑 Post to Lobsters if the technical angle is strong enough
Order: 39

Specification: Only pursue Lobsters if there is a genuinely technical artifact to discuss, such as the protocol model, distributed testing, MCP architecture, or payment semantics. A generic product announcement should be skipped.

Definition of done: Either a narrowly technical Lobsters post is published and defended in comments, or this channel is explicitly skipped as a poor fit.

Execution: 🧑 Manual-heavy. The LLM can draft the technical framing, but a human should decide whether the post genuinely fits Lobsters.

### ⬜ 🔭 🧑 Publish a Dev.to technical deep dive
Order: 36

Specification: Write a technical post that teaches something concrete, such as how Froglet bridges MCP, hosted runtime, and payment flows, rather than just repeating the release announcement. This should create searchable long-tail value.

Definition of done: A Dev.to post is live, technically useful on its own, and links back to the docs and release.

Execution: 🧑 Manual-heavy. The LLM can draft it; a human must publish and own the author voice.

### ⬜ 🔭 🧑 Publish X and LinkedIn threads
Order: 34

Specification: Prepare a short thread version of the launch for broader social distribution, optimized for screenshots, demo clips, and one clear CTA back to the canonical release assets.

Definition of done: The threads are published with working links, no overclaiming, and replies are monitored for the first response window.

Execution: 🧑 Manual-heavy. The LLM can draft the threads, but a human must publish them from real accounts.

### ⬜ 🔭 🧑 Share in relevant Discord, Slack, forum, and community channels
Order: 35

Specification: Build a short list of communities where Froglet is welcome and context matters, such as AI engineering groups, open-source infra communities, or protocol-specific groups. Every post should be adapted to the local channel and should disclose affiliation where appropriate.

Definition of done: There is a vetted list of communities plus tailored copy for each, and any published posts respect community rules and moderation norms.

Execution: 🧑 Manual-heavy. The LLM can prepare the matrix and copy, but a human must use real memberships and relationships.

### ⬜ 🔭 🧑 Submit an arXiv paper or note if there is real research content
Order: 40

Specification: Only pursue arXiv if Froglet has a paper-worthy artifact, such as a protocol design note, benchmark methodology, or scientifically framed systems result. This should not be treated as a launch directory.

Definition of done: A real paper or note exists, meets arXiv submission requirements, is accepted or under review, and is worth citing independently of the product launch.

Execution: 🧑 Manual-heavy. The LLM can help draft, but a human author has to decide whether the work genuinely belongs on arXiv and handle submission identity requirements.

## Suggested Next Actions

### ✅ 🚀 🤖 Draft the marketplace extraction proposal
Order: 01

Specification: Start by writing a reviewable marketplace split proposal so implementation can proceed without re-deciding the boundary mid-edit. The draft should propose what moves out, what stays public, what public interfaces must remain stable after the split, and which questions still require human approval.

Definition of done: One short draft decision document or issue comment exists, is reviewable, and clearly distinguishes proposed boundary decisions from unresolved questions.

Execution: 🤖 Entirely LLM-doable.

### ✅ 🚀 🤖 Prepare the release-candidate checklist and hosted smoke script skeleton
Order: 05

Specification: Convert the validated checks that already exist into a reusable release gate and stub the missing hosted checks so the remaining gaps are visible and finite.

Definition of done: The release gate file exists and shows completed versus missing validations.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Close the Claude auth blocker and rerun the live MCP smoke
Order: 11

Specification: Supply valid Claude auth, keep the compose or hosted stack up, and rerun the same project-config MCP smoke that already works through Codex.

Definition of done: Claude host validation is either green or precisely classified.

Fallback: if Claude auth remains blocked by 2026-05-15, ship with Claude host validation explicitly classified as pending. The release notes and launch post must link to the specific auth or account blocker so the gap is visible and not hidden. This prevents a single external dependency from blocking the launch indefinitely.

Execution: 🤝 Mixed.

### ⬜ 🚀 🤝 Stand up the docs host and the first public Froglet environment
Order: 20

Specification: Once the boundary and release gate are clear, publish the docs site and one hosted runtime so the launch content can point to real URLs.

Definition of done: Public URLs exist for docs and the hosted product, and both are smoke-verified.

Execution: 🤝 Mixed.
