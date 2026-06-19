# OpenClaw

OpenClaw uses the shared `froglet` plugin and exposes one bot tool: `froglet`.

## Deployment

OpenClaw runs the plugin on the host. The plugin talks to the local Froglet
provider API, usually:

- `http://127.0.0.1:8080` (provider)
- `http://127.0.0.1:8081` (runtime)

Current OpenClaw releases require Node.js `22.14.0` or newer for plugin
install/inspect and gateway-mediated invocation. Use Node 22 before running
`npx openclaw@2026.5.5 ...`.

For Gateway-mediated local actions, keep the Froglet token paths available in
the Gateway process environment:

```bash
export FROGLET_PROVIDER_AUTH_TOKEN_PATH="$PWD/data/runtime/froglet-control.token"
export FROGLET_RUNTIME_AUTH_TOKEN_PATH="$PWD/data/runtime/auth.token"
```

Use the checked-in example config:

- [../integrations/openclaw/froglet/examples/openclaw.config.example.json](../integrations/openclaw/froglet/examples/openclaw.config.example.json)

## What The Tool Does

`froglet` covers the full node workflow:

- discover and invoke remote named/data services
- inspect and publish local services
- inspect status, tasks, and settlement state
- use marketplace-native wrappers for search, provider details, receipts,
  domain claims, registration, and arbiter complaints
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

Agent-grade publication goes through `marketplace_publish`: the tool shells out
to `froglet-node publish`, builds the service, signs it through the local
provider daemon, registers it, and verifies marketplace projection. The
lower-level `publish_artifact` action remains available for local publication of
prebuilt Wasm modules, inline-source services, and OCI-backed/container profiles.

Current API note:

- the public OpenClaw surface does not currently expose project authoring,
  log tailing, or node restart actions
- local and remote service detail views expose `offer_kind` and `resource_kind`
  so models can distinguish a listed service binding from direct compute
- `invoke_service` can resolve a unique `service_id` automatically
- `run_compute` is the direct path for open-ended compute and currently supports
  inline Wasm, inline Python, OCI-backed Wasm, and OCI-backed python/container
  execution inputs; it targets an explicit provider rather than discovery by
  service id
- `summary` is metadata only; it does not generate service code

## Install Check

OpenClaw blocks plugins that include executable launcher wrappers or
`child_process` helpers inside the plugin directory. The Froglet OpenClaw plugin
therefore ships only the plugin runtime, examples, and doctor/test scripts. Use
OpenClaw's own plugin install command against this directory or a marketplace
entry; do not install a Froglet-owned `openclaw` binary wrapper.

Verified status (2026-05-06): OpenClaw `2026.5.5` on Node 22 passed
`plugins install --link`, `config validate`, `plugins inspect froglet --runtime
--json`, Gateway startup, `tools.catalog`, and HTTP `/tools/invoke` for
`action=status` and `action=list_local_services` against a local Froglet
provider/runtime stack.
