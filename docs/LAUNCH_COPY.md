# Froglet Launch Copy

Status: pre-publication draft.

This file is for manual posting only. Do not automate posting. Do not ask for
upvotes, likes, reposts, comments, saves, or coordinated engagement. Do not ask
friends, coworkers, communities, private groups, or mailing lists to "support"
the launch. Share the project once per channel where it is appropriate, answer
questions honestly, and let the ranking systems work normally.

## Shared Positioning

Froglet is a signed-artifact protocol for agents to discover services, form
deals, execute work, and receive signed receipts.

Current limits to keep in every discussion:

- Hosted trial: free `demo.add` only.
- Hosted paid rails: v0.2, not v0.1.0.
- TEE/confidential execution: experimental.
- Marketplace: future/layered direction Froglet could support, not a current
  claim of millions of live services.

## Publication Gate

Do not publish any external post until this list is complete:

- Latest GitHub Actions run for the launch-prep branch is green.
- Hosted node reports the public release version. Verified on 2026-04-24 after
  Lightsail deployment 9:
  `https://ai.froglet.dev/v1/node/capabilities` reported `version=0.1.0`.
- `https://froglet.dev/`, `https://froglet.dev/learn/quickstart/`,
  `https://ai.froglet.dev/health`, `https://try.froglet.dev/llms.txt`, and
  `https://marketplace.froglet.dev/healthz` return 200.
- A fresh hosted demo returns `status=succeeded`, `sum=12`, and a receipt.
- Public status page or equivalent monitoring URL is live and recorded below.
- Final LLM prompt is run through at least one LLM host and produces a useful,
  honest explanation of observed statuses, result, mismatches, proved/not-proved
  boundaries, and one next experiment.
- No copy claims hosted paid rails, production TEE, persistent hosted identity,
  general hosted compute, or a large live marketplace.

Status page URL:

```text
https://froglet.dev/status/

This is a public deploy-time status snapshot. Confirm it returns 200 after the
docs deploy before posting.
```

LLM prompt test:

```text
Claude Code 2.1.119, 2026-04-24: passed the final hosted-demo prompt, fetched
/llms.txt, minted a session, ran demo.add, observed sum=12 plus a receipt, and
produced a useful proved/not-proved assessment. It also found the hosted
version drift before deployment 9; that drift has since been cleared.
```

## Show HN

Rules source:

- Show HN guidelines: <https://news.ycombinator.com/showhn.html>
- HN submission/comment rules: <https://news.ycombinator.com/newsguidelines.html>

Posting status: approved only after the publication gate above is complete.

Title:

```text
Show HN: Froglet - signed deals and receipts for agent services
```

URL:

```text
https://github.com/armanas/froglet
```

First comment:

```text
I built Froglet, a signed-artifact protocol for agents to discover services,
form deals, execute work, and receive signed receipts.

The kernel is a small evidence chain: descriptor -> offer -> quote -> deal ->
optional invoice bundle -> receipt. Each artifact is signed and hash-linked.
Discovery, marketplace ranking, payment adapters, reputation, policy, and
transport sit around that kernel rather than inside it.

The repo includes a Rust reference node, MCP and OpenClaw/NemoClaw integrations,
local/self-hosted payment adapters for Lightning, Stripe, and x402, and docs for
running locally.

The hosted trial is intentionally constrained: try.froglet.dev proves one free
demo.add discover -> deal -> result -> receipt round-trip. It is not a hosted
paid-rails launch, not persistent hosted identity, and not general hosted
compute. Hosted paid rails are v0.2. TEE/confidential execution is experimental.

The larger direction is a marketplace layer where agents can publish and consume
small services, data-backed tools, and bounded compute offers using the same
signed deal and receipt flow. For this release, the concrete thing to inspect is
the protocol, node, integrations, and the small hosted proof.

I would especially like feedback on the artifact model, what should be in the
kernel versus adapters, and whether the receipt chain is enough for practical
agent-to-agent service calls.
```

## Reddit Variants

Rules sources:

- Reddiquette: <https://support.reddithelp.com/hc/en-us/articles/205926439-Reddiquette>
- r/rust rules: <https://www.reddit.com/r/rust/wiki/rules/>

Global Reddit rule for this launch: if the current community sidebar/wiki
conflicts with this file, follow the community rule and mark the subreddit
`modmail pending` or `do not post`.

Status vocabulary:

- `approved`: OK for manual posting after the launch checklist is green. This
  is internal launch approval, not a claim of moderator pre-approval.
- `modmail pending`: Do not post until moderators approve or rules clearly allow
  the submission.
- `do not post`: Do not submit for this launch.

### r/rust - modmail pending

Title:

```text
Froglet: a Rust reference node for signed agent-service deals and receipts
```

Body:

```text
I am launching Froglet, a signed-artifact protocol and Rust reference node for
agent-to-agent services.

The kernel is intentionally small: descriptor -> offer -> quote -> deal ->
optional invoice bundle -> receipt. Each artifact is signed and hash-linked.
The Rust node can act as provider, runtime, or both, and the repo includes tests,
MCP/OpenClaw integrations, and local/self-hosted payment adapters.

Important scope limits: the hosted trial is free demo.add only, hosted paid rails
are v0.2, and TEE/confidential execution is experimental. The marketplace story
is future/layered infrastructure Froglet could support, not a claim that a large
live marketplace exists today.

Repo: https://github.com/armanas/froglet
```

### r/selfhosted - modmail pending

Title:

```text
Froglet: self-hostable signed deals and receipts for agent services
```

Body:

```text
Froglet is a self-hostable protocol and node for agents to discover services,
form signed deals, execute work, and keep signed receipts.

The public hosted trial only proves a free demo.add flow. The more interesting
path for this subreddit is local/self-hosted: run the Froglet node, publish or
invoke services, and configure local/self-hosted payment adapters if you want to
experiment with Lightning, Stripe, or x402.

Scope limits: hosted paid rails are v0.2, TEE/confidential execution is
experimental, and marketplace ranking/reputation are higher-layer work rather
than a current large live marketplace claim.

Repo: https://github.com/armanas/froglet
Docs: https://froglet.dev
```

### r/opensource - modmail pending

Title:

```text
Froglet: open source signed-artifact protocol for agent service calls
```

Body:

```text
I am preparing the public launch of Froglet, an open source signed-artifact
protocol for agent service calls.

It gives agents a signed chain for discovery, quote/deal formation, execution,
and receipts. The repo includes the Rust reference node, protocol docs,
conformance material, MCP and OpenClaw/NemoClaw integrations, and local
self-hosted payment adapters.

The hosted trial is deliberately small: free demo.add only. Hosted paid rails
are v0.2. TEE/confidential execution is experimental. The larger marketplace
direction is something the protocol could support, not something I am claiming
already exists at scale.

Repo: https://github.com/armanas/froglet
```

### r/LocalLLaMA - modmail pending

Title:

```text
Froglet: signed receipts for agent-to-agent tool and service calls
```

Body:

```text
I am looking for feedback from people building local agents and tool-using LLM
systems.

Froglet is a protocol and node for agents to discover services, form signed
deals, execute work, and receive signed receipts. It is not a model-serving
framework; it is the service/deal/receipt layer around agent calls.

The repo includes an MCP server and OpenClaw/NemoClaw integration. The hosted
trial is a free demo.add proof only. Local/self-hosted nodes are the path for
publishing or invoking your own services. Hosted paid rails are v0.2, and
TEE/confidential execution is experimental.

Repo: https://github.com/armanas/froglet
```

### r/programming - modmail pending

Title:

```text
Froglet: signed artifacts for agent service calls
```

Body:

```text
Froglet is a signed-artifact protocol for agents to discover services, form
deals, execute work, and receive signed receipts.

The core chain is descriptor -> offer -> quote -> deal -> optional invoice
bundle -> receipt. The implementation is a Rust node with MCP and
OpenClaw/NemoClaw integrations. Payment rails are adapter-level: local and
self-hosted Lightning, Stripe, and x402 are in the repo, while hosted paid rails
are v0.2.

The hosted trial is intentionally small: free demo.add only. TEE/confidential
execution is experimental. Marketplace language should be read as the future
layer this could support, not as a claim that a large live marketplace exists.

Repo: https://github.com/armanas/froglet
```

### r/MachineLearning - do not post

Reason: Froglet is infrastructure for agent service calls, not a research paper
or ML result. Do not post unless there is a paper, benchmark, or moderator
approval for an infrastructure discussion.

### r/Bitcoin - do not post

Reason: Lightning is one supported local/self-hosted adapter, but the launch is
not primarily a Bitcoin project and hosted paid Lightning is explicitly v0.2.

### r/cryptocurrency - do not post

Reason: The protocol uses signed artifacts and has payment adapters, but the
launch is not a token, chain, or speculative asset announcement.

## X Thread

Posting status: approved only after the publication gate above is complete.

Post 1:

```text
I built Froglet: a signed-artifact protocol for agents to discover services,
form deals, execute work, and receive signed receipts.

The goal is a small evidence chain for agent-to-agent service calls.
```

Post 2:

```text
The kernel is:

descriptor -> offer -> quote -> deal -> optional invoice bundle -> receipt

Each artifact is signed and hash-linked. Discovery, marketplaces, payment rails,
reputation, policy, and transport live around the kernel.
```

Post 3:

```text
The public repo includes a Rust reference node, MCP server,
OpenClaw/NemoClaw integration, conformance docs, local/self-hosted payment
adapters for Lightning/Stripe/x402, and self-host docs.
```

Post 4:

```text
The hosted trial is deliberately narrow: try.froglet.dev proves one free
demo.add discover -> deal -> result -> receipt round-trip.

It is not hosted paid rails, persistent hosted identity, or general hosted
compute.
```

Post 5:

```text
The bigger direction is a marketplace layer where agents could publish and
consume small services, data-backed tools, and bounded compute offers through
the same signed deal and receipt flow.

For v0.1.0, the concrete claim is narrower.
```

Post 6:

```text
Current limits:

- hosted trial is free demo.add only
- hosted paid rails are v0.2
- TEE/confidential execution is experimental
- no claim of a large live marketplace today
```

Post 7:

```text
Repo: https://github.com/armanas/froglet
Docs: https://froglet.dev
Hosted trial: https://try.froglet.dev

Feedback I want most: what belongs in the signed kernel, and what should stay
as an adapter or marketplace layer?
```

## LinkedIn Post

Posting status: approved only after the publication gate above is complete.

```text
I am launching Froglet, a signed-artifact protocol for agents to discover
services, form deals, execute work, and receive signed receipts.

The motivation is straightforward: as agents start calling services operated by
other agents or independent providers, a plain HTTP response is not enough. You
need a portable record of what was offered, what was quoted, what was accepted,
what payment terms applied, and what receipt the provider signed after the work
completed.

Froglet models that as a small signed chain: descriptor, offer, quote, deal,
optional invoice bundle, and receipt. The repo includes a Rust reference node,
MCP and OpenClaw/NemoClaw integrations, local/self-hosted payment adapters, and
docs for running locally.

The first hosted trial is intentionally constrained. It proves one free
demo.add discover -> deal -> result -> receipt flow. Hosted paid rails are v0.2,
and TEE/confidential execution is experimental.

The longer-term direction is a marketplace layer where agents could publish and
consume small services, data-backed tools, and bounded compute offers through
the same signed deal and receipt flow. For this release, the concrete launch is
the protocol, reference node, local integrations, and a small hosted proof.

Repo: https://github.com/armanas/froglet
Docs: https://froglet.dev
Hosted trial: https://try.froglet.dev
```

## Manual Posting Checklist

- Confirm the repo is public and the intended release/tag is visible.
- Confirm the public status page or monitoring URL is live.
- Confirm the final LLM prompt test is recorded above.
- Confirm latest launch-prep GitHub Actions run is green.
- Confirm `https://froglet.dev` and `https://try.froglet.dev` resolve.
- Run the hosted trial smoke and record the observed statuses, result, and
  receipt evidence before posting.
- Confirm the launch post still says hosted trial is free `demo.add` only.
- Confirm the launch post still says hosted paid rails are v0.2.
- Confirm the launch post still says TEE/confidential execution is
  experimental.
- Confirm no copy claims a large live marketplace, millions of services,
  production confidential compute, or hosted paid rails.
- Post Show HN once, using the exact title above.
- Add the Show HN first comment after the submission is live.
- Post only to Reddit communities marked `approved`, and only after checking
  current subreddit rules.
- For `modmail pending` communities, send modmail first and wait. Do not post
  while approval is unknown.
- Do not post to communities marked `do not post`.
- Do not ask for votes, likes, reposts, saves, comments, or coordinated
  engagement.
- Do not reuse identical Reddit text in rapid succession across many
  subreddits.
- Monitor replies for factual corrections and update docs/copy if a real
  discrepancy is found.
