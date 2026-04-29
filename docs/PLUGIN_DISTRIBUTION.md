# Plugin And Registry Distribution

Status date: 2026-04-29.

This page tracks how Froglet reaches agent hosts and ChatGPT app distribution.
The invariant for coding-agent hosts is that `froglet-mcp` is the execution
artifact. Host-specific plugins should wrap that package instead of
reimplementing Froglet behavior. ChatGPT App Directory distribution is a
separate Apps SDK surface: it needs a hosted public MCP app server and reviewable
UI, not the local stdio package alone.

## Distribution Order

1. **Official MCP Registry** — publish `froglet-mcp` metadata after the matching
   npm package version exists. This is the highest-leverage listing because MCP
   clients and downstream catalogs can consume it without host-specific plugin
   logic.
2. **ChatGPT App Directory** — build an Apps SDK app backed by a public hosted
   MCP server, test it in Developer Mode, then submit through the OpenAI
   dashboard review flow. An approved and published app is also the current
   public path into the Codex Plugin Directory.
3. **Codex plugin bundle** — ship the repo-local Codex plugin under
   `plugins/froglet/` and list it in `.agents/plugins/marketplace.json`. The
   repo-local path stays useful while ChatGPT Apps SDK review is pending or for
   users who want local coding-agent install.
4. **Claude Code plugin marketplace** — ship `.claude-plugin/marketplace.json`
   plus the same `plugins/froglet/` bundle. Claude Code users can add the repo as
   a marketplace and install the plugin.
5. **OpenClaw package** — keep the current `integrations/openclaw/froglet/`
   plugin as the source package, with hosted-proof-first onboarding and local
   install planning.
6. **NemoClaw package** — publish only after verifying NemoClaw-specific config,
   network, and sandbox staging deltas in a real NemoClaw environment.
7. **Third-party MCP directories** — submit metadata only after the official MCP
   Registry listing is live, so every directory points at the same npm package,
   license, docs, and hosted-proof boundary.

## Current Artifacts

| Surface | File | Current state |
| --- | --- | --- |
| npm package | `package.json` | `froglet-mcp`, Apache-2.0, stdio binary, `mcpName` set |
| MCP Registry | `server.json` | Ready for `mcp-publisher` after npm `0.1.2` publish |
| ChatGPT App Directory | `https://apps.froglet.dev/mcp` | Hosted Apps SDK MCP app deployed; needs MCP Inspector, ChatGPT Developer Mode, and OpenAI dashboard submission |
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

## ChatGPT App Directory

This is now in scope, but it is not the same artifact as `froglet-mcp`.
OpenAI's current public distribution path is Apps SDK submission through the
Platform dashboard after Developer Mode testing. If approved and published, the
app can appear in the ChatGPT Apps Directory and OpenAI creates a Codex plugin
for Codex distribution.

Current status:

- first hosted Apps/MCP Worker scaffold exists in the sibling services repo at
  `../froglet-services/ops/cloudflare-workers/chatgpt-app/`
- public endpoint is deployed at `https://apps.froglet.dev/mcp`
- local tests cover MCP JSON-RPC routes, widget metadata, install planning,
  use-case explanation, and mocked hosted proof execution
- live smoke through the deployed MCP endpoint returned the expected three tools
  and a successful `demo.add` result `{ "sum": 12 }` with receipt present and a
  feed artifact envelope
- Streamable HTTP MCP SDK smoke against the deployed endpoint passed, including
  `demo.add`, the fixed `demo.fetch-witness` follow-up, receipt presence, and
  feed artifact envelope verification
- privacy policy URL is `https://froglet.dev/privacy/`
- not yet done: MCP Inspector, ChatGPT Developer Mode on web and mobile,
  dashboard submission, approval, or publication

Required before submission:

- a hosted public MCP server endpoint, not a localhost or testing URL
- a CSP that names the exact domains the app fetches from
- OpenAI organization verification for the publishing name
- `api.apps.write` and `api.apps.read` permissions in the OpenAI project
- app name, logo, description, company URL `https://froglet.dev`,
  privacy-policy URL `https://froglet.dev/privacy/`, MCP/tool
  details, screenshots, test prompts and expected responses, and localization
  details
- a complete app that behaves reliably; a trial-only or demo-only submission is
  not enough

Froglet app shape:

- consumer-first UI: run hosted proof, show `demo.add`, witness/hash evidence,
  receipt presence, and feed artifact shape
- install planner: guide Docker/local/Tor/clearnet/Lightning/Stripe choices only
  after the user asks to install locally
- provider path: explain service publication and paid rails as local/self-hosted
  work until those hosted flows are separately verified

Do not mark this complete until a real ChatGPT Developer Mode test passes on web
and mobile, the dashboard review is submitted, and the published directory URL is
captured.

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
