import assert from "node:assert/strict"
import { once } from "node:events"
import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { createServer } from "node:http"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"

import { invokeService, runCompute } from "../../../shared/froglet-lib/froglet-client.js"
import { executionResultDetails } from "../../../shared/froglet-lib/summarize.js"
import { frogletToolInputSchema, handleToolCall } from "../lib/tools.js"
import { frogletToolParameters, registerFrogletTool } from "../../../openclaw/froglet/lib/froglet-tool.js"

const PROVIDER_URL = "https://provider.example"
const pythonWorkload = {
  runtime: "python",
  package_kind: "inline_source",
  entrypoint_kind: "handler",
  entrypoint: "handler",
  contract_version: "froglet.python.handler_json.v1",
  inline_source: "def handler(event, context): return event",
  input: { value: 7 },
}

function terminalResponse(extra = {}) {
  return {
    provider_id: "provider-1",
    provider_url: PROVIDER_URL,
    quote: { hash: "quote-hash", payload: {} },
    deal: {
      deal_id: "deal-1",
      provider_id: "provider-1",
      status: "succeeded",
      result: { value: 7 },
      result_hash: "result-hash",
      quote: { hash: "quote-hash", payload: {} },
      deal: { hash: "deal-hash", payload: {} },
      receipt: {
        hash: "receipt-hash",
        payload: { result_hash: "result-hash", execution_state: "succeeded", settlement_state: "none" },
      },
    },
    ...extra,
  }
}

async function withRuntime(fn, { payload = terminalResponse(), responseStatus = 200 } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "froglet-execution-controls-"))
  const requests = []
  const server = createServer(async (request, response) => {
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    const text = Buffer.concat(chunks).toString("utf8")
    requests.push({ path: request.url, method: request.method, auth: request.headers.authorization, body: text ? JSON.parse(text) : null })
    response.setHeader("Content-Type", "application/json")
    if (request.url === "/v1/runtime/providers/provider-1") {
      response.end(JSON.stringify({ provider: { provider_id: "provider-1", transport_endpoints: [{ uri: PROVIDER_URL, features: ["quote_http"] }] } }))
    } else if (request.url === "/v1/runtime/deals" || request.url === "/v1/runtime/deals/deal-1") {
      response.statusCode = responseStatus
      response.end(JSON.stringify(payload))
    } else {
      response.statusCode = 404
      response.end(JSON.stringify({ error: "unexpected fixture route" }))
    }
  })
  try {
    const tokenPath = join(directory, "token")
    await writeFile(tokenPath, "test-controls-token")
    server.listen(0, "127.0.0.1")
    await once(server, "listening")
    const runtimeUrl = `http://127.0.0.1:${server.address().port}`
    const clientDeps = {
      providerUrl: { lookup: async () => [{ address: "93.184.216.34", family: 4 }] },
      providerJsonRequest: async (url, options) => {
        assert.equal(url, `${PROVIDER_URL}/v1/provider/services/echo`)
        assert.equal(options.pin.pinnedAddress, "93.184.216.34")
        return { status: 200, payload: { service: {
          service_id: "echo", offer_id: "echo", provider_id: "provider-1",
          ...pythonWorkload, binding_hash: "source-hash", mounts: [],
        } } }
      },
    }
    const client = { runtimeUrl, runtimeAuthTokenPath: tokenPath, requestTimeoutMs: 1000, _deps: clientDeps }
    const config = {
      runtimeUrl, runtimeAuthTokenPath: tokenPath, providerUrl: runtimeUrl,
      providerAuthTokenPath: tokenPath, requestTimeoutMs: 1000,
      defaultSearchLimit: 10, maxSearchLimit: 50, _deps: { client: clientDeps },
    }
    await fn({ client, config, requests })
  } finally {
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  }
}

test("named service execution sends a zero per-call cap by default and invents no retry key", async () => {
  await withRuntime(async ({ client, requests }) => {
    const result = await invokeService({ ...client, request: { provider_id: "provider-1", service_id: "echo", input: { value: 7 } } })
    const posted = requests.find((request) => request.method === "POST")
    assert.equal(posted.path, "/v1/runtime/deals")
    assert.equal(posted.auth, "Bearer test-controls-token")
    assert.equal(posted.body.max_price_sats, 0)
    assert.equal(Object.hasOwn(posted.body, "idempotency_key"), false)
    assert.equal(posted.body.execution.security.service_id, "echo")
    assert.equal(result.terminal, true)
  })
})

test("service retries preserve the explicit price cap and exact retry identity on the wire", async () => {
  await withRuntime(async ({ client, requests }) => {
    const request = { provider_id: "provider-1", service_id: "echo", input: { value: 7 }, max_price_sats: 42, idempotency_key: "  stable-key  " }
    await invokeService({ ...client, request })
    await invokeService({ ...client, request })
    const posted = requests.filter((request) => request.method === "POST")
    assert.equal(posted.length, 2)
    assert.equal(posted[0].body.max_price_sats, 42)
    assert.equal(posted[0].body.idempotency_key, "  stable-key  ")
    assert.deepEqual(posted[0].body, posted[1].body)
  })
})

test("all compute payload paths carry the same cap and byte-bounded retry controls", async () => {
  const workloads = [
    { ...pythonWorkload },
    { wasm_module_hex: "0061736d01000000", input: { value: 7 } },
    { runtime: "wasm", package_kind: "oci_image", oci_reference: "registry.example/worker:latest", oci_digest: `sha256:${"a".repeat(64)}`, input: null },
  ]
  await withRuntime(async ({ client, requests }) => {
    for (const workload of workloads) {
      await runCompute({ ...client, request: { provider_id: "provider-1", ...workload, max_price_sats: 9, idempotency_key: "é".repeat(64) } })
    }
    const posted = requests.filter((request) => request.method === "POST")
    assert.deepEqual(posted.map((request) => request.body.kind), ["execution", "wasm", "oci_wasm"])
    for (const request of posted) {
      assert.equal(request.body.max_price_sats, 9)
      assert.equal(request.body.idempotency_key, "é".repeat(64))
    }
    await runCompute({ ...client, request: { provider_id: "provider-1", ...pythonWorkload } })
    assert.equal(requests.at(-1).body.max_price_sats, 0)
    assert.equal(Object.hasOwn(requests.at(-1).body, "idempotency_key"), false)
  })
})

test("both client entrypoints reject malformed or unsafe controls before network activity", async () => {
  await withRuntime(async ({ client, requests }) => {
    for (const call of [invokeService, runCompute]) {
      const request = { provider_id: "provider-1", service_id: "echo", ...pythonWorkload }
      for (const maxPrice of [-1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, "1", null]) {
        await assert.rejects(call({ ...client, request: { ...request, max_price_sats: maxPrice } }), /max_price_sats.*safe integer/)
      }
      for (const key of ["", " \n\t ", null, 123, "a".repeat(129), "é".repeat(65)]) {
        await assert.rejects(call({ ...client, request: { ...request, idempotency_key: key } }), /idempotency_key/)
      }
    }
    assert.deepEqual(requests, [])
  })
})

test("MCP invocation forwards controls and returns signed evidence without claiming verification", async () => {
  await withRuntime(async ({ config, requests }) => {
    const result = await handleToolCall("froglet", { action: "invoke_service", provider_id: "provider-1", service_id: "echo", max_price_sats: 5, idempotency_key: "service-retry", input: { value: 7 } }, config)
    assert.equal(result.isError, undefined)
    assert.equal(requests.at(-1).body.max_price_sats, 5)
    assert.equal(requests.at(-1).body.idempotency_key, "service-retry")
    const details = result.structuredContent
    assert.deepEqual(details.result, { value: 7 })
    assert.equal(details.result_hash, "result-hash")
    assert.equal(details.receipt_hash, "receipt-hash")
    assert.equal(details.signed_artifacts.deal.hash, "deal-hash")
    assert.equal(details.signed_artifacts.receipt.hash, "receipt-hash")
    assert.equal(details.receipt_verification.verified, false)
    assert.deepEqual(details.receipt_verification.checks, [])
    assert.match(result.content[0].text, /result_hash: result-hash/)
    assert.match(result.content[0].text, /has not independently verified/)
  })
})

test("MCP compute preserves an explicit upstream verification failure and evidence in ordinary output", async () => {
  const verification = { status: "failed", verified: false, checks: ["result hash mismatch"], boundary: "fixture verifier" }
  const evidence = { export_url: "https://provider.example/evidence/deal-1" }
  await withRuntime(async ({ config, requests }) => {
    const result = await handleToolCall("froglet", { action: "run_compute", provider_id: "provider-1", ...pythonWorkload, max_price_sats: 12, idempotency_key: "compute-retry" }, config)
    assert.equal(requests.at(-1).body.max_price_sats, 12)
    assert.equal(requests.at(-1).body.idempotency_key, "compute-retry")
    assert.deepEqual(result.structuredContent.receipt_verification, verification)
    assert.deepEqual(result.structuredContent.evidence, evidence)
    assert.match(result.content[0].text, /result hash mismatch/)
  }, { payload: terminalResponse({ receipt_verification: verification, evidence }) })
})

test("get_task and wait_task preserve runtime verification reports and falsy JSON results", async () => {
  const payload = terminalResponse({ receipt_verification: { status: "verified", verified: true, checks: ["signature", "result binding"], boundary: "upstream runtime" }, verification: { checked_by: "upstream" } })
  payload.deal.result = false
  await withRuntime(async ({ config }) => {
    for (const action of ["get_task", "wait_task"]) {
      const result = await handleToolCall("froglet", { action, task_id: "deal-1" }, config)
      assert.equal(result.structuredContent.result, false)
      assert.deepEqual(result.structuredContent.receipt_verification, payload.receipt_verification)
      assert.deepEqual(result.structuredContent.verification, payload.verification)
      assert.match(result.content[0].text, /result: false/)
    }
  }, { payload })
})

test("an explicit per-call cap cannot override a daemon budget refusal", async () => {
  const refusal = { code: "spend_budget_exceeded", error: "cumulative budget exhausted", remaining_msat: 1000 }
  await withRuntime(async ({ config, requests }) => {
    const result = await handleToolCall("froglet", { action: "run_compute", provider_id: "provider-1", ...pythonWorkload, max_price_sats: 1000, idempotency_key: "budget-refused" }, config)
    assert.equal(result.isError, true)
    assert.equal(requests.at(-1).body.max_price_sats, 1000)
    assert.equal(result.structuredContent.error.code, "spend_budget_exceeded")
    assert.deepEqual(result.structuredContent.error.details, refusal)
    assert.match(result.content[0].text, /requester spend policy/)
  }, { responseStatus: 402, payload: refusal })
})

test("pending execution retains its identity and evidence without a fabricated verification report", async () => {
  const payload = terminalResponse({ evidence: { quote_hash: "quote-hash" }, next_action: "wait_task" })
  payload.deal.status = "running"
  delete payload.deal.receipt
  delete payload.deal.result
  delete payload.deal.result_hash
  await withRuntime(async ({ config }) => {
    const result = await handleToolCall("froglet", { action: "run_compute", provider_id: "provider-1", ...pythonWorkload }, config)
    assert.equal(result.structuredContent.terminal, false)
    assert.equal(result.structuredContent.task_id, "deal-1")
    assert.deepEqual(result.structuredContent.evidence, payload.evidence)
    assert.equal(result.structuredContent.next_action, "wait_task")
    assert.equal(result.structuredContent.receipt_hash, null)
    assert.equal(result.structuredContent.receipt_verification.verified, false)
    assert.equal(result.structuredContent.receipt_verification.status, "not_available")
  }, { payload })
})

test("a runtime retry conflict remains an error with its original refusal details", async () => {
  const refusal = { error: "idempotency key reused with a different requester deal" }
  await withRuntime(async ({ config, requests }) => {
    const result = await handleToolCall("froglet", { action: "run_compute", provider_id: "provider-1", ...pythonWorkload, idempotency_key: "reused-key" }, config)
    assert.equal(result.isError, true)
    assert.equal(requests.at(-1).body.idempotency_key, "reused-key")
    assert.equal(result.structuredContent.error.http_status, 409)
    assert.deepEqual(result.structuredContent.error.details, refusal)
    assert.equal(Object.hasOwn(result.structuredContent.error, "code"), false)
    assert.match(result.content[0].text, /idempotency key reused/)
  }, { responseStatus: 409, payload: refusal })
})

test("receipt metadata alone and a claimed completed state never become a verification success", () => {
  const details = executionResultDetails({ task: { status: "completed", result: 0, receipt: { hash: "claimed-receipt", payload: { result_hash: "claimed-result" } } } })
  assert.equal(details.result, 0)
  assert.equal(details.receipt_verification.verified, false)
  assert.deepEqual(details.receipt_verification.checks, [])
  assert.equal(details.result_hash, "claimed-result")
})

test("MCP schema describes per-call defaults, global authority, and exact retry identity", () => {
  const properties = frogletToolInputSchema({ maxSearchLimit: 50 }).properties
  assert.equal(properties.max_price_sats.maximum, Number.MAX_SAFE_INTEGER)
  assert.match(properties.max_price_sats.description, /omitted means 0/)
  assert.match(properties.max_price_sats.description, /cumulative budget/)
  assert.equal(properties.idempotency_key.maxLength, 128)
  assert.match(properties.idempotency_key.description, /128 UTF-8 bytes/)
})

test("OpenClaw and MCP expose identical per-call cap and exact retry-key contracts", () => {
  const config = { maxSearchLimit: 50 }
  const mcp = frogletToolInputSchema(config).properties
  const openclaw = frogletToolParameters(config).properties
  for (const field of ["max_price_sats", "idempotency_key"]) {
    assert.deepEqual(openclaw[field], mcp[field])
  }
})

test("the registered OpenClaw tool forwards untrimmed retry identity for service and compute", async () => {
  await withRuntime(async ({ config, requests }) => {
    let tool
    registerFrogletTool({ registerTool(definition) { tool = definition } }, config)
    const key = ` ${"é".repeat(63)} `
    assert.equal(Buffer.byteLength(key, "utf8"), 128)
    for (const action of ["invoke_service", "run_compute"]) {
      const result = await tool.execute("tool-call", {
        action, provider_id: "provider-1", service_id: "echo", ...pythonWorkload,
        max_price_sats: 7, idempotency_key: key,
      })
      assert.equal(result.isError, undefined)
      assert.equal(requests.at(-1).body.idempotency_key, key)
      assert.equal(requests.at(-1).body.max_price_sats, 7)
    }
    const requestCount = requests.length
    const refused = await tool.execute("tool-call", {
      action: "run_compute", provider_id: "provider-1", ...pythonWorkload,
      idempotency_key: "é".repeat(65),
    })
    assert.equal(refused.isError, true)
    assert.match(refused.content[0].text, /128 UTF-8 bytes/)
    assert.equal(requests.length, requestCount)
  })
})
