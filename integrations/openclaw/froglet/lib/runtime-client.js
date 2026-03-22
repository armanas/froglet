import { createHash } from "node:crypto"
import { readFile } from "node:fs/promises"

import { requestJson } from "./shared.js"

export const DEFAULT_WAIT_STATUSES = ["succeeded", "failed", "rejected"]

function unixTimeNow() {
  return Math.floor(Date.now() / 1000)
}

async function readRuntimeToken(tokenPath) {
  const token = (await readFile(tokenPath, "utf8")).trim()
  if (token.length === 0) {
    throw new Error(`runtime auth token file ${tokenPath} is empty`)
  }
  return token
}

async function runtimeRequest(runtimeUrl, token, timeoutMs, method, path, { jsonBody } = {}) {
  return requestJson(`${runtimeUrl}${path}`, {
    method,
    timeoutMs,
    headers: {
      Authorization: `Bearer ${token}`
    },
    jsonBody
  })
}

function sha256Hex(value) {
  return createHash("sha256").update(value).digest("hex")
}

function canonicalJson(value) {
  if (value === null) {
    return "null"
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalJson(item)).join(",")}]`
  }
  switch (typeof value) {
    case "boolean":
      return value ? "true" : "false"
    case "number":
      if (!Number.isFinite(value)) {
        throw new Error("Canonical JSON does not allow non-finite numbers")
      }
      return JSON.stringify(value)
    case "string":
      return JSON.stringify(value)
    case "object": {
      const keys = Object.keys(value).sort()
      return `{${keys.map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`
    }
    default:
      throw new Error(`Canonical JSON does not support ${typeof value}`)
  }
}

function isHex(value) {
  return typeof value === "string" && value.length > 0 && value.length % 2 === 0 && /^[0-9a-fA-F]+$/.test(value)
}

function validateWasmModuleBytes(moduleBytes) {
  try {
    return WebAssembly.validate(moduleBytes)
  } catch {
    return false
  }
}

function normalizeExecuteWasmRequest(request) {
  if (request === null || typeof request !== "object" || Array.isArray(request)) {
    return request
  }
  if (request.kind !== undefined || request.offer_id !== "execute.wasm") {
    return request
  }
  const submission = request.submission
  if (submission === null || typeof submission !== "object" || Array.isArray(submission)) {
    return request
  }
  const moduleHex =
    typeof submission.module_bytes_hex === "string"
      ? submission.module_bytes_hex.trim()
      : typeof submission.wasm_module_hex === "string"
        ? submission.wasm_module_hex.trim()
        : ""
  if (!moduleHex) {
    return request
  }
  if (!isHex(moduleHex)) {
    throw new Error("execute.wasm shorthand requires submission.wasm_module_hex to be even-length hex")
  }
  const moduleBytes = Buffer.from(moduleHex, "hex")
  if (!validateWasmModuleBytes(moduleBytes)) {
    throw new Error("execute.wasm shorthand requires submission.wasm_module_hex to be a valid WebAssembly module")
  }
  const inputValue = Object.prototype.hasOwnProperty.call(submission, "input") ? submission.input : null
  const requestedCapabilities = Array.isArray(submission.requested_capabilities)
    ? submission.requested_capabilities
    : []
  return {
    ...request,
    kind: "wasm",
    submission: {
      schema_version: "froglet/v1",
      submission_type: "wasm_submission",
      workload: {
        schema_version: "froglet/v1",
        workload_kind: "compute.wasm.v1",
        abi_version: "froglet.wasm.run_json.v1",
        module_format: "application/wasm",
        module_hash: sha256Hex(moduleBytes),
        input_format: "application/json+jcs",
        input_hash: sha256Hex(Buffer.from(canonicalJson(inputValue), "utf8")),
        requested_capabilities: requestedCapabilities
      },
      module_bytes_hex: moduleHex,
      input: inputValue
    }
  }
}

export async function walletBalance({
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs
}) {
  const token = await readRuntimeToken(runtimeAuthTokenPath)
  const response = await runtimeRequest(
    runtimeUrl,
    token,
    requestTimeoutMs,
    "GET",
    "/v1/runtime/wallet/balance"
  )
  return {
    runtime_url: runtimeUrl,
    ...response
  }
}

export async function searchRuntime({
  runtimeUrl,
  runtimeAuthTokenPath,
  limit,
  includeInactive = false,
  requestTimeoutMs
}) {
  const token = await readRuntimeToken(runtimeAuthTokenPath)
  const response = await runtimeRequest(
    runtimeUrl,
    token,
    requestTimeoutMs,
    "POST",
    "/v1/runtime/search",
    {
      jsonBody: {
        limit,
        include_inactive: includeInactive === true
      }
    }
  )
  return {
    runtime_url: runtimeUrl,
    ...response
  }
}

export async function getProvider({
  runtimeUrl,
  runtimeAuthTokenPath,
  providerId,
  requestTimeoutMs
}) {
  const token = await readRuntimeToken(runtimeAuthTokenPath)
  const response = await runtimeRequest(
    runtimeUrl,
    token,
    requestTimeoutMs,
    "GET",
    `/v1/runtime/providers/${encodeURIComponent(providerId)}`
  )
  return {
    runtime_url: runtimeUrl,
    ...response
  }
}

export async function buyWithRuntime({
  runtimeUrl,
  runtimeAuthTokenPath,
  request,
  requestTimeoutMs
}) {
  const token = await readRuntimeToken(runtimeAuthTokenPath)
  const normalizedRequest = normalizeExecuteWasmRequest(request)
  const response = await runtimeRequest(
    runtimeUrl,
    token,
    requestTimeoutMs,
    "POST",
    "/v1/runtime/deals",
    {
      jsonBody: normalizedRequest
    }
  )
  const status = response?.deal?.status
  return {
    runtime_url: runtimeUrl,
    quote: response.quote,
    deal: response.deal,
    payment_intent_path: response.payment_intent_path ?? null,
    payment_intent: response.payment_intent ?? null,
    terminal: ["succeeded", "failed", "rejected"].includes(status)
  }
}

export async function eventsQueryWithRuntime({
  runtimeUrl,
  runtimeAuthTokenPath,
  provider,
  kinds,
  limit,
  maxPriceSats,
  requestTimeoutMs
}) {
  const request = {
    provider,
    offer_id: "events.query",
    kind: "events_query",
    kinds
  }
  if (limit !== undefined) {
    request.limit = limit
  }
  if (maxPriceSats !== undefined) {
    request.max_price_sats = maxPriceSats
  }
  return buyWithRuntime({
    runtimeUrl,
    runtimeAuthTokenPath,
    request,
    requestTimeoutMs
  })
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

export async function getDeal({
  dealId,
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs
}) {
  const token = await readRuntimeToken(runtimeAuthTokenPath)
  const response = await runtimeRequest(
    runtimeUrl,
    token,
    requestTimeoutMs,
    "GET",
    `/v1/runtime/deals/${encodeURIComponent(dealId)}`
  )
  return {
    runtime_url: runtimeUrl,
    deal: response.deal
  }
}

export async function waitForDeal({
  dealId,
  runtimeUrl,
  runtimeAuthTokenPath,
  waitStatuses = DEFAULT_WAIT_STATUSES,
  timeoutSecs = 15,
  pollIntervalSecs = 0.2,
  requestTimeoutMs
}) {
  const startedAt = unixTimeNow()
  while (unixTimeNow() - startedAt < timeoutSecs) {
    const response = await getDeal({
      dealId,
      runtimeUrl,
      runtimeAuthTokenPath,
      requestTimeoutMs
    })
    if (waitStatuses.includes(response.deal?.status)) {
      return {
        runtime_url: runtimeUrl,
        wait_statuses: waitStatuses,
        deal: response.deal
      }
    }
    await delay(Math.max(0.05, pollIntervalSecs) * 1000)
  }

  throw new Error(
    `timed out waiting for deal ${dealId} to reach ${JSON.stringify([...waitStatuses].sort())}`
  )
}

export async function paymentIntentForDeal({
  dealId,
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs
}) {
  const token = await readRuntimeToken(runtimeAuthTokenPath)
  const response = await runtimeRequest(
    runtimeUrl,
    token,
    requestTimeoutMs,
    "GET",
    `/v1/runtime/deals/${encodeURIComponent(dealId)}/payment-intent`
  )
  return {
    runtime_url: runtimeUrl,
    deal_id: dealId,
    payment_intent: response.payment_intent
  }
}

export async function mockPayForDeal({
  dealId,
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs
}) {
  const token = await readRuntimeToken(runtimeAuthTokenPath)
  const response = await runtimeRequest(
    runtimeUrl,
    token,
    requestTimeoutMs,
    "POST",
    `/v1/runtime/deals/${encodeURIComponent(dealId)}/mock-pay`
  )
  return {
    runtime_url: runtimeUrl,
    deal_id: dealId,
    deal: response.deal,
    payment_intent_path: response.payment_intent_path ?? null,
    payment_intent: response.payment_intent ?? null
  }
}

export async function acceptResultForDeal({
  dealId,
  runtimeUrl,
  runtimeAuthTokenPath,
  expectedResultHash,
  requestTimeoutMs
}) {
  const token = await readRuntimeToken(runtimeAuthTokenPath)
  const response = await runtimeRequest(
    runtimeUrl,
    token,
    requestTimeoutMs,
    "POST",
    `/v1/runtime/deals/${encodeURIComponent(dealId)}/accept`,
    {
      jsonBody:
        expectedResultHash === undefined
          ? {}
          : { expected_result_hash: expectedResultHash }
    }
  )
  return {
    runtime_url: runtimeUrl,
    deal_id: dealId,
    deal: response.deal
  }
}
