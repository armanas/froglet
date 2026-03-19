import assert from "node:assert/strict"
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import test from "node:test"

import register from "../index.js"
import { assertAgentTranscript, extractJsonSection } from "./matrix-assertions.mjs"

function buildTestApi(config = {}) {
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

function jsonResponse(res, statusCode, payload) {
  res.writeHead(statusCode, { "content-type": "application/json" })
  res.end(JSON.stringify(payload))
}

async function readJsonRequest(req) {
  const chunks = []
  for await (const chunk of req) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk))
  }
  if (chunks.length === 0) {
    return null
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"))
}

async function withRuntimeServer(handler, fn) {
  const server = http.createServer(handler)
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
  const address = server.address()
  const runtimeUrl = `http://127.0.0.1:${address.port}`
  try {
    await fn(runtimeUrl)
  } finally {
    await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())))
  }
}

test("registers only runtime-centric Froglet tools", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const tools = buildTestApi({
      runtimeUrl: "http://127.0.0.1:8081",
      runtimeAuthTokenPath: tokenPath
    })
    assert.deepEqual(
      [...tools.keys()].sort(),
      [
        "froglet_accept_result",
        "froglet_buy",
        "froglet_get_provider",
        "froglet_payment_intent",
        "froglet_search",
        "froglet_wait_deal",
        "froglet_wallet_balance"
      ]
    )
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("froglet_search and froglet_get_provider go through the local runtime", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  const seen = []
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    await withRuntimeServer(async (req, res) => {
      const body = await readJsonRequest(req)
      seen.push({ method: req.method, url: req.url, body, auth: req.headers.authorization })
      if (req.method === "POST" && req.url === "/v1/runtime/search") {
        return jsonResponse(res, 200, {
          nodes: [
            {
              descriptor: {
                node_id: "provider-1",
                version: "0.1.0",
                transports: {
                  clearnet_url: "https://provider.example",
                  onion_url: null,
                  tor_status: "disabled"
                },
                services: [
                  { service_id: "execute.wasm", payment_required: true, price_sats: 10 }
                ]
              },
              status: "active",
              last_seen_at: 1_700_000_000
            }
          ]
        })
      }
      if (req.method === "GET" && req.url === "/v1/runtime/providers/provider-1") {
        return jsonResponse(res, 200, {
          discovery: {
            descriptor: {
              node_id: "provider-1",
              version: "0.1.0",
              transports: {
                clearnet_url: "https://provider.example",
                onion_url: null,
                tor_status: "disabled"
              },
              services: [{ service_id: "execute.wasm", payment_required: true, price_sats: 10 }]
            },
            status: "active",
            last_seen_at: 1_700_000_000
          },
          descriptor: {
            payload: {
              provider_id: "provider-1",
              protocol_version: "froglet/v1",
              descriptor_seq: 1,
              capabilities: {
                service_kinds: ["compute.wasm.v1"],
                execution_runtimes: ["wasm"],
                max_concurrent_deals: 4
              },
              transport_endpoints: [{ transport: "https", uri: "https://provider.example" }],
              linked_identities: []
            }
          },
          offers: [
            {
              payload: {
                offer_id: "execute.wasm",
                offer_kind: "compute.wasm.v1",
                settlement_method: "lightning.base_fee_plus_success_fee.v1",
                quote_ttl_secs: 60,
                price_schedule: { base_fee_msat: 0, success_fee_msat: 10_000 }
              }
            }
          ]
        })
      }
      res.statusCode = 404
      res.end()
    }, async (runtimeUrl) => {
      const tools = buildTestApi({
        runtimeUrl,
        runtimeAuthTokenPath: tokenPath
      })

      const search = await tools.get("froglet_search").definition.execute("1", {
        limit: 5,
        include_raw: true
      })
      const provider = await tools
        .get("froglet_get_provider")
        .definition.execute("2", { provider_id: "provider-1", include_raw: true })

      const searchRaw = extractJsonSection(search.content[0].text, "search_response_json:")
      const providerRaw = extractJsonSection(provider.content[0].text, "provider_response_json:")

      assert.equal(searchRaw.nodes[0].descriptor.node_id, "provider-1")
      assert.equal(providerRaw.discovery.descriptor.node_id, "provider-1")
      assert.equal(providerRaw.descriptor.payload.provider_id, "provider-1")

      assertAgentTranscript(search.content[0].text, {
        mustContain: ["runtime_url:", "returned_nodes: 1", "provider-1"],
        mustContainOrdered: ["runtime_url:", "returned_nodes: 1", "provider-1"]
      })
      assertAgentTranscript(provider.content[0].text, {
        mustContain: ["offers_returned: 1", "offer_id=execute.wasm"],
        mustContainOrdered: ["provider_id: provider-1", "offers_returned: 1", "offer_id=execute.wasm"]
      })
    })
    assert.equal(seen[0].auth, "Bearer froglet-test-token")
    assert.equal(seen[0].url, "/v1/runtime/search")
    assert.equal(seen[1].url, "/v1/runtime/providers/provider-1")
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("buy, wait, payment intent, accept, and wallet tools use runtime-only endpoints", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-plugin-"))
  const dealStates = [
    { status: "payment_pending", result_hash: null, receipt: null },
    { status: "result_ready", result_hash: "ab".repeat(32), receipt: null },
    {
      status: "succeeded",
      result_hash: "ab".repeat(32),
      receipt: { hash: "receipt-1" }
    }
  ]
  let dealReads = 0
  const seen = []
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    await withRuntimeServer(async (req, res) => {
      const body = await readJsonRequest(req)
      seen.push({ method: req.method, url: req.url, body })
      if (req.method === "GET" && req.url === "/v1/runtime/wallet/balance") {
        return jsonResponse(res, 200, {
          backend: "lightning",
          mode: "mock",
          balance_known: true,
          balance_sats: 21,
          accepted_payment_methods: ["lightning"]
        })
      }
      if (req.method === "POST" && req.url === "/v1/runtime/deals") {
        return jsonResponse(res, 200, {
          quote: { hash: "quote-1" },
          deal: {
            deal_id: "deal-1",
            provider_id: "provider-1",
            provider_url: "https://provider.example",
            status: "payment_pending",
            receipt: null,
            result_hash: null
          },
          payment_intent_path: "/v1/runtime/deals/deal-1/payment-intent",
          payment_intent: {
            backend: "lightning",
            session_id: "session-1",
            deal_status: "payment_pending",
            admission_ready: false,
            result_ready: false,
            can_release_preimage: false,
            payment_requests: [],
            release_action: null
          }
        })
      }
      if (req.method === "GET" && req.url === "/v1/runtime/deals/deal-1") {
        const current = dealStates[Math.min(dealReads, dealStates.length - 1)]
        dealReads += 1
        return jsonResponse(res, 200, {
          deal: {
            deal_id: "deal-1",
            provider_id: "provider-1",
            provider_url: "https://provider.example",
            status: current.status,
            result_hash: current.result_hash,
            receipt: current.receipt
          }
        })
      }
      if (req.method === "GET" && req.url === "/v1/runtime/deals/deal-1/payment-intent") {
        return jsonResponse(res, 200, {
          payment_intent: {
            backend: "lightning",
            session_id: "session-1",
            deal_status: "result_ready",
            admission_ready: true,
            result_ready: true,
            can_release_preimage: true,
            payment_requests: [],
            release_action: {
              endpoint_path: "/v1/runtime/deals/deal-1/accept",
              expected_result_hash: "ab".repeat(32)
            }
          }
        })
      }
      if (req.method === "POST" && req.url === "/v1/runtime/deals/deal-1/accept") {
        return jsonResponse(res, 200, {
          deal: {
            deal_id: "deal-1",
            provider_id: "provider-1",
            provider_url: "https://provider.example",
            status: "succeeded",
            result_hash: "ab".repeat(32),
            receipt: { hash: "receipt-1" }
          }
        })
      }
      res.statusCode = 404
      res.end()
    }, async (runtimeUrl) => {
      const tools = buildTestApi({
        runtimeUrl,
        runtimeAuthTokenPath: tokenPath
      })

      const wallet = await tools.get("froglet_wallet_balance").definition.execute("1", {
        include_raw: true
      })
      const buy = await tools.get("froglet_buy").definition.execute("2", {
        request: {
          provider: { provider_id: "provider-1" },
          offer_id: "execute.wasm",
          submission: { wasm_module_hex: "00" }
        },
        include_raw: true
      })
      const wait = await tools.get("froglet_wait_deal").definition.execute("3", {
        deal_id: "deal-1",
        wait_statuses: ["result_ready"],
        include_raw: true
      })
      const intent = await tools.get("froglet_payment_intent").definition.execute("4", {
        deal_id: "deal-1",
        include_raw: true
      })
      const accept = await tools.get("froglet_accept_result").definition.execute("5", {
        deal_id: "deal-1",
        include_raw: true
      })

      const walletRaw = extractJsonSection(wallet.content[0].text, "wallet_balance_response_json:")
      const buyRaw = extractJsonSection(buy.content[0].text, "buy_response_json:")
      const waitRaw = extractJsonSection(wait.content[0].text, "wait_response_json:")
      const intentRaw = extractJsonSection(intent.content[0].text, "payment_intent_response_json:")
      const acceptRaw = extractJsonSection(accept.content[0].text, "accept_response_json:")

      assert.equal(walletRaw.backend, "lightning")
      assert.equal(walletRaw.balance_sats, 21)
      assert.equal(buyRaw.deal.deal_id, "deal-1")
      assert.equal(buyRaw.payment_intent_path, "/v1/runtime/deals/deal-1/payment-intent")
      assert.equal(waitRaw.deal.status, "result_ready")
      assert.equal(intentRaw.payment_intent.release_action.endpoint_path, "/v1/runtime/deals/deal-1/accept")
      assert.equal(acceptRaw.deal.status, "succeeded")

      assertAgentTranscript(wallet.content[0].text, {
        mustContain: ["runtime_url:", "backend: lightning", "balance_sats: 21"],
        mustContainOrdered: ["runtime_url:", "backend: lightning", "balance_sats: 21"]
      })
      assertAgentTranscript(buy.content[0].text, {
        mustContain: ["deal_id: deal-1", "status: payment_pending", "payment_intent_path: /v1/runtime/deals/deal-1/payment-intent"],
        mustContainOrdered: ["runtime_url:", "deal_id: deal-1", "status: payment_pending"]
      })
      assertAgentTranscript(wait.content[0].text, {
        mustContain: ["deal_id: deal-1", "status: result_ready"],
        mustContainOrdered: ["runtime_url:", "wait_statuses: result_ready", "status: result_ready"]
      })
      assertAgentTranscript(intent.content[0].text, {
        mustContain: ["deal_id: deal-1", "release_endpoint: /v1/runtime/deals/deal-1/accept"],
        mustContainOrdered: ["runtime_url:", "deal_id: deal-1", "release_endpoint: /v1/runtime/deals/deal-1/accept"]
      })
      assertAgentTranscript(accept.content[0].text, {
        mustContain: ["deal_id: deal-1", "status: succeeded", "receipt_hash: receipt-1"],
        mustContainOrdered: ["runtime_url:", "deal_id: deal-1", "status: succeeded"]
      })
    })

    assert.ok(seen.every((entry) => !String(entry.url).includes("/v1/provider/")))
    assert.ok(seen.every((entry) => !String(entry.url).includes("/v1/discovery/")))
    const token = await readFile(tokenPath, "utf8")
    assert.match(token, /froglet-test-token/)
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})
