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
  const response = await runtimeRequest(
    runtimeUrl,
    token,
    requestTimeoutMs,
    "POST",
    "/v1/runtime/deals",
    {
      jsonBody: request
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
