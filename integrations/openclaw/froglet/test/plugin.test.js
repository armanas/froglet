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
              runtime: "python",
              package_kind: "inline_source",
              entrypoint_kind: "handler",
              entrypoint: "handler.py",
              contract_version: "froglet.compute.python.v1",
              mounts: [],
              mode: "sync",
              price_sats: 0,
              publication_state: "active",
              input_schema: null,
              output_schema: { const: "lol5" }
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
    assert.match(text, /publish_status: already_published/)
    assert.match(text, /published_service_id: lol5/)
    assert.equal(callCount, 1)
  } finally {
    global.fetch = previousFetch
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
      baseUrl: "http://127.0.0.1:9191",
      authTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    const froglet = tools.get("froglet")
    assert.match(froglet.definition.description, /list_local_services/)
    assert.match(froglet.definition.description, /discover_services/)
    assert.match(froglet.definition.description, /invoke_service/)
    assert.match(froglet.definition.description, /result_json="pong"/)
    assert.match(froglet.definition.description, /runtime/)
    assert.match(froglet.definition.description, /package_kind/)
    assert.match(froglet.definition.description, /entrypoint_kind/)
    assert.match(froglet.definition.description, /contract_version/)
    assert.match(froglet.definition.description, /mounts/)
    assert.match(froglet.definition.parameters.properties.action.description, /Do not invent actions/)
    assert.match(
      froglet.definition.parameters.properties.summary.description,
      /Summary never generates code/
    )
    assert.match(
      froglet.definition.parameters.properties.result_json.description,
      /Use this for simple constant-return services/
    )
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("get_local_service output stays authoritative and schema-based", async () => {
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
    global.fetch = async (url) => {
      if (String(url).endsWith("/v1/froglet/services/local/ping")) {
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
      throw new Error(`unexpected URL ${url}`)
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

test("run_compute forwards inline Wasm requests", async () => {
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
    global.fetch = async (url, options) => {
      if (String(url).endsWith("/v1/froglet/compute/run")) {
        const body = JSON.parse(options.body)
        assert.equal(body.runtime, "wasm")
        assert.equal(body.package_kind, "inline_module")
        assert.equal(body.wasm_module_hex, "0061736d01000000")
        return new Response(
          JSON.stringify({ status: "succeeded", result: { ok: true } }),
          { status: 200, headers: { "Content-Type": "application/json" } }
        )
      }
      throw new Error(`unexpected URL ${url}`)
    }
    const froglet = tools.get("froglet")
    const result = await froglet.definition.execute("tool-3", {
      action: "run_compute",
      runtime: "wasm",
      package_kind: "inline_module",
      contract_version: "froglet.wasm.run_json.v1",
      wasm_module_hex: "0061736d01000000",
      input: { ping: true }
    })
    const text = result.content?.[0]?.text ?? ""
    assert.match(text, /status: succeeded/)
    assert.match(text, /result: {"ok":true}/)
  } finally {
    global.fetch = previousFetch
    await rm(tempDir, { recursive: true, force: true })
  }
})
