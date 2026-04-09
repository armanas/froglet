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
      providerUrl: "http://127.0.0.1:8080",
      runtimeUrl: "http://127.0.0.1:8081",
      providerAuthTokenPath: tokenPath,
      runtimeAuthTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    assert.deepEqual([...tools.keys()], ["froglet"])
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("plugin accepts legacy baseUrl/authTokenPath as fallback", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    // Legacy single-URL config still resolves both providerUrl and runtimeUrl
    const tools = buildTools({
      hostProduct: "openclaw",
      baseUrl: "http://127.0.0.1:8080",
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
  const previousProviderUrl = process.env.FROGLET_PROVIDER_URL
  const previousRuntimeUrl = process.env.FROGLET_RUNTIME_URL
  const previousTokenPath = process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH
  const previousRuntimeTokenPath = process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    process.env.FROGLET_PROVIDER_URL = "http://127.0.0.1:8080"
    process.env.FROGLET_RUNTIME_URL = "http://127.0.0.1:8081"
    process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH = tokenPath
    process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH = tokenPath
    const tools = buildTools({})
    assert.deepEqual([...tools.keys()], ["froglet"])
  } finally {
    if (previousProviderUrl === undefined) {
      delete process.env.FROGLET_PROVIDER_URL
    } else {
      process.env.FROGLET_PROVIDER_URL = previousProviderUrl
    }
    if (previousRuntimeUrl === undefined) {
      delete process.env.FROGLET_RUNTIME_URL
    } else {
      process.env.FROGLET_RUNTIME_URL = previousRuntimeUrl
    }
    if (previousTokenPath === undefined) {
      delete process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH
    } else {
      process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH = previousTokenPath
    }
    if (previousRuntimeTokenPath === undefined) {
      delete process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH
    } else {
      process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH = previousRuntimeTokenPath
    }
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("plugin falls back to legacy FROGLET_BASE_URL env var", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  const previousBaseUrl = process.env.FROGLET_BASE_URL
  const previousAuthTokenPath = process.env.FROGLET_AUTH_TOKEN_PATH
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    process.env.FROGLET_BASE_URL = "http://127.0.0.1:8080"
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

test("create_project action returns isError with project-authoring message", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const tools = buildTools({
      hostProduct: "openclaw",
      providerUrl: "http://127.0.0.1:8080",
      runtimeUrl: "http://127.0.0.1:8081",
      providerAuthTokenPath: tokenPath,
      runtimeAuthTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    const froglet = tools.get("froglet")
    const result = await froglet.definition.execute("tool-1", {
      action: "create_project",
      name: "lol5",
      result_json: "lol5",
      publication_state: "active"
    })
    // create_project is removed — should throw / return error text
    const text = result.content?.[0]?.text ?? ""
    assert.ok(text.includes("not available") || text.includes("Error"), "must surface removal error")
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("plugin description points the model at canonical froglet actions", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const tools = buildTools({
      hostProduct: "openclaw",
      providerUrl: "http://127.0.0.1:8080",
      runtimeUrl: "http://127.0.0.1:8081",
      providerAuthTokenPath: tokenPath,
      runtimeAuthTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    const froglet = tools.get("froglet")
    assert.match(froglet.definition.description, /list_local_services/)
    assert.match(froglet.definition.description, /discover_services/)
    assert.match(froglet.definition.description, /invoke_service/)
    assert.match(froglet.definition.parameters.properties.action.description, /Do not invent actions/)
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("get_local_service hits /v1/provider/services/:id and renders authoritative output", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  const previousFetch = global.fetch
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const tools = buildTools({
      hostProduct: "openclaw",
      providerUrl: "http://127.0.0.1:8080",
      runtimeUrl: "http://127.0.0.1:8081",
      providerAuthTokenPath: tokenPath,
      runtimeAuthTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    global.fetch = async (url) => {
      const urlStr = String(url)
      if (urlStr.includes("/v1/provider/services/ping")) {
        return new Response(
          JSON.stringify({
            service: {
              service_id: "ping",
              offer_id: "ping",
              offer_kind: "compute.execution.v1",
              resource_kind: "service",
              project_id: "ping",
              summary: "Returns pong",
              runtime: "python",
              package_kind: "inline_source",
              entrypoint_kind: "handler",
              entrypoint: "handler.py",
              contract_version: "froglet.compute.python.v1",
              mounts: [{ kind: "filesystem", name: "workspace" }],
              mode: "sync",
              price_sats: 0,
              publication_state: "active",
              provider_id: "provider-1",
              input_schema: null,
              output_schema: { const: "pong" }
            }
          }),
          { status: 200, headers: { "Content-Type": "application/json" } }
        )
      }
      throw new Error(`unexpected URL ${urlStr}`)
    }
    const froglet = tools.get("froglet")
    const result = await froglet.definition.execute("tool-2", {
      action: "get_local_service",
      service_id: "ping"
    })
    const text = result.content?.[0]?.text ?? ""
    assert.match(text, /offer_kind: compute\.execution\.v1/)
    assert.match(text, /resource_kind: service/)
    assert.match(text, /runtime: python/)
    assert.match(text, /package_kind: inline_source/)
    assert.match(text, /entrypoint_kind: handler/)
    assert.match(text, /contract_version: froglet\.compute\.python\.v1/)
    assert.match(text, /mounts: \[\{"kind":"filesystem","name":"workspace"\}\]/)
    assert.match(text, /input_schema: null/)
    assert.match(text, /output_schema: {"const":"pong"}/)
    assert.match(text, /Only listed fields are authoritative/)
    assert.doesNotMatch(text, /template/i)
    assert.doesNotMatch(text, /execution_kind/i)
    assert.doesNotMatch(text, /abi_version/i)
  } finally {
    global.fetch = previousFetch
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("run_compute executes deal flow on target provider URL", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  const previousFetch = global.fetch
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const tools = buildTools({
      hostProduct: "openclaw",
      providerUrl: "http://127.0.0.1:8080",
      runtimeUrl: "http://127.0.0.1:8081",
      providerAuthTokenPath: tokenPath,
      runtimeAuthTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    const hitUrls = []
    global.fetch = async (url, options) => {
      const urlStr = String(url)
      hitUrls.push(urlStr)
      if (urlStr.endsWith("/v1/runtime/deals")) {
        const body = JSON.parse(options.body)
        assert.equal(body.kind, "wasm")
        assert.equal(body.offer_id, "execute.compute")
        assert.equal(body.submission.module_bytes_hex, "0061736d01000000")
        return new Response(
          JSON.stringify({
            provider_id: "prov-1",
            provider_url: "http://127.0.0.2:8080",
            quote: { hash: "quote-hash" },
            deal: { deal_id: "deal-1", status: "succeeded", result: { ok: true } }
          }),
          { status: 200, headers: { "Content-Type": "application/json" } }
        )
      }
      throw new Error(`unexpected URL ${urlStr}`)
    }
    const froglet = tools.get("froglet")
    const result = await froglet.definition.execute("tool-3", {
      action: "run_compute",
      provider_url: "http://127.0.0.2:8080",
      runtime: "wasm",
      package_kind: "inline_module",
      contract_version: "froglet.wasm.run_json.v1",
      wasm_module_hex: "0061736d01000000",
      input: { ping: true }
    })
    const text = result.content?.[0]?.text ?? ""
    assert.match(text, /status: succeeded/)
    assert.match(text, /result: {"ok":true}/)
    assert.ok(hitUrls.some((u) => u.endsWith("/v1/runtime/deals")))
  } finally {
    global.fetch = previousFetch
    await rm(tempDir, { recursive: true, force: true })
  }
})
