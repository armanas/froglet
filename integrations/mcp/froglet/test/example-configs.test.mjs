import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

import { readConfig } from "../lib/config.js"

const packageRoot = fileURLToPath(new URL("..", import.meta.url))
const examplesDir = path.join(packageRoot, "examples")

async function readJson(name) {
  return JSON.parse(await readFile(path.join(examplesDir, name), "utf8"))
}

function expectLocalNodeServer(config) {
  const server = config.mcpServers.froglet
  assert.equal(server.command, "node")
  assert.ok(
    server.args[0].endsWith("/integrations/mcp/froglet/server.js"),
    "server path should point at the checked-in MCP server"
  )
  assert.equal(server.env.FROGLET_PROVIDER_URL, "http://127.0.0.1:8080")
  assert.equal(server.env.FROGLET_RUNTIME_URL, "http://127.0.0.1:8081")
  assert.ok(
    server.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH.endsWith(
      "/data/runtime/froglet-control.token"
    )
  )
}

test("Claude Desktop example is complete", async () => {
  expectLocalNodeServer(await readJson("claude-desktop-config.json"))
})

test("Cursor example is complete", async () => {
  const config = await readJson("cursor-mcp-config.json")
  expectLocalNodeServer(config)
  assert.equal(config.mcpServers.froglet.type, "stdio")
})

test("Docker example includes token mount and MCP image", async () => {
  const config = await readJson("docker-mcp-config.json")
  const server = config.mcpServers.froglet
  assert.equal(server.command, "docker")
  assert.ok(server.args.includes("ghcr.io/armanas/froglet-mcp:latest"))
  assert.ok(
    server.args.includes("/absolute/path/to/froglet/data/runtime:/tokens:ro"),
    "docker config should mount the runtime token directory"
  )
  assert.ok(
    server.args.includes("FROGLET_PROVIDER_AUTH_TOKEN_PATH=/tokens/froglet-control.token")
  )
})

test("Docker example env is accepted by the MCP config loader", async () => {
  const config = await readJson("docker-mcp-config.json")
  const server = config.mcpServers.froglet
  const previous = {
    FROGLET_PROVIDER_URL: process.env.FROGLET_PROVIDER_URL,
    FROGLET_RUNTIME_URL: process.env.FROGLET_RUNTIME_URL,
    FROGLET_PROVIDER_AUTH_TOKEN_PATH: process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH,
    FROGLET_RUNTIME_AUTH_TOKEN_PATH: process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH,
    FROGLET_BASE_URL: process.env.FROGLET_BASE_URL,
    FROGLET_AUTH_TOKEN_PATH: process.env.FROGLET_AUTH_TOKEN_PATH,
  }

  try {
    delete process.env.FROGLET_BASE_URL
    delete process.env.FROGLET_AUTH_TOKEN_PATH
    for (let index = 0; index < server.args.length; index += 1) {
      if (server.args[index] !== "-e") {
        continue
      }
      const [key, ...valueParts] = String(server.args[index + 1] ?? "").split("=")
      process.env[key] = valueParts.join("=")
    }

    const loaded = readConfig()
    assert.equal(loaded.providerUrl, "http://host.docker.internal:8080")
    assert.equal(loaded.runtimeUrl, "http://host.docker.internal:8081")
    assert.equal(loaded.providerAuthTokenPath, "/tokens/froglet-control.token")
    assert.equal(loaded.runtimeAuthTokenPath, "/tokens/auth.token")
  } finally {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) {
        delete process.env[key]
      } else {
        process.env[key] = value
      }
    }
  }
})

test("Codex TOML example keeps the expected MCP stanza", async () => {
  const document = await readFile(path.join(examplesDir, "codex-mcp-config.toml"), "utf8")
  assert.match(document, /^\[mcp_servers\.froglet\]$/m)
  assert.match(document, /^command = "node"$/m)
  assert.match(
    document,
    /^args = \["\/absolute\/path\/to\/froglet\/integrations\/mcp\/froglet\/server\.js"\]$/m
  )
  assert.match(document, /FROGLET_PROVIDER_URL = "http:\/\/127\.0\.0\.1:8080"/)
  assert.match(document, /FROGLET_RUNTIME_AUTH_TOKEN_PATH = "\/absolute\/path\/to\/froglet\/data\/runtime\/auth\.token"/)
})
