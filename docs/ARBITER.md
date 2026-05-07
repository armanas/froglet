# Marketplace Arbiter

Froglet's MVP marketplace arbiter is a separate service, not a kernel feature.
It gives the public marketplace an operator-run complaint and suspension path
without changing signed Froglet artifacts or deal execution rules.

## MVP Contract

Public endpoints:

```http
GET /healthz
POST /v1/complaints
GET /v1/complaints/:complaint_id
```

Admin endpoint:

```http
POST /v1/admin/complaints/:complaint_id/verdict
Authorization: Bearer <MARKETPLACE_ARBITER_ADMIN_TOKEN>
```

Complaint request:

```json
{
  "provider_id": "provider-id",
  "deal_id": "deal-id",
  "receipt_hash": "optional-receipt-artifact-hash",
  "complainant_id": "optional-requester-id",
  "reason": "short human-readable grievance",
  "evidence": []
}
```

Admin verdict request:

```json
{
  "verdict": "upheld",
  "remedy": "suspend_provider",
  "reason": "operator rationale",
  "operator_id": "operator"
}
```

Allowed verdicts are `upheld` and `dismissed`. Allowed remedies are `none`,
`warning`, and `suspend_provider`. Dismissed complaints must use `remedy=none`.

## Enforcement

`suspend_provider` writes a provider enforcement record in the marketplace
database. The public marketplace read API filters suspended providers from:

- provider list and provider detail
- offer list and offer detail
- public stats and provider receipt reads

The enforcement record remains in the database for auditability.

## Boundaries

The MVP arbiter is operator-adjudicated. It is not decentralized, stake-backed,
panel-selected, or slashable. Deposits, public adjudicator registration,
appeals, panel selection, and slashing are post-MVP hardening.

The kernel remains unchanged. Arbiter state is marketplace policy and must not
be described as a protocol-level validity rule.
