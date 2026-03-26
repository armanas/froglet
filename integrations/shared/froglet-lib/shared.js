import path from "node:path"

export const DEFAULT_TIMEOUT_MS = 10_000
export const DEFAULT_SEARCH_LIMIT = 10
export const DEFAULT_MAX_SEARCH_LIMIT = 50
export const DEFAULT_PYTHON_EXECUTABLE = "python3"
export const MIN_TIMEOUT_MS = 1_000
export const MAX_TIMEOUT_MS = 60_000
export const MIN_SEARCH_LIMIT = 1
export const ABSOLUTE_MAX_SEARCH_LIMIT = 200

export function clampInteger(value, fallback, minimum, maximum) {
  const parsed = Number.parseInt(String(value ?? ""), 10)
  if (!Number.isFinite(parsed)) {
    return fallback
  }
  return Math.min(Math.max(parsed, minimum), maximum)
}

export function clampNumber(value, fallback, minimum, maximum) {
  const parsed = Number.parseFloat(String(value ?? ""))
  if (!Number.isFinite(parsed)) {
    return fallback
  }
  return Math.min(Math.max(parsed, minimum), maximum)
}

const LOOPBACK_HTTP_HOSTS = new Set(["127.0.0.1", "localhost", "::1", "[::1]"])

export function normalizeBaseUrl(value, fieldName, { allowInsecure = false } = {}) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${fieldName} must be a non-empty URL`)
  }

  let parsed
  try {
    parsed = new URL(value)
  } catch (error) {
    throw new Error(`${fieldName} is not a valid URL: ${error.message}`)
  }

  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error(`${fieldName} must use http or https`)
  }

  if (!allowInsecure && parsed.protocol === "http:" && !LOOPBACK_HTTP_HOSTS.has(parsed.hostname)) {
    throw new Error(`${fieldName} must use https:// (http:// is only allowed for loopback addresses)`)
  }

  return parsed.toString().replace(/\/$/, "")
}

export function normalizeFilesystemPath(value, fieldName) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${fieldName} must be a non-empty filesystem path`)
  }
  return path.resolve(value.trim())
}

export function normalizeCommand(value, fieldName) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${fieldName} must be a non-empty command`)
  }
  return value.trim()
}

export function toolTextResult(text) {
  return {
    content: [
      {
        type: "text",
        text
      }
    ]
  }
}

export function formatTimestamp(seconds) {
  if (typeof seconds !== "number" || !Number.isFinite(seconds)) {
    return "unknown"
  }
  return new Date(seconds * 1000).toISOString()
}

export async function requestJson(
  url,
  {
    method = "GET",
    timeoutMs,
    headers = {},
    jsonBody,
    expectedStatuses = [200]
  } = {}
) {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), timeoutMs)

  try {
    const response = await fetch(url, {
      method,
      headers: {
        Accept: "application/json",
        ...(jsonBody !== undefined ? { "Content-Type": "application/json" } : {}),
        ...headers
      },
      ...(jsonBody !== undefined ? { body: JSON.stringify(jsonBody) } : {}),
      signal: controller.signal
    })

    const body = await response.text()
    let payload
    try {
      payload = body.length > 0 ? JSON.parse(body) : null
    } catch (error) {
      const preview = body.length > 0 ? body.slice(0, 200) : "<empty>"
      throw new Error(
        `Expected JSON from ${url}, got invalid payload: ${error.message}; body=${JSON.stringify(preview)}`
      )
    }

    if (!expectedStatuses.includes(response.status)) {
      throw new Error(
        `Request to ${url} failed with ${response.status}: ${JSON.stringify(payload)}`
      )
    }

    return payload
  } catch (error) {
    if (error?.name === "AbortError") {
      throw new Error(`Request to ${url} timed out after ${timeoutMs}ms`)
    }
    throw error
  } finally {
    clearTimeout(timer)
  }
}

export async function fetchJson(url, timeoutMs) {
  return requestJson(url, {
    method: "GET",
    timeoutMs,
    expectedStatuses: [200]
  })
}
