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

## Choose a Mode

Use one of these three paths:

- read-only: quickest way to inspect marketplace and provider state
- full runtime, direct: easiest host-process setup for buy, wait, and accept
- full runtime, Docker: easiest containerized setup for buy, wait, and accept

All full-runtime paths below use the same host URLs and token path:

- provider API: `http://127.0.0.1:8080`
- runtime API: `http://127.0.0.1:8081`
- marketplace: `http://127.0.0.1:9090`
- runtime token: `/absolute/path/to/froglet/data/runtime/auth.token`

That means the same OpenClaw config works for both direct and Docker full
runtime mode.

## Read-Only Setup

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

## Full Runtime Setup

### Prerequisites

Privileged runtime tools use the public Python bridge at
[../integrations/openclaw/froglet/bridge.py](../integrations/openclaw/froglet/bridge.py),
which wraps [../python/froglet_client.py](../python/froglet_client.py).

Install the required Python packages on the machine running OpenClaw:

```bash
python3 -m pip install aiohttp ecdsa
```

Start from the checked-in full-runtime config at
[../integrations/openclaw/froglet/examples/openclaw.config.full-runtime.example.json](../integrations/openclaw/froglet/examples/openclaw.config.full-runtime.example.json)
and replace `/absolute/path/to/froglet` with your local checkout root.

This config enables the privileged tools and points OpenClaw at the standard
host-local URLs and token path:

```json
{
  "plugins": {
    "entries": {
      "froglet": {
        "config": {
          "enablePrivilegedRuntimeTools": true,
          "marketplaceUrl": "http://127.0.0.1:9090",
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

### Full Runtime, Direct

Start the marketplace:

```bash
cargo run --bin marketplace
```

Start Froglet in another terminal:

```bash
FROGLET_DISCOVERY_MODE=marketplace \
FROGLET_MARKETPLACE_URL=http://127.0.0.1:9090 \
FROGLET_MARKETPLACE_PUBLISH=true \
FROGLET_PRICE_EXEC_WASM=10 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet
```

This writes the runtime auth token to `./data/runtime/auth.token`, which matches
the full-runtime OpenClaw example config.

### Full Runtime, Docker

Use the dedicated Compose file:

```bash
mkdir -p ./data
docker compose -f compose.full-runtime.yaml up --build
```

Unlike the starter [../compose.yaml](../compose.yaml), this full-runtime file:

- publishes the privileged runtime on `127.0.0.1:8081`
- bind-mounts `./data` so the token is visible on the host
- opts into non-loopback runtime binding only inside the container so the host port can reach it
- keeps the same host URLs and token path as the direct process flow

That means you can use the same
[../integrations/openclaw/froglet/examples/openclaw.config.full-runtime.example.json](../integrations/openclaw/froglet/examples/openclaw.config.full-runtime.example.json)
file for both direct and Docker full-runtime mode.

### Sanity Check

Before starting OpenClaw, verify that the runtime is reachable and that the
token path is correct:

```bash
TOKEN=$(cat /absolute/path/to/froglet/data/runtime/auth.token)
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8081/v1/runtime/wallet/balance
```

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
- the checked-in read-only example config already enables the plugin for a
  `main` agent and keeps privileged runtime tools disabled by default

## Example Prompts

- `List the newest active Froglet marketplace nodes.`
- `Fetch the marketplace record for node_id <id>.`
- `Read the provider descriptor and offers from http://127.0.0.1:8080.`
- `Buy the execute.wasm offer through my local Froglet runtime.`
- `Wait for deal_id <id> until it reaches result_ready.`
- `Show the payment intent for deal_id <id>.`
- `Accept the result for deal_id <id>.`
- `Publish my current provider surface through the local runtime.`
