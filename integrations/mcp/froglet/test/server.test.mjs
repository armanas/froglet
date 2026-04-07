import { describe, it, before, after, beforeEach } from "node:test"
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

before(async () => {
  tmpDir = await mkdtemp(join(tmpdir(), "froglet-mcp-test-"))
  tokenPath = join(tmpDir, "token")
  await writeFile(tokenPath, "test-token-abc")
  config = {
    baseUrl: "http://127.0.0.1:9191",
    authTokenPath: tokenPath,
    requestTimeoutMs: 5000,
    defaultSearchLimit: 10,
    maxSearchLimit: 50
  }
})

after(async () => {
  if (tmpDir) await rm(tmpDir, { recursive: true, force: true })
})

describe("tool definitions", () => {
  it("returns a single froglet tool", () => {
    const tools = buildToolDefinitions(config)
    assert.equal(tools.length, 1)
    const names = tools.map((t) => t.name)
    assert.deepEqual(names, ["froglet"])
  })

  it("each tool has name, description, and inputSchema", () => {
    const tools = buildToolDefinitions(config)
    for (const tool of tools) {
      assert.ok(typeof tool.name === "string" && tool.name.length > 0)
      assert.ok(typeof tool.description === "string" && tool.description.length > 0)
      assert.ok(tool.inputSchema && typeof tool.inputSchema === "object")
      assert.equal(tool.inputSchema.type, "object")
    }
  })
})

describe("froglet status action", () => {
  it("returns formatted status text", async () => {
    const restore = mockFetch(async (url, opts) => {
      assert.ok(url.endsWith("/v1/froglet/status"))
      assert.equal(opts.headers.Authorization, "Bearer test-token-abc")
      return new Response(
        JSON.stringify({
          service: "froglet",
          healthy: true,
          node_id: "node-1",
          components: {
            runtime: { healthy: true },
            provider: { healthy: true }
          },
          discovery: { mode: "reference" },
          reference_discovery: { enabled: true, connected: true, url: "http://disc.example" },
          projects_root: "/tmp/projects",
          raw_compute_offer_id: "compute.v1",
          raw_compute_offer_ids: ["compute.v1", "compute.generic.v1"]
        })
      )
    })
    try {
      const result = await handleToolCall("froglet", { action: "status" }, config)
      assert.equal(result.content.length, 1)
      assert.equal(result.content[0].type, "text")
      assert.ok(result.content[0].text.includes("service: froglet"))
      assert.ok(result.content[0].text.includes("healthy: true"))
      assert.ok(result.content[0].text.includes("node_id: node-1"))
      assert.ok(result.content[0].text.includes("runtime_healthy: true"))
      assert.ok(result.content[0].text.includes("compute_offer_ids: compute.v1, compute.generic.v1"))
      assert.equal(result.isError, undefined)
    } finally {
      restore()
    }
  })
})

describe("froglet discover_services action", () => {
  it("sends correct request and formats services", async () => {
    let capturedBody
    const restore = mockFetch(async (url, opts) => {
      assert.ok(url.endsWith("/v1/froglet/services/discover"))
      capturedBody = JSON.parse(opts.body)
      return new Response(
        JSON.stringify({
          services: [
            {
              service_id: "svc-1",
              offer_id: "offer-1",
              runtime: "python",
              price_sats: 5,
              provider_id: "prov-1"
            }
          ],
          provider_nodes_discovered: 1,
          provider_fetch_failures: []
        })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "discover_services", query: "test", limit: 5 },
        config
      )
      assert.equal(capturedBody.query, "test")
      assert.equal(capturedBody.limit, 5)
      assert.ok(result.content[0].text.includes("service_id: svc-1"))
      assert.ok(result.content[0].text.includes("services: 1"))
    } finally {
      restore()
    }
  })
})

describe("froglet invoke_service action", () => {
  it("returns sync result", async () => {
    const restore = mockFetch(async (url, opts) => {
      assert.ok(url.endsWith("/v1/froglet/services/invoke"))
      const body = JSON.parse(opts.body)
      assert.equal(body.service_id, "svc-1")
      assert.deepEqual(body.input, { text: "hello" })
      return new Response(
        JSON.stringify({ status: "ok", result: "world" })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "invoke_service", service_id: "svc-1", input: { text: "hello" } },
        config
      )
      assert.ok(result.content[0].text.includes('result: "world"'))
      assert.ok(result.content[0].text.includes("status: ok"))
    } finally {
      restore()
    }
  })

  it("returns async task reference", async () => {
    const restore = mockFetch(async () =>
      new Response(
        JSON.stringify({
          task: { task_id: "task-1", status: "running", provider_id: "prov-1" },
          terminal: false
        })
      )
    )
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "invoke_service", service_id: "svc-1" },
        config
      )
      assert.ok(result.content[0].text.includes("task_id: task-1"))
      assert.ok(result.content[0].text.includes("pending: use wait_task"))
    } finally {
      restore()
    }
  })
})

describe("froglet local service actions", () => {
  it("lists all local services when no service_id", async () => {
    const restore = mockFetch(async (url) => {
      assert.ok(url.endsWith("/v1/froglet/services/local"))
      return new Response(
        JSON.stringify({
          services: [
            { service_id: "local-1", runtime: "wasm" },
            { service_id: "local-2", runtime: "python" }
          ]
        })
      )
    })
    try {
      const result = await handleToolCall("froglet", { action: "list_local_services" }, config)
      assert.ok(result.content[0].text.includes("services: 2"))
      assert.ok(result.content[0].text.includes("service_id: local-1"))
      assert.ok(result.content[0].text.includes("service_id: local-2"))
    } finally {
      restore()
    }
  })

  it("gets specific local service when service_id given", async () => {
    const restore = mockFetch(async (url) => {
      assert.ok(url.includes("/v1/froglet/services/local/local-1"))
      return new Response(
        JSON.stringify({
          service: { service_id: "local-1", runtime: "wasm", price_sats: 0 }
        })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "get_local_service", service_id: "local-1" },
        config
      )
      assert.ok(result.content[0].text.includes("service_id: local-1"))
      assert.ok(result.content[0].text.includes("input_contract:"))
    } finally {
      restore()
    }
  })

  it("resolves seeded local service aliases", async () => {
    const restore = mockFetch(async (url) => {
      assert.ok(url.includes("/v1/froglet/services/local/local-2"))
      return new Response(
        JSON.stringify({
          service: { service_id: "local-2", runtime: "python", price_sats: 0 }
        })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "get_local_service", async_service_id: "local-2" },
        config
      )
      assert.ok(result.content[0].text.includes("service_id: local-2"))
      assert.ok(result.content[0].text.includes("runtime: python"))
    } finally {
      restore()
    }
  })
})

describe("froglet project actions", () => {
  it("lists projects", async () => {
    const restore = mockFetch(async (url) => {
      assert.ok(url.endsWith("/v1/froglet/projects"))
      return new Response(
        JSON.stringify({
          projects: [{ project_id: "proj-1", service_id: "svc-1" }]
        })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "list_projects" },
        config
      )
      assert.ok(result.content[0].text.includes("projects: 1"))
      assert.ok(result.content[0].text.includes("project_id: proj-1"))
    } finally {
      restore()
    }
  })

  it("creates and auto-publishes active projects", async () => {
    let callCount = 0
    const restore = mockFetch(async (url, opts) => {
      callCount++
      assert.ok(url.endsWith("/v1/froglet/projects"))
      const body = JSON.parse(opts.body)
      assert.equal(body.name, "ping")
      assert.equal(body.result_json, "pong")
      assert.equal(body.publication_state, "active")
      return new Response(
        JSON.stringify({
          project: {
            project_id: "ping",
            service_id: "ping",
            offer_id: "ping",
            publication_state: "active"
          }
        }),
        { status: 201 }
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        {
          action: "create_project",
          name: "ping",
          result_json: "pong",
          price_sats: 0,
          publication_state: "active"
        },
        config
      )
      assert.equal(callCount, 1)
      assert.ok(result.content[0].text.includes("published: true"))
      assert.ok(result.content[0].text.includes("publish_status: already_published"))
      assert.ok(result.content[0].text.includes("published_service_id: ping"))
    } finally {
      restore()
    }
  })

  it("creates hidden project without auto-publish", async () => {
    let callCount = 0
    const restore = mockFetch(async (url) => {
      callCount++
      return new Response(
        JSON.stringify({
          project: {
            project_id: "my-proj",
            service_id: "my-svc",
            publication_state: "hidden"
          }
        }),
        { status: 201 }
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "create_project", name: "my-proj" },
        config
      )
      assert.equal(callCount, 1)
      assert.ok(result.content[0].text.includes("published: false"))
      assert.ok(result.content[0].text.includes("next_step:"))
    } finally {
      restore()
    }
  })

  it("rejects unknown action", async () => {
    const result = await handleToolCall("froglet", { action: "explode" }, config)
    assert.equal(result.isError, true)
    assert.ok(result.content[0].text.includes("Unknown Froglet action"))
  })
})

describe("froglet task actions", () => {
  it("gets task status without wait", async () => {
    const restore = mockFetch(async (url, opts) => {
      assert.ok(url.includes("/v1/froglet/tasks/task-1"))
      assert.equal(opts.method, "GET")
      return new Response(
        JSON.stringify({
          task: { task_id: "task-1", status: "succeeded", result: 42 }
        })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "get_task", task_id: "task-1" },
        config
      )
      assert.ok(result.content[0].text.includes("task_id: task-1"))
      assert.ok(result.content[0].text.includes("status: succeeded"))
    } finally {
      restore()
    }
  })

  it("waits for task when wait=true", async () => {
    const restore = mockFetch(async (url, opts) => {
      assert.ok(url.includes("/v1/froglet/tasks/task-2/wait"))
      assert.equal(opts.method, "POST")
      const body = JSON.parse(opts.body)
      assert.equal(body.timeout_secs, 30)
      return new Response(
        JSON.stringify({
          task: { task_id: "task-2", status: "succeeded", result: "done" }
        })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "wait_task", task_id: "task-2", timeout_secs: 30 },
        config
      )
      assert.ok(result.content[0].text.includes("status: succeeded"))
    } finally {
      restore()
    }
  })
})

describe("froglet run_compute action", () => {
  it("sends inline Wasm compute requests", async () => {
    const restore = mockFetch(async (url, opts) => {
      assert.ok(url.endsWith("/v1/froglet/compute/run"))
      assert.equal(opts.method, "POST")
      const body = JSON.parse(opts.body)
      assert.equal(body.runtime, "wasm")
      assert.equal(body.package_kind, "inline_module")
      assert.equal(body.wasm_module_hex, "0061736d01000000")
      return new Response(
        JSON.stringify({
          status: "succeeded",
          result: { ok: true }
        })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        {
          action: "run_compute",
          runtime: "wasm",
          package_kind: "inline_module",
          contract_version: "froglet.wasm.run_json.v1",
          wasm_module_hex: "0061736d01000000",
          input: { ping: true }
        },
        config
      )
      assert.ok(result.content[0].text.includes('result: {"ok":true}'))
      assert.ok(result.content[0].text.includes("status: succeeded"))
    } finally {
      restore()
    }
  })
})

describe("error handling", () => {
  it("returns isError on unknown tool", async () => {
    const result = await handleToolCall("nonexistent_tool", {}, config)
    assert.equal(result.isError, true)
    assert.ok(result.content[0].text.includes("Unknown tool"))
  })

  it("returns isError on fetch failure", async () => {
    const restore = mockFetch(async () => {
      throw new Error("connection refused")
    })
    try {
      const result = await handleToolCall("froglet", { action: "status" }, config)
      assert.equal(result.isError, true)
      assert.ok(result.content[0].text.includes("connection refused"))
    } finally {
      restore()
    }
  })

  it("returns isError on non-200 response", async () => {
    const restore = mockFetch(async () =>
      new Response(JSON.stringify({ error: "not found" }), { status: 404 })
    )
    try {
      const result = await handleToolCall("froglet", { action: "status" }, config)
      assert.equal(result.isError, true)
      assert.ok(result.content[0].text.includes("failed with 404"))
    } finally {
      restore()
    }
  })
})
