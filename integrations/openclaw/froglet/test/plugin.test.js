import assert from "node:assert/strict"
import { mkdtemp, rm, writeFile } from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import test from "node:test"

import register from "../index.js"

function buildTools(config = {}) {
  const tools = new Map()
  register({
    config,
    registerTool(definition, options) {
      tools.set(definition.name, { definition, options: options ?? {} })
    },
    logger: { info() {} }
  })
  return tools
}

test("plugin registers exactly one froglet tool", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const tools = buildTools({
      hostProduct: "openclaw",
      baseUrl: "http://127.0.0.1:9191",
      authTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    assert.deepEqual([...tools.keys()], ["froglet"])
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("plugin falls back to shell env when config is omitted", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  const previousBaseUrl = process.env.FROGLET_BASE_URL
  const previousAuthTokenPath = process.env.FROGLET_AUTH_TOKEN_PATH
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    process.env.FROGLET_BASE_URL = "http://127.0.0.1:9191"
    process.env.FROGLET_AUTH_TOKEN_PATH = tokenPath
    const tools = buildTools({})
    assert.deepEqual([...tools.keys()], ["froglet"])
  } finally {
    if (previousBaseUrl === undefined) {
      delete process.env.FROGLET_BASE_URL
    } else {
      process.env.FROGLET_BASE_URL = previousBaseUrl
    }
    if (previousAuthTokenPath === undefined) {
      delete process.env.FROGLET_AUTH_TOKEN_PATH
    } else {
      process.env.FROGLET_AUTH_TOKEN_PATH = previousAuthTokenPath
    }
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("create_project auto-publishes active services", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  const previousFetch = global.fetch
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const tools = buildTools({
      hostProduct: "openclaw",
      baseUrl: "http://127.0.0.1:9191",
      authTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    const froglet = tools.get("froglet")
    let callCount = 0
    global.fetch = async (url) => {
      callCount += 1
      if (String(url).endsWith("/v1/froglet/projects")) {
        return new Response(
          JSON.stringify({
            project: {
              project_id: "lol5",
              service_id: "lol5",
              offer_id: "lol5",
              summary: "Returns \"lol5\"",
              execution_kind: "wasm_inline",
              mode: "sync",
              price_sats: 0,
              publication_state: "active",
              entrypoint: "source/main.wat"
            }
          }),
          { status: 201, headers: { "Content-Type": "application/json" } }
        )
      }
      if (String(url).endsWith("/v1/froglet/projects/lol5/publish")) {
        return new Response(
          JSON.stringify({
            request_id: "req-1",
            status: "passed",
            evidence: {
              service_id: "lol5",
              offer_id: "lol5"
            }
          }),
          { status: 201, headers: { "Content-Type": "application/json" } }
        )
      }
      throw new Error(`unexpected URL ${url}`)
    }
    const result = await froglet.definition.execute("tool-1", {
      action: "create_project",
      name: "lol5",
      result_json: "lol5",
      publication_state: "active"
    })
    const text = result.content?.[0]?.text ?? ""
    assert.match(text, /published: true/)
    assert.match(text, /published_service_id: lol5/)
    assert.equal(callCount, 2)
  } finally {
    global.fetch = previousFetch
    await rm(tempDir, { recursive: true, force: true })
  }
})
