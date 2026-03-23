# OpenClaw

OpenClaw uses the shared `froglet` plugin and exposes one bot tool: `froglet`.

## Deployment

OpenClaw runs the plugin on the host. The plugin talks to a local Froglet
control API, usually:

- `http://127.0.0.1:9191`

Use the checked-in example config:

- [integrations/openclaw/froglet/examples/openclaw.config.example.json](/Users/armanas/Projects/github.com/armanas/froglet/integrations/openclaw/froglet/examples/openclaw.config.example.json)

## What The Tool Does

`froglet` covers the full node workflow:

- discover and invoke remote services
- inspect local services
- create/edit/build/test/publish local projects
- inspect status and logs
- restart managed node processes
- run expert raw compute

The default path is named services. `run_compute` is the low-level fallback.

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
- `create_project` auto-publishes when `publication_state=active`
- `invoke_service` can resolve a unique `service_id` automatically

## Managed Launcher

For Froglet-managed OpenClaw hosts:

```bash
./integrations/openclaw/froglet/scripts/install-openclaw-launcher.sh
```

That makes plain `openclaw` open a local Froglet chat loop.
