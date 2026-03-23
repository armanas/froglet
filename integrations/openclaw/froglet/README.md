# Froglet OpenClaw Plugin

This plugin exposes exactly one tool: `froglet`.

The same plugin contract is used for both OpenClaw and NemoClaw. The only
difference is where the Froglet control API runs:

- OpenClaw: loopback on the host
- NemoClaw: HTTPS from the sandbox to the host

The node model is the same in both products: a Froglet node can publish local
services and invoke remote services through the same single tool.

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

## Authoring Model

Service authoring is WAT-first in this cutover:

- create a project
- edit source
- build a real Wasm artifact
- test locally
- publish a named service

Starter templates are only scaffolding. They are not first-class tool actions.

Practical shortcuts:

- `create_project` can derive `project_id`, `service_id`, and `offer_id` from
  `name` when explicit ids are omitted.
- `create_project` accepts optional `result_json` to scaffold a simple static
  JSON response service.
- `create_project` auto-publishes when `publication_state=active`.
- `invoke_service` waits briefly by default for sync services and can resolve a
  unique `service_id` without an explicit provider reference.

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
