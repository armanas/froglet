import assert from "node:assert/strict"
import { mkdir, readFile, writeFile } from "node:fs/promises"
import path from "node:path"
import { parseArgs } from "node:util"
import { fileURLToPath } from "node:url"

import register from "../../../integrations/openclaw/froglet/index.js"

const harnessDir = fileURLToPath(new URL("./", import.meta.url))

export const repoRoot = path.resolve(harnessDir, "../../..")
export const defaultModel = process.env.OPENAI_MODEL ?? "gpt-4.1-mini"

export function parseCliArgs(definitions, options = {}) {
  return parseArgs({
    args: process.argv.slice(2),
    options: definitions,
    allowPositionals: options.allowPositionals ?? false,
    strict: options.strict ?? true,
  })
}

export async function readJson(filePath) {
  return JSON.parse(await readFile(filePath, "utf8"))
}

export async function writeJson(filePath, value) {
  await mkdir(path.dirname(filePath), { recursive: true })
  await writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8")
}

export async function writeText(filePath, value) {
  await mkdir(path.dirname(filePath), { recursive: true })
  await writeFile(filePath, value, "utf8")
}

export function extractAppendedJson(text) {
  const haystack = String(text ?? "")
  const objectStart = haystack.lastIndexOf("\n{")
  const arrayStart = haystack.lastIndexOf("\n[")
  const start = Math.max(objectStart, arrayStart)
  assert.notEqual(start, -1, "missing appended JSON payload in tool output")
  return JSON.parse(haystack.slice(start + 1))
}

export function normalizeResultValue(value) {
  if (typeof value !== "string") {
    return value
  }
  try {
    return JSON.parse(value)
  } catch {
    return value
  }
}

export function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

export function getJsonPath(value, dottedPath) {
  return dottedPath.split(".").reduce((current, key) => {
    if (current == null) {
      return undefined
    }
    if (Array.isArray(current)) {
      const index = Number.parseInt(key, 10)
      return Number.isInteger(index) ? current[index] : undefined
    }
    return current[key]
  }, value)
}

export function loadFrogletTool({
  providerUrl,
  runtimeUrl,
  providerAuthTokenPath,
  runtimeAuthTokenPath,
  baseUrl,
  authTokenPath,
  requestTimeoutMs = Number.parseInt(process.env.FROGLET_REQUEST_TIMEOUT_MS ?? "15000", 10),
  defaultSearchLimit = Number.parseInt(process.env.FROGLET_DEFAULT_SEARCH_LIMIT ?? "10", 10),
  maxSearchLimit = Number.parseInt(process.env.FROGLET_MAX_SEARCH_LIMIT ?? "50", 10),
} = {}) {
  const tools = new Map()
  register({
    config: {
      hostProduct: "openclaw",
      ...(providerUrl ? { providerUrl } : {}),
      ...(runtimeUrl ? { runtimeUrl } : {}),
      ...(providerAuthTokenPath ? { providerAuthTokenPath } : {}),
      ...(runtimeAuthTokenPath ? { runtimeAuthTokenPath } : {}),
      ...(baseUrl ? { baseUrl } : {}),
      ...(authTokenPath ? { authTokenPath } : {}),
      requestTimeoutMs,
      defaultSearchLimit,
      maxSearchLimit,
    },
    registerTool(definition) {
      tools.set(definition.name, definition)
    },
    logger: {
      info() {},
      error() {},
    },
  })

  const froglet = tools.get("froglet")
  if (!froglet) {
    throw new Error("froglet tool was not registered")
  }
  return froglet
}

export async function executeTool(tool, args) {
  const result = await tool.execute(tool.name, args)
  const text = result.content?.[0]?.text ?? ""
  if (text.startsWith("Error:")) {
    throw new Error(text.slice("Error:".length).trim())
  }
  const raw = args.include_raw === true ? extractAppendedJson(text) : null
  return { text, raw, result }
}

export function resolveInventoryRole(inventory, roleName) {
  const role = inventory.roles?.[roleName]
  if (!role) {
    throw new Error(`missing role ${roleName} in inventory`)
  }
  return role
}

export function inventoryTokenPath(inventory, roleName, tokenKind) {
  const role = resolveInventoryRole(inventory, roleName)
  const tokenPath = role.token_paths?.[tokenKind]
  if (!tokenPath) {
    throw new Error(`missing token path ${tokenKind} for role ${roleName}`)
  }
  return tokenPath
}

export function inventoryProviderUrl(inventory, roleName = "froglet-marketplace") {
  return resolveInventoryRole(inventory, roleName).provider_local_url
}

export function inventoryRuntimeUrl(inventory, roleName = "froglet-marketplace") {
  return resolveInventoryRole(inventory, roleName).runtime_url
}

export async function requestJson(url, {
  method = "GET",
  headers = {},
  jsonBody,
  expectedStatuses = [200],
  timeoutMs = 15_000,
} = {}) {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const response = await fetch(url, {
      method,
      headers: {
        Accept: "application/json",
        ...(jsonBody === undefined ? {} : { "Content-Type": "application/json" }),
        ...headers,
      },
      ...(jsonBody === undefined ? {} : { body: JSON.stringify(jsonBody) }),
      signal: controller.signal,
    })
    const bodyText = await response.text()
    let payload = null
    if (bodyText.length > 0) {
      try {
        payload = JSON.parse(bodyText)
      } catch (error) {
        throw new Error(
          `Expected JSON from ${url}, got invalid payload: ${error.message}; body=${JSON.stringify(bodyText.slice(0, 200))}`
        )
      }
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
    clearTimeout(timeout)
  }
}

function apiKeyFromEnv() {
  return process.env.OPENCLAW_API_KEY || process.env.OPENAI_API_KEY
}

export function requireApiKey() {
  const apiKey = apiKeyFromEnv()
  if (!apiKey) {
    throw new Error("OPENCLAW_API_KEY or OPENAI_API_KEY is required")
  }
  process.env.OPENAI_API_KEY = apiKey
  return apiKey
}

export async function callResponses(body) {
  const apiKey = requireApiKey()
  const response = await fetch("https://api.openai.com/v1/responses", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${apiKey}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  })
  const text = await response.text()
  let json
  try {
    json = JSON.parse(text)
  } catch {
    throw new Error(`non-JSON OpenAI response (${response.status}): ${text}`)
  }
  if (!response.ok) {
    throw new Error(`OpenAI error (${response.status}): ${JSON.stringify(json)}`)
  }
  return json
}

export async function ensureFinalText(previousResponseId, prompt) {
  const response = await callResponses({
    model: defaultModel,
    previous_response_id: previousResponseId,
    input: prompt,
    max_output_tokens: 700,
  })
  return {
    responseId: response.id,
    finalText: response.output_text ?? "",
  }
}
