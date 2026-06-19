# Froglet OpenClaw Plugin

This plugin exposes exactly one tool: `froglet`.

Current OpenClaw releases require Node.js `22.14.0` or newer. Use Node 22 for
OpenClaw plugin install/inspect and gateway-mediated invocation.

For Gateway-mediated local actions, pass the Froglet token paths in the Gateway
process environment:

```bash
export FROGLET_PROVIDER_AUTH_TOKEN_PATH="$PWD/data/runtime/froglet-control.token"
export FROGLET_RUNTIME_AUTH_TOKEN_PATH="$PWD/data/runtime/auth.token"
```

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
- `providerUrl`
- `runtimeUrl`
- `marketplaceUrl`
- `authTokenPath`
- `providerAuthTokenPath`
- `runtimeAuthTokenPath`
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
- `plan_install`
- `get_install_guide`
- `plan_use_case`
- `marketplace_register`
- `marketplace_domain_claim`
- `marketplace_domain_complete`
- `marketplace_search`
- `marketplace_provider`
- `marketplace_receipts`
- `marketplace_file_complaint`
- `marketplace_get_complaint`
- `marketplace_publish`

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
- publish user-described services with `marketplace_publish`
- inspect and publish local services with `list_local_services`,
  `get_local_service`, and `publish_artifact`
- poll async work with `get_task` / `wait_task`
- inspect settlement state with `get_wallet_balance`,
  `list_settlement_activity`, `get_payment_intent`, and `get_invoice_bundle`
- use the marketplace wrappers when you want marketplace-native search,
  provider detail, receipts, registration, domain claims, or arbiter complaints
- use `plan_install` before local setup to collect agent, footprint, role,
  payment, network, marketplace, and use-case choices
- use `get_install_guide` after the install profile is confirmed to return
  host-shell commands
- use `plan_use_case` after health checks pass and before implementing
  consumer, provider, evidence, payments, batch, or GPU workflows

The current public tool surface does not include project authoring, log tailing,
or node restarts.

`summary` remains descriptive metadata only. It never generates code
implicitly.

## Verification

```bash
node --check integrations/openclaw/froglet/index.js
node --check integrations/openclaw/froglet/scripts/doctor.mjs
node --test integrations/openclaw/froglet/test/plugin.test.js \
  integrations/openclaw/froglet/test/config-profiles.test.mjs \
  integrations/openclaw/froglet/test/doctor.test.mjs
```

OpenClaw `2026.5.5` Gateway invocation was verified on 2026-05-06 with
`plugins install --link`, runtime inspection, `tools.catalog`, and
`/tools/invoke` calls for `status` and `list_local_services`.
