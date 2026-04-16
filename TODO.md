# Froglet TODO

As of 2026-04-14, this file tracks the release backlog for taking Froglet from verified local/distributed testing to a public hosted launch.

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
2. Publish Docker artifacts and stand up the hosted docs site plus the hosted Froglet environment.
3. Make hosted verification repeatable: health, MCP, payment, and rollback checks.
4. Close the remaining MVP host-validation gap on Claude.
5. Prepare the release packet: changelog, screenshots, demo flow, pricing/payment notes, launch FAQs.
6. Launch from owned channels first, then distribute to community channels that fit the audience and rules.

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

Execution: 🤝 Mixed. The LLM can write and test the deploy scripts, but a human must provide GCP credentials, service accounts, and any final production approvals.

### ⬜ 🔭 🤝 Automate AWS deploys
Order: 42

Specification: Add a second deploy target only after GCP is stable. For new AWS work, default to ECS/Fargate or EC2-based automation rather than App Runner. The goal is portability and operator confidence, not cloud symmetry for its own sake.

Definition of done: There is one reproducible AWS deployment path with environment bootstrap, secrets wiring, health checks, and rollback guidance, and it is documented as a secondary target rather than the primary launch path.

Execution: 🤝 Mixed. The LLM can build the automation, but a human must provide AWS account access, IAM approval, and cost ownership.

### ⬜ 🚀 🤖 Add hosted verification scripts for docs and Froglet
Order: 16

Specification: Create repeatable post-deploy smoke checks that hit docs routes, health endpoints, one MCP flow, and one public runtime flow. The scripts should fail loudly, produce machine-readable output, and be runnable both locally and in CI.

Definition of done: A single verification entrypoint can be run after deploy and clearly reports pass/fail for docs, health, runtime, and MCP coverage.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Add monitoring, alerting, and rollback runbooks
Order: 17

Specification: Define the minimum hosted operations layer: logs, uptime checks, alert routing, deployment history, and rollback procedures. This should stay intentionally lightweight, but it has to exist before public launch.

Definition of done: An operator can detect a broken deploy, identify the failing component, and roll back to the previous known-good state without improvising.

Execution: 🤝 Mixed. The LLM can prepare runbooks and configs, but a human must connect real alert destinations and decide the on-call path.

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

### ⬜ 🚀 🤝 Expand the payment verification matrix
Order: 25

Specification: Turn payments into an explicit matrix instead of scattered one-off tests. The matrix should cover local/regtest, hosted sandbox, failure injection, restart recovery, and observability expectations per supported rail.

Definition of done: There is a documented table of supported payment rails and a repeatable test for each promised mode, with unsupported cells called out explicitly.

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

### ⬜ 🚀 🤝 Build the public website or startup front
Order: 27

Specification: Define the non-docs public-facing site or front page that explains Froglet in one pass for early adopters. This should link cleanly to docs, hosted Froglet, GitHub, and launch posts, and it should make the hosted versus self-hosted distinction obvious.

Definition of done: There is a public landing/front page with working navigation, crisp product framing, demo or screenshots, and clear calls to the hosted version and technical docs.

Execution: 🤝 Mixed. The LLM can build the site, but a human should approve positioning, branding, domain, and any customer-facing claims.

### ⬜ 🚀 🤖 Add an operator deployment and verification guide
Order: 26

Specification: Write one operator-focused guide that covers image selection, tokens, compose or cloud deployment, hosted smoke checks, payment verification, and rollback. This should be the document followed during release week.

Definition of done: A new operator can deploy and verify the stack from the guide without relying on tribal knowledge or old chat logs.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Create the full release plan
Order: 29

Specification: Build a dated release plan covering code freeze, release candidate validation, image publishing, docs cutover, hosted cutover, launch content prep, launch day posting order, and post-launch monitoring. The plan should separate blockers from stretch goals.

Definition of done: There is one release plan with owners, order, required inputs, go/no-go criteria, and a launch-day checklist.

Execution: 🤝 Mixed. The LLM can draft the plan, but a human has to approve dates, owners, and public commitments.

### ⬜ 🚀 🤖 Create the release-candidate gate
Order: 28

Specification: Turn the pre-launch bar into a named release gate that combines strict checks, docs-site build/tests, compose smoke, MCP smokes, and hosted smoke scripts into one checklist or automation entrypoint.

Definition of done: A candidate release can be marked pass or fail from one place, and every line item has an evidence artifact or log.

Execution: 🤖 Entirely LLM-doable.

### ⬜ 🚀 🤝 Publish the GitHub release and changelog
Order: 30

Specification: Prepare a real release note set that explains what Froglet is, what changed, how to run it, what is hosted versus self-hosted, and what remains intentionally out of scope. This is the authoritative launch artifact that other channels should point back to.

Definition of done: A tagged release exists with a changelog, install paths, image references, docs links, and known limitations, and it matches the actual shipped artifacts.

Execution: 🤝 Mixed. The LLM can draft and assemble the release, but a human typically owns the final publish action and version choice.

## Zero-Cost Launch Channels

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

Specification: Write a technical post that teaches something concrete, such as how Froglet bridges MCP, hosted runtime, and payment flows, rather than just repeating release marketing. This should create searchable long-tail value.

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

Execution: 🤝 Mixed.

### ⬜ 🚀 🤝 Stand up the docs host and the first public Froglet environment
Order: 20

Specification: Once the boundary and release gate are clear, publish the docs site and one hosted runtime so the launch content can point to real URLs.

Definition of done: Public URLs exist for docs and the hosted product, and both are smoke-verified.

Execution: 🤝 Mixed.
