# Plugin And Registry Distribution

Status date: 2026-04-29.

This page tracks how Froglet reaches agent hosts. The invariant is that
`froglet-mcp` is the execution artifact. Host-specific plugins should wrap that
package instead of reimplementing Froglet behavior.

## Distribution Order

1. **Official MCP Registry** — publish `froglet-mcp` metadata after the matching
   npm package version exists. This is the highest-leverage listing because MCP
   clients and downstream catalogs can consume it without host-specific plugin
   logic.
2. **Codex plugin bundle** — ship the repo-local Codex plugin under
   `plugins/froglet/` and list it in `.agents/plugins/marketplace.json`. The
   official Codex public Plugin Directory is not self-serve yet, so this is the
   testable path until submission opens.
3. **Claude Code plugin marketplace** — ship `.claude-plugin/marketplace.json`
   plus the same `plugins/froglet/` bundle. Claude Code users can add the repo as
   a marketplace and install the plugin.
4. **OpenClaw package** — keep the current `integrations/openclaw/froglet/`
   plugin as the source package, with hosted-proof-first onboarding and local
   install planning.
5. **NemoClaw package** — publish only after verifying NemoClaw-specific config,
   network, and sandbox staging deltas in a real NemoClaw environment.
6. **Third-party MCP directories** — submit metadata only after the official MCP
   Registry listing is live, so every directory points at the same npm package,
   license, docs, and hosted-proof boundary.

ChatGPT App Directory is intentionally excluded from this slice.

## Current Artifacts

| Surface | File | Current state |
| --- | --- | --- |
| npm package | `package.json` | `froglet-mcp`, Apache-2.0, stdio binary, `mcpName` set |
| MCP Registry | `server.json` | Ready for `mcp-publisher` after npm `0.1.2` publish |
| Codex plugin | `plugins/froglet/.codex-plugin/plugin.json` | Repo-local marketplace/test bundle |
| Codex marketplace | `.agents/plugins/marketplace.json` | Local Codex marketplace entry |
| Claude plugin | `plugins/froglet/.claude-plugin/plugin.json` | Claude Code plugin metadata |
| Claude marketplace | `.claude-plugin/marketplace.json` | GitHub-hosted marketplace entry |
| Shared plugin MCP | `plugins/froglet/.mcp.json` | Starts `npx -y froglet-mcp` with `FROGLET_PROFILE=hosted-proof` |
| Shared plugin skill | `plugins/froglet/skills/froglet/SKILL.md` | Tells agents to run proof, plan install, then guide use-case implementation |
| OpenClaw/NemoClaw | `integrations/openclaw/froglet/` | Source plugin and examples; registry package still pending |

## MCP Registry Publish

The MCP Registry validates that the package metadata matches `server.json`, so
`package.json` contains:

```json
"mcpName": "io.github.armanas/froglet"
```

The registry metadata lives at repo root in `server.json` and points at the npm
package version `0.1.2`. Because npm tarballs are immutable, publish the npm
version first, then publish the registry metadata.

```bash
npm run check:mcp
npm run test:mcp
npm pack --dry-run
npm publish --provenance=false --otp <npm-otp>
```

Then install and authenticate the MCP publisher:

```bash
curl -L "https://github.com/modelcontextprotocol/registry/releases/latest/download/mcp-publisher_$(uname -s | tr '[:upper:]' '[:lower:]')_$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/').tar.gz" \
  | tar xz mcp-publisher
sudo mv mcp-publisher /usr/local/bin/
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
# From this repository root, install the marketplace/plugin through the Codex UI
# or use the repo-local marketplace if the host exposes plugin marketplace import.
cat .agents/plugins/marketplace.json
cat plugins/froglet/.codex-plugin/plugin.json
```

The plugin does not vendor Froglet code. It starts the published MCP server:

```json
{
  "mcpServers": {
    "froglet": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "froglet-mcp"],
      "env": { "FROGLET_PROFILE": "hosted-proof" }
    }
  }
}
```

Operational rule: the first task should call `run_hosted_proof`; local install
starts only after `plan_install` has collected the user's profile.

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

Expected proof: the plugin installs, starts the bundled MCP server, and the
agent can call `run_hosted_proof` without hand-editing MCP config.

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
installs the package and runs at least one hosted proof or local Froglet action.

## Hosted-Proof Boundary

Every distribution surface must preserve this boundary:

- hosted proof: public free `demo.*` services, `run_hosted_proof`, receipt/feed
  evidence
- local/self-hosted: paid rails, persistent identity, service publication,
  marketplace write flows, long jobs, batch, GPU, and production use cases
