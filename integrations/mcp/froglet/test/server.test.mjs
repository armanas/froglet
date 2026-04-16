import { after, before, describe, it } from "node:test"
import assert from "node:assert/strict"
import { mkdtemp, writeFile, rm } from "node:fs/promises"
import { join } from "node:path"
import { tmpdir } from "node:os"

import { buildToolDefinitions, handleToolCall } from "../lib/tools.js"

let tmpDir
let tokenPath
let config

function mockFetch(handler) {
  const original = globalThis.fetch
  globalThis.fetch = handler
  return () => {
    globalThis.fetch = original
  }
}

function providerSearchResponse() {
  return {
    providers: [
      {
        provider_id: "prov-1",
        descriptor_hash: "desc-1",
        transport_endpoints: [
          { uri: "https://provider.example", features: ["quote_http"], priority: 1 }
        ],
        offers: [
          {
            offer_hash: "offer-hash-1",
            offer_id: "svc-1",
            offer_kind: "execution",
            runtime: "python",
            settlement_method: "none",
            base_fee_msat: 0,
            success_fee_msat: 0,
            execution_profile: {
              runtime: "python",
              package_kind: "inline_source",
              contract_version: "froglet.python.handler_json.v1",
              access_handles: ["mount.filesystem.read.workspace"]
            }
          }
        ],
        last_seen_at: "2026-04-09T00:00:00Z"
      }
    ],
    cursor: null,
    has_more: false
  }
}

before(async () => {
  tmpDir = await mkdtemp(join(tmpdir(), "froglet-mcp-test-"))
  tokenPath = join(tmpDir, "token")
  await writeFile(tokenPath, "test-token-abc")
  config = {
    providerUrl: "http://127.0.0.1:8080",
    runtimeUrl: "http://127.0.0.1:8081",
    providerAuthTokenPath: tokenPath,
    runtimeAuthTokenPath: tokenPath,
    requestTimeoutMs: 5000,
    defaultSearchLimit: 10,
    maxSearchLimit: 50
  }
})

after(async () => {
  if (tmpDir) await rm(tmpDir, { recursive: true, force: true })
})

describe("tool definitions", () => {
  it("returns a single froglet tool with live actions only", () => {
    const tools = buildToolDefinitions(config)
    assert.equal(tools.length, 1)
    assert.equal(tools[0].name, "froglet")
    const actionEnum = tools[0].inputSchema.properties.action.enum
    assert.deepEqual(actionEnum, [
      "discover_services",
      "get_service",
      "invoke_service",
      "list_local_services",
      "get_local_service",
      "publish_artifact",
      "status",
      "get_task",
      "wait_task",
      "run_compute"
    ])
    assert.match(tools[0].description, /provider_id/)
  })
})

describe("froglet MCP actions", () => {
  it("formats dual-health status output", async () => {
    const restore = mockFetch(async (url) => {
      const urlStr = String(url)
      if (urlStr === "http://127.0.0.1:8080/health") {
        return new Response(JSON.stringify({ healthy: true }))
      }
      if (urlStr === "http://127.0.0.1:8080/v1/node/capabilities") {
        return new Response(JSON.stringify({ compute_offer_ids: ["execute.compute"] }))
      }
      if (urlStr === "http://127.0.0.1:8080/v1/node/identity") {
        return new Response(JSON.stringify({ node_id: "node-1", discovery: { mode: "marketplace" } }))
      }
      if (urlStr === "http://127.0.0.1:8081/health") {
        return new Response(JSON.stringify({ status: "ok" }))
      }
      throw new Error(`unexpected path: ${urlStr}`)
    })
    try {
      const result = await handleToolCall("froglet", { action: "status" }, config)
      const text = result.content[0].text
      assert.match(text, /healthy: true/)
      assert.match(text, /provider_healthy: true/)
      assert.match(text, /runtime_healthy: true/)
      assert.match(text, /node_id: node-1/)
      assert.equal(result.isError, undefined)
    } finally {
      restore()
    }
  })

  it("discovers marketplace providers and renders flattened services", async () => {
    const restore = mockFetch(async (url, opts) => {
      assert.equal(String(url), "http://127.0.0.1:8081/v1/runtime/search")
      assert.deepEqual(JSON.parse(opts.body), { limit: 5, include_inactive: false })
      return new Response(JSON.stringify(providerSearchResponse()))
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "discover_services", limit: 5, query: "svc-1" },
        config
      )
      const text = result.content[0].text
      assert.match(text, /providers: 1/)
      assert.match(text, /services: 1/)
      assert.match(text, /service_id: svc-1/)
      assert.match(text, /provider_id: prov-1/)
    } finally {
      restore()
    }
  })

  it("invokes services through runtime deals after provider lookup and public service fetch", async () => {
    let runtimeDealBody = null
    const restore = mockFetch(async (url, opts) => {
      const urlStr = String(url)
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/providers/prov-1") {
        return new Response(
          JSON.stringify({
            provider: {
              provider_id: "prov-1",
              transport_endpoints: [
                { uri: "https://provider.example", features: ["quote_http"], priority: 1 }
              ]
            }
          })
        )
      }
      if (urlStr === "https://provider.example/v1/provider/services/svc-1") {
        return new Response(
          JSON.stringify({
            service: {
              service_id: "svc-1",
              offer_id: "svc-1",
              provider_id: "prov-1",
              runtime: "python",
              package_kind: "inline_source",
              binding_hash: "feedface"
            }
          })
        )
      }
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/deals") {
        runtimeDealBody = JSON.parse(opts.body)
        return new Response(
          JSON.stringify({
            provider_id: "prov-1",
            provider_url: "https://provider.example",
            quote: { hash: "quote-hash" },
            deal: { deal_id: "deal-1", status: "succeeded", result: "world" }
          })
        )
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    })
    try {
      const result = await handleToolCall(
        "froglet",
        {
          action: "invoke_service",
          provider_id: "prov-1",
          service_id: "svc-1",
          input: { text: "hello" }
        },
        config
      )
      const text = result.content[0].text
      assert.match(text, /status: succeeded/)
      assert.match(text, /result: "world"/)
      assert.equal(runtimeDealBody.offer_id, "svc-1")
      assert.equal(runtimeDealBody.kind, "execution")
      assert.equal(runtimeDealBody.execution.security.service_id, "svc-1")
    } finally {
      restore()
    }
  })

  it("runs compute through runtime deals", async () => {
    let runtimeDealBody = null
    const restore = mockFetch(async (url, opts) => {
      const urlStr = String(url)
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/deals") {
        runtimeDealBody = JSON.parse(opts.body)
        return new Response(
          JSON.stringify({
            provider_id: "prov-1",
            provider_url: "https://provider.example",
            quote: { hash: "quote-hash" },
            deal: { deal_id: "deal-2", status: "succeeded", result: { ok: true } }
          })
        )
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    })
    try {
      const result = await handleToolCall(
        "froglet",
        {
          action: "run_compute",
          provider_id: "prov-1",
          provider_url: "https://provider.example",
          runtime: "wasm",
          package_kind: "inline_module",
          wasm_module_hex: "0061736d01000000"
        },
        config
      )
      const text = result.content[0].text
      assert.match(text, /status: succeeded/)
      assert.match(text, /result: {"ok":true}/)
      assert.equal(runtimeDealBody.kind, "wasm")
      assert.equal(runtimeDealBody.offer_id, "execute.compute")
    } finally {
      restore()
    }
  })

  it("reads runtime deals first and falls back to provider jobs on a shared API base", async () => {
    const sharedSurfaceConfig = { ...config, runtimeUrl: config.providerUrl }
    const restore = mockFetch(async (url) => {
      const urlStr = String(url)
      if (urlStr === "http://127.0.0.1:8080/v1/runtime/deals/task-1") {
        return new Response(JSON.stringify({ error: "deal not found" }), { status: 404 })
      }
      if (urlStr === "http://127.0.0.1:8080/v1/node/jobs/task-1") {
        return new Response(JSON.stringify({ task: { task_id: "task-1", state: "completed", result: 42 } }))
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "get_task", task_id: "task-1" },
        sharedSurfaceConfig
      )
      const text = result.content[0].text
      assert.match(text, /task_id: task-1/)
      assert.match(text, /result: 42/)
    } finally {
      restore()
    }
  })

  it("reports job not found without provider fallback on split deployments", async () => {
    const restore = mockFetch(async (url) => {
      const urlStr = String(url)
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/deals/task-missing") {
        return new Response(JSON.stringify({ error: "deal not found" }), { status: 404 })
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "get_task", task_id: "task-missing" },
        config
      )
      assert.equal(result.isError, true)
      assert.match(result.content[0].text, /job not found/)
    } finally {
      restore()
    }
  })
})

describe("error handling", () => {
  it("returns isError on removed and unknown actions", async () => {
    const removed = await handleToolCall("froglet", { action: "create_project" }, config)
    assert.equal(removed.isError, true)
    assert.match(removed.content[0].text, /not available/)

    const unknown = await handleToolCall("froglet", { action: "explode" }, config)
    assert.equal(unknown.isError, true)
    assert.match(unknown.content[0].text, /Unknown Froglet action/)
  })

  it("returns isError on unknown tool and fetch failures", async () => {
    const unknownTool = await handleToolCall("missing", {}, config)
    assert.equal(unknownTool.isError, true)
    assert.match(unknownTool.content[0].text, /Unknown tool/)

    const restore = mockFetch(async () => {
      throw new Error("connection refused")
    })
    try {
      const result = await handleToolCall("froglet", { action: "list_local_services" }, config)
      assert.equal(result.isError, true)
      assert.match(result.content[0].text, /connection refused/)
    } finally {
      restore()
    }
  })
})
