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

At this layer:

- named and data-service bindings are listed through `/services/*`
- direct open-ended compute goes through `/compute/run`
- service detail responses include `offer_kind` plus a coarse `resource_kind`
  helper so bot hosts do not need to guess from runtime fields alone

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
  `starter`, `result_json`, or `inline_source`
- blank projects are scaffolds only; they must remain hidden until source is
  written and published explicitly
- project authoring currently covers inline-source Python and project-backed
  WAT->Wasm workflows; `POST /v1/froglet/artifacts/publish` is the direct
  publication path for prebuilt Wasm modules and OCI-backed/container profiles
- `POST /v1/froglet/services/invoke` waits briefly by default for sync services
- `POST /v1/froglet/services/invoke` resolves a unique `service_id` automatically when possible
- `POST /v1/froglet/compute/run` is the direct compute path and currently accepts
  inline Wasm, inline Python, OCI-backed Wasm, and OCI-backed python/container
  execution inputs; it expects `provider_id` or `provider_url` so the runtime
  knows which provider to target, and zip/archive packaging is future work
- `GET /v1/froglet/status` includes discovery mode, reference discovery wiring,
  and the last discovery error when present
