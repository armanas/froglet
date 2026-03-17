# OpenClaw

Froglet ships a public OpenClaw plugin at
[../integrations/openclaw/froglet](../integrations/openclaw/froglet).

The package stays on the public Froglet/OpenClaw boundary. It includes:

- marketplace discovery via `GET /v1/marketplace/search`
- marketplace node detail via `GET /v1/marketplace/nodes/:node_id`
- provider descriptor and offers via `GET /v1/descriptor` and `GET /v1/offers`
- optional local runtime helpers for `buy`, `wait`, `payment-intent`,
  `accept-result`, and `publish-services` when privileged tools are explicitly
  enabled

The public plugin is not the place for closed marketplace/catalog/broker logic.
If you later add first-party marketplace product integrations, ship them as a
separate private plugin/package that still consumes public Froglet interfaces.

## Tools

Default tools:

- `froglet_marketplace_search`
- `froglet_marketplace_node`
- `froglet_provider_surface`

Optional privileged tools (`enablePrivilegedRuntimeTools=true`):

- `froglet_runtime_buy`
- `froglet_runtime_wait_deal`
- `froglet_runtime_payment_intent`
- `froglet_runtime_accept_result`
- `froglet_runtime_publish_services`

## Local Setup

Start the local Froglet stack first:

```bash
docker compose up --build
```

That gives you:

- provider API on `http://127.0.0.1:8080`
- reference marketplace on `http://127.0.0.1:9090`

Start from the checked-in example config at
[../integrations/openclaw/froglet/examples/openclaw.config.example.json](../integrations/openclaw/froglet/examples/openclaw.config.example.json)
and replace `/absolute/path/to/froglet` with your local checkout root.

The important part is that `plugins.load.paths` points at the plugin directory:

```json
{
  "plugins": {
    "load": {
      "paths": [
        "/absolute/path/to/froglet/integrations/openclaw/froglet"
      ]
    }
  }
}
```

Read-only mode is the default. This example keeps privileged runtime tools
disabled:

```json
{
  "plugins": {
    "entries": {
      "froglet": {
        "enabled": true,
        "config": {
          "marketplaceUrl": "http://127.0.0.1:9090",
          "enablePrivilegedRuntimeTools": false
        }
      }
    }
  }
}
```

To enable local runtime buy/publish helpers, add:

```json
{
  "plugins": {
    "entries": {
      "froglet": {
        "config": {
          "enablePrivilegedRuntimeTools": true,
          "providerUrl": "http://127.0.0.1:8080",
          "runtimeUrl": "http://127.0.0.1:8081",
          "runtimeAuthTokenPath": "/absolute/path/to/froglet/data/runtime/auth.token",
          "pythonExecutable": "python3"
        }
      }
    }
  }
}
```

Privileged runtime tools use a small public Python bridge that wraps
[`../python/froglet_client.py`](../python/froglet_client.py), so the OpenClaw
host needs `python3` plus the Froglet Python dependencies available.

## Usage Notes

- `froglet_marketplace_search` is recency-ordered discovery, not keyword search.
- `froglet_marketplace_node` returns the raw marketplace record in addition to a
  compact summary when `include_raw` is set.
- `froglet_provider_surface` returns the signed descriptor plus current offers
  from the node's public API, with raw JSON available through `include_raw`.
- privileged runtime tools are opt-in and never register unless
  `enablePrivilegedRuntimeTools` is `true`
- `froglet_runtime_buy` stores local per-deal release state beside the runtime
  auth token under `openclaw-froglet/`, so `froglet_runtime_accept_result` only
  needs the `deal_id`
- `froglet_runtime_wait_deal` defaults to waiting for `result_ready`,
  `succeeded`, `failed`, or `rejected`
- the checked-in example config already enables the plugin for a `main` agent
  and keeps privileged runtime tools disabled by default

## Example Prompts

- `List the newest active Froglet marketplace nodes.`
- `Fetch the marketplace record for node_id <id>.`
- `Read the provider descriptor and offers from http://127.0.0.1:8080.`
- `Buy the execute.wasm offer through my local Froglet runtime.`
- `Wait for deal_id <id> until it reaches result_ready.`
- `Publish my current provider surface through the local runtime.`
