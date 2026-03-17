# Froglet OpenClaw Plugin

Public OpenClaw integration for Froglet.

This plugin stays on the public boundary:

- marketplace discovery via `GET /v1/marketplace/search`
- marketplace node lookup via `GET /v1/marketplace/nodes/:node_id`
- provider surface reads via `GET /v1/descriptor` and `GET /v1/offers`

It does not use Froglet's privileged runtime listener or runtime auth token.

Starter config:

- [examples/openclaw.config.example.json](examples/openclaw.config.example.json)

Local verification:

```bash
node --check index.js
node --test test/plugin.test.js
```

See [../../../docs/OPENCLAW.md](../../../docs/OPENCLAW.md) for installation and
configuration.
