# NemoClaw

NemoClaw uses the same `froglet` plugin contract as OpenClaw.

The node model is the same too: a Froglet node can publish local services and
invoke remote services. NemoClaw only changes where the plugin runs and how it
reaches the Froglet control API.

Named services, data services, and open-ended compute are all the same Froglet
primitive at the deal layer.

Current implementation note:

- the checked-in execution profiles are current reference implementations
- broader interpreted/container compute is part of the same Froglet primitive

Remote service listing still goes through Froglet discovery. If discovery is
misconfigured or unhealthy, `discover_services` returns a structured error.

## Deployment

In NemoClaw the plugin runs inside the sandbox and talks to the host-side
Froglet control API over HTTPS.

Use the checked-in configs:

- [integrations/openclaw/froglet/examples/openclaw.config.nemoclaw.example.json](/Users/armanas/Projects/github.com/armanas/froglet/integrations/openclaw/froglet/examples/openclaw.config.nemoclaw.example.json)
- [integrations/openclaw/froglet/examples/openclaw.config.nemoclaw.hosted.example.json](/Users/armanas/Projects/github.com/armanas/froglet/integrations/openclaw/froglet/examples/openclaw.config.nemoclaw.hosted.example.json)

Supported plugin keys are the same as OpenClaw:

- `hostProduct`
- `baseUrl`
- `authTokenPath`
- `requestTimeoutMs`
- `defaultSearchLimit`
- `maxSearchLimit`

## Native Staging

Use documented OpenShell/NemoClaw commands only.

Stage the plugin:

```bash
openshell sandbox upload my-node \
  /absolute/path/to/froglet/integrations/openclaw/froglet \
  /sandbox/froglet/integrations/openclaw/froglet
```

Stage the Froglet control token:

```bash
openshell sandbox upload my-node \
  /absolute/path/to/froglet-control.token \
  /sandbox/.openclaw/froglet-control.token
```

If the host uses a private CA, stage that CA as well.

## Verification

```bash
nemoclaw my-node status
openshell sandbox get my-node
nemoclaw my-node connect
```

Inside the sandbox:

```bash
TOKEN=$(cat /sandbox/.openclaw/froglet-control.token)
curl -H "Authorization: Bearer $TOKEN" \
  https://node.example/health
```

The bot-facing tool contract is identical to OpenClaw: one tool named
`froglet`.

The live tool contract is the same as OpenClaw:

- `summary` is metadata only
- local publication currently goes through `publish_artifact`
- service metadata exposes `offer_kind` and `resource_kind`; direct compute
  still goes through `run_compute` rather than service listing
- `run_compute` should include `provider_id` or `provider_url` because direct
  compute is provider-targeted rather than service-discovered
- project authoring, log tailing, and node restart actions are not part of the
  current public API surface
