# Froglet OpenClaw Plugin

This plugin exposes exactly one tool: `froglet`.

The same plugin contract is used for both OpenClaw and NemoClaw. The only
difference is where the Froglet control API runs:

- OpenClaw: loopback on the host
- NemoClaw: HTTPS from the sandbox to the host

The node model is the same in both products: a Froglet node can publish local
resources and invoke remote ones through the same single tool.

Named services, data services, and open-ended compute are all product-layer
bindings over the same Froglet primitive.

## Config

Start from the checked-in complete configs:

- [examples/openclaw.config.example.json](examples/openclaw.config.example.json)
- [examples/openclaw.config.nemoclaw.example.json](examples/openclaw.config.nemoclaw.example.json)
- [examples/openclaw.config.nemoclaw.hosted.example.json](examples/openclaw.config.nemoclaw.hosted.example.json)

Supported plugin keys:

- `hostProduct`
- `baseUrl`
- `authTokenPath`
- `requestTimeoutMs`
- `defaultSearchLimit`
- `maxSearchLimit`

## Tool Actions

The plugin registers one tool named `froglet`. It supports these actions:

- `discover_services`
- `get_service`
- `invoke_service`
- `list_local_services`
- `get_local_service`
- `publish_artifact`
- `status`
- `get_task`
- `wait_task`
- `run_compute`
- `get_wallet_balance`
- `list_settlement_activity`
- `get_payment_intent`
- `get_invoice_bundle`
- `get_install_guide`
- `marketplace_search`
- `marketplace_provider`
- `marketplace_receipts`
- `marketplace_stake`
- `marketplace_topup`

Named services are the default UX. Raw compute is the expert path.

Listed services are named/data service bindings. Open-ended compute is not a
service listing; it uses the provider's direct compute offer through
`run_compute`.

Current implementation note:

- the checked-in execution profiles are current reference implementations
- the current implementation state is not the intended permanent Froglet
  boundary

Discovery is the authoritative remote-listing path. `discover_services` should
be used for registry-backed remote listings. If discovery is misconfigured or
unhealthy, Froglet returns a structured error instead of pretending there are no
services.

## Current API Surface

The current checked-in API is service- and artifact-oriented:

- discover remote services with `discover_services` / `get_service`
- invoke named/data services with `invoke_service`
- inspect and publish local services with `list_local_services`,
  `get_local_service`, and `publish_artifact`
- poll async work with `get_task` / `wait_task`
- inspect settlement state with `get_wallet_balance`,
  `list_settlement_activity`, `get_payment_intent`, and `get_invoice_bundle`
- use the marketplace wrappers when you want marketplace-native search,
  provider detail, receipts, stake, or top-up operations
- use `get_install_guide` when the user asks to install Froglet on the host

The current public tool surface does not include project authoring, log tailing,
or node restarts.

`summary` remains descriptive metadata only. It never generates code
implicitly.

## Managed Host Launcher

For Froglet-managed OpenClaw hosts:

```bash
./integrations/openclaw/froglet/scripts/install-openclaw-launcher.sh
```

That installs an `openclaw` wrapper that:

- opens a local Froglet chat loop when called with no args
- forwards to the upstream OpenClaw CLI when args are present

## Verification

```bash
node --check integrations/openclaw/froglet/index.js
node --check integrations/openclaw/froglet/scripts/doctor.mjs
node --test integrations/openclaw/froglet/test/plugin.test.js \
  integrations/openclaw/froglet/test/config-profiles.test.mjs \
  integrations/openclaw/froglet/test/doctor.test.mjs
```
