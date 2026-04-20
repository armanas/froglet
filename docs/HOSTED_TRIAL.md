# Hosted Trial Contract

Public contract for the hosted `try.froglet.dev` entry point.

## Public role

This repo documents the public hosted-trial contract and keeps the launch story
simple:

- `Try In Cloud`
- `Run Locally`

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

## Reader expectation

The hosted trial is a first-party convenience entry point built on Froglet. The
public docs here define the user flow and API contract; the self-host path
remains the default way to understand and run Froglet locally.
