import { readFile } from "node:fs/promises"

import { requestJson } from "./shared.js"

const tokenCache = new Map()

async function readAuthToken(tokenPath) {
  const cached = tokenCache.get(tokenPath)
  if (cached) return cached
  const token = (await readFile(tokenPath, "utf8")).trim()
  if (token.length === 0) {
    throw new Error(`froglet auth token file ${tokenPath} is empty`)
  }
  tokenCache.set(tokenPath, token)
  return token
}

async function frogletRequest(baseUrl, tokenPath, timeoutMs, method, path, { jsonBody, expectedStatuses } = {}) {
  const token = await readAuthToken(tokenPath)
  return requestJson(`${baseUrl}${path}`, {
    method,
    timeoutMs,
    headers: {
      Authorization: `Bearer ${token}`
    },
    jsonBody,
    expectedStatuses
  })
}

function normalizeLegacyExecutionFields(request = {}) {
  const body = { ...request }

  if (body.execution_kind === undefined) {
    if (body.runtime === "wasm" && body.package_kind === "inline_module") {
      body.execution_kind = "wasm_inline"
    } else if (body.runtime === "wasm" && body.package_kind === "oci_image") {
      body.execution_kind = "wasm_oci"
    } else if (body.runtime === "builtin") {
      body.execution_kind = "builtin"
    }
  }

  if (body.abi_version === undefined && typeof body.contract_version === "string") {
    body.abi_version = body.contract_version
  }

  return body
}

export async function frogletStatus({ baseUrl, authTokenPath, requestTimeoutMs }) {
  return frogletRequest(baseUrl, authTokenPath, requestTimeoutMs, "GET", "/v1/froglet/status")
}

export async function frogletTailLogs({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  target,
  lines
}) {
  const query = new URLSearchParams()
  if (typeof target === "string" && target.trim().length > 0) {
    query.set("target", target.trim())
  }
  if (Number.isInteger(lines) && lines > 0) {
    query.set("lines", String(lines))
  }
  const suffix = query.size > 0 ? `?${query.toString()}` : ""
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/froglet/logs${suffix}`
  )
}

export async function frogletRestart({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  target
}) {
  return frogletRequest(baseUrl, authTokenPath, requestTimeoutMs, "POST", "/v1/froglet/restart", {
    jsonBody: target ? { target } : {}
  })
}

export async function listProjects({ baseUrl, authTokenPath, requestTimeoutMs }) {
  return frogletRequest(baseUrl, authTokenPath, requestTimeoutMs, "GET", "/v1/froglet/projects")
}

export async function createProject({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  request
}) {
  return frogletRequest(baseUrl, authTokenPath, requestTimeoutMs, "POST", "/v1/froglet/projects", {
    jsonBody: normalizeLegacyExecutionFields(request),
    expectedStatuses: [200, 201]
  })
}

export async function getProject({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  projectId
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/froglet/projects/${encodeURIComponent(projectId)}`
  )
}

export async function readProjectFile({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  projectId,
  path
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/froglet/projects/${encodeURIComponent(projectId)}/files/${encodeURIComponent(path)}`
  )
}

export async function writeProjectFile({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  projectId,
  path,
  contents
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "PUT",
    `/v1/froglet/projects/${encodeURIComponent(projectId)}/files/${encodeURIComponent(path)}`,
    {
      jsonBody: { contents }
    }
  )
}

export async function buildProject({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  projectId
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "POST",
    `/v1/froglet/projects/${encodeURIComponent(projectId)}/build`
  )
}

export async function testProject({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  projectId,
  input
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "POST",
    `/v1/froglet/projects/${encodeURIComponent(projectId)}/test`,
    {
      jsonBody: input === undefined ? {} : { input }
    }
  )
}

export async function publishProject({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  projectId
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "POST",
    `/v1/froglet/projects/${encodeURIComponent(projectId)}/publish`,
    { expectedStatuses: [200, 201] }
  )
}

export async function publishArtifact({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  request
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/froglet/artifacts/publish",
    {
      jsonBody: normalizeLegacyExecutionFields(request),
      expectedStatuses: [200, 201]
    }
  )
}

export async function listLocalServices({ baseUrl, authTokenPath, requestTimeoutMs }) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "GET",
    "/v1/froglet/services/local"
  )
}

export async function getLocalService({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  serviceId
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/froglet/services/local/${encodeURIComponent(serviceId)}`
  )
}

export async function discoverServices({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  limit,
  includeInactive,
  query
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/froglet/services/discover",
    {
      jsonBody: {
        limit,
        include_inactive: includeInactive,
        query
      }
    }
  )
}

export async function getService({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  request
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/froglet/services/get",
    { jsonBody: request }
  )
}

export async function invokeService({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  request
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/froglet/services/invoke",
    { jsonBody: request }
  )
}

export async function runCompute({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  request
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/froglet/compute/run",
    { jsonBody: normalizeLegacyExecutionFields(request) }
  )
}

export async function getTask({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  taskId
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/froglet/tasks/${encodeURIComponent(taskId)}`
  )
}

export async function waitTask({
  baseUrl,
  authTokenPath,
  requestTimeoutMs,
  taskId,
  timeoutSecs,
  pollIntervalSecs
}) {
  return frogletRequest(
    baseUrl,
    authTokenPath,
    requestTimeoutMs,
    "POST",
    `/v1/froglet/tasks/${encodeURIComponent(taskId)}/wait`,
    {
      jsonBody: {
        timeout_secs: timeoutSecs,
        poll_interval_secs: pollIntervalSecs
      }
    }
  )
}
