# Hosted Trial Contract

Public contract for the separate hosted `try.froglet.dev` product.

## Boundary

The hosted trial is **not** implemented in this repo. This repo documents the
contract and keeps the public launch story strict:

- `Try In Cloud`
- `Run Locally`

The hosted implementation belongs in a separate private repo because it needs:

- rate limiting and abuse controls
- TTL cleanup
- audit logging
- uptime and error monitoring
- email delivery
- operator runbooks
- human-account lifecycle and identity recovery flows

## Hosted launch promise

- shared GCP-hosted gateway
- temporary identity with a 15-minute TTL
- free-only launch scope
- optional email claim to convert the temporary identity into long-term access

## Public endpoints

```text
POST /api/sessions
POST /api/sessions/claim
POST /api/sessions/verify
POST /api/sessions/resume
```

Session-scoped discovery, quote, deal, and execute routes wrap Froglet flows
without exposing raw private-key management to the user.

## Why this stays separate

The local public repo should remain focused on self-host install, agent
integration, payment-rail onboarding, and release integrity. The hosted trial
has a different operational and security boundary, so it should ship from its
own private codebase.
