# Froglet MCP surfaces

Froglet exposes two MCP stdio adapters:

- `froglet-node mcp`: dependency-minimal native bridge installed on the
  clean-host path; and
- `froglet-mcp`: broader JavaScript compatibility server for discovery,
  compute, settlement, install planning, and existing host integrations.

Both expose one tool named `froglet`. Publication delegates to the same Rust
publication/consent/provider-control modules; the JavaScript layer must not
become a second policy implementation.

## Requirements

- Native bridge: a released `froglet-node`; no Node.js or Docker
- JavaScript compatibility server: Node.js 18+ or a digest-pinned MCP image
- A running Froglet provider/runtime is required for provider, runtime,
  marketplace, payment, and publication actions
- The no-install hosted compute trial is currently unavailable. The browser
  verifier/playground and the local candidate demo are separate alternatives.

## Support and qualification

The [shared functionality matrix source](../../../docs-site/src/data/support-matrix.ts)
feeds the local website build/preview; these additions are not yet publicly
deployed. It distinguishes implemented, locally verified candidate,
experimental, disabled, planned and unqualified paths. Check the exact connected binary's `tools/list`;
these source instructions do not add actions to an older immutable release.

Native MCP includes preparation, publication lifecycle, safeguards and supplied
Wasm execution. The JavaScript/OpenClaw surface additionally exposes marketplace
discovery, waiting and settlement inspection, but does not expose every native
preparation/lifecycle action. Neither surface compiles a program merely because
the user describes one. A2A is optional, private-configured and polling-only.

Actual Codex tool use on one Mac and the extracted local demo bundles are
recorded in [Release](../../../docs/RELEASE.md). Claude Code is logged out;
clean-machine installation, public compute operation and real-money settlement
remain separate gates. The later fresh Bob/Alice generated-program attempt had
correct independently checked outputs but incomplete chain export and a
persistent Codex configuration change. Its guarded fresh-session repeat remains
unrun; a separate deterministic replay cannot qualify those earlier sessions.
That separate replay verified seven new signed chains and four independent input
cases. With Bob stopped and Alice restarted, exact saved `run_compute` and
`invoke_service` retries recovered results. In that historical replay, `get_task`
returned HTTP 502 while Bob was offline. The `v0.4.7-beta.1` requester runtime
subsequently added local revalidation of saved terminal Quote/Deal/Receipt
evidence, so those reads can succeed without Bob. The JavaScript adapter calls
that configured runtime API; it does not independently verify signatures.
Requester deals without a saved Receipt still require provider status refresh;
invalid saved evidence is refused. Older runtimes and provider-job fallback retain their own availability requirements;
the fallback is used only after a requester-runtime 404 on a shared API base.

## Quick Start

### No-clone local node

For a user who wants a local Froglet node plus MCP config:

Use the copyable immutable-release resolver in the
[Quickstart](../../../docs-site/src/content/docs/learn/quickstart.mdx). It
requires `immutable: true`, verifies the uploaded `agent-bootstrap.sh` API
digest before executing it, then runs the non-mutating plan. Present that plan
and wait before running its exact approved execute command.

The first call writes only temporary files and binds the exact release, target
binary and script digests, paths, profile, process-manager impact, and command.
The second recomputes that contract before any host mutation. It then installs the checksum-verified native binary,
starts the dual-role launchd/systemd service, and writes config that launches
`froglet-node mcp`. It returns only after node health, a non-seeding native MCP
status proof, and a transient read-only publication/invocation proof that is
confirmed-unpublished with no active offer remaining. A digest-pinned dual-role image is
used only when native service management is unavailable.

The native tool supports `status`, `invoke_service`, `run_compute`, `get_task`, two-step
`marketplace_publish`, `publication_status`, `publication_logs`,
`publication_pause`, `publication_resume`, `publication_rollback`, and
`publication_unpublish`, plus managed-operation status, confirmed
reconciliation, and confirmed compensation. The first publish call is non-mutating; the second
must carry the exact user-approved `consent_hash`. Rollback requires an exact
revision hash and unpublish requires an exact `confirm_service_id` before the
bridge sends an HTTP request. Managed mutation similarly requires an exact
`confirm_operation_id`. `local_proof` is an operator-enabled demo action;
the clean install does not seed the demo it requires.

Native `run_compute` accepts a Wasm v1 binary in `wasm_module_hex` or
`wasm_module_path`, an explicit JSON `input` (including `null`), and an explicit
`idempotency_key` of 1–128 UTF-8 bytes. The file option requires an absolute path
to a readable regular nonsymlink file of at most 262144 bytes and is mutually
exclusive with hex. It reads the exact compiled bytes locally and delegates to
the same inline submission and verification path; the module is not echoed in
the response. The JavaScript bridge still accepts hex only. The program uses
the `froglet.wasm.run_json.v1` ABI. Source
compilation, OCI workloads and host capabilities are outside this native action.
Execution limits come from the provider's signed Quote and appear in the report.

Calls default to `max_price_sats: 0`. A paid Lightning call requires an explicit
ceiling and the requester's configured wallet and cumulative budget. A timeout
returns an existing `deal_id` when admission was persisted; `get_task` reads
that reference without submitting work or authorizing payment. Reconcile an
uncertain call using the same program, input, provider and retry key. A changed
request with the same key is refused.

The JavaScript bridge also forwards per-call price ceilings and retry keys.
Its structured output carries result and signed evidence from the requester
runtime. It does not independently verify signatures: a verification report
must identify which component performed the checks. Legacy JavaScript calls may
omit a retry key, but then lack an explicit recovery identity.

Optional A2A transport is selected by the local Node's private configuration;
agents keep using the same MCP compute action and provider reference. See the
[interoperability specification](../../../docs/AGENT_INTEROPERABILITY_PLAN.md)
for supported operations and the configured-counterparty boundary.

For a repeatable source-checkout setup, run the
[local MCP/A2A compute demo](../../../examples/README.md#local-mcp-and-a2a-compute-demo).
It builds the checkout, starts separate loopback Nodes, scopes private A2A
credentials to Alice's identity and Bob's actual Offer, and calls native MCP.
Use a binary built from this checkout or a release containing `run_compute` and
the optional A2A adapter; the presence of these instructions does not prove an
older installed binary has those features.

### Local npm profile

```bash
npx froglet-mcp
```

The npm package defaults to `FROGLET_PROFILE=local` with provider/runtime URLs
pointing at `http://127.0.0.1:8080` and `http://127.0.0.1:8081`. Agents should
call `status` first. If the local node or token files are missing, call
`plan_install` and present its exact immutable release, manifest/bootstrap
hashes, persistent paths, process-manager impact, and command preview. After
the user approves that exact plan, pass its `release_tag` and
`install_approval_hash` unchanged to `get_install_guide`; only then run the
returned host-shell command. After local health is verified, call
`plan_use_case` before
implementing consumer, provider, evidence, payments, batch, or GPU workflows.

### Local source checkout

```bash
# Install dependencies
npm ci --prefix integrations/mcp/froglet

# Start the server (stdio transport)
FROGLET_PROVIDER_URL=http://127.0.0.1:8080 \
FROGLET_RUNTIME_URL=http://127.0.0.1:8081 \
FROGLET_PROVIDER_AUTH_TOKEN_PATH=./data/runtime/froglet-control.token \
FROGLET_RUNTIME_AUTH_TOKEN_PATH=./data/runtime/auth.token \
  node integrations/mcp/froglet/server.js
```

Source checkout is contributor mode. For project-local launch files in a clone,
use the helper:

```bash
cd froglet && ./scripts/setup-agent.sh --target claude-code
cd froglet && ./scripts/setup-agent.sh --target codex
```

Agents should call the Froglet `plan_install` action before local setup when the
user has not specified the target agent, install footprint, role, payment rail,
network mode, marketplace URL, or first use case. If `payment_rail` is omitted,
the tool returns `decision_required`; recommend `none` for the first local demo.
A complete `plan_install` returns `status=approval_required` and makes no local
changes. `get_install_guide` withholds executable commands until it can
recompute and match the exact `release_tag` plus `install_approval_hash`.
After health checks pass, `plan_use_case` returns a bounded first-workflow plan
and names unsupported edges before execution. In particular, true batch
fan-out, GPU scheduling/provider selection, marketplace GPU routing, and
production capacity management are still separate implementation work.
Historical GPU metadata and a GCP T4 smoke belong to an earlier execution path.
The current isolated worker refuses GPU execution. Those observations do not
qualify GPU rental, device isolation or the current disabled path. See the
[staged CPU/GPU scope](../../../docs/FILE_AND_COMPUTE_SCOPE.md).

### Explicit local npm profile

```bash
FROGLET_PROFILE=local \
FROGLET_PROVIDER_URL=http://127.0.0.1:8080 \
FROGLET_RUNTIME_URL=http://127.0.0.1:8081 \
FROGLET_PROVIDER_AUTH_TOKEN_PATH=/absolute/path/to/froglet/data/runtime/froglet-control.token \
FROGLET_RUNTIME_AUTH_TOKEN_PATH=/absolute/path/to/froglet/data/runtime/auth.token \
  npx froglet-mcp
```

## Configuration

The table below is the broader JavaScript server configuration. Native mode
uses `FROGLET_PROVIDER_URL`/`FROGLET_DAEMON_URL`, `FROGLET_RUNTIME_URL`, the
matching provider/runtime token paths, and `FROGLET_DATA_DIR`; normal installs
write these values rather than asking the user to copy them.

JavaScript configuration is through environment variables:

| Variable | Required | Description |
|----------|----------|-------------|
| `FROGLET_PROFILE` | No | `local` by default |
| `FROGLET_PROVIDER_URL` | No | Provider base URL (fallback: `FROGLET_BASE_URL`; default: `http://127.0.0.1:8080`) |
| `FROGLET_RUNTIME_URL` | No | Runtime base URL (fallback: `FROGLET_BASE_URL`; default: `http://127.0.0.1:8081`) |
| `FROGLET_MARKETPLACE_URL` | No | Marketplace API base URL for `marketplace_register` (default: `https://marketplace.froglet.dev`) |
| `FROGLET_MARKETPLACE_ARBITER_URL` | No | Marketplace arbiter base URL for complaint filing/reads (default: `https://arbiter.froglet.dev`) |
| `FROGLET_PROVIDER_AUTH_TOKEN_PATH` | No | Path to provider auth token file |
| `FROGLET_RUNTIME_AUTH_TOKEN_PATH` | No | Path to runtime auth token file |
| `FROGLET_REQUEST_TIMEOUT_MS` | No | HTTP timeout in ms (default: 10000) |
| `FROGLET_DEFAULT_SEARCH_LIMIT` | No | Default search results (default: 10) |
| `FROGLET_MAX_SEARCH_LIMIT` | No | Max search results (default: 50) |
| `FROGLET_EGRESS_MODE` | No | `strict` applies the same DNS-pinning + SSRF validation used for LLM-controlled URLs to operator-configured `FROGLET_PROVIDER_URL` / `FROGLET_RUNTIME_URL`. Use when the operator host sits behind public DNS and you want uniform rebind-resistance. Lenient mode (the default) keeps operator-configured local/dev HTTP topologies working, including loopback and Docker host bridges such as `http://host.docker.internal:8080`. |

Legacy shortcuts: `FROGLET_BASE_URL` sets both provider and runtime URLs.
`FROGLET_AUTH_TOKEN_PATH` sets both auth token paths.

Actions that hit provider/runtime APIs require the matching token path at call
time. `plan_install`, `get_install_guide`, and `plan_use_case` do not require
local token files.
The hosted demo is intentionally not an MCP action; use
`https://froglet.dev/llms.txt` for the no-install proof.

Marketplace registration helpers:

- `marketplace_register` posts a public HTTPS or Tor provider URL to
  `/v1/registrations`.
- `registration_transport=tor` is required for onion registration.
- `marketplace_domain_claim` and `marketplace_domain_complete` claim a
  Froglet-managed `*.providers.froglet.dev` hostname before HTTPS registration.
- General `provider_url` overrides still reject onion URLs outside the
  controlled registration path.

---

## IDE / Agent Integration

For the normal native path, run the verified agent bootstrap or generate config
with `FROGLET_MCP_MODE=native` and an executable `FROGLET_NODE_BIN`. The
Node.js examples below are contributor/compatibility configurations.

### Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS)
or `%APPDATA%/Claude/claude_desktop_config.json` (Windows):

```json
{
  "mcpServers": {
    "froglet": {
      "command": "node",
      "args": ["<path-to-repo>/integrations/mcp/froglet/server.js"],
      "env": {
        "FROGLET_PROVIDER_URL": "http://127.0.0.1:8080",
        "FROGLET_RUNTIME_URL": "http://127.0.0.1:8081",
        "FROGLET_PROVIDER_AUTH_TOKEN_PATH": "/absolute/path/to/froglet/data/runtime/froglet-control.token",
        "FROGLET_RUNTIME_AUTH_TOKEN_PATH": "/absolute/path/to/froglet/data/runtime/auth.token"
      }
    }
  }
}
```

### Claude Code (CLI)

Drop `.mcp.json` in the project root (already included in this repo):

```json
{
  "mcpServers": {
    "froglet": {
      "type": "stdio",
      "command": "node",
      "args": ["integrations/mcp/froglet/server.js"],
      "env": {
        "FROGLET_PROVIDER_URL": "http://127.0.0.1:8080",
        "FROGLET_RUNTIME_URL": "http://127.0.0.1:8081",
        "FROGLET_PROVIDER_AUTH_TOKEN_PATH": "data/runtime/froglet-control.token",
        "FROGLET_RUNTIME_AUTH_TOKEN_PATH": "data/runtime/auth.token"
      }
    }
  }
}
```

Or generate it directly:

```bash
cd froglet && ./scripts/setup-agent.sh --target claude-code
```

Or add via CLI: `claude mcp add froglet -- node integrations/mcp/froglet/server.js`

### Cursor

This repo includes a project config at `.cursor/mcp.json` that runs the
published package with local defaults. Add the same shape to
`~/.cursor/mcp.json` if you want a global config:

```json
{
  "mcpServers": {
    "froglet": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "froglet-mcp"],
      "env": {
        "FROGLET_PROFILE": "local",
        "FROGLET_PROVIDER_URL": "http://127.0.0.1:8080",
        "FROGLET_RUNTIME_URL": "http://127.0.0.1:8081",
        "FROGLET_PROVIDER_AUTH_TOKEN_PATH": "data/runtime/froglet-control.token",
        "FROGLET_RUNTIME_AUTH_TOKEN_PATH": "data/runtime/auth.token"
      }
    }
  }
}
```

Cursor host verification requires Cursor or `cursor-agent` to be installed. If
neither is available, use the MCP server tests and direct stdio smoke as the
available non-host evidence.

### OpenAI Codex CLI

Add to `~/.codex/config.toml` (global) or `.codex/config.toml` (project):

```toml
[mcp_servers.froglet]
command = "node"
args = ["integrations/mcp/froglet/server.js"]
env = { "FROGLET_PROVIDER_URL" = "http://127.0.0.1:8080", "FROGLET_RUNTIME_URL" = "http://127.0.0.1:8081", "FROGLET_PROVIDER_AUTH_TOKEN_PATH" = "/absolute/path/to/froglet/data/runtime/froglet-control.token", "FROGLET_RUNTIME_AUTH_TOKEN_PATH" = "/absolute/path/to/froglet/data/runtime/auth.token" }
```

Or generate the project-local file:

```bash
cd froglet && ./scripts/setup-agent.sh --target codex
```

### Docker

The MCP server is published as `ghcr.io/armanas/froglet-mcp`. No Node.js is
required, but a successful install must use the exact `image_mcp` digest from a
verified Release Bundle, never a mutable tag.

```bash
# Obtain this exact value from verified release-manifest.json.
export FROGLET_MCP_IMAGE='ghcr.io/armanas/froglet-mcp@sha256:<64-lowercase-hex>'
docker pull "$FROGLET_MCP_IMAGE"

# Run (connects to a Froglet node reachable from inside the container)
docker run --rm -i \
  -v /absolute/path/to/froglet/data/runtime:/tokens:ro \
  -e FROGLET_PROVIDER_URL=http://host.docker.internal:8080 \
  -e FROGLET_RUNTIME_URL=http://host.docker.internal:8081 \
  -e FROGLET_PROVIDER_AUTH_TOKEN_PATH=/tokens/froglet-control.token \
  -e FROGLET_RUNTIME_AUTH_TOKEN_PATH=/tokens/auth.token \
  "$FROGLET_MCP_IMAGE"
```

Use in any MCP client config:

```json
{
  "mcpServers": {
    "froglet": {
      "command": "docker",
      "args": ["run", "--rm", "-i",
        "-v", "/absolute/path/to/froglet/data/runtime:/tokens:ro",
        "-e", "FROGLET_PROVIDER_URL=http://host.docker.internal:8080",
        "-e", "FROGLET_RUNTIME_URL=http://host.docker.internal:8081",
        "-e", "FROGLET_PROVIDER_AUTH_TOKEN_PATH=/tokens/froglet-control.token",
        "-e", "FROGLET_RUNTIME_AUTH_TOKEN_PATH=/tokens/auth.token",
        "ghcr.io/armanas/froglet-mcp@sha256:<64-lowercase-hex>"],
      "type": "stdio"
    }
  }
}
```

Build locally from source:

```bash
docker build -f integrations/mcp/froglet/Dockerfile -t froglet-mcp .
```

---

## Example Config Files

| Platform | File | Format |
|----------|------|--------|
| Claude Desktop | `examples/claude-desktop-config.json` | JSON |
| Cursor | `examples/cursor-mcp-config.json` | JSON |
| Codex CLI | `examples/codex-mcp-config.toml` | TOML |
| Docker | `examples/docker-mcp-config.json` | JSON |
| Claude Code | `.mcp.json` (repo root) | JSON |

The checked-in examples are covered by `integrations/mcp/froglet/test/example-configs.test.mjs`.

## Compose Stack

When running the Docker Compose stack, the MCP server connects to the
locally-bound ports:

```bash
FROGLET_PROVIDER_URL=http://127.0.0.1:8080 \
FROGLET_RUNTIME_URL=http://127.0.0.1:8081 \
FROGLET_PROVIDER_AUTH_TOKEN_PATH=./data/runtime/froglet-control.token \
FROGLET_RUNTIME_AUTH_TOKEN_PATH=./data/runtime/auth.token \
  node integrations/mcp/froglet/server.js
```

## Publishing

The npm package is defined at the repository root so the tarball can include
both `integrations/mcp/froglet` and `integrations/shared/froglet-lib`.

```bash
npm run check:mcp
npm run test:mcp
npm pack --dry-run
npm publish --provenance=false
```

Do not publish `integrations/mcp/froglet/package.json` directly; it is a
repo-local development package and cannot include the shared library by itself.
Local manual publishes must disable provenance. Use CI/OIDC for a later
provenance-enabled publish flow.

### MCP Registry

The official MCP Registry entry is driven by the repo-root `server.json`.
Publish the matching npm version first because the registry validates the
package `mcpName` against `server.json`.

```bash
npm publish --provenance=false --otp <npm-otp>
mcp-publisher login github
mcp-publisher publish
curl "https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.armanas/froglet"
```

Expected proof: the registry search returns `io.github.armanas/froglet` with
package identifier `froglet-mcp`.

If you want to use the generated host-side agent configs against Docker Compose,
start Compose with `FROGLET_HOST_READABLE_CONTROL_TOKEN=true` so
`./data/runtime/froglet-control.token` is readable on the host.
The checked-in Compose stack also points the runtime at the default public read
marketplace, so `discover_services` works without running a local marketplace.

## Tests

```bash
# Unit tests
npm test --prefix integrations/mcp/froglet

# Compose smoke test (requires running stack)
npm run smoke:compose --prefix integrations/mcp/froglet
```

## Troubleshooting

**Connection refused** — Ensure the Froglet provider is running and healthy:
```bash
curl http://127.0.0.1:8080/health
```

**401 Unauthorized** — The endpoint requires an auth token. Set the provider
and runtime token paths to the matching files for the action you are calling:
`FROGLET_PROVIDER_AUTH_TOKEN_PATH=./data/runtime/froglet-control.token` and
`FROGLET_RUNTIME_AUTH_TOKEN_PATH=./data/runtime/auth.token`.

**Timeout errors** — Increase `FROGLET_REQUEST_TIMEOUT_MS` for slow networks
or large responses.

**Docker: connection refused to host** — Use `host.docker.internal` instead
of `127.0.0.1` for URLs when the Froglet node runs on the host machine.
Those operator-configured Docker bridge URLs are accepted in the MCP server's
default lenient mode.
