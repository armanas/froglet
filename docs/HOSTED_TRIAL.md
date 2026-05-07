# Hosted Trial Contract

Public contract for the hosted `try.froglet.dev` entry point.

## Public role

This repo documents the public hosted-trial contract and keeps the launch story
simple:

- `Try In Cloud`
- `Run Locally`

## Hosted launch promise

- first-party hosted gateway in front of the reference node
- temporary session token with a 15-minute TTL drawn from a fixed-size pool
- free-only scope: `demo.add` is the canonical proof, and
  `demo.fetch-witness`, `demo.hash-verify`, and `demo.notarize` are optional
  higher-signal follow-ups
- anonymous — no email, no persistence, no account claim
- `try.froglet.dev` is the only public hosted-trial ingress
- `try.froglet.dev` stays free-only; separate hosted paid surfaces require
  live evidence before launch claims

## Prompt for an LLM

Use this when you want an LLM to run the hosted demo first and then judge
whether Froglet is useful for your actual environment.

```md
Read https://try.froglet.dev/llms.txt, follow the hosted demo flow exactly if you can access it, otherwise say only that you could not access it, then give me an honest, evidence-backed assessment that reports the observed HTTP statuses, observed service IDs, observed deal status, observed result, whether a receipt was present, and any mismatch between these docs and live behavior before explaining what Froglet just proved, what it did not prove, and the single most relevant next experiment for my files, tools, data, configuration, workflows, constraints, and goals.
```

If the model is chat-only, or its tools cannot fetch URLs, POST JSON, send
Bearer auth, and poll HTTP routes, it should exit gracefully: say that a proper
test requires an agentic HTTP-capable environment or the manual curl flow, and
do not claim live Froglet evidence.

## Installed MCP boundary

The installed `froglet-mcp` package is local/actionable-first. It targets a
local or self-hosted provider/runtime and no longer exposes a hosted demo
action. Use this page or `https://froglet.dev/llms.txt` for the no-install
hosted proof; use MCP when the user wants to run, publish, settle, or inspect
real Froglet work through a configured node.

## Public endpoints

```text
GET  /api/preflight               no-auth capability and scope check for agent clients
POST /api/sessions                mint a session token from the pool
GET  /v1/provider/services        list the hosted demo services
GET  /v1/provider/services/:id    inspect a hosted demo service
GET  /v1/feed                     inspect the signed artifact envelope emitted by the hosted node
POST /v1/runtime/deals            create a hosted demo deal;
                                  requires `Authorization: Bearer <session-token>`
GET  /v1/runtime/deals/:deal_id   poll a hosted demo deal;
                                  requires `Authorization: Bearer <session-token>`
GET  /llms.txt                    machine-readable bootstrap for LLM clients
GET  /.well-known/mcp.json        MCP manifest for native MCP hosts
```

Session tokens are not general runtime credentials. They do not authorize
runtime search, provider-detail lookup, wallet or settlement routes,
Lightning accept/payment-intent endpoints, or legacy `/v1/node/*`
compute/job endpoints.

Authorized scope is intentionally narrow: call only `https://try.froglet.dev`
and only the public trial paths listed above. Do not scan, fuzz, enumerate
unrelated paths, attack third-party hosts, or use arbitrary user-supplied URLs
unless the user owns or controls them.

`GET /v1/feed` returns a Froglet artifact envelope. Treat entries as signed
artifacts such as provider descriptors, offers, and receipts. Do not interpret
the response as an events stream, an `items` collection, or a generic activity
feed. Do not expect the feed to contain the opaque runtime `deal_id`; receipt
artifacts reference signed `deal_hash` and `quote_hash` values instead.

`ai.froglet.dev` is the worker's upstream origin, not a second public trial
entry point. Direct public requests to `ai.froglet.dev/api/sessions`,
`ai.froglet.dev/api/sessions/validate`, `ai.froglet.dev/v1/runtime/deals`, and
`ai.froglet.dev/v1/runtime/deals/{deal_id}` are outside the hosted-trial
contract and should return `404`.

## Canonical hosted demo

The current free hosted catalog has five demo services:

| Service | Role |
| --- | --- |
| `demo.add` | Canonical proof. Adds `{ "a": 7, "b": 5 }` and returns `{ "sum": 12 }` with a receipt. |
| `demo.echo` | Simple round-trip check that returns caller input unchanged. |
| `demo.fetch-witness` | Optional stronger follow-up. Fetches a URL and signs observed status, hash, type, length, and timestamp. |
| `demo.hash-verify` | Optional stronger follow-up. Fetches a URL and signs whether the live SHA-256 matches an expected hash. |
| `demo.notarize` | Optional stronger follow-up. Signs a caller-supplied content hash with a timestamp. |

Only `demo.*` services are part of the public hosted proof. Other service IDs
may appear on the reference node, but they are outside this public hosted trial
contract and should not be used to judge the hosted proof.

```bash
curl -sS https://try.froglet.dev/api/preflight | jq .
TOKEN=$(curl -sS -X POST https://try.froglet.dev/api/sessions | jq -r .session_token)
PROVIDER_ID=$(curl -sS -H "Authorization: Bearer $TOKEN" \
  https://try.froglet.dev/v1/provider/services \
  | jq -r '.services[] | select(.service_id=="demo.add") | .provider_id')
CREATE=$(curl -sS -H "Authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -X POST https://try.froglet.dev/v1/runtime/deals \
  --data "{\"provider\":{\"provider_id\":\"$PROVIDER_ID\",\"provider_url\":\"https://ai.froglet.dev\"},\"offer_id\":\"demo.add\",\"kind\":\"execution\",\"execution\":{\"schema_version\":\"froglet/v1\",\"workload_kind\":\"demo.add\",\"runtime\":\"builtin\",\"package_kind\":\"builtin\",\"entrypoint\":{\"kind\":\"builtin\",\"value\":\"demo.add\"},\"contract_version\":\"froglet.builtin.demo.add.v1\",\"input_format\":\"application/json+jcs\",\"input_hash\":\"728a671a0a05e573bb0c3e37688fc3302d913187cb274f2e0b2940e1c2e4b719\",\"requested_access\":[],\"security\":{\"mode\":\"standard\"},\"mounts\":[],\"input\":{\"a\":7,\"b\":5},\"builtin_name\":\"demo.add\"}}")
DEAL_ID=$(printf '%s' "$CREATE" | jq -r '.deal.deal_id')
for _ in 1 2 3; do
  RESULT=$(curl -sS -H "Authorization: Bearer $TOKEN" \
    "https://try.froglet.dev/v1/runtime/deals/$DEAL_ID")
  STATUS=$(printf '%s' "$RESULT" | jq -r '.deal.status')
  [ "$STATUS" = "succeeded" ] && break
  sleep 1
done
printf '%s\n' "$RESULT"
curl -sS -H "Authorization: Bearer $TOKEN" https://try.froglet.dev/v1/feed
```

Expected demo outcome:

- preflight returns `200` and confirms the client needs HTTPS GET, HTTPS POST
  with JSON bodies, custom Bearer auth headers, and polling
- create response returns `200` with a deal record; the initial `deal.status` may already be `accepted`, `running`, or `succeeded`
- follow-up `GET /v1/runtime/deals/{deal_id}` returns `200` with `deal.status = "succeeded"`
- the succeeded result includes `{ "sum": 12 }`
- the succeeded deal includes a `receipt`
- `/v1/feed` contains signed artifact envelopes, including receipts that
  reference signed `deal_hash` / `quote_hash` values; it is not keyed by the
  runtime `deal_id`

The session token is authentication only. Every signed artifact the node
produces in response to session-driven requests is signed by the node's own
identity, not a per-session key. The trial is a shared demo surface, not a
per-user cryptographic isolation boundary — this is why the scope is
free-only.

Session tokens expire after 15 minutes and their pool slots recycle for a
new user. Two consecutive users of the same slot share no signing identity
and no cryptographic identity, but they may share a slot_id. Receipts do
not uniquely identify a session; they identify the hosted node.

## What this proves

- the hosted node can mint a shared 15-minute session token
- the live service catalog is reachable
- the hosted catalog exposes all five free demo services
- the canonical free `demo.add` discover -> deal -> sync-result -> receipt
  round-trip works end to end
- optional `demo.fetch-witness`, `demo.hash-verify`, and `demo.notarize`
  follow-ups can produce stronger evidence for URL observation, reproducibility,
  or timestamped content-hash notarization when the caller supplies suitable inputs

## What this does not prove

- paid settlement rails
- persistent identity or account recovery
- service publication or current marketplace depth; an open marketplace of independently operated services is future potential, not part of the hosted proof
- long-running, batch, or GPU workloads

## Failure taxonomy for LLMs

- If `GET /llms.txt` fails, report the host or document as unreachable.
- If `GET /api/preflight` works but the client cannot POST JSON, send Bearer
  auth, or poll, report a client/tool limitation, not a Froglet outage.
- If `GET /api/sessions` returns `404`, report wrong method; session minting is
  `POST /api/sessions`.
- If `/v1/*` returns `401` without a Bearer token, report missing session auth,
  not runtime failure.
- If an agent shell reports an egress allowlist error such as `host_not_allowed`,
  report sandbox/network policy, not Froglet runtime status.
- If `POST /api/sessions` returns `503`, report session pool exhaustion and
  suggest retrying later.
- If a deal returns `failed`, `rejected`, or times out, report that exact live
  status and body.

## Privacy posture

The hosted trial has a zero-product-analytics posture: no account, email,
analytics cookie, or conversion tracking is part of the public trial contract.
The service still may emit minimal edge/origin operational logs needed to run
and abuse-protect the gateway.

`POST /v1/runtime/deals/{deal_id}/accept` is reserved for Lightning settlement
flows. Free hosted demo proofs complete through `GET /v1/runtime/deals/{deal_id}`.

## Reader expectation

The hosted trial is a first-party convenience entry point built on Froglet. The
public docs here define the user flow and API contract; the self-host path
remains the default way to understand and run Froglet locally — and the only
path for non-trial, persistent identity, paid deals, and service publication.
An open marketplace of independently operated services is separate from the
hosted-trial proof. First-party hosted paid rails must not be claimed live until
Lightning and Stripe have public payment transcripts.

## LLM handoff after the proof

An agent should not jump from the hosted proof straight into local install. It
should first report evidence, explain the boundary for the user's actual
context, then ask for the local profile:

- target agent: `claude-code`, `codex`, or `manual` for no-clone setup;
  `openclaw` only when the user accepts source/plugin mode
- footprint: `binary`, `docker`, or `source`
- role: `consumer`, `provider`, or `both`
- payment rail: `none`, `lightning-lnd-rest`, `stripe-test`,
  `stripe-live`, or `x402`
- network mode: `clearnet`, `tor`, or `dual`
- marketplace URL: none, `https://marketplace.froglet.dev`, or custom
- first use case: consume, publish, witness, hash-verify, notarize, paid
  compute, or prepare batch/GPU work later

If the agent has Froglet MCP/OpenClaw tools, it should call `plan_install`
before `get_install_guide`. `plan_install` returns remaining questions,
prerequisites, required secrets, command preview, validation checks, and
post-install playbooks. `get_install_guide` is for confirmed command execution
through the host shell. A disposable-host proof of the default quickstart is
available with
`curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/fresh_host_quickstart_smoke.sh | bash`.
After local health is verified, call `plan_use_case`
before implementing consumer, provider, evidence, payments, batch, or GPU
workflows. Batch starts with the existing async task status primitives but
multi-item fan-out remains Order 44 work. GPU requires provider-advertised
capability and real hardware evidence. Self-hosted GPU capability
advertisement, generic-compute offer metadata, Docker `--gpus all` gating,
no-CPU-fallback errors, and one GCP T4 container workload with a signed receipt
are verified. Provider scheduling policy, marketplace routing, and production
GPU capacity/selection docs remain Order 45 work.

If the LLM is only a web-chat interface with no HTTP, shell, or MCP access, it
must not pretend to test Froglet. It should say it cannot run the hosted proof
from that interface, point the user to an agentic environment or the curl block
above, and offer to interpret pasted outputs.
