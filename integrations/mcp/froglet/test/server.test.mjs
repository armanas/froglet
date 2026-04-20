import { after, before, describe, it } from "node:test"
import assert from "node:assert/strict"
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises"
import { join, dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { tmpdir } from "node:os"

import { buildToolDefinitions, handleToolCall } from "../lib/tools.js"

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)
const REPO_ROOT = resolve(__dirname, "../../../../")

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
      "run_compute",
      "get_wallet_balance",
      "list_settlement_activity",
      "get_payment_intent",
      "get_invoice_bundle",
      "get_install_guide",
      "marketplace_search",
      "marketplace_provider",
      "marketplace_receipts",
      "marketplace_stake",
      "marketplace_topup"
    ])
    assert.match(tools[0].description, /provider_id/)
  })
})

describe("froglet MCP actions", () => {
  it("exposes wallet balance through the settlement MCP action", async () => {
    const restore = mockFetch(async (url) => {
      assert.equal(String(url), "http://127.0.0.1:8081/v1/runtime/wallet/balance")
      return new Response(
        JSON.stringify({
          backend: "lightning",
          mode: "mock",
          balance_known: true,
          balance_sats: 4242,
          accepted_payment_methods: ["lightning"],
          reservations: true,
          receipts: true
        })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "get_wallet_balance" },
        config
      )
      const text = result.content[0].text
      assert.match(text, /backend: lightning/)
      assert.match(text, /balance_sats: 4242/)
      assert.equal(result.isError, undefined)
    } finally {
      restore()
    }
  })

  it("surfaces settlement activity rows to the LLM", async () => {
    const restore = mockFetch(async (url) => {
      assert.ok(
        String(url).startsWith("http://127.0.0.1:8081/v1/runtime/settlement/activity"),
        `unexpected url: ${url}`
      )
      return new Response(
        JSON.stringify({
          items: [
            {
              deal_id: "deal-xyz",
              provider_id: "prov-1",
              status: "succeeded",
              workload_kind: "events.query",
              settlement_method: "none",
              base_fee_msat: 0,
              success_fee_msat: 0,
              has_receipt: true,
              has_result: true,
              created_at: 1,
              updated_at: 2
            }
          ],
          limit: 25
        })
      )
    })
    try {
      const result = await handleToolCall(
        "froglet",
        { action: "list_settlement_activity" },
        config
      )
      const text = result.content[0].text
      assert.match(text, /count: 1/)
      assert.match(text, /deal_id: deal-xyz/)
      assert.match(text, /status: succeeded/)
      assert.equal(result.isError, undefined)
    } finally {
      restore()
    }
  })

  it("returns the canonical install block for claude-code + lightning by default", async () => {
    // The helper never makes an HTTP call, so no fetch mock is needed.
    const result = await handleToolCall(
      "froglet",
      { action: "get_install_guide" },
      config
    )
    assert.equal(result.isError, undefined)
    const text = result.content[0].text
    assert.match(text, /target_agent: claude-code/)
    assert.match(text, /payment_rail: lightning/)
    assert.match(text, /run_as: user-host-shell/)
    assert.match(text, /curl -fsSL https:\/\/raw\.githubusercontent\.com\/armanas\/froglet\/main\/scripts\/install\.sh \| sh/)
    assert.match(text, /\.\/scripts\/setup-agent\.sh --target claude-code/)
    assert.match(text, /\.\/scripts\/setup-payment\.sh lightning/)
    assert.match(text, /docker compose up --build -d/)
    assert.match(text, /FROGLET_HOST_READABLE_CONTROL_TOKEN=true/)
  })

  it("swaps the agent and rail placeholders when the LLM picks different targets", async () => {
    const result = await handleToolCall(
      "froglet",
      { action: "get_install_guide", target_agent: "codex", payment_rail: "stripe" },
      config
    )
    assert.equal(result.isError, undefined)
    const text = result.content[0].text
    assert.match(text, /target_agent: codex/)
    assert.match(text, /payment_rail: stripe/)
    assert.match(text, /\.\/scripts\/setup-agent\.sh --target codex/)
    assert.match(text, /FROGLET_STRIPE_SECRET_KEY=sk_test_\.\.\. \.\/scripts\/setup-payment\.sh stripe/)
    assert.match(text, /set -a && \. \.\/\.froglet\/payment\/stripe\.env/)
  })

  it("rejects unknown target_agent or payment_rail with a clear error", async () => {
    const badAgent = await handleToolCall(
      "froglet",
      { action: "get_install_guide", target_agent: "emacs" },
      config
    )
    assert.equal(badAgent.isError, true)
    assert.match(badAgent.content[0].text, /target_agent must be one of/)

    const badRail = await handleToolCall(
      "froglet",
      { action: "get_install_guide", payment_rail: "gold-pieces" },
      config
    )
    assert.equal(badRail.isError, true)
    assert.match(badRail.content[0].text, /payment_rail must be one of/)
  })

  it("keeps the install guide synchronized with README and docs-site quickstart", async () => {
    // The canonical copy-paste block lives in three places today: the MCP
    // action response, README.md, and docs-site/.../quickstart.mdx. If any
    // one drifts, humans and LLMs will disagree about what to run. This
    // test fails if the MCP output no longer matches the README block.
    const result = await handleToolCall(
      "froglet",
      { action: "get_install_guide" },
      config
    )
    const text = result.content[0].text
    const steps = text
      .split("\n")
      .filter((line) => /^\s*\d+\.\s+/.test(line))
      .map((line) => line.replace(/^\s*\d+\.\s+/, "").trim())
    assert.equal(steps.length, 4, `expected 4 commands, got ${steps.length}`)

    const readme = await readFile(join(REPO_ROOT, "README.md"), "utf8")
    for (const step of steps) {
      assert.ok(
        readme.includes(step),
        `README.md is missing install-guide step: ${step}`
      )
    }

    const quickstart = await readFile(
      join(REPO_ROOT, "docs-site/src/content/docs/learn/quickstart.mdx"),
      "utf8"
    )
    for (const step of steps) {
      assert.ok(
        quickstart.includes(step),
        `docs-site quickstart is missing install-guide step: ${step}`
      )
    }
  })

  it("marketplace_search routes through invoke_service with marketplace.search", async () => {
    // Natural flow: no provider_url in args (that would trigger the
    // LLM-controlled-URL validator + pinned fetch, which bypasses the
    // global.fetch mock). Discovery resolves the marketplace provider
    // through runtime search, matching how the operator-configured
    // FROGLET_MARKETPLACE_URL is exposed in production.
    let capturedBody
    const restore = mockFetch(async (url, opts) => {
      const urlStr = String(url)
      if (urlStr.endsWith("/v1/runtime/search")) {
        return new Response(
          JSON.stringify({
            providers: [
              {
                provider_id: "prov-mkt",
                descriptor_hash: "desc-mkt",
                transport_endpoints: [
                  { uri: "https://mkt.example", features: ["quote_http"], priority: 1 }
                ],
                offers: [
                  {
                    offer_hash: "marketplace-search-hash",
                    offer_id: "marketplace.search",
                    offer_kind: "builtin",
                    runtime: "builtin"
                  }
                ],
                last_seen_at: "2026-04-16T00:00:00Z"
              }
            ]
          }),
          { status: 200 }
        )
      }
      if (urlStr === "https://mkt.example/v1/provider/services/marketplace.search") {
        return new Response(
          JSON.stringify({
            service: {
              service_id: "marketplace.search",
              offer_id: "marketplace.search",
              provider_id: "prov-mkt",
              runtime: "builtin",
              package_kind: "builtin",
              binding_hash: "marketplace-search-binding-hash"
            }
          }),
          { status: 200 }
        )
      }
      if (urlStr.endsWith("/v1/runtime/deals")) {
        capturedBody = JSON.parse(opts.body)
        return new Response(
          JSON.stringify({
            provider_id: "prov-mkt",
            provider_url: "https://mkt.example",
            deal: { status: "succeeded", result: { providers: [{ provider_id: "prov-9" }] } },
            result: { providers: [{ provider_id: "prov-9" }], has_more: false }
          }),
          { status: 200 }
        )
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    })
    try {
      const result = await handleToolCall(
        "froglet",
        {
          action: "marketplace_search",
          offer_kind: "named.v1",
          runtime: "python",
          limit: 25
        },
        config
      )
      assert.equal(result.isError, undefined, result.content?.[0]?.text)
      assert.ok(capturedBody, "expected a runtime deal invocation")
      const body = JSON.stringify(capturedBody)
      assert.match(body, /marketplace\.search/)
      assert.match(body, /named\.v1/)
      assert.match(body, /python/)
    } finally {
      restore()
    }
  })

  it("marketplace_stake requires marketplace_provider_id", async () => {
    const result = await handleToolCall(
      "froglet",
      { action: "marketplace_stake", amount_msat: 1000 },
      config
    )
    assert.equal(result.isError, true)
    assert.match(result.content[0].text, /marketplace_provider_id is required/)
  })

  it("marketplace_stake requires a positive amount_msat", async () => {
    const result = await handleToolCall(
      "froglet",
      {
        action: "marketplace_stake",
        marketplace_provider_id: "prov-1",
        amount_msat: 0
      },
      config
    )
    assert.equal(result.isError, true)
    assert.match(result.content[0].text, /amount_msat must be a positive number/)
  })

  it("marketplace_topup forwards provider_id and amount_msat", async () => {
    let capturedInput
    const restore = mockFetch(async (url, opts) => {
      const urlStr = String(url)
      if (urlStr.endsWith("/v1/runtime/search")) {
        return new Response(
          JSON.stringify({
            providers: [
              {
                provider_id: "prov-mkt",
                descriptor_hash: "desc-mkt",
                transport_endpoints: [
                  { uri: "https://mkt.example", features: ["quote_http"], priority: 1 }
                ],
                offers: [
                  {
                    offer_hash: "marketplace-topup-hash",
                    offer_id: "marketplace.topup",
                    offer_kind: "builtin",
                    runtime: "builtin"
                  }
                ],
                last_seen_at: "2026-04-16T00:00:00Z"
              }
            ]
          }),
          { status: 200 }
        )
      }
      if (urlStr === "https://mkt.example/v1/provider/services/marketplace.topup") {
        return new Response(
          JSON.stringify({
            service: {
              service_id: "marketplace.topup",
              offer_id: "marketplace.topup",
              provider_id: "prov-mkt",
              runtime: "builtin",
              package_kind: "builtin",
              binding_hash: "marketplace-topup-binding-hash"
            }
          }),
          { status: 200 }
        )
      }
      if (urlStr.endsWith("/v1/runtime/deals")) {
        capturedInput = JSON.parse(opts.body)
        return new Response(
          JSON.stringify({
            provider_id: "prov-mkt",
            provider_url: "https://mkt.example",
            deal: { status: "succeeded", result: { total_staked_msat: 2000 } },
            result: { total_staked_msat: 2000 }
          }),
          { status: 200 }
        )
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    })
    try {
      const result = await handleToolCall(
        "froglet",
        {
          action: "marketplace_topup",
          marketplace_provider_id: "prov-7",
          amount_msat: 1000
        },
        config
      )
      assert.equal(result.isError, undefined, result.content?.[0]?.text)
      const body = JSON.stringify(capturedInput)
      assert.match(body, /marketplace\.topup/)
      assert.match(body, /prov-7/)
      assert.match(body, /1000/)
    } finally {
      restore()
    }
  })

  it("returns isError when get_payment_intent is called without a deal_id", async () => {
    const result = await handleToolCall(
      "froglet",
      { action: "get_payment_intent" },
      config
    )
    assert.equal(result.isError, true)
    assert.match(result.content[0].text, /deal_id is required/)
  })

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
      // Use a public https literal so the LLM-controlled provider_url
      // validator passes without real DNS; the runtime call is intercepted by
      // mockFetch, so no outbound traffic reaches 1.1.1.1.
      const result = await handleToolCall(
        "froglet",
        {
          action: "run_compute",
          provider_id: "prov-1",
          provider_url: "https://1.1.1.1",
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

  it("runs compute through runtime deals with the operator-configured provider fallback", async () => {
    let runtimeDealBody = null
    const restore = mockFetch(async (url, opts) => {
      const urlStr = String(url)
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/deals") {
        runtimeDealBody = JSON.parse(opts.body)
        return new Response(
          JSON.stringify({
            provider_url: "http://127.0.0.1:8080",
            quote: { hash: "quote-hash" },
            deal: { deal_id: "deal-2b", status: "succeeded", result: { ok: true } }
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
          runtime: "wasm",
          package_kind: "inline_module",
          wasm_module_hex: "0061736d01000000"
        },
        config
      )
      const text = result.content[0].text
      assert.match(text, /status: succeeded/)
      assert.match(text, /result: {"ok":true}/)
      assert.deepEqual(runtimeDealBody.provider, {
        provider_id: "prov-1",
        provider_url: "http://127.0.0.1:8080"
      })
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
