# Froglet Launch Narrative

Status: pre-publication draft.

Use this as the canonical long-form launch narrative for the public repo. Keep
claims tied to what v0.1.0 actually proves. Do not expand this into "millions of
services", "production confidential compute", or "hosted paid rails" language
unless the corresponding public evidence exists.

## Short Version

Froglet is a signed-artifact protocol for agents to discover services, form
deals, execute work, and receive signed receipts.

The first release is intentionally narrow: a Rust reference node, local and
self-hosted payment adapters, OpenClaw/NemoClaw and MCP integrations, and a
hosted trial that proves one free `demo.add` round-trip. The larger marketplace
idea is that many independent agents could publish and consume services through
the same signed deal and receipt flow, but v0.1.0 should be described as the
protocol and reference implementation that makes that possible, not as proof of
a large live marketplace.

## Main Narrative

Froglet starts from a simple premise: if agents are going to call each other's
tools, services, and compute, they need more than a plain HTTP request. They
need a way to know what was offered, what was agreed, what was executed, and
what the provider signed at the end.

Froglet models that flow as a signed evidence chain:

- a provider descriptor
- an offer
- a quote
- a deal
- an optional invoice bundle for paid deals
- a receipt

Each artifact is signed and hash-linked, so the result of a service call can be
audited after the fact. The kernel is deliberately small. Discovery,
marketplace ranking, payment adapters, policy, reputation, and transport choices
live above or beside the signed protocol instead of being baked into one
central service.

The public repo includes a Rust node that can act as provider, runtime, or both.
Agents can discover a service, ask for a quote, create a deal, execute the work,
and inspect the resulting receipt. The same node can be run locally or
self-hosted, with clearnet, Tor, Nostr-facing, MCP, and OpenClaw/NemoClaw
surfaces documented in the repo.

The first hosted trial is intentionally modest. `try.froglet.dev` lets a caller
mint a short-lived session token and run a free `demo.add` service. That proves
the hosted discover -> deal -> result -> receipt path for one constrained demo
service. It does not prove paid hosted settlement, persistent hosted identity,
open-ended hosted compute, or broad marketplace depth.

Paid rails are present as local and self-hosted adapters. Lightning, Stripe,
and x402 are part of the repo's launch surface for operators, but first-party
hosted paid rails are v0.2 work. The hosted v0.1.0 proof is free-only.

TEE and confidential execution material should also be described carefully.
Froglet has confidential/TEE-oriented artifacts and provider-facing primitives,
but TEE support is experimental in this release. It should not be marketed as
production confidential execution.

The future marketplace direction is larger: agents could publish small services,
data-backed tools, and bounded compute offers; other agents could discover them,
form signed deals, pay through adapter rails, and keep receipts for audit,
refunds, reputation, or policy. That is a direction Froglet is built to support.
For this launch, the accurate claim is narrower: Froglet provides the signed
protocol, reference node, local integrations, and a small hosted proof that the
basic loop works.

## What Ships Now

- Signed `froglet/v1` artifact kernel for descriptors, offers, quotes, deals,
  invoice bundles, and receipts.
- Rust reference node for provider/runtime use.
- Local and self-hosted execution paths for named services, data-backed
  services, and bounded compute.
- Local and self-hosted settlement adapters for Lightning, Stripe, and x402.
- Free `PaymentBackend::None` path for local and hosted demo flows.
- OpenClaw/NemoClaw plugin and MCP server.
- Self-host and local setup docs.
- Hosted trial at `try.froglet.dev` for a short-lived, free `demo.add` flow.

## Current Limits To State Plainly

- The hosted trial is free `demo.add` only.
- Hosted paid rails are deferred to v0.2.
- Hosted sessions are anonymous, short-lived, and not persistent identities.
- The hosted trial is not a general compute endpoint.
- TEE/confidential execution is experimental, not a production confidentiality
  claim.
- Marketplace, ranking, incentives, reputation, and broker policy are higher
  layers. Froglet can support those layers, but v0.1.0 is not a claim that a
  large live marketplace already exists.

## Suggested Long-Form Post

I built Froglet, a signed-artifact protocol for agents to discover services,
form deals, execute work, and receive signed receipts.

The core idea is that agent-to-agent services need an evidence trail. A normal
HTTP call can tell you that a request returned something. It does not give you a
portable chain of what the provider advertised, what was quoted, what the
requester accepted, what payment terms applied, and what receipt the provider
signed after execution.

Froglet's kernel is that chain: descriptor, offer, quote, deal, optional invoice
bundle, and receipt. Each artifact is signed and hash-linked. Discovery,
marketplaces, transport, payment rails, reputation, and policy sit around that
kernel instead of being hard-coded into it.

The repo includes a Rust node that can provide services, invoke services, or do
both. It has bot-facing integrations through MCP and OpenClaw/NemoClaw, local
and self-hosted payment adapters for Lightning, Stripe, and x402, and docs for
running locally.

There is also a hosted trial, but it is deliberately constrained:
`try.froglet.dev` proves one free `demo.add` discover -> deal -> result ->
receipt round-trip. It is not a hosted paid-rails launch, not a persistent
identity system, and not a general hosted compute surface.

The marketplace direction is where this gets more interesting. The same signed
deal and receipt flow could support agents publishing small services,
data-backed tools, or bounded compute offers that other agents can discover and
pay for. That is the direction Froglet is built toward. The v0.1.0 release is
the protocol, reference node, local integrations, and a small hosted proof of
the loop.

If you want to try it, start with the hosted demo to inspect the receipt, then
run the node locally if you want to publish or invoke your own services.

## Links For Posting

- Repo: `https://github.com/armanas/froglet`
- Docs: `https://froglet.dev`
- Status: `https://froglet.dev/status/`
- Hosted trial: `https://try.froglet.dev`
- Release: `https://github.com/armanas/froglet/releases/tag/v0.1.0`
- Demo evidence:
  `/Users/armanas/Projects/github.com/armanas/froglet-services/_tmp/post_deploy_verify/20260424T073839Z/hosted_smoke.log`
  and a 2026-04-24 spot-check returning a succeeded `demo.add` deal with
  `sum=12` plus a receipt.

## Publication Gate

Do not publish this post externally until all of the following are true:

- The latest GitHub Actions run for the launch-prep branch is green. The last
  observed red scheduled run failed only because the cloud-backed GCP rig had no
  GCP secrets; the workflow now needs to skip that job cleanly when secrets are
  absent.
- The hosted node reports the public release version. Verified on 2026-04-24
  after Lightsail deployment 9:
  `https://ai.froglet.dev/v1/node/capabilities` reported `version=0.1.0`.
- The public status page at `https://froglet.dev/status/` is live after the
  docs deploy and returns 200.
- The final hosted-demo prompt has been run through at least one LLM host.
  Claude Code `2.1.119` passed this on 2026-04-24 and found the hosted version
  drift before deployment 9; that drift has since been cleared.
- Any subreddit marked `modmail pending` in [LAUNCH_COPY.md](LAUNCH_COPY.md)
  has moderator approval before posting there.

## Claim Guardrails

Use:

- "could support a marketplace of agent services"
- "built toward independent providers and requesters"
- "local and self-hosted paid rails"
- "hosted paid rails are v0.2"
- "TEE/confidential execution is experimental"

Do not use:

- "millions of services"
- "production confidential compute"
- "hosted Lightning/Stripe/x402 is live"
- "general hosted compute"
- "persistent hosted identities"
- "the marketplace is already deep/liquid/permissionless at scale"
