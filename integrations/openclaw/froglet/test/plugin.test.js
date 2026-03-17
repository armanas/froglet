import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { mkdtemp, mkdir, chmod, readdir, readFile, rm, writeFile } from "node:fs/promises"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import test from "node:test"
import { setTimeout as delay } from "node:timers/promises"

import register from "../index.js"
import { BRIDGE_SCRIPT_PATH } from "../lib/runtime-tools.js"

function buildTestApi(config = {}, extraApi = {}) {
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
    },
    ...extraApi
  })

  return tools
}

function jsonResponse(res, statusCode, payload, headers = {}) {
  res.writeHead(statusCode, { "content-type": "application/json", ...headers })
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

function fixtureDescriptor(nodeId) {
  return {
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
}

function fixtureOffers() {
  return [
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
}

function marketplaceSearchNode(nodeId) {
  return {
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
}

function marketplaceNodeRecord(nodeId) {
  return {
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
  }
}

function makeQuote(body, nodeId, quoteId) {
  const workloadHash = createHash("sha256")
    .update(JSON.stringify(body ?? {}))
    .digest("hex")
  return {
    artifact_type: "quote",
    hash: `quote-${quoteId}`,
    payload: {
      provider_id: nodeId,
      requester_id: body?.requester_id ?? "ef".repeat(32),
      offer_id: body?.offer_id ?? "execute.wasm",
      workload_hash: workloadHash,
      expires_at: Math.floor(Date.now() / 1000) + 300,
      execution_limits: {
        max_runtime_ms: 1_000
      },
      settlement_terms: {
        method: "lightning.base_fee_plus_success_fee.v1",
        base_fee_msat: 0,
        success_fee_msat: 10_000,
        max_success_hold_expiry_secs: 30,
        max_base_invoice_expiry_secs: 30
      }
    }
  }
}

function runtimePaymentIntent(record) {
  const status = record.status
  const releaseReady = status === "result_ready"
  const succeeded = status === "succeeded"
  return {
    backend: "lightning",
    deal_id: record.dealId,
    deal_status: status,
    session_id: record.sessionId,
    admission_ready: status !== "payment_pending",
    result_ready: releaseReady,
    can_release_preimage: releaseReady,
    payment_requests: [
      {
        role: "success_fee_hold",
        state: succeeded ? "settled" : releaseReady ? "accepted" : "open",
        invoice: `lnmock-${record.dealId}`,
        payment_hash: record.successPaymentHash
      }
    ],
    release_action: releaseReady
      ? {
          endpoint_path: `/v1/deals/${record.dealId}/release-preimage`,
          expected_result_hash: record.resultHash
        }
      : null
  }
}

function publicDealRecord(record) {
  return {
    deal_id: record.dealId,
    status: record.status,
    result_hash:
      record.status === "result_ready" || record.status === "succeeded"
        ? record.resultHash
        : null,
    result:
      record.status === "result_ready" || record.status === "succeeded" ? 42 : null,
    receipt: record.receipt,
    deal: record.signedDeal
  }
}

async function startFixtureServer(options = {}) {
  const nodeId = "ab".repeat(32)
  const descriptor = fixtureDescriptor(nodeId)
  const offers = fixtureOffers()
  const runtimeToken = options.runtimeToken ?? "test-runtime-token"
  let quoteCounter = 0
  let dealCounter = 0
  const deals = new Map()
  const idempotencyDeals = new Map()

  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1")
    const body = req.method === "GET" ? null : await readJsonRequest(req)

    if (typeof options.handler === "function") {
      const handled = await options.handler(req, res, url, body, {
        descriptor,
        offers,
        deals,
        runtimeToken
      })
      if (handled) {
        return
      }
    }

    const authHeader = req.headers.authorization
    const isAuthorized = authHeader === `Bearer ${runtimeToken}`

    if (req.method === "GET" && url.pathname === "/v1/marketplace/search") {
      jsonResponse(res, 200, { nodes: [marketplaceSearchNode(nodeId)] })
      return
    }

    if (
      req.method === "GET" &&
      url.pathname === `/v1/marketplace/nodes/${encodeURIComponent(nodeId)}`
    ) {
      jsonResponse(res, 200, marketplaceNodeRecord(nodeId))
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

    if (req.method === "POST" && url.pathname === "/v1/quotes") {
      quoteCounter += 1
      jsonResponse(res, 201, makeQuote(body, nodeId, quoteCounter))
      return
    }

    if (req.method === "POST" && url.pathname === "/v1/runtime/services/buy") {
      if (!isAuthorized) {
        jsonResponse(res, 401, { error: "unauthorized" })
        return
      }

      const idempotencyKey =
        typeof body?.idempotency_key === "string" && body.idempotency_key.length > 0
          ? body.idempotency_key
          : null
      let record =
        idempotencyKey !== null ? deals.get(idempotencyDeals.get(idempotencyKey)) ?? null : null
      if (record === null) {
        dealCounter += 1
        const dealId = `deal-${dealCounter}`
        const resultHash = createHash("sha256")
          .update(JSON.stringify({ dealId, result: 42 }))
          .digest("hex")
        record = {
          dealId,
          sessionId: `session-${dealId}`,
          status: "payment_pending",
          resultHash,
          signedDeal: body.deal,
          signedQuote: body.quote,
          successPaymentHash: body.deal.payload.success_payment_hash,
          pollCount: 0,
          receipt: null
        }
        deals.set(dealId, record)
        if (idempotencyKey !== null) {
          idempotencyDeals.set(idempotencyKey, dealId)
        }
      }

      jsonResponse(res, 200, {
        quote: record.signedQuote,
        deal: publicDealRecord(record),
        terminal: false,
        payment_intent_path: `/v1/runtime/deals/${record.dealId}/payment-intent`,
        payment_intent: runtimePaymentIntent(record)
      })
      return
    }

    if (req.method === "GET" && url.pathname.startsWith("/v1/deals/")) {
      const match = url.pathname.match(/^\/v1\/deals\/([^/]+)$/)
      if (match !== null) {
        const dealId = decodeURIComponent(match[1])
        const record = deals.get(dealId)
        if (record === undefined) {
          jsonResponse(res, 404, { error: "missing deal" })
          return
        }
        if (record.status === "payment_pending") {
          record.pollCount += 1
          if (record.pollCount >= 1) {
            record.status = "result_ready"
          }
        }
        jsonResponse(res, 200, publicDealRecord(record))
        return
      }
    }

    if (
      req.method === "GET" &&
      url.pathname.startsWith("/v1/runtime/deals/") &&
      url.pathname.endsWith("/payment-intent")
    ) {
      if (!isAuthorized) {
        jsonResponse(res, 401, { error: "unauthorized" })
        return
      }

      const match = url.pathname.match(/^\/v1\/runtime\/deals\/([^/]+)\/payment-intent$/)
      const dealId = match === null ? null : decodeURIComponent(match[1])
      const record = dealId === null ? null : deals.get(dealId)
      if (record === undefined || record === null) {
        jsonResponse(res, 404, { error: "missing deal" })
        return
      }
      jsonResponse(res, 200, { payment_intent: runtimePaymentIntent(record) })
      return
    }

    if (
      req.method === "POST" &&
      url.pathname.startsWith("/v1/deals/") &&
      url.pathname.endsWith("/release-preimage")
    ) {
      const match = url.pathname.match(/^\/v1\/deals\/([^/]+)\/release-preimage$/)
      const dealId = match === null ? null : decodeURIComponent(match[1])
      const record = dealId === null ? null : deals.get(dealId)
      if (record === undefined || record === null) {
        jsonResponse(res, 404, { error: "missing deal" })
        return
      }

      const submittedPreimage =
        typeof body?.success_preimage === "string" ? body.success_preimage : null
      const submittedHash =
        submittedPreimage === null
          ? null
          : createHash("sha256").update(Buffer.from(submittedPreimage, "hex")).digest("hex")
      if (submittedHash !== record.successPaymentHash) {
        jsonResponse(res, 400, { error: "invalid success preimage" })
        return
      }
      if (
        typeof body?.expected_result_hash === "string" &&
        body.expected_result_hash !== record.resultHash
      ) {
        jsonResponse(res, 400, { error: "expected_result_hash mismatch" })
        return
      }

      record.status = "succeeded"
      record.receipt = {
        hash: `receipt-${record.dealId}`,
        payload: {
          deal_hash: record.signedDeal.hash ?? "unknown",
          deal_state: "succeeded",
          result_hash: record.resultHash
        }
      }
      jsonResponse(res, 200, {
        deal_id: record.dealId,
        status: "succeeded",
        result_hash: record.resultHash,
        receipt: record.receipt
      })
      return
    }

    if (req.method === "POST" && url.pathname === "/v1/runtime/services/publish") {
      if (!isAuthorized) {
        jsonResponse(res, 401, { error: "unauthorized" })
        return
      }
      jsonResponse(res, 200, { descriptor, offers })
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
    runtimeToken,
    deals,
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

async function withTempDir(t) {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-openclaw-test-"))
  t.after(async () => {
    await rm(tempDir, { recursive: true, force: true })
  })
  return tempDir
}

async function writeExecutableScript(tempDir, name, source) {
  const scriptPath = path.join(tempDir, name)
  await writeFile(scriptPath, source, { mode: 0o755 })
  await chmod(scriptPath, 0o755)
  return scriptPath
}

async function writeTokenFile(tempDir, token) {
  const tokenDir = path.join(tempDir, "runtime")
  const tokenPath = path.join(tokenDir, "auth.token")
  await mkdir(tokenDir, { recursive: true })
  await writeFile(tokenPath, token, { encoding: "utf8" })
  return tokenPath
}

function extractJsonSection(text, label) {
  const marker = `${label}\n`
  const index = text.indexOf(marker)
  assert.notEqual(index, -1, `missing ${label} in tool output`)
  return JSON.parse(text.slice(index + marker.length))
}

test("registers the expected OpenClaw tools by default", () => {
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

test("privileged runtime tools register only when explicitly enabled", () => {
  const disabled = buildTestApi({
    marketplaceUrl: "http://127.0.0.1:9090",
    runtimeUrl: "http://127.0.0.1:8081",
    providerUrl: "http://127.0.0.1:8080",
    runtimeAuthTokenPath: "/tmp/auth.token"
  })
  assert(!disabled.has("froglet_runtime_buy"))

  const enabled = buildTestApi({
    marketplaceUrl: "http://127.0.0.1:9090",
    runtimeUrl: "http://127.0.0.1:8081",
    providerUrl: "http://127.0.0.1:8080",
    runtimeAuthTokenPath: "/tmp/auth.token",
    enablePrivilegedRuntimeTools: true
  })
  assert.deepEqual(
    [...enabled.keys()].sort(),
    [
      "froglet_marketplace_node",
      "froglet_marketplace_search",
      "froglet_provider_surface",
      "froglet_runtime_accept_result",
      "froglet_runtime_buy",
      "froglet_runtime_payment_intent",
      "froglet_runtime_publish_services",
      "froglet_runtime_wait_deal"
    ].sort()
  )
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

test("provider surface can use config.providerUrl when the tool caller omits provider_url", async (t) => {
  const fixture = await startFixtureServer()
  t.after(async () => {
    await fixture.close()
  })

  const tools = buildTestApi({ providerUrl: fixture.baseUrl })
  const provider = await tools.get("froglet_provider_surface").definition.execute("provider", {})

  assert.match(provider.content[0].text, new RegExp(`provider_url: ${fixture.baseUrl}`))
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

test("provider surface requires config or per-call provider_url", async () => {
  const tools = buildTestApi()

  await assert.rejects(
    tools.get("froglet_provider_surface").definition.execute("provider", {}),
    /provider_url is required/
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

test("runtime buy uses override precedence and shells out to the configured Python bridge", async (t) => {
  const tempDir = await withTempDir(t)
  const configTokenPath = await writeTokenFile(tempDir, "config-token")
  const overrideTokenPath = path.join(tempDir, "override-runtime", "auth.token")
  await mkdir(path.dirname(overrideTokenPath), { recursive: true })
  await writeFile(overrideTokenPath, "override-token", { encoding: "utf8" })

  const fakePython = await writeExecutableScript(
    tempDir,
    "fake-python.js",
    `#!/usr/bin/env node
const fs = require("node:fs")
const payload = JSON.parse(fs.readFileSync(0, "utf8"))
process.stdout.write(JSON.stringify({
  argv: process.argv.slice(2),
  stdin: payload,
  runtime_url: payload.runtime_url,
  provider_url: payload.provider_url,
  runtime_auth_token_path: payload.runtime_auth_token_path,
  deal: { deal_id: "stub-deal", status: "payment_pending" },
  terminal: false,
  payment_intent_path: "/v1/runtime/deals/stub-deal/payment-intent",
  payment_intent: {
    backend: "lightning",
    deal_id: "stub-deal",
    deal_status: "payment_pending",
    payment_requests: []
  },
  stored_preimage: true,
  stored_state_path: "/tmp/openclaw-froglet/stub-deal.json"
}))`
  )

  const tools = buildTestApi({
    enablePrivilegedRuntimeTools: true,
    runtimeUrl: "http://config-runtime.invalid",
    providerUrl: "http://config-provider.invalid",
    runtimeAuthTokenPath: configTokenPath,
    pythonExecutable: fakePython
  })

  const result = await tools.get("froglet_runtime_buy").definition.execute("buy", {
    request: {
      offer_id: "execute.wasm",
      kind: "wasm"
    },
    runtime_url: "http://override-runtime.example",
    provider_url: "http://override-provider.example",
    runtime_auth_token_path: overrideTokenPath,
    include_raw: true
  })

  const raw = extractJsonSection(result.content[0].text, "buy_response_json:")
  assert.equal(raw.stdin.action, "buy")
  assert.equal(raw.stdin.runtime_url, "http://override-runtime.example")
  assert.equal(raw.stdin.provider_url, "http://override-provider.example")
  assert.equal(raw.stdin.runtime_auth_token_path, overrideTokenPath)
  assert.equal(raw.stdin.request.offer_id, "execute.wasm")
  assert.deepEqual(raw.argv, [BRIDGE_SCRIPT_PATH])
})

test("runtime tools surface Python bridge execution errors", async (t) => {
  const tempDir = await withTempDir(t)
  const tokenPath = await writeTokenFile(tempDir, "config-token")
  const failingPython = await writeExecutableScript(
    tempDir,
    "failing-python.js",
    `#!/usr/bin/env node
process.stderr.write("bridge exploded")
process.exit(7)`
  )

  const tools = buildTestApi({
    enablePrivilegedRuntimeTools: true,
    runtimeUrl: "http://127.0.0.1:8081",
    providerUrl: "http://127.0.0.1:8080",
    runtimeAuthTokenPath: tokenPath,
    pythonExecutable: failingPython
  })

  await assert.rejects(
    tools.get("froglet_runtime_publish_services").definition.execute("publish", {}),
    /Runtime bridge exited with code 7: bridge exploded/
  )
})

test("runtime buy, wait, payment-intent, accept-result, and publish work through the public Python bridge", async (t) => {
  const fixture = await startFixtureServer()
  t.after(async () => {
    await fixture.close()
  })
  const tempDir = await withTempDir(t)
  const tokenPath = await writeTokenFile(tempDir, fixture.runtimeToken)

  const tools = buildTestApi({
    enablePrivilegedRuntimeTools: true,
    runtimeUrl: fixture.baseUrl,
    providerUrl: fixture.baseUrl,
    runtimeAuthTokenPath: tokenPath
  })

  const buy = await tools.get("froglet_runtime_buy").definition.execute("buy", {
    request: {
      offer_id: "execute.wasm",
      kind: "wasm",
      submission: {
        workload_kind: "compute.wasm.v1"
      }
    },
    include_raw: true
  })
  assert.match(buy.content[0].text, /deal_id: deal-1/)
  assert.match(buy.content[0].text, /deal_status: payment_pending/)
  assert.match(buy.content[0].text, /managed_preimage: true/)
  const buyRaw = extractJsonSection(buy.content[0].text, "buy_response_json:")
  assert.equal(buyRaw.payment_intent.deal_status, "payment_pending")

  const stateDir = path.join(path.dirname(tokenPath), "openclaw-froglet")
  const stateFiles = await readdir(stateDir)
  assert.deepEqual(stateFiles, ["deal-1.json"])
  const storedState = JSON.parse(
    await readFile(path.join(stateDir, "deal-1.json"), { encoding: "utf8" })
  )
  assert.equal(storedState.deal_id, "deal-1")
  assert.equal(storedState.runtime_url, fixture.baseUrl)
  assert.equal(storedState.provider_url, fixture.baseUrl)

  const wait = await tools.get("froglet_runtime_wait_deal").definition.execute("wait", {
    deal_id: "deal-1",
    include_raw: true
  })
  assert.match(wait.content[0].text, /wait_statuses: result_ready, succeeded, failed, rejected/)
  assert.match(wait.content[0].text, /deal_status: result_ready/)
  const waitRaw = extractJsonSection(wait.content[0].text, "wait_response_json:")
  assert.equal(waitRaw.deal.status, "result_ready")

  const intent = await tools
    .get("froglet_runtime_payment_intent")
    .definition.execute("intent", { deal_id: "deal-1", include_raw: true })
  assert.match(intent.content[0].text, /can_release_preimage: true/)
  assert.match(intent.content[0].text, /release_endpoint: \/v1\/deals\/deal-1\/release-preimage/)
  const intentRaw = extractJsonSection(intent.content[0].text, "payment_intent_response_json:")
  assert.equal(intentRaw.payment_intent.release_action.expected_result_hash, fixture.deals.get("deal-1").resultHash)

  const accepted = await tools
    .get("froglet_runtime_accept_result")
    .definition.execute("accept", { deal_id: "deal-1", include_raw: true })
  assert.match(accepted.content[0].text, /terminal_status: succeeded/)
  assert.match(accepted.content[0].text, /receipt_hash: receipt-deal-1/)
  const acceptRaw = extractJsonSection(accepted.content[0].text, "accept_response_json:")
  assert.equal(acceptRaw.terminal.status, "succeeded")

  const published = await tools
    .get("froglet_runtime_publish_services")
    .definition.execute("publish", {})
  assert.match(published.content[0].text, /provider_id: abab/)
  assert.match(published.content[0].text, /offers_returned: 2/)
})

test("runtime accept_result fails clearly when local helper state is missing", async (t) => {
  const fixture = await startFixtureServer()
  t.after(async () => {
    await fixture.close()
  })
  const tempDir = await withTempDir(t)
  const tokenPath = await writeTokenFile(tempDir, fixture.runtimeToken)

  const tools = buildTestApi({
    enablePrivilegedRuntimeTools: true,
    runtimeUrl: fixture.baseUrl,
    providerUrl: fixture.baseUrl,
    runtimeAuthTokenPath: tokenPath
  })

  await assert.rejects(
    tools
      .get("froglet_runtime_accept_result")
      .definition.execute("accept", { deal_id: "missing-deal" }),
    /Local OpenClaw Froglet state was not found/
  )
})
