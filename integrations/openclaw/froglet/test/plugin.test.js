import assert from "node:assert/strict"
import http from "node:http"
import test from "node:test"
import { setTimeout as delay } from "node:timers/promises"

import register from "../index.js"

function buildTestApi(config = {}) {
  const tools = new Map()

  register({
    config,
    registerTool(definition, options) {
      tools.set(definition.name, {
        definition,
        options: options ?? {}
      })
    },
    logger: {
      info() {}
    }
  })

  return tools
}

function jsonResponse(res, statusCode, payload, headers = {}) {
  res.writeHead(statusCode, { "content-type": "application/json", ...headers })
  res.end(JSON.stringify(payload))
}

async function startFixtureServer(options = {}) {
  const nodeId = "ab".repeat(32)
  const descriptor = {
    artifact_type: "descriptor",
    payload: {
      provider_id: nodeId,
      protocol_version: "froglet/v1",
      descriptor_seq: 3,
      linked_identities: [
        {
          identity_kind: "nostr",
          identity: "cd".repeat(32)
        }
      ],
      transport_endpoints: [
        {
          transport: "http",
          uri: "http://127.0.0.1:8080"
        }
      ],
      capabilities: {
        service_kinds: ["compute.wasm.v1", "events.query"],
        execution_runtimes: ["wasm"],
        max_concurrent_deals: 16
      }
    }
  }
  const offers = [
    {
      payload: {
        offer_id: "events.query",
        offer_kind: "events.query",
        settlement_method: "lightning.base_fee_plus_success_fee.v1",
        quote_ttl_secs: 10,
        price_schedule: {
          base_fee_msat: 0,
          success_fee_msat: 0
        }
      }
    },
    {
      payload: {
        offer_id: "execute.wasm",
        offer_kind: "compute.wasm.v1",
        settlement_method: "lightning.base_fee_plus_success_fee.v1",
        quote_ttl_secs: 10,
        price_schedule: {
          base_fee_msat: 0,
          success_fee_msat: 10_000
        }
      }
    }
  ]

  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1")

    if (typeof options.handler === "function") {
      const handled = await options.handler(req, res, url)
      if (handled) {
        return
      }
    }

    if (req.method === "GET" && url.pathname === "/v1/marketplace/search") {
      jsonResponse(res, 200, {
        nodes: [
          {
            descriptor: {
              node_id: nodeId,
              version: "0.1.0",
              transports: {
                clearnet_url: "http://127.0.0.1:8080",
                onion_url: null,
                tor_status: "disabled"
              },
              services: [
                {
                  service_id: "events.query",
                  price_sats: 0,
                  payment_required: false
                },
                {
                  service_id: "execute.wasm",
                  price_sats: 10,
                  payment_required: true
                }
              ]
            },
            status: "active",
            last_seen_at: 1_773_673_719
          }
        ]
      })
      return
    }

    if (
      req.method === "GET" &&
      url.pathname === `/v1/marketplace/nodes/${encodeURIComponent(nodeId)}`
    ) {
      jsonResponse(res, 200, {
        descriptor: {
          node_id: nodeId,
          version: "0.1.0",
          transports: {
            clearnet_url: "http://127.0.0.1:8080",
            onion_url: null,
            tor_status: "disabled"
          },
          services: [
            {
              service_id: "events.query",
              price_sats: 0,
              payment_required: false
            }
          ]
        },
        status: "active",
        last_seen_at: 1_773_673_719
      })
      return
    }

    if (req.method === "GET" && url.pathname === "/v1/descriptor") {
      jsonResponse(res, 200, descriptor)
      return
    }

    if (req.method === "GET" && url.pathname === "/v1/offers") {
      jsonResponse(res, 200, { offers })
      return
    }

    jsonResponse(res, 404, { error: "not found" })
  })

  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
  const address = server.address()
  if (address === null || typeof address === "string") {
    throw new Error("Failed to bind fixture server")
  }

  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    async close() {
      await new Promise((resolve, reject) => {
        server.close((error) => {
          if (error) {
            reject(error)
            return
          }
          resolve()
        })
      })
    }
  }
}

test("registers the expected OpenClaw tools", () => {
  const tools = buildTestApi({ marketplaceUrl: "http://127.0.0.1:9090" })

  assert.deepEqual([...tools.keys()].sort(), [
    "froglet_marketplace_node",
    "froglet_marketplace_search",
    "froglet_provider_surface"
  ])
  for (const tool of tools.values()) {
    assert.equal(tool.options.optional, true)
    assert.equal(typeof tool.definition.execute, "function")
    assert.equal(typeof tool.definition.parameters, "object")
  }
})

test("marketplace search and node tools summarize marketplace responses", async (t) => {
  const fixture = await startFixtureServer()
  t.after(async () => {
    await fixture.close()
  })

  const tools = buildTestApi({ marketplaceUrl: fixture.baseUrl })
  const search = await tools
    .get("froglet_marketplace_search")
    .definition.execute("search", { limit: 5 })
  const node = await tools
    .get("froglet_marketplace_node")
    .definition.execute("node", { node_id: "ab".repeat(32) })

  assert.match(search.content[0].text, /returned_nodes: 1/)
  assert.match(search.content[0].text, /execute\.wasm=10 sats/)
  assert.match(node.content[0].text, /node_id: abab/)
  assert.doesNotMatch(node.content[0].text, /raw_record_json:/)
})

test("provider surface tool summarizes descriptor and offers", async (t) => {
  const fixture = await startFixtureServer()
  t.after(async () => {
    await fixture.close()
  })

  const tools = buildTestApi()
  const provider = await tools
    .get("froglet_provider_surface")
    .definition.execute("provider", { provider_url: fixture.baseUrl })

  assert.match(provider.content[0].text, /provider_id: abab/)
  assert.match(provider.content[0].text, /offers_returned: 2/)
  assert.match(provider.content[0].text, /offer_id=execute\.wasm/)
  assert.match(provider.content[0].text, /transport_endpoints: http:\/\/127\.0\.0\.1:8080/)
  assert.doesNotMatch(provider.content[0].text, /descriptor_json:/)
})

test("marketplace tools support a per-call marketplace_url override", async (t) => {
  const fixture = await startFixtureServer()
  t.after(async () => {
    await fixture.close()
  })

  const tools = buildTestApi()
  const result = await tools
    .get("froglet_marketplace_search")
    .definition.execute("search", { marketplace_url: fixture.baseUrl })

  assert.match(result.content[0].text, new RegExp(`marketplace: ${fixture.baseUrl}`))
})

test("marketplace node tool can include raw JSON when requested", async (t) => {
  const fixture = await startFixtureServer()
  t.after(async () => {
    await fixture.close()
  })

  const tools = buildTestApi({ marketplaceUrl: fixture.baseUrl })
  const result = await tools
    .get("froglet_marketplace_node")
    .definition.execute("node", { node_id: "ab".repeat(32), include_raw: true })

  assert.match(result.content[0].text, /raw_record_json:/)
})

test("provider surface tool can include raw JSON when requested", async (t) => {
  const fixture = await startFixtureServer()
  t.after(async () => {
    await fixture.close()
  })

  const tools = buildTestApi()
  const result = await tools
    .get("froglet_provider_surface")
    .definition.execute("provider", { provider_url: fixture.baseUrl, include_raw: true })

  assert.match(result.content[0].text, /descriptor_json:/)
  assert.match(result.content[0].text, /offers_json:/)
})

test("marketplace search requires config or per-call marketplace_url", async () => {
  const tools = buildTestApi()

  await assert.rejects(
    tools.get("froglet_marketplace_search").definition.execute("search", {}),
    /marketplace_url is required/
  )
})

test("marketplace node surfaces 404 errors", async (t) => {
  const fixture = await startFixtureServer({
    async handler(_req, res, url) {
      if (url.pathname === "/v1/marketplace/nodes/missing") {
        jsonResponse(res, 404, { error: "missing" })
        return true
      }
      return false
    }
  })
  t.after(async () => {
    await fixture.close()
  })

  const tools = buildTestApi({ marketplaceUrl: fixture.baseUrl })
  await assert.rejects(
    tools.get("froglet_marketplace_node").definition.execute("node", { node_id: "missing" }),
    /failed with 404/
  )
})

test("provider surface rejects non-json responses", async (t) => {
  const fixture = await startFixtureServer({
    async handler(_req, res, url) {
      if (url.pathname === "/v1/descriptor") {
        res.writeHead(200, { "content-type": "text/plain" })
        res.end("not json")
        return true
      }
      return false
    }
  })
  t.after(async () => {
    await fixture.close()
  })

  const tools = buildTestApi()
  await assert.rejects(
    tools
      .get("froglet_provider_surface")
      .definition.execute("provider", { provider_url: fixture.baseUrl }),
    /Expected JSON/
  )
})

test("marketplace search surfaces timeout errors", async (t) => {
  const fixture = await startFixtureServer({
    async handler(req, res, url) {
      if (req.method === "GET" && url.pathname === "/v1/marketplace/search") {
        await delay(1_100)
        jsonResponse(res, 200, { nodes: [] })
        return true
      }
      return false
    }
  })
  t.after(async () => {
    await fixture.close()
  })

  const tools = buildTestApi({
    marketplaceUrl: fixture.baseUrl,
    requestTimeoutMs: 1_000
  })
  await assert.rejects(
    tools.get("froglet_marketplace_search").definition.execute("search", {}),
    /timed out/
  )
})
