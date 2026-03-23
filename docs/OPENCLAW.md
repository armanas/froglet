# OpenClaw

Froglet’s public OpenClaw plugin is runtime-only.

OpenClaw talks to a local `froglet-runtime`. The runtime talks to remote provider and discovery services. OpenClaw does not call provider or discovery directly.
The same Froglet plugin contract is also used by NemoClaw; only the deployment
profile changes.

## Supported Profiles

| Profile | Runtime placement | This page |
| --- | --- | --- |
| `openclaw-local` | local host runtime | baseline covered here |
| `nemoclaw-local-runtime` | runtime inside the sandbox | see [NEMOCLAW.md](NEMOCLAW.md) |
| `nemoclaw-hosted-runtime` | runtime on the consumer host over HTTPS | see [NEMOCLAW.md](NEMOCLAW.md) |

## Tools

- `froglet_search`
- `froglet_get_provider`
- `froglet_events_query`
- `froglet_buy`
- `froglet_payment_intent`
- `froglet_mock_pay`
- `froglet_wait_deal`
- `froglet_accept_result`
- `froglet_wallet_balance`

## Config

Start from [../integrations/openclaw/froglet/examples/openclaw.config.example.json](../integrations/openclaw/froglet/examples/openclaw.config.example.json).
This checked-in JSON file is a complete user-edited config, not a fragment.

Required plugin config:

```json
{
  "plugins": {
    "entries": {
      "froglet": {
        "enabled": true,
        "config": {
          "runtimeUrl": "http://127.0.0.1:8081",
          "runtimeAuthTokenPath": "/absolute/path/to/froglet/data/runtime/auth.token"
        }
      }
    }
  }
}
```

## Local Stack

Run the local three-role stack:

```bash
docker compose up --build
```

Then verify the runtime:

```bash
TOKEN=$(cat ./data/runtime/auth.token)
curl -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8081/v1/runtime/wallet/balance
```

## Typical Flow

1. `froglet_search`
2. `froglet_get_provider`
3. `froglet_buy`
4. `froglet_payment_intent`
5. `froglet_mock_pay` when the returned intent exposes a mock action
6. `froglet_wait_deal` with `wait_statuses=["result_ready","succeeded","failed","rejected"]` for accept flows
7. `froglet_accept_result`

For the standard `execute.wasm` flow, use this minimal buy request:

```json
{
  "request": {
    "provider": { "provider_id": "provider-1" },
    "offer_id": "execute.wasm",
    "submission": { "wasm_module_hex": "<valid_wasm_module_hex>" }
  }
}
```

`wasm_module_hex` must be a valid hex-encoded Wasm module, not just arbitrary hex bytes.

## Verification

```bash
node --check integrations/openclaw/froglet/index.js
node --test integrations/openclaw/froglet/test/plugin.test.js \
  integrations/openclaw/froglet/test/config-profiles.test.mjs \
  integrations/openclaw/froglet/test/doctor.test.mjs
```

Optional Froglet-owned config validation after you replace the placeholder paths:

```bash
node integrations/openclaw/froglet/scripts/doctor.mjs \
  --config /absolute/path/to/openclaw.config.json \
  --target openclaw
```

For NemoClaw, see [NEMOCLAW.md](NEMOCLAW.md).
