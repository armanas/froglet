# API Error Reference

All Froglet API errors are returned as JSON with an `error` field.

## HTTP Status Codes

| Code | Meaning | When |
|------|---------|------|
| 400 | Bad Request | Invalid input, missing fields, hash mismatch, schema violation |
| 401 | Unauthorized | Missing or invalid Bearer token |
| 402 | Payment Required | Priced endpoint called without a valid deal |
| 404 | Not Found | Deal, offer, or resource does not exist |
| 409 | Conflict | Duplicate submission or state conflict |
| 429 | Too Many Requests | Rate limit exceeded |
| 500 | Internal Error | Server-side failure (details logged, not exposed) |
| 504 | Gateway Timeout | Request exceeded the configured timeout |

## Common Errors

### Timeout (504)

```json
{ "error": "request timed out" }
```

The operation exceeded the route timeout. Provider routes default to 10s,
runtime routes to 65s. For WASM execution, the timeout is controlled by
`FROGLET_EXECUTION_TIMEOUT_SECS` (default: 10, max: 300).

### Authentication (401)

```json
{ "error": "unauthorized" }
```

The endpoint requires a Bearer token. Pass the token from the auth token file
in the `Authorization: Bearer <token>` header.

### Payment Required (402)

```json
{ "error": "this endpoint requires a protocol deal", "price_sats": 10 }
```

The provider charges for this service. Create a deal through the
`/v1/provider/quotes` and `/v1/provider/deals` flow first.

### Requester Spend Policy Refusals (402)

`POST /v1/runtime/deals` refuses paid deals that violate the node's local
spend policy. These carry a stable `code` field:

```json
{ "error": "paid deals are disabled: no requester spend budget is configured; …",
  "code": "spend_budget_unconfigured", "quoted_total_msat": 30000 }
```

```json
{ "error": "deal price 30000 msat exceeds the per-deal spend cap (FROGLET_REQUESTER_MAX_DEAL_MSAT=29000)",
  "code": "spend_cap_exceeded", "quoted_total_msat": 30000, "max_deal_msat": 29000,
  "provider_id": "…" }
```

```json
{ "error": "cumulative spend budget exhausted: 950000 of 1000000 msat committed; this deal needs 250000 msat. Raise FROGLET_REQUESTER_SPEND_BUDGET_MSAT or POST /v1/runtime/spend/reset.",
  "code": "spend_budget_exceeded", "quoted_total_msat": 250000,
  "spend_budget_msat": 1000000, "spent_msat": 950000, "remaining_msat": 50000 }
```

Fail-closed by design: paid deals require `FROGLET_REQUESTER_SPEND_BUDGET_MSAT`
to be set. Free deals are unaffected. Inspect current totals with
`GET /v1/runtime/spend`; archive committed spend (restoring headroom) with
`POST /v1/runtime/spend/reset`. See `CONFIGURATION.md`.

### Invalid Submission (400)

```json
{ "error": "module hash does not match module bytes" }
```

WASM submission integrity check failed. Ensure `module_hash` is the SHA-256
of the raw module bytes, and `input_hash` is the SHA-256 of the canonical
JSON input.

### Deal Not Found (404)

```json
{ "error": "deal not found", "deal_id": "abc123..." }
```

The requested deal does not exist on this node. Verify the deal ID and that
you are querying the correct provider.

### Quote Already Used (409)

```json
{ "error": "quote already used by a different deal" }
```

Provider-side quotes are single-use for distinct accepted executions. Exact
replay of the same deal payload, or replay through the same idempotency key,
returns the existing deal; a different deal against the same unexpired quote is
rejected.

### Internal Error (500)

```json
{ "error": "internal error" }
```

A server-side failure occurred. Details are logged server-side but not exposed
to prevent information leakage. Check the node logs for the full error.

## WASM Execution Errors

| Error | Cause |
|-------|-------|
| `wasm module too large` | Module exceeds 512 KB hex-encoded limit |
| `wasm input too large` | Input exceeds size limit |
| `unsupported wasm abi_version` | ABI must be `froglet.wasm.run_json.v1` or `froglet.wasm.host_json.v1` |
| `Wasm concurrency limit reached` | All execution slots are in use. Retry after a short delay |
| `Wasm module output size limit exceeded` | Module output exceeds 128 KB |
| `Wasm fuel exhausted` | Execution exceeded 50M fuel units (computation limit) |

## Settlement Errors

| Error | Cause |
|-------|-------|
| `invoice is expired` | The Lightning invoice has expired |
| `preimage does not match payment hash` | Released preimage failed verification |
| `deal admission deadline exceeded` | Deal was not accepted within the admission window |
