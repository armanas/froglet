# Froglet OpenClaw Plugin

Public OpenClaw integration for the Froglet requester runtime.

This plugin is runtime-only. It does not call provider or discovery APIs directly.
OpenClaw and NemoClaw both use this same Froglet plugin contract; only the
deployment profile changes.

## Supported Profiles

| Profile | Runtime placement | Froglet-owned config differences |
| --- | --- | --- |
| `openclaw-local` | local host runtime | host plugin path, local runtime URL, local token path |
| `nemoclaw-local-runtime` | runtime inside the sandbox | sandbox plugin path, sandbox-local runtime URL, sandbox token path |
| `nemoclaw-hosted-runtime` | runtime on the consumer host over HTTPS | sandbox plugin path, hosted runtime URL, sandbox token path |

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

Minimal `execute.wasm` buy request:

```json
{
  "request": {
    "provider": { "provider_id": "provider-1" },
    "offer_id": "execute.wasm",
    "submission": { "wasm_module_hex": "<valid_wasm_module_hex>" }
  }
}
```

For accept flows, call `froglet_wait_deal` with `wait_statuses` including `result_ready`; the default wait behavior only stops on terminal statuses.

## Config

- [examples/openclaw.config.example.json](examples/openclaw.config.example.json)
- [examples/openclaw.config.nemoclaw.example.json](examples/openclaw.config.nemoclaw.example.json)
- [examples/openclaw.config.nemoclaw.hosted.example.json](examples/openclaw.config.nemoclaw.hosted.example.json)

These are complete user-edited JSON configs, not rendered fragments.

Required keys:

- `runtimeUrl`
- `runtimeAuthTokenPath`

## Verification

```bash
node --check index.js
node --test test/plugin.test.js test/config-profiles.test.mjs test/doctor.test.mjs
```

Optional config validation:

```bash
node scripts/doctor.mjs --config /absolute/path/to/openclaw.config.json --target openclaw
node scripts/doctor.mjs --config /absolute/path/to/openclaw.config.nemoclaw.json --target nemoclaw
```

See [../../../docs/OPENCLAW.md](../../../docs/OPENCLAW.md).
