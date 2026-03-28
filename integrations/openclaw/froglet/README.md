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
- `create_project`
- `list_projects`
- `get_project`
- `read_file`
- `write_file`
- `build_project`
- `test_project`
- `publish_project`
- `publish_artifact`
- `status`
- `tail_logs`
- `restart`
- `get_task`
- `wait_task`
- `run_compute`

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

## Authoring Model

The current checked-in authoring implementation is project-first:

- create a project
- edit source
- build a real artifact for the current project-backed profiles
- test locally
- publish a named service or compute binding

Current implementation note:

- project authoring currently covers inline-source Python and project-backed
  WAT->Wasm
- OCI-backed Wasm and OCI/container profiles are published directly through
  `publish_artifact`

Starter templates are only scaffolding. They are not first-class tool actions.

Practical shortcuts:

- `create_project` can derive `project_id`, `service_id`, and `offer_id` from
  `name` when explicit ids are omitted.
- `create_project` accepts optional `result_json` to scaffold a simple static
  JSON response service.
- `create_project` accepts optional `inline_source` when you want to provide
  explicit source at creation time.
- `create_project` and `publish_artifact` accept explicit execution metadata
  such as `runtime`, `package_kind`, `entrypoint_kind`, `entrypoint`,
  `contract_version`, and `mounts`.
- `create_project` auto-publishes only when `publication_state=active` and an
  explicit runnable scaffold is provided via `starter`, `result_json`, or
  `inline_source`.
- blank projects are scaffolds only; create them with `publication_state=hidden`
  and then `write_file`, `build_project`, `test_project`, and `publish_project`.
- service detail output includes `offer_kind` and `resource_kind` so bot hosts
  can distinguish listed service bindings from direct compute.
- `invoke_service` waits briefly by default for sync services and can resolve a
  unique `service_id` without an explicit provider reference.

`summary` is descriptive metadata only. It never generates code implicitly.

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
