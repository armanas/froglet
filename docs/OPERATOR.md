# Operator

`froglet-operator` is the host-side control API used by the `froglet` plugin.

It exposes one public control surface:

- `/v1/froglet/*`

That surface covers:

- local status
- discovery status and last discovery error
- bounded log tails
- managed restarts
- local project workspaces
- build/test/publish
- local service listing
- remote service discovery and invocation
- task polling
- expert raw compute

It is the local control surface for the generic Froglet execution primitive,
not a role-specific node API.

## Important Paths

- runtime auth token: `./data/runtime/auth.token`
- control token: `./data/runtime/froglet-control.token`
- local projects root: `./data/projects`

## Key Routes

- `GET /v1/froglet/status`
- `GET /v1/froglet/logs`
- `POST /v1/froglet/restart`
- `GET /v1/froglet/projects`
- `POST /v1/froglet/projects`
- `GET /v1/froglet/projects/:project_id`
- `GET /v1/froglet/projects/:project_id/files/*path`
- `PUT /v1/froglet/projects/:project_id/files/*path`
- `POST /v1/froglet/projects/:project_id/build`
- `POST /v1/froglet/projects/:project_id/test`
- `POST /v1/froglet/projects/:project_id/publish`
- `POST /v1/froglet/artifacts/publish`
- `GET /v1/froglet/services/local`
- `GET /v1/froglet/services/local/:service_id`
- `POST /v1/froglet/services/discover`
- `POST /v1/froglet/services/get`
- `POST /v1/froglet/services/invoke`
- `POST /v1/froglet/compute/run`
- `GET /v1/froglet/tasks/:task_id`
- `POST /v1/froglet/tasks/:task_id/wait`

Only `/v1/froglet/*` is part of the supported product contract.

Notes:

- `POST /v1/froglet/projects` can derive ids from `name` if explicit ids are omitted
- `POST /v1/froglet/projects` can scaffold a fixed JSON response via `result_json`
- `POST /v1/froglet/projects` rejects `publication_state=active` unless you provide
  `starter` or `result_json`
- blank projects are scaffolds only; they must remain hidden until source is
  written and published explicitly
- `POST /v1/froglet/services/invoke` waits briefly by default for sync services
- `POST /v1/froglet/services/invoke` resolves a unique `service_id` automatically when possible
- `GET /v1/froglet/status` includes discovery mode, reference discovery wiring,
  and the last discovery error when present
