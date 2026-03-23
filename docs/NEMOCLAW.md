# NemoClaw

NemoClaw uses the same public Froglet plugin as OpenClaw, but the supported
requester topology is stricter:

- `froglet-runtime` runs on the consumer host as a supervised service
- remote `froglet-provider` stays outside the sandbox
- remote `froglet-discovery` stays outside the sandbox
- the bot talks only to the consumer-host runtime over HTTPS

These checked-in JSON files are complete user-edited configs, not rendered
fragments.

## Supported Profiles

| Profile | Runtime placement | Status |
| --- | --- | --- |
| `openclaw-local` | local host runtime | see [OPENCLAW.md](OPENCLAW.md) |
| `nemoclaw-local-runtime` | runtime inside the sandbox | compatibility path only |
| `nemoclaw-hosted-runtime` | runtime on the consumer host over HTTPS | supported baseline |

## Supported Baseline

The supported baseline for NemoClaw is:

- Linux consumer hosts and Linux remote gateways first
- consumer-host runtime on normal HTTPS, for example
  `https://consumer.example`
- runtime auth token staged into the sandbox, for example
  `/sandbox/.openclaw/froglet-runtime.token`
- remote provider and discovery over clearnet HTTPS
- model provider over normal outbound HTTPS

This is the baseline used by the matrix bring-up under `/_tmp`.

macOS and WSL remain compatibility paths until upstream platform behavior is
proven stable there.

Start from
[../integrations/openclaw/froglet/examples/openclaw.config.nemoclaw.hosted.example.json](../integrations/openclaw/froglet/examples/openclaw.config.nemoclaw.hosted.example.json).

The Froglet plugin contract is unchanged:

```json
{
  "plugins": {
    "entries": {
      "froglet": {
        "enabled": true,
        "config": {
          "runtimeUrl": "https://consumer.example",
          "runtimeAuthTokenPath": "/sandbox/.openclaw/froglet-runtime.token"
        }
      }
    }
  }
}
```

For current `openclaw agent --local` compatibility, the same two values may
also be provided via shell environment if the embedded agent path fails to pass
nested plugin config through to `api.config`:

- `FROGLET_RUNTIME_URL=https://consumer.example`
- `FROGLET_RUNTIME_AUTH_TOKEN_PATH=/sandbox/.openclaw/froglet-runtime.token`

Config-file values remain the primary supported contract. The environment
fallback exists so the same host-side runtime can be used reliably through
OpenClaw's current local-agent execution path.

If the consumer-host runtime uses a private CA instead of a public certificate
chain, the sandbox process must also trust that CA, for example:

- `NODE_EXTRA_CA_CERTS=/sandbox/froglet/_tmp/runs/current/froglet-root-ca.pem`

OpenClaw's current `--local` agent path also expects the hosted model provider
credential in the shell. For example:

- OpenAI-compatible: `OPENAI_API_KEY=...`
- Anthropic: `ANTHROPIC_API_KEY=...`

The matrix runner exports those for NemoClaw hosted-model rows.

Hosted model/provider configuration lives outside the Froglet plugin contract.
Froglet owns only the plugin load path and the `runtimeUrl` /
`runtimeAuthTokenPath` pair.

## Native Staging

Use documented OpenShell and NemoClaw commands for staging. Do not rely on
generated SSH config as the supported integration path.

Upload the Froglet plugin directory into the sandbox:

```bash
openshell sandbox upload my-assistant \
  /absolute/path/to/froglet/integrations/openclaw/froglet \
  /sandbox/froglet/integrations/openclaw/froglet
```

Upload the runtime auth token into the sandbox:

```bash
openshell sandbox upload my-assistant \
  /absolute/path/to/froglet-runtime.token \
  /sandbox/.openclaw/froglet-runtime.token
```

If the runtime uses a private CA, upload that as well:

```bash
openshell sandbox upload my-assistant \
  /absolute/path/to/froglet-root-ca.pem \
  /sandbox/froglet/runtime-ca.pem
```

Then use native status and connection commands:

```bash
nemoclaw my-assistant status
openshell sandbox get my-assistant
nemoclaw my-assistant connect
```

## Compatibility Paths

Local host Ollama is treated as a compatibility path, not the baseline
contract. If it works in a given OpenShell/NemoClaw deployment without special
bridges or proxy hacks, it is acceptable. It is not the supported default for
the matrix.

Do not substitute `host.openshell.internal` for the consumer-host Froglet
runtime in the current remote multi-VM matrix topology. In the GCP matrix it
does resolve inside the sandbox, but direct probes still fail to reach services
bound on the consumer VM host. Upstream NemoClaw uses that alias for
host-local inference providers such as local Ollama or vLLM, not as a proven
general replacement for a remote Froglet runtime endpoint.

Tor-only NemoClaw is also not part of the baseline contract. Only attempt it
after the clearnet baseline is clean and the platform has a documented
supported SOCKS path for sandbox egress.

## External Services

The consumer-host runtime still needs outbound access to:

- the chosen discovery service
- the selected remote provider

Those URLs are runtime configuration, not plugin configuration.

If the sandbox-local runtime needs to trust a private CA for those HTTPS
endpoints, set:

- `FROGLET_HTTP_CA_CERT_PATH=/sandbox/.../your-ca.pem`

## Sanity Checks

Inside the sandbox, the staged runtime credentials should be usable directly:

```bash
TOKEN=$(cat /sandbox/.openclaw/froglet-runtime.token)
NODE_EXTRA_CA_CERTS=/sandbox/froglet/_tmp/runs/current/froglet-root-ca.pem \
curl --cacert /sandbox/froglet/_tmp/runs/current/froglet-root-ca.pem \
  -H "Authorization: Bearer $TOKEN" \
  https://consumer.example/v1/runtime/wallet/balance
```

Before any agent prompt is trusted, the baseline bring-up should also verify:

- the OpenClaw/NemoClaw config is valid JSON
- the Froglet plugin config contains non-empty `runtimeUrl` and
  `runtimeAuthTokenPath`
- the model provider can answer one tiny prompt over HTTPS
- the Froglet tool inventory is visible before the first agent task

Once those checks are green, the first agent gate is:

- `froglet_search`
- `froglet_get_provider`

Optional Froglet-owned config validation on the host:

```bash
node integrations/openclaw/froglet/scripts/doctor.mjs \
  --config /absolute/path/to/openclaw.config.nemoclaw.json \
  --target nemoclaw
```

## Three-Window Usage

If you run three separate NemoClaw instances:

- consumer: host runtime plus remote provider/discovery
- provider: public provider role
- discovery: public discovery role

the consumer session is the one that should complete buy, wait, and accept.
