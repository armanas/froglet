import assert from "node:assert/strict"
import { mkdtemp, rm, writeFile } from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import test from "node:test"

import {
  buildProject,
  createProject,
  discoverServices,
  frogletRestart,
  frogletStatus,
  frogletTailLogs,
  getDealInvoiceBundle,
  getDealPaymentIntent,
  getProject,
  getService,
  getTask,
  getWalletBalance,
  invokeService,
  listProjects,
  listSettlementActivity,
  publishArtifact,
  publishProject,
  readProjectFile,
  runCompute,
  testProject,
  waitTask,
  writeProjectFile
} from "../lib/froglet-client.js"
import {
  buildExecutionWorkload,
  buildServiceAddressedExecution,
  canonicalJsonBytes,
  flattenMarketplaceProviders,
  selectTransportEndpoint,
  sha256Hex
} from "../../../shared/froglet-lib/froglet-client.js"

async function withTokenPath(fn) {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-client-"))
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    await fn(tokenPath)
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
}

function providerSearchResponse() {
  return {
    providers: [
      {
        provider_id: "prov-1",
        descriptor_hash: "desc-1",
        transport_endpoints: [
          { uri: "https://93.184.216.34", features: ["quote_http"], priority: 10 },
          { uri: "http://127.0.0.1:8080", features: ["quote_http"], priority: 20 }
        ],
        offers: [
          {
            offer_hash: "offer-hash-1",
            offer_id: "svc-1",
            offer_kind: "execution",
            runtime: "python",
            settlement_method: "none",
            base_fee_msat: 1000,
            success_fee_msat: 2000,
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

// ---------------------------------------------------------------------------
// Removed function stubs
// ---------------------------------------------------------------------------

test("frogletTailLogs throws removed error", async () => {
  await assert.rejects(() => frogletTailLogs({}), /systemd journal/)
})

test("frogletRestart throws removed error", async () => {
  await assert.rejects(() => frogletRestart({}), /systemctl/)
})

test("project authoring helpers throw removal errors", async () => {
  await assert.rejects(() => listProjects({}), /Project authoring not available/)
  await assert.rejects(() => createProject({}), /Project authoring not available/)
  await assert.rejects(() => getProject({}), /Project authoring not available/)
  await assert.rejects(() => readProjectFile({}), /Project authoring not available/)
  await assert.rejects(() => writeProjectFile({}), /Project authoring not available/)
  await assert.rejects(() => buildProject({}), /Project authoring not available/)
  await assert.rejects(() => testProject({}), /Project authoring not available/)
  await assert.rejects(() => publishProject({}), /Project authoring not available/)
})

// ---------------------------------------------------------------------------
// Canonical helpers
// ---------------------------------------------------------------------------

test("selectTransportEndpoint prefers lowest-priority quote_http https endpoint", () => {
  const endpoint = selectTransportEndpoint([
    { uri: "http://93.184.216.34", features: ["quote_http"], priority: 20 },
    { uri: "https://93.184.216.34", features: ["quote_http"], priority: 10 },
    { uri: "https://93.184.216.34/no-quote", features: [], priority: 1 }
  ])
  assert.equal(endpoint?.uri, "https://93.184.216.34")
})

test("flattenMarketplaceProviders preserves providers and flattens offers into compatibility services", () => {
  const services = flattenMarketplaceProviders(providerSearchResponse(), { query: "svc-1" })
  assert.equal(services.length, 1)
  assert.deepEqual(services[0], {
    service_id: "svc-1",
    offer_id: "svc-1",
    offer_kind: "execution",
    resource_kind: "service",
    summary: "none",
    runtime: "python",
    package_kind: "inline_source",
    contract_version: "froglet.python.handler_json.v1",
    requested_access: ["mount.filesystem.read.workspace"],
    mode: "unknown",
    price_sats: 3,
    publication_state: "unknown",
    provider_id: "prov-1",
    provider_url: "https://93.184.216.34",
    descriptor_hash: "desc-1",
    settlement_method: "none"
  })
})

test("buildExecutionWorkload applies JS defaults matching runtime execution helpers", () => {
  const workload = buildExecutionWorkload({
    runtime: "python",
    package_kind: "inline_source",
    inline_source: "def handler(event):\n    return event\n",
    input: { pong: true }
  })
  assert.equal(workload.entrypoint.kind, "handler")
  assert.equal(workload.entrypoint.value, "handler")
  assert.equal(workload.contract_version, "froglet.python.handler_json.v1")
  assert.equal(workload.source_hash, sha256Hex(Buffer.from("def handler(event):\n    return event\n", "utf8")))
  assert.equal(
    workload.input_hash,
    sha256Hex(canonicalJsonBytes({ pong: true }))
  )
})

test("buildServiceAddressedExecution uses binding hash and service defaults", () => {
  const execution = buildServiceAddressedExecution(
    {
      service_id: "svc-1",
      runtime: "python",
      package_kind: "inline_source",
      binding_hash: "deadbeef",
      mounts: [{ kind: "filesystem", handle: "workspace", read_only: true }]
    },
    { ping: true }
  )
  assert.equal(execution.entrypoint.kind, "handler")
  assert.equal(execution.entrypoint.value, "handler")
  assert.equal(execution.contract_version, "froglet.python.handler_json.v1")
  assert.equal(execution.source_hash, "deadbeef")
  assert.equal(execution.security.service_id, "svc-1")
  assert.deepEqual(execution.requested_access, ["mount.filesystem.read.workspace"])
})

// ---------------------------------------------------------------------------
// Provider + runtime HTTP clients
// ---------------------------------------------------------------------------

test("publishArtifact posts to provider API and accepts HTTP 201", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let capturedUrl
    global.fetch = async (url, options = {}) => {
      capturedUrl = String(url)
      const body = JSON.parse(options.body)
      assert.equal(body.runtime, "wasm")
      assert.equal(body.package_kind, "inline_module")
      return new Response(JSON.stringify({ status: "published" }), {
        status: 201,
        headers: { "Content-Type": "application/json" }
      })
    }
    try {
      const response = await publishArtifact({
        providerUrl: "http://127.0.0.1:8080",
        providerAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        request: {
          service_id: "svc-1",
          offer_id: "svc-1",
          runtime: "wasm",
          package_kind: "inline_module",
          artifact_path: "/tmp/lol.wasm"
        }
      })
      assert.ok(capturedUrl.endsWith("/v1/provider/artifacts/publish"))
      assert.equal(response.status, "published")
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("frogletStatus probes provider and runtime health", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    const hitUrls = []
    global.fetch = async (url) => {
      const urlStr = String(url)
      hitUrls.push(urlStr)
      if (urlStr === "http://127.0.0.1:8080/health") {
        return new Response(JSON.stringify({ healthy: true }), { status: 200 })
      }
      if (urlStr === "http://127.0.0.1:8080/v1/node/capabilities") {
        return new Response(JSON.stringify({ compute_offer_ids: ["execute.compute"] }), { status: 200 })
      }
      if (urlStr === "http://127.0.0.1:8080/v1/node/identity") {
        return new Response(JSON.stringify({ node_id: "node-abc", discovery: { mode: "marketplace" } }), { status: 200 })
      }
      if (urlStr === "http://127.0.0.1:8081/health") {
        return new Response(JSON.stringify({ status: "ok" }), { status: 200 })
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    }
    try {
      const response = await frogletStatus({
        providerUrl: "http://127.0.0.1:8080",
        providerAuthTokenPath: tokenPath,
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000
      })
      assert.equal(response.healthy, true)
      assert.equal(response.provider.healthy, true)
      assert.equal(response.runtime.healthy, true)
      assert.equal(response.node_id, "node-abc")
      assert.deepEqual(response.raw_compute_offer_ids, ["execute.compute"])
      assert.ok(hitUrls.includes("http://127.0.0.1:8081/health"))
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("discoverServices reads marketplace providers and flattens compatibility services", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let capturedBody = null
    global.fetch = async (url, options = {}) => {
      assert.equal(String(url), "http://127.0.0.1:8081/v1/runtime/search")
      capturedBody = JSON.parse(options.body)
      return new Response(JSON.stringify(providerSearchResponse()), {
        status: 200,
        headers: { "Content-Type": "application/json" }
      })
    }
    try {
      const response = await discoverServices({
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        limit: 5,
        includeInactive: false,
        query: "svc-1"
      })
      assert.deepEqual(capturedBody, { limit: 5, include_inactive: false })
      assert.equal(response.providers.length, 1)
      assert.equal(response.services.length, 1)
      assert.equal(response.services[0].provider_url, "https://93.184.216.34")
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("getService resolves provider from runtime search and fetches provider public service detail", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    const hitUrls = []
    global.fetch = async (url) => {
      const urlStr = String(url)
      hitUrls.push(urlStr)
      if (urlStr.endsWith("/v1/runtime/search")) {
        return new Response(JSON.stringify(providerSearchResponse()), { status: 200 })
      }
      if (urlStr === "https://93.184.216.34/v1/provider/services/svc-1") {
        return new Response(
          JSON.stringify({
            service: {
              service_id: "svc-1",
              offer_id: "svc-1",
              runtime: "python",
              package_kind: "inline_source",
              binding_hash: "feedface"
            }
          }),
          { status: 200 }
        )
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    }
    try {
      const response = await getService({
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        request: { service_id: "svc-1" }
      })
      assert.equal(response.service.service_id, "svc-1")
      assert.equal(response.service.provider_url, "https://93.184.216.34")
      assert.ok(hitUrls.some((url) => url.endsWith("/v1/runtime/search")))
      assert.ok(hitUrls.some((url) => url.endsWith("/v1/provider/services/svc-1")))
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("invokeService resolves provider details, fetches canonical service record, and posts runtime deal", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let runtimeDealBody = null
    global.fetch = async (url, options = {}) => {
      const urlStr = String(url)
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/providers/prov-1") {
        return new Response(
          JSON.stringify({
            provider: {
              provider_id: "prov-1",
              transport_endpoints: [
                { uri: "https://93.184.216.34", features: ["quote_http"], priority: 1 }
              ]
            }
          }),
          { status: 200 }
        )
      }
      if (urlStr === "https://93.184.216.34/v1/provider/services/svc-1") {
        return new Response(
          JSON.stringify({
            service: {
              service_id: "svc-1",
              offer_id: "svc-1",
              provider_id: "prov-1",
              runtime: "python",
              package_kind: "inline_source",
              entrypoint_kind: "handler",
              entrypoint: "ignored/path.py",
              contract_version: "froglet.python.handler_json.v1",
              binding_hash: "feedface",
              mounts: [{ kind: "filesystem", handle: "workspace", read_only: true }]
            }
          }),
          { status: 200 }
        )
      }
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/deals") {
        runtimeDealBody = JSON.parse(options.body)
        return new Response(
          JSON.stringify({
            provider_id: "prov-1",
            provider_url: "https://93.184.216.34",
            quote: { hash: "quote-hash" },
            deal: { deal_id: "deal-1", status: "succeeded", result: { pong: true } }
          }),
          { status: 200 }
        )
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    }
    try {
      const response = await invokeService({
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        request: {
          provider_id: "prov-1",
          service_id: "svc-1",
          input: { ping: true }
        }
      })
      assert.equal(response.terminal, true)
      assert.equal(response.status, "succeeded")
      assert.deepEqual(response.result, { pong: true })
      assert.deepEqual(runtimeDealBody.provider, {
        provider_id: "prov-1",
        provider_url: "https://93.184.216.34"
      })
      assert.equal(runtimeDealBody.offer_id, "svc-1")
      assert.equal(runtimeDealBody.kind, "execution")
      assert.equal(runtimeDealBody.execution.security.service_id, "svc-1")
      assert.equal(runtimeDealBody.execution.source_hash, "feedface")
      assert.deepEqual(runtimeDealBody.execution.requested_access, ["mount.filesystem.read.workspace"])
      assert.equal(runtimeDealBody.execution.entrypoint.value, "handler")
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("runCompute posts a canonical runtime Wasm deal and normalizes terminal deals", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let runtimeDealBody = null
    global.fetch = async (url, options = {}) => {
      const urlStr = String(url)
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/deals") {
        runtimeDealBody = JSON.parse(options.body)
        return new Response(
          JSON.stringify({
            provider_id: "prov-1",
            provider_url: "https://93.184.216.34",
            quote: { hash: "quote-hash" },
            deal: { deal_id: "deal-2", status: "succeeded", result: 42 }
          }),
          { status: 200 }
        )
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    }
    try {
      const response = await runCompute({
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        request: {
          provider_id: "prov-1",
          provider_url: "https://93.184.216.34",
          runtime: "wasm",
          package_kind: "inline_module",
          wasm_module_hex: "0061736d01000000",
          input: { ping: true }
        }
      })
      assert.equal(response.status, "succeeded")
      assert.equal(response.result, 42)
      assert.equal(runtimeDealBody.offer_id, "execute.compute")
      assert.equal(runtimeDealBody.kind, "wasm")
      assert.equal(runtimeDealBody.submission.workload.workload_kind, "compute.wasm.v1")
      assert.equal(
        runtimeDealBody.submission.workload.input_hash,
        sha256Hex(canonicalJsonBytes({ ping: true }))
      )
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("runCompute uses execute.compute.generic for execution workloads", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let runtimeDealBody = null
    global.fetch = async (url, options = {}) => {
      const urlStr = String(url)
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/deals") {
        runtimeDealBody = JSON.parse(options.body)
        return new Response(
          JSON.stringify({
            provider_id: "prov-1",
            provider_url: "https://93.184.216.34",
            quote: { hash: "quote-hash" },
            deal: { deal_id: "deal-3", status: "succeeded", result: { ok: true } }
          }),
          { status: 200 }
        )
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    }
    try {
      const response = await runCompute({
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        request: {
          provider_id: "prov-1",
          provider_url: "https://93.184.216.34",
          runtime: "python",
          package_kind: "inline_source",
          inline_source: "def handler(event, context):\n    return event\n",
          input: { ping: true }
        }
      })
      assert.equal(response.status, "succeeded")
      assert.deepEqual(response.result, { ok: true })
      assert.equal(runtimeDealBody.offer_id, "execute.compute.generic")
      assert.equal(runtimeDealBody.kind, "execution")
      assert.equal(runtimeDealBody.execution.runtime, "python")
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("getTask falls back to provider jobs when runtime and provider share one API base", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    const hitUrls = []
    global.fetch = async (url) => {
      const urlStr = String(url)
      hitUrls.push(urlStr)
      if (urlStr === "http://127.0.0.1:8080/v1/runtime/deals/task-1") {
        return new Response(JSON.stringify({ deal: { deal_id: "task-1", status: "running" } }), { status: 200 })
      }
      if (urlStr === "http://127.0.0.1:8080/v1/runtime/deals/task-2") {
        return new Response(JSON.stringify({ error: "deal not found" }), { status: 404 })
      }
      if (urlStr === "http://127.0.0.1:8080/v1/node/jobs/task-2") {
        return new Response(JSON.stringify({ task: { task_id: "task-2", state: "completed" } }), { status: 200 })
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    }
    try {
      const runtimeTask = await getTask({
        providerUrl: "http://127.0.0.1:8080",
        providerAuthTokenPath: tokenPath,
        runtimeUrl: "http://127.0.0.1:8080",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        taskId: "task-1"
      })
      assert.equal(runtimeTask.task.deal_id, "task-1")

      const providerTask = await getTask({
        providerUrl: "http://127.0.0.1:8080",
        providerAuthTokenPath: tokenPath,
        runtimeUrl: "http://127.0.0.1:8080",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        taskId: "task-2"
      })
      assert.equal(providerTask.task.task_id, "task-2")
      assert.ok(hitUrls.includes("http://127.0.0.1:8080/v1/node/jobs/task-2"))
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("getTask returns job not found without probing provider jobs on split deployments", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    const hitUrls = []
    global.fetch = async (url) => {
      const urlStr = String(url)
      hitUrls.push(urlStr)
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/deals/task-missing") {
        return new Response(JSON.stringify({ error: "deal not found" }), { status: 404 })
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    }
    try {
      await assert.rejects(
        () =>
          getTask({
            providerUrl: "http://127.0.0.1:8080",
            providerAuthTokenPath: tokenPath,
            runtimeUrl: "http://127.0.0.1:8081",
            runtimeAuthTokenPath: tokenPath,
            requestTimeoutMs: 1000,
            taskId: "task-missing"
          }),
        /job not found/
      )
      assert.deepEqual(hitUrls, ["http://127.0.0.1:8081/v1/runtime/deals/task-missing"])
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("waitTask polls runtime deals until terminal state", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let callCount = 0
    global.fetch = async (url) => {
      assert.equal(String(url), "http://127.0.0.1:8081/v1/runtime/deals/task-99")
      callCount += 1
      const status = callCount < 3 ? "running" : "succeeded"
      return new Response(
        JSON.stringify({ deal: { deal_id: "task-99", status, result: "done" } }),
        { status: 200 }
      )
    }
    try {
      const response = await waitTask({
        providerUrl: "http://127.0.0.1:8080",
        providerAuthTokenPath: tokenPath,
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        taskId: "task-99",
        timeoutSecs: 5,
        pollIntervalSecs: 0.05
      })
      assert.ok(callCount >= 3)
      assert.equal(response.task.status, "succeeded")
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("waitTask surfaces job not found immediately on split deployments", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    const hitUrls = []
    global.fetch = async (url) => {
      const urlStr = String(url)
      hitUrls.push(urlStr)
      if (urlStr === "http://127.0.0.1:8081/v1/runtime/deals/task-missing") {
        return new Response(JSON.stringify({ error: "deal not found" }), { status: 404 })
      }
      throw new Error(`unexpected URL: ${urlStr}`)
    }
    try {
      await assert.rejects(
        () =>
          waitTask({
            providerUrl: "http://127.0.0.1:8080",
            providerAuthTokenPath: tokenPath,
            runtimeUrl: "http://127.0.0.1:8081",
            runtimeAuthTokenPath: tokenPath,
            requestTimeoutMs: 1000,
            taskId: "task-missing",
            timeoutSecs: 5,
            pollIntervalSecs: 0.05
          }),
        /job not found/
      )
      assert.deepEqual(hitUrls, ["http://127.0.0.1:8081/v1/runtime/deals/task-missing"])
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("getWalletBalance fetches the runtime wallet snapshot with bearer auth", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let capturedUrl
    let capturedAuth
    global.fetch = async (url, options = {}) => {
      capturedUrl = String(url)
      capturedAuth = options.headers?.Authorization
      return new Response(
        JSON.stringify({
          backend: "lightning",
          mode: "mock",
          balance_known: true,
          balance_sats: 21,
          accepted_payment_methods: ["lightning"],
          reservations: true,
          receipts: true
        }),
        { status: 200 }
      )
    }
    try {
      const response = await getWalletBalance({
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000
      })
      assert.equal(capturedUrl, "http://127.0.0.1:8081/v1/runtime/wallet/balance")
      assert.equal(capturedAuth, "Bearer froglet-test-token")
      assert.equal(response.balance_sats, 21)
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("listSettlementActivity passes limit as query string", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let capturedUrl
    global.fetch = async (url) => {
      capturedUrl = String(url)
      return new Response(
        JSON.stringify({
          items: [
            {
              deal_id: "deal-1",
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
          limit: 10
        }),
        { status: 200 }
      )
    }
    try {
      const response = await listSettlementActivity({
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        limit: 10
      })
      assert.ok(capturedUrl.endsWith("/v1/runtime/settlement/activity?limit=10"))
      assert.equal(response.items.length, 1)
      assert.equal(response.items[0].deal_id, "deal-1")
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("getDealPaymentIntent URL-encodes the deal id", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let capturedUrl
    global.fetch = async (url) => {
      capturedUrl = String(url)
      return new Response(
        JSON.stringify({ payment_intent: { deal_id: "a/b", amount_msat: 1000 } }),
        { status: 200 }
      )
    }
    try {
      await getDealPaymentIntent({
        runtimeUrl: "http://127.0.0.1:8081",
        runtimeAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        dealId: "a/b"
      })
      assert.equal(
        capturedUrl,
        "http://127.0.0.1:8081/v1/runtime/deals/a%2Fb/payment-intent"
      )
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("getDealInvoiceBundle hits the provider invoice-bundle endpoint", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let capturedUrl
    global.fetch = async (url) => {
      capturedUrl = String(url)
      return new Response(
        JSON.stringify({ bundle: { deal_id: "deal-2", legs: [] } }),
        { status: 200 }
      )
    }
    try {
      await getDealInvoiceBundle({
        providerUrl: "http://127.0.0.1:8080",
        providerAuthTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        dealId: "deal-2"
      })
      assert.equal(
        capturedUrl,
        "http://127.0.0.1:8080/v1/provider/deals/deal-2/invoice-bundle"
      )
    } finally {
      global.fetch = previousFetch
    }
  })
})
