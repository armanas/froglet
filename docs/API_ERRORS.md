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
