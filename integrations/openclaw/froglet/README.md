# Froglet OpenClaw Plugin

Public OpenClaw integration for Froglet.

This plugin stays on the public boundary:

- marketplace discovery via `GET /v1/marketplace/search`
- marketplace node lookup via `GET /v1/marketplace/nodes/:node_id`
- provider surface reads via `GET /v1/descriptor` and `GET /v1/offers`
- optional local runtime helpers for buy, wait, payment-intent,
  accept-result, and publish-services when `enablePrivilegedRuntimeTools=true`

Closed marketplace/catalog/broker logic does not belong in this package. Keep
that as a separate private plugin/package even if it still consumes public
Froglet APIs.

Starter config:

- [examples/openclaw.config.example.json](examples/openclaw.config.example.json)

Local verification:

```bash
node --check index.js
node --test test/plugin.test.js
python3 -m py_compile bridge.py
```

See [../../../docs/OPENCLAW.md](../../../docs/OPENCLAW.md) for installation and
configuration.
