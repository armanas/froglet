# OpenClaw

Froglet ships a public OpenClaw plugin at
[../integrations/openclaw/froglet](../integrations/openclaw/froglet).

The plugin is intentionally read-only and stays on Froglet's public boundary:

- marketplace discovery via `GET /v1/marketplace/search`
- marketplace node detail via `GET /v1/marketplace/nodes/:node_id`
- provider descriptor and offers via `GET /v1/descriptor` and `GET /v1/offers`

It does not call the privileged runtime surface and it does not require the
runtime auth token.

## Tools

The plugin registers three optional tools:

- `froglet_marketplace_search`
- `froglet_marketplace_node`
- `froglet_provider_surface`

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

`marketplaceUrl` is optional. If it is omitted, the marketplace tools require a
per-call `marketplace_url` argument, while `froglet_provider_surface` can still
read a provider directly with `provider_url`.

## Usage Notes

- `froglet_marketplace_search` is recency-ordered discovery, not keyword search.
- `froglet_marketplace_node` returns the raw marketplace record in addition to a
  compact summary when `include_raw` is set.
- `froglet_provider_surface` returns the signed descriptor plus current offers
  from the node's public API, with raw JSON available through `include_raw`.
- The checked-in example config already enables the plugin for a `main` agent
  and points at the default local marketplace URL.

## Example Prompts

- `List the newest active Froglet marketplace nodes.`
- `Fetch the marketplace record for node_id <id>.`
- `Read the provider descriptor and offers from http://127.0.0.1:8080.`
