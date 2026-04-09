# OpenClaw

OpenClaw uses the shared `froglet` plugin and exposes one bot tool: `froglet`.

## Deployment

OpenClaw runs the plugin on the host. The plugin talks to the local Froglet
provider API, usually:

- `http://127.0.0.1:8080` (provider)
- `http://127.0.0.1:8081` (runtime)

Use the checked-in example config:

- [integrations/openclaw/froglet/examples/openclaw.config.example.json](/Users/armanas/Projects/github.com/armanas/froglet/integrations/openclaw/froglet/examples/openclaw.config.example.json)

## What The Tool Does

`froglet` covers the full node workflow:

- discover and invoke remote named/data services
- inspect local services
- create/edit/build/test/publish local projects
- inspect status and logs
- restart managed node processes
- run expert raw compute through the direct compute offer

The default path is named services. `run_compute` is the low-level fallback and
should include `provider_id` or `provider_url`.
Named services, data services, and open-ended compute are all the same Froglet
primitive at the deal layer.

Current implementation note:

- the checked-in execution profiles are current reference implementations
- broader interpreted/container compute is part of the generic Froglet
  execution model, not a separate product line

Remote discovery is authoritative: `discover_services` should list remote
services through Froglet discovery rather than by direct peer guessing.

## Typical Flow

1. `froglet` with `action=discover_services`
2. `froglet` with `action=get_service`
3. `froglet` with `action=invoke_service`

Publishing from the same node:

1. `create_project`
2. `write_file`
3. `build_project`
4. `test_project`
5. `publish_project`

Useful defaults:

- `create_project` may use `name` instead of explicit ids
- `create_project` may use `result_json` for a simple fixed-response service
- `create_project` may use `inline_source` when you want to provide explicit
  authored source up front
- `create_project` auto-publishes only when `publication_state=active` and you
  provided `starter`, `result_json`, or `inline_source`
- blank projects are scaffolds only; use `publication_state=hidden` until you
  have written real source
- `publish_artifact` is the direct publication path for prebuilt Wasm modules
  and OCI-backed/container profiles
- local and remote service detail views expose `offer_kind` and `resource_kind`
  so models can distinguish a listed service binding from direct compute
- `invoke_service` can resolve a unique `service_id` automatically
- `run_compute` is the direct path for open-ended compute and currently supports
  inline Wasm, inline Python, OCI-backed Wasm, and OCI-backed python/container
  execution inputs; it targets an explicit provider rather than discovery by
  service id
- `summary` is metadata only; it does not generate service code

## Managed Launcher

For Froglet-managed OpenClaw hosts:

```bash
./integrations/openclaw/froglet/scripts/install-openclaw-launcher.sh
```

That makes plain `openclaw` open a local Froglet chat loop.
