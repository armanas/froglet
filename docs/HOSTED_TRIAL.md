# Hosted Trial Contract

Public contract for the hosted `try.froglet.dev` entry point.

## Public role

This repo documents the public hosted-trial contract and keeps the launch story
simple:

- `Try In Cloud`
- `Run Locally`

## Hosted launch promise

- shared Cloudflare-fronted gateway in front of a Lightsail container service
- temporary session token with a 15-minute TTL drawn from a fixed-size pool
- free-only scope: the public proof is the zero-sat `demo.add` round-trip
- anonymous — no email, no persistence, no account claim
- `try.froglet.dev` is the only public hosted-trial ingress
- first-party hosted Lightning, Stripe, and x402 paid rails are deferred to v0.2

## Prompt for an LLM

Use this when you want an LLM to run the hosted demo first and then judge
whether Froglet is useful for your actual environment.

```md
Read https://try.froglet.dev/llms.txt, follow the hosted demo flow exactly if you can access it, otherwise say only that you could not, then give me an honest, objective assessment that reports the observed HTTP statuses, observed result, and any mismatches between the docs and the live behavior before explaining what Froglet just proved, what it did not prove, and the single most relevant next experiment for my files, tools, data, configuration, workflows, constraints, and goals.
```

## Public endpoints

```text
POST /api/sessions                mint a session token from the pool
GET  /v1/provider/services        list the hosted demo services
GET  /v1/provider/services/:id    inspect a hosted demo service
GET  /v1/feed                     inspect the signed artifacts emitted by the hosted node
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

`ai.froglet.dev` is the worker's upstream origin, not a second public trial
entry point. Direct public requests to `ai.froglet.dev/api/sessions`,
`ai.froglet.dev/api/sessions/validate`, `ai.froglet.dev/v1/runtime/deals`, and
`ai.froglet.dev/v1/runtime/deals/{deal_id}` are outside the hosted-trial
contract and should return `404`. The worker presents an internal
`X-Froglet-Hosted-Trial-Secret` header when it reaches those upstream routes.

## Canonical hosted demo

```bash
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

- create response returns `200` with a deal record; the initial `deal.status` may already be `accepted`, `running`, or `succeeded`
- follow-up `GET /v1/runtime/deals/{deal_id}` returns `200` with `deal.status = "succeeded"`
- the succeeded result includes `{ "sum": 12 }`
- the succeeded deal includes a `receipt`

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
- one free `demo.add` discover -> deal -> sync-result -> receipt round-trip works end to end

## What this does not prove

- paid settlement rails
- persistent identity or account recovery
- service publication or marketplace depth
- long-running, batch, or GPU workloads

## Privacy posture

The v0.1.0 hosted trial has a zero-product-analytics posture: no account,
email, analytics cookie, or conversion tracking is part of the public trial
contract. The service still may emit minimal edge/origin operational logs
needed to run and abuse-protect the gateway.

`POST /v1/runtime/deals/{deal_id}/accept` is reserved for Lightning settlement
flows. The free hosted `demo.add` proof completes through
`GET /v1/runtime/deals/{deal_id}`.

## Reader expectation

The hosted trial is a first-party convenience entry point built on Froglet. The
public docs here define the user flow and API contract; the self-host path
remains the default way to understand and run Froglet locally — and the only
path for non-trial, persistent identity, paid deals, and service publication.
First-party hosted paid rails are deferred to v0.2.
