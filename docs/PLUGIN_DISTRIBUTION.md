# Plugin And Registry Distribution

Status date: 2026-04-29.

Froglet distribution has two distinct paths:

1. `https://froglet.dev/llms.txt` for the no-install hosted proof.
2. `froglet-mcp` and host plugin wrappers for real local/self-hosted work.

Do not blur those paths. Installed plugins should connect an agent to a local or
self-hosted Froglet node so it can inspect status, discover services, invoke
work, publish artifacts, inspect settlement, and guide installation. The hosted
proof is intentionally HTTP-only and demo-scoped.

## Distribution Order

1. **Official MCP Registry** - publish `froglet-mcp` metadata after the matching
   npm package version exists.
2. **Codex plugin bundle** - ship the repo-local Codex plugin under
   `plugins/froglet/` and list it in `.agents/plugins/marketplace.json`.
3. **Claude Code plugin marketplace** - ship `.claude-plugin/marketplace.json`
   plus the same `plugins/froglet/` bundle.
4. **OpenClaw package** - package the checked-in OpenClaw plugin as an
   actionable local/self-hosted integration.
5. **NemoClaw package** - verify NemoClaw-specific sandbox and HTTPS deltas in a
   real NemoClaw environment before claiming support.
6. **Third-party MCP directories** - submit only after the official MCP Registry
   entry is live, so every directory points at the same npm package, license,
   docs, and local/actionable boundary.

A hosted consumer app directory path is intentionally out of scope for this repo
slice. If that product path returns later, it should be designed as its own app,
not as a repackaging of the local stdio MCP tool.

## Current Artifacts

| Surface | File | Current state |
| --- | --- | --- |
| npm package | `package.json` | `froglet-mcp`, Apache-2.0, stdio binary, `mcpName` set |
| MCP Registry | `server.json` | Published for `io.github.armanas/froglet` after npm `0.1.3` |
| Codex plugin | `plugins/froglet/.codex-plugin/plugin.json` | Repo-local marketplace/test bundle |
| Codex marketplace | `.agents/plugins/marketplace.json` | Local Codex marketplace entry |
| Claude plugin | `plugins/froglet/.claude-plugin/plugin.json` | Claude Code plugin metadata |
| Claude marketplace | `.claude-plugin/marketplace.json` | GitHub-hosted marketplace entry |
| Shared plugin MCP | `plugins/froglet/.mcp.json` | Starts `npx -y froglet-mcp` with local provider/runtime env |
| Shared plugin skill | `plugins/froglet/skills/froglet/SKILL.md` | Tells agents to check local status, plan install, then execute real actions |
| OpenClaw/NemoClaw | `integrations/openclaw/froglet/` | Source plugin and examples; registry package still pending |

## MCP Registry Publish

The MCP Registry validates that the package metadata matches `server.json`, so
`package.json` contains:

```json
"mcpName": "io.github.armanas/froglet"
```

The registry metadata lives at repo root in `server.json`. Because npm tarballs
are immutable, publish the npm version first, then publish the registry
metadata.

```bash
npm run check:mcp
npm run test:mcp
npm pack --dry-run
npm publish --provenance=false --otp <npm-otp>
mcp-publisher login github
mcp-publisher publish
curl "https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.armanas/froglet"
```

Expected proof after publish: the registry search result includes
`"name":"io.github.armanas/froglet"` and package identifier `froglet-mcp`.

Use compact verification commands after publishing:

```bash
npm view froglet-mcp version
node -e 'fetch("https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.armanas/froglet").then(r=>r.json()).then(j=>{const s=j.servers?.[0]?.server; console.log(JSON.stringify({name:s?.name, package:s?.packages?.[0]?.identifier, version:s?.version, count:j.metadata?.count}, null, 2));})'
node -e 'fetch("https://froglet.dev/learn/plugin-distribution/").then(r=>console.log(r.status, r.headers.get("content-type")))'
```

Do not use `curl https://froglet.dev/learn/plugin-distribution/` as a proof
command. That URL is a rendered documentation page, so raw `curl` output is
HTML by design.

## Codex Plugin

Local/repo test path:

```bash
cat .agents/plugins/marketplace.json
cat plugins/froglet/.codex-plugin/plugin.json
```

The plugin does not vendor Froglet code. It starts the published MCP server in
local mode:

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

Operational rule: the first task should call `status`. If the local node is not
running or tokens are missing, call `plan_install`; only after the profile is
confirmed should the agent call `get_install_guide` and run host-shell commands.

## Claude Code Plugin Marketplace

Claude Code can consume a GitHub-hosted marketplace from the repository root.
Use sparse checkout so Claude only fetches plugin metadata and the plugin
bundle:

```bash
claude plugin marketplace add armanas/froglet --sparse .claude-plugin plugins
claude plugin install froglet@froglet
```

Local test path:

```bash
claude plugin validate .
claude plugin marketplace add . --scope local
claude plugin install froglet@froglet --scope local
```

Expected proof: the plugin installs, starts the bundled MCP server, reports
local `status`, then performs one real local action such as `list_local_services`
or `invoke_service` after the provider/runtime and token paths are available.

## OpenClaw And NemoClaw

OpenClaw/NemoClaw remain source-plugin paths until their registry/package story
is verified. The checked-in plugin is:

```text
integrations/openclaw/froglet/
```

Validation commands:

```bash
node --check integrations/openclaw/froglet/index.js
node --check integrations/openclaw/froglet/scripts/doctor.mjs
node --test integrations/openclaw/froglet/test/plugin.test.js \
  integrations/openclaw/froglet/test/config-profiles.test.mjs \
  integrations/openclaw/froglet/test/doctor.test.mjs \
  integrations/openclaw/froglet/test/froglet-client.test.mjs
```

OpenClaw can use loopback host URLs. NemoClaw must be verified separately
because the plugin usually runs inside a sandbox and reaches the host over HTTPS.
Do not claim NemoClaw distribution is complete until a real NemoClaw environment
installs the package and runs at least one local Froglet action.

## Boundary

Every distribution surface must preserve this boundary:

- no-install hosted proof: `https://froglet.dev/llms.txt`, public free `demo.*` services,
  receipt/feed evidence, and no local secrets
- installed MCP/plugins: local/self-hosted provider/runtime actions, paid rails,
  persistent identity, service publication, marketplace write flows, long jobs,
  batch, GPU, and production use cases
