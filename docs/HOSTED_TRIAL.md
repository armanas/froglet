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
- free-only scope (demo seed services publish at 0 sats)
- anonymous — no email, no persistence, no account claim

## Public endpoints

```text
POST /api/sessions          mint a session token from the pool
GET  /v1/*                  normal Froglet runtime+provider surface;
                            requires `Authorization: Bearer <session-token>`
GET  /llms.txt              machine-readable bootstrap for LLM clients
GET  /.well-known/mcp.json  MCP manifest for native MCP hosts
```

The session token is authentication only. Every signed artifact the node
produces in response to session-driven requests is signed by the node's own
identity, not a per-session key. The trial is a shared demo surface, not a
per-user cryptographic isolation boundary — this is why the scope is
free-only.

Session tokens expire after 15 minutes and their pool slots recycle for a
new user. Two consecutive users of the same slot share no signing identity
and no cryptographic identity, but they may share a slot_id. Receipts do
not uniquely identify a session; they identify the hosted node.

## Reader expectation

The hosted trial is a first-party convenience entry point built on Froglet. The
public docs here define the user flow and API contract; the self-host path
remains the default way to understand and run Froglet locally — and the only
path for non-trial, persistent identity, paid deals, and service publication.
