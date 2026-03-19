# NemoClaw

NemoClaw uses the same public Froglet plugin as OpenClaw, but the supported topology is stricter:

- `froglet-runtime` runs inside the same sandbox/process environment as OpenClaw
- remote `froglet-provider` is outside the sandbox
- remote `froglet-discovery` is outside the sandbox

Bots still talk only to the local runtime on loopback.

## Runtime Placement

The runtime must be local to the sandbox. That is the supported architecture for requester-side Froglet in NemoClaw.

Use loopback inside the sandbox:

- `http://127.0.0.1:8081`
- token path under `/sandbox/...`, for example `/sandbox/state/froglet/runtime/auth.token`

## Example Config

Start from [../integrations/openclaw/froglet/examples/openclaw.config.nemoclaw.example.json](../integrations/openclaw/froglet/examples/openclaw.config.nemoclaw.example.json).

```json
{
  "plugins": {
    "entries": {
      "froglet": {
        "enabled": true,
        "config": {
          "runtimeUrl": "http://127.0.0.1:8081",
          "runtimeAuthTokenPath": "/sandbox/state/froglet/runtime/auth.token"
        }
      }
    }
  }
}
```

## External Services

The local runtime still needs outbound access to:

- your chosen discovery service
- the remote provider selected for the deal

Those URLs are runtime configuration, not plugin configuration.

If the sandbox-local runtime needs to trust a private CA for those HTTPS
endpoints, set:

- `FROGLET_HTTP_CA_CERT_PATH=/sandbox/.../your-ca.pem`

The GCP harness under [/_tmp/testing/README.md](/Users/armanas/Projects/github.com/armanas/froglet/_tmp/testing/README.md)
uses that exact mechanism while keeping the plugin pointed at
`http://127.0.0.1:8081`.

## Sanity Check

Inside the sandbox:

```bash
TOKEN=$(cat /sandbox/state/froglet/runtime/auth.token)
curl -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8081/v1/runtime/wallet/balance
```

Once that works, OpenClaw should be able to use:

- `froglet_search`
- `froglet_get_provider`
- `froglet_buy`

## Three-Window Usage

If you run three separate NemoClaw instances:

- consumer: local runtime plus remote provider/discovery
- provider: local runtime for inspection plus public provider role
- discovery: local runtime for inspection plus public discovery role

you can open three independent sessions and ask each one to inspect Froglet. The consumer session is the one that should complete buy/wait/accept.
