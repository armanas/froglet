# Froglet OpenClaw Plugin

Public OpenClaw integration for the Froglet requester runtime.

This plugin is runtime-only. It does not call provider or discovery APIs directly.

## Tools

- `froglet_search`
- `froglet_get_provider`
- `froglet_buy`
- `froglet_payment_intent`
- `froglet_mock_pay`
- `froglet_wait_deal`
- `froglet_accept_result`
- `froglet_wallet_balance`

## Config

- [examples/openclaw.config.example.json](examples/openclaw.config.example.json)
- [examples/openclaw.config.nemoclaw.example.json](examples/openclaw.config.nemoclaw.example.json)

Required keys:

- `runtimeUrl`
- `runtimeAuthTokenPath`

## Verification

```bash
node --check index.js
node --test test/plugin.test.js
```

See [../../../docs/OPENCLAW.md](../../../docs/OPENCLAW.md).
