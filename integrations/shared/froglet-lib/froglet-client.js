import { createHash } from "node:crypto"
import { readFile, stat } from "node:fs/promises"

const FROGLET_SCHEMA_V1 = "froglet/v1"
const WORKLOAD_KIND_EXECUTION_V1 = "compute.execution.v1"
const WORKLOAD_KIND_COMPUTE_WASM_V1 = "compute.wasm.v1"
const WORKLOAD_KIND_COMPUTE_WASM_OCI_V1 = "compute.wasm.oci.v1"
const WASM_SUBMISSION_TYPE_V1 = "wasm_submission"
const WASM_OCI_SUBMISSION_TYPE_V1 = "wasm_oci_submission"
const WASM_RUN_JSON_ABI_V1 = "froglet.wasm.run_json.v1"
const WASM_MODULE_FORMAT = "application/wasm"
const WASM_MODULE_OCI_FORMAT = "application/vnd.oci.image.manifest.v1+json"
const JCS_JSON_FORMAT = "application/json+jcs"
const CONTRACT_BUILTIN_EVENTS_QUERY_V1 = "froglet.builtin.events_query.v1"
const CONTRACT_CONTAINER_JSON_V1 = "froglet.container.stdin_json.v1"
const CONTRACT_PYTHON_HANDLER_JSON_V1 = "froglet.python.handler_json.v1"
const CONTRACT_PYTHON_SCRIPT_JSON_V1 = "froglet.python.script_json.v1"
const TERMINAL_DEAL_STATES = new Set(["succeeded", "failed", "rejected", "cancelled", "completed", "done", "error"])
const TERMINAL_TASK_STATES = new Set(["succeeded", "failed", "rejected", "cancelled", "completed", "done", "error"])

/** @type {Map<string, { token: string, mtimeMs: number }>} */
const tokenCache = new Map()

function ensureJsonValue(value, label) {
  if (value === null) {
    return value
  }
  if (Array.isArray(value)) {
    return value.map((entry) => ensureJsonValue(entry, label))
  }
  switch (typeof value) {
    case "string":
    case "boolean":
      return value
    case "number":
      if (!Number.isFinite(value)) {
        throw new Error(`${label} contains a non-finite number`)
      }
      return value
    case "object":
      return Object.fromEntries(
        Object.entries(value)
          .filter(([, entry]) => entry !== undefined)
          .map(([key, entry]) => [key, ensureJsonValue(entry, label)])
      )
    default:
      throw new Error(`${label} contains an unsupported JSON value`)
  }
}

export function canonicalJsonStringify(value) {
  if (value === null) {
    return "null"
  }
  if (Array.isArray(value)) {
    return `[${value.map((entry) => canonicalJsonStringify(entry)).join(",")}]`
  }
  switch (typeof value) {
    case "string":
      return JSON.stringify(value)
    case "boolean":
      return value ? "true" : "false"
    case "number":
      if (!Number.isFinite(value)) {
        throw new Error("canonical JSON does not support non-finite numbers")
      }
      return JSON.stringify(value)
    case "object": {
      const entries = Object.entries(value)
        .filter(([, entry]) => entry !== undefined)
        .sort(([left], [right]) => left.localeCompare(right))
      return `{${entries.map(([key, entry]) => `${JSON.stringify(key)}:${canonicalJsonStringify(entry)}`).join(",")}}`
    }
    default:
      throw new Error(`canonical JSON does not support values of type ${typeof value}`)
  }
}

export function canonicalJsonBytes(value) {
  return Buffer.from(canonicalJsonStringify(ensureJsonValue(value, "canonical JSON")), "utf8")
}

export function sha256Hex(data) {
  return createHash("sha256").update(data).digest("hex")
}

function normalizedInput(input) {
  return input === undefined ? null : ensureJsonValue(input, "workload input")
}

function inputHash(input) {
  return sha256Hex(canonicalJsonBytes(normalizedInput(input)))
}

function normalizeUrl(value) {
  if (typeof value !== "string" || value.trim().length === 0) {
    return null
  }
  return value.trim().replace(/\/$/, "")
}

function sameApiBaseUrl(left, right) {
  const normalizedLeft = normalizeUrl(left)
  const normalizedRight = normalizeUrl(right)
  return normalizedLeft !== null && normalizedLeft === normalizedRight
}

function missingTaskMessage(payload) {
  const error =
    typeof payload?.error === "string" && payload.error.trim().length > 0 ? payload.error.trim() : null
  if (!error || error === "deal not found" || error === "deal not found after sync") {
    return "job not found"
  }
  return error
}

function normalizeProviderJobLookupError(error) {
  const message = String(error?.message ?? error)
  if (message.includes("/v1/node/jobs/") && message.includes("failed with 404")) {
    return new Error("job not found")
  }
  return error
}

function isHealthyResponse(payload) {
  return payload?.healthy === true || payload?.status === "ok"
}

async function readAuthToken(tokenPath) {
  const fileStat = await stat(tokenPath)
  const cached = tokenCache.get(tokenPath)
  if (cached && cached.mtimeMs === fileStat.mtimeMs) {
    return cached.token
  }
  const token = (await readFile(tokenPath, "utf8")).trim()
  if (token.length === 0) {
    throw new Error(`froglet auth token file ${tokenPath} is empty`)
  }
  tokenCache.set(tokenPath, { token, mtimeMs: fileStat.mtimeMs })
  return token
}

async function jsonRequest(
  url,
  {
    method = "GET",
    timeoutMs,
    headers = {},
    jsonBody,
    expectedStatuses = [200],
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
        ...headers,
      },
      ...(jsonBody !== undefined ? { body: JSON.stringify(jsonBody) } : {}),
      signal: controller.signal,
    })
    const body = await response.text()
    let payload = null
    if (body.length > 0) {
      try {
        payload = JSON.parse(body)
      } catch (error) {
        const preview = body.slice(0, 200)
        throw new Error(
          `Expected JSON from ${url}, got invalid payload: ${error.message}; body=${JSON.stringify(preview)}`
        )
      }
    }
    if (!expectedStatuses.includes(response.status)) {
      throw new Error(
        `Request to ${url} failed with ${response.status}: ${JSON.stringify(payload)}`
      )
    }
    return { status: response.status, payload }
  } catch (error) {
    if (error?.name === "AbortError") {
      throw new Error(`Request to ${url} timed out after ${timeoutMs}ms`)
    }
    throw error
  } finally {
    clearTimeout(timer)
  }
}

/**
 * Make an authenticated JSON request.
 *
 * @param {string} baseUrl
 * @param {string} tokenPath
 * @param {number} timeoutMs
 * @param {string} method
 * @param {string} path
 * @param {{ jsonBody?: unknown, expectedStatuses?: number[] }} [opts]
 */
async function frogletRequest(baseUrl, tokenPath, timeoutMs, method, path, { jsonBody, expectedStatuses } = {}) {
  const token = await readAuthToken(tokenPath)
  const { payload } = await jsonRequest(`${baseUrl}${path}`, {
    method,
    timeoutMs,
    headers: {
      Authorization: `Bearer ${token}`,
    },
    jsonBody,
    expectedStatuses,
  })
  return payload
}

async function frogletRequestWithStatus(
  baseUrl,
  tokenPath,
  timeoutMs,
  method,
  path,
  { jsonBody, expectedStatuses } = {}
) {
  const token = await readAuthToken(tokenPath)
  return jsonRequest(`${baseUrl}${path}`, {
    method,
    timeoutMs,
    headers: {
      Authorization: `Bearer ${token}`,
    },
    jsonBody,
    expectedStatuses,
  })
}

async function frogletPublicRequest(baseUrl, timeoutMs, path, { expectedStatuses } = {}) {
  const { payload } = await jsonRequest(`${baseUrl}${path}`, {
    method: "GET",
    timeoutMs,
    expectedStatuses,
  })
  return payload
}

function normalizedPriority(value) {
  return typeof value === "number" && Number.isFinite(value) ? value : Number.MAX_SAFE_INTEGER
}

function endpointPreference(endpoint) {
  const uri = normalizeUrl(endpoint?.uri)
  if (!uri) {
    return null
  }
  if (uri.startsWith("https://")) {
    return "https"
  }
  if (uri.startsWith("http://")) {
    return "http"
  }
  return null
}

export function selectTransportEndpoint(transportEndpoints) {
  const endpoints = Array.isArray(transportEndpoints) ? [...transportEndpoints] : []
  const supported = endpoints
    .map((endpoint) => ({
      endpoint,
      scheme: endpointPreference(endpoint),
      hasQuoteHttp: Array.isArray(endpoint?.features) && endpoint.features.includes("quote_http"),
      priority: normalizedPriority(endpoint?.priority),
    }))
    .filter((candidate) => candidate.scheme !== null)
  const preferred = supported.some((candidate) => candidate.hasQuoteHttp)
    ? supported.filter((candidate) => candidate.hasQuoteHttp)
    : supported
  preferred.sort((left, right) => {
    if (left.priority !== right.priority) {
      return left.priority - right.priority
    }
    if (left.scheme !== right.scheme) {
      return left.scheme === "https" ? -1 : 1
    }
    return 0
  })
  return preferred[0]?.endpoint ?? null
}

function priceSatsFromOffer(offer) {
  const base = Number.isFinite(offer?.base_fee_msat) ? offer.base_fee_msat : 0
  const success = Number.isFinite(offer?.success_fee_msat) ? offer.success_fee_msat : 0
  return Math.ceil((base + success) / 1000)
}

function queryMatchesService(service, query) {
  if (typeof query !== "string" || query.trim().length === 0) {
    return true
  }
  const needle = query.trim().toLowerCase()
  const haystacks = [
    service.service_id,
    service.offer_id,
    service.provider_id,
    service.provider_url,
    service.runtime,
    service.package_kind,
    service.contract_version,
  ]
  return haystacks.some((entry) => typeof entry === "string" && entry.toLowerCase().includes(needle))
}

function flattenProviderOffer(provider, offer) {
  const executionProfile = offer?.execution_profile ?? {}
  const endpoint = selectTransportEndpoint(provider?.transport_endpoints)
  return {
    service_id: offer?.offer_id ?? "unknown",
    offer_id: offer?.offer_id ?? "unknown",
    offer_kind: offer?.offer_kind ?? "unknown",
    resource_kind: "service",
    summary: "none",
    runtime: executionProfile?.runtime ?? offer?.runtime ?? "unknown",
    package_kind: executionProfile?.package_kind ?? "unknown",
    contract_version: executionProfile?.contract_version ?? executionProfile?.abi_version ?? "unknown",
    requested_access: Array.isArray(executionProfile?.access_handles) ? executionProfile.access_handles : [],
    mode: "unknown",
    price_sats: priceSatsFromOffer(offer),
    publication_state: "unknown",
    provider_id: provider?.provider_id ?? "unknown",
    provider_url: normalizeUrl(endpoint?.uri) ?? null,
    descriptor_hash: provider?.descriptor_hash,
    settlement_method: offer?.settlement_method,
  }
}

export function flattenMarketplaceProviders(response, { query } = {}) {
  const providers = Array.isArray(response?.providers) ? response.providers : []
  const services = providers.flatMap((provider) => {
    const offers = Array.isArray(provider?.offers) ? provider.offers : []
    return offers.map((offer) => flattenProviderOffer(provider, offer))
  })
  return services.filter((service) => queryMatchesService(service, query))
}

function defaultEntrypointKindFor(runtime) {
  return runtime === "builtin" ? "builtin" : "handler"
}

function defaultEntrypointFor(runtime, entrypointKind) {
  if (runtime === "builtin") {
    return "events.query"
  }
  if (runtime === "any") {
    return ""
  }
  if (entrypointKind === "script") {
    return "__main__"
  }
  if (runtime === "python" || runtime === "tee_python") {
    return "handler"
  }
  return "run"
}

function defaultContractVersionFor(runtime, packageKind, entrypointKind) {
  if (runtime === "any") {
    return ""
  }
  if ((runtime === "python" || runtime === "tee_python") && packageKind === "inline_source" && entrypointKind === "script") {
    return CONTRACT_PYTHON_SCRIPT_JSON_V1
  }
  if ((runtime === "python" || runtime === "tee_python") && packageKind === "inline_source") {
    return CONTRACT_PYTHON_HANDLER_JSON_V1
  }
  if ((runtime === "container" || runtime === "python") && packageKind === "oci_image") {
    return CONTRACT_CONTAINER_JSON_V1
  }
  if (runtime === "builtin" && packageKind === "builtin") {
    return CONTRACT_BUILTIN_EVENTS_QUERY_V1
  }
  return WASM_RUN_JSON_ABI_V1
}

function inferRuntime(request) {
  if (typeof request?.runtime === "string" && request.runtime.trim().length > 0) {
    return request.runtime.trim()
  }
  if (typeof request?.wasm_module_hex === "string" && request.wasm_module_hex.trim().length > 0) {
    return "wasm"
  }
  if (typeof request?.inline_source === "string" && request.inline_source.trim().length > 0) {
    return "python"
  }
  return null
}

function inferPackageKind(request) {
  if (typeof request?.package_kind === "string" && request.package_kind.trim().length > 0) {
    return request.package_kind.trim()
  }
  if (typeof request?.wasm_module_hex === "string" && request.wasm_module_hex.trim().length > 0) {
    return "inline_module"
  }
  if (typeof request?.inline_source === "string" && request.inline_source.trim().length > 0) {
    return "inline_source"
  }
  if (
    typeof request?.oci_reference === "string" &&
    request.oci_reference.trim().length > 0 &&
    typeof request?.oci_digest === "string" &&
    request.oci_digest.trim().length > 0
  ) {
    return "oci_image"
  }
  return null
}

function requestedAccessFromMounts(mounts) {
  if (!Array.isArray(mounts)) {
    return []
  }
  return mounts
    .filter((mount) => mount && typeof mount === "object")
    .map((mount) => `mount.${mount.kind}.${mount.read_only === true ? "read" : "write"}.${mount.handle}`)
}

function normalizedExecutionProfile(service) {
  const runtime = typeof service?.runtime === "string" && service.runtime.trim().length > 0 ? service.runtime : "unknown"
  const packageKind =
    typeof service?.package_kind === "string" && service.package_kind.trim().length > 0
      ? service.package_kind
      : "unknown"
  const entrypointKind =
    typeof service?.entrypoint_kind === "string" && service.entrypoint_kind.trim().length > 0
      ? service.entrypoint_kind
      : defaultEntrypointKindFor(runtime)
  const entrypointValue =
    typeof service?.entrypoint === "string" && service.entrypoint.trim().length > 0 ? service.entrypoint : ""
  const useDefaultEntrypoint =
    entrypointValue.length === 0 ||
    (entrypointKind === "handler" &&
      (entrypointValue.includes("/") || entrypointValue.endsWith(".py") || entrypointValue.includes("\\")))
  const entrypoint = useDefaultEntrypoint
    ? defaultEntrypointFor(runtime, entrypointKind)
    : entrypointValue
  const contractVersion =
    typeof service?.contract_version === "string" && service.contract_version.trim().length > 0
      ? service.contract_version
      : defaultContractVersionFor(runtime, packageKind, entrypointKind)
  return { runtime, packageKind, entrypointKind, entrypoint, contractVersion }
}

export function buildWasmSubmission({
  moduleBytesHex,
  input = null,
  contractVersion = WASM_RUN_JSON_ABI_V1,
  requestedCapabilities = [],
}) {
  if (typeof moduleBytesHex !== "string" || moduleBytesHex.trim().length === 0) {
    throw new Error("inline Wasm submission requires wasm_module_hex")
  }
  const moduleBytes = Buffer.from(moduleBytesHex.trim(), "hex")
  const normalized = normalizedInput(input)
  return {
    schema_version: FROGLET_SCHEMA_V1,
    submission_type: WASM_SUBMISSION_TYPE_V1,
    workload: {
      schema_version: FROGLET_SCHEMA_V1,
      workload_kind: WORKLOAD_KIND_COMPUTE_WASM_V1,
      abi_version: contractVersion,
      module_format: WASM_MODULE_FORMAT,
      module_hash: sha256Hex(moduleBytes),
      input_format: JCS_JSON_FORMAT,
      input_hash: inputHash(normalized),
      requested_capabilities: [...requestedCapabilities],
    },
    module_bytes_hex: moduleBytesHex.trim(),
    input: normalized,
  }
}

export function buildOciWasmSubmission({
  ociReference,
  ociDigest,
  input = null,
  contractVersion = WASM_RUN_JSON_ABI_V1,
  requestedCapabilities = [],
}) {
  if (typeof ociReference !== "string" || ociReference.trim().length === 0) {
    throw new Error("OCI Wasm submission requires oci_reference")
  }
  if (typeof ociDigest !== "string" || ociDigest.trim().length === 0) {
    throw new Error("OCI Wasm submission requires oci_digest")
  }
  const normalized = normalizedInput(input)
  return {
    schema_version: FROGLET_SCHEMA_V1,
    submission_type: WASM_OCI_SUBMISSION_TYPE_V1,
    workload: {
      schema_version: FROGLET_SCHEMA_V1,
      workload_kind: WORKLOAD_KIND_COMPUTE_WASM_OCI_V1,
      abi_version: contractVersion,
      module_format: WASM_MODULE_OCI_FORMAT,
      oci_reference: ociReference.trim(),
      oci_digest: ociDigest.trim(),
      input_format: JCS_JSON_FORMAT,
      input_hash: inputHash(normalized),
      requested_capabilities: [...requestedCapabilities],
    },
    input: normalized,
  }
}

export function buildExecutionWorkload(request = {}) {
  const runtime = inferRuntime(request)
  if (!runtime) {
    throw new Error("run_compute requires runtime, or enough fields to infer it")
  }
  const packageKind = inferPackageKind(request)
  if (!packageKind) {
    throw new Error("run_compute requires package_kind, or enough fields to infer it")
  }
  const entrypointKind =
    typeof request?.entrypoint_kind === "string" && request.entrypoint_kind.trim().length > 0
      ? request.entrypoint_kind.trim()
      : defaultEntrypointKindFor(runtime)
  const entrypoint =
    typeof request?.entrypoint === "string" && request.entrypoint.trim().length > 0
      ? request.entrypoint.trim()
      : defaultEntrypointFor(runtime, entrypointKind)
  const contractVersion =
    typeof request?.contract_version === "string" && request.contract_version.trim().length > 0
      ? request.contract_version.trim()
      : defaultContractVersionFor(runtime, packageKind, entrypointKind)
  const mounts = Array.isArray(request?.mounts) ? request.mounts : []
  const input = normalizedInput(request?.input)
  const workload = {
    schema_version: FROGLET_SCHEMA_V1,
    workload_kind: WORKLOAD_KIND_EXECUTION_V1,
    runtime,
    package_kind: packageKind,
    entrypoint: {
      kind: entrypointKind,
      value: entrypoint,
    },
    contract_version: contractVersion,
    input_format: JCS_JSON_FORMAT,
    input_hash: inputHash(input),
    requested_access: requestedAccessFromMounts(mounts),
    security: {
      mode: "standard",
    },
    mounts,
    input,
  }

  if (packageKind === "inline_module") {
    if (typeof request?.wasm_module_hex !== "string" || request.wasm_module_hex.trim().length === 0) {
      throw new Error("inline_module execution requires wasm_module_hex")
    }
    workload.module_hash = sha256Hex(Buffer.from(request.wasm_module_hex.trim(), "hex"))
    workload.module_bytes_hex = request.wasm_module_hex.trim()
  } else if (packageKind === "inline_source") {
    if (typeof request?.inline_source !== "string" || request.inline_source.trim().length === 0) {
      throw new Error("inline_source execution requires inline_source")
    }
    workload.source_hash = sha256Hex(Buffer.from(request.inline_source, "utf8"))
    workload.inline_source = request.inline_source
  } else if (packageKind === "oci_image") {
    if (typeof request?.oci_reference !== "string" || request.oci_reference.trim().length === 0) {
      throw new Error("oci_image execution requires oci_reference")
    }
    if (typeof request?.oci_digest !== "string" || request.oci_digest.trim().length === 0) {
      throw new Error("oci_image execution requires oci_digest")
    }
    workload.module_hash = request.oci_digest.trim()
    workload.oci_reference = request.oci_reference.trim()
    workload.oci_digest = request.oci_digest.trim()
  } else if (packageKind === "builtin" && typeof request?.builtin_name === "string" && request.builtin_name.trim().length > 0) {
    workload.builtin_name = request.builtin_name.trim()
  }

  return workload
}

export function buildServiceAddressedExecution(service, input = null) {
  if (!service || typeof service !== "object") {
    throw new Error("invoke_service requires a provider service record")
  }
  const { runtime, packageKind, entrypointKind, entrypoint, contractVersion } = normalizedExecutionProfile(service)
  const mounts = Array.isArray(service.mounts) ? service.mounts : []
  const normalized = normalizedInput(input)
  const bindingHash =
    typeof service?.binding_hash === "string" && service.binding_hash.trim().length > 0
      ? service.binding_hash
      : typeof service?.module_hash === "string" && service.module_hash.trim().length > 0
        ? service.module_hash
        : null
  if (!bindingHash) {
    throw new Error(`service ${service.service_id ?? "unknown"} does not expose a binding hash`)
  }
  const execution = {
    schema_version: FROGLET_SCHEMA_V1,
    workload_kind: WORKLOAD_KIND_EXECUTION_V1,
    runtime,
    package_kind: packageKind,
    entrypoint: {
      kind: entrypointKind,
      value: entrypoint,
    },
    contract_version: contractVersion,
    input_format: JCS_JSON_FORMAT,
    input_hash: inputHash(normalized),
    requested_access: requestedAccessFromMounts(mounts),
    security: {
      mode: "standard",
      service_id: service.service_id,
    },
    mounts,
    input: normalized,
  }
  if (packageKind === "inline_source") {
    execution.source_hash = bindingHash
  } else if (packageKind === "inline_module" || packageKind === "oci_image") {
    execution.module_hash = bindingHash
  }
  return execution
}

async function runtimeSearchProviders({
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  limit = 100,
}) {
  return frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/runtime/search",
    {
      jsonBody: {
        limit,
      },
    }
  )
}

async function runtimeProviderDetails({
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  providerId,
}) {
  return frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/runtime/providers/${encodeURIComponent(providerId)}`
  )
}

async function fetchPublicProviderService({ providerUrl, requestTimeoutMs, serviceId }) {
  return frogletPublicRequest(
    providerUrl,
    requestTimeoutMs,
    `/v1/provider/services/${encodeURIComponent(serviceId)}`
  )
}

function providerUrlFromRuntimeDetail(detail, providerId) {
  const endpoint = selectTransportEndpoint(detail?.transport_endpoints)
  const providerUrl = normalizeUrl(endpoint?.uri)
  if (!providerUrl) {
    throw new Error(`provider ${providerId} does not advertise an http(s) quote_http endpoint`)
  }
  return providerUrl
}

async function resolveProviderReference({
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  request,
  searchLimit = 100,
}) {
  const explicitProviderUrl = normalizeUrl(request?.provider_url)
  const explicitProviderId =
    typeof request?.provider_id === "string" && request.provider_id.trim().length > 0
      ? request.provider_id.trim()
      : null
  const serviceId =
    typeof request?.service_id === "string" && request.service_id.trim().length > 0
      ? request.service_id.trim()
      : null

  if (explicitProviderUrl) {
    return {
      providerId: explicitProviderId,
      providerUrl: explicitProviderUrl,
      matchSource: "provider_url",
    }
  }

  if (explicitProviderId) {
    const providerResponse = await runtimeProviderDetails({
      runtimeUrl,
      runtimeAuthTokenPath,
      requestTimeoutMs,
      providerId: explicitProviderId,
    })
    const detail = providerResponse?.provider
    if (!detail) {
      throw new Error(`provider ${explicitProviderId} not found`)
    }
    return {
      providerId: explicitProviderId,
      providerUrl: providerUrlFromRuntimeDetail(detail, explicitProviderId),
      providerDetail: detail,
      matchSource: "provider_id",
    }
  }

  if (!serviceId) {
    throw new Error("provider_id or provider_url is required")
  }

  const searchResponse = await runtimeSearchProviders({
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    limit: searchLimit,
  })
  const exactMatches = flattenMarketplaceProviders(searchResponse, { query: serviceId }).filter(
    (service) => service.service_id === serviceId
  )
  if (exactMatches.length === 0) {
    throw new Error(`service not found: ${serviceId}`)
  }
  const uniqueMatches = new Map()
  for (const match of exactMatches) {
    const key = `${match.provider_id}::${match.provider_url ?? ""}`
    uniqueMatches.set(key, match)
  }
  if (uniqueMatches.size > 1) {
    throw new Error(`service_id ${serviceId} matched multiple providers; supply provider_id`)
  }
  const match = [...uniqueMatches.values()][0]
  if (!match.provider_url) {
    throw new Error(`service ${serviceId} did not expose a usable provider_url`)
  }
  return {
    providerId: match.provider_id,
    providerUrl: match.provider_url,
    discoveryService: match,
    matchSource: "service_id",
  }
}

async function resolveRemoteService({
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  request,
  searchLimit = 100,
}) {
  const serviceId =
    typeof request?.service_id === "string" && request.service_id.trim().length > 0
      ? request.service_id.trim()
      : null
  if (!serviceId) {
    throw new Error("service_id is required")
  }
  const provider = await resolveProviderReference({
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    request,
    searchLimit,
  })
  const serviceResponse = await fetchPublicProviderService({
    providerUrl: provider.providerUrl,
    requestTimeoutMs,
    serviceId,
  })
  const service = serviceResponse?.service
  if (!service) {
    throw new Error(`provider ${provider.providerId ?? provider.providerUrl} did not return a service record for ${serviceId}`)
  }
  if (
    provider.providerId &&
    typeof service?.provider_id === "string" &&
    service.provider_id.length > 0 &&
    service.provider_id !== provider.providerId
  ) {
    throw new Error(
      `service ${serviceId} belongs to provider ${service.provider_id}, not requested provider ${provider.providerId}`
    )
  }
  return {
    providerId: service?.provider_id ?? provider.providerId,
    providerUrl: provider.providerUrl,
    service,
    provider,
  }
}

function normalizeRuntimeDealCreation(response) {
  const deal = response?.deal ?? {}
  const normalized = {
    provider_id: response?.provider_id,
    provider_url: response?.provider_url,
    quote: response?.quote,
    deal,
    payment_intent_path: response?.payment_intent_path,
    payment_intent: response?.payment_intent,
  }
  if (TERMINAL_DEAL_STATES.has(String(deal?.status ?? "").toLowerCase())) {
    return {
      ...normalized,
      terminal: true,
      status: deal.status ?? "unknown",
      result: deal.result,
      error: deal.error,
    }
  }
  return {
    ...normalized,
    terminal: false,
    task: deal,
  }
}

function normalizeRuntimeTaskResponse(response) {
  const deal = response?.deal ?? {}
  return {
    task: deal,
    deal,
  }
}

function normalizedTaskState(response) {
  const task = response?.task ?? response?.deal ?? response
  const state = task?.state ?? task?.status
  return typeof state === "string" ? state.toLowerCase() : null
}

// ---------------------------------------------------------------------------
// Removed functions — stubs that throw descriptive errors
// ---------------------------------------------------------------------------

const PROJECT_AUTHORING_ERROR = "Project authoring not available in current API"

/** @deprecated Removed — use systemd journal directly */
export async function frogletTailLogs(_opts) {
  throw new Error("Log tailing removed; use systemd journal directly")
}

/** @deprecated Removed — use systemctl directly */
export async function frogletRestart(_opts) {
  throw new Error("Restart removed; use systemctl directly")
}

/** @deprecated Removed */
export async function listProjects(_opts) {
  throw new Error(PROJECT_AUTHORING_ERROR)
}

/** @deprecated Removed */
export async function createProject(_opts) {
  throw new Error(PROJECT_AUTHORING_ERROR)
}

/** @deprecated Removed */
export async function getProject(_opts) {
  throw new Error(PROJECT_AUTHORING_ERROR)
}

/** @deprecated Removed */
export async function readProjectFile(_opts) {
  throw new Error(PROJECT_AUTHORING_ERROR)
}

/** @deprecated Removed */
export async function writeProjectFile(_opts) {
  throw new Error(PROJECT_AUTHORING_ERROR)
}

/** @deprecated Removed */
export async function buildProject(_opts) {
  throw new Error(PROJECT_AUTHORING_ERROR)
}

/** @deprecated Removed */
export async function testProject(_opts) {
  throw new Error(PROJECT_AUTHORING_ERROR)
}

/** @deprecated Removed */
export async function publishProject(_opts) {
  throw new Error(PROJECT_AUTHORING_ERROR)
}

// ---------------------------------------------------------------------------
// Active functions — new dual-URL API
// ---------------------------------------------------------------------------

/**
 * Fetch node status by composing parallel requests to the provider and runtime APIs.
 *
 * @param {{ providerUrl: string, providerAuthTokenPath: string, runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number }} config
 */
export async function frogletStatus({
  providerUrl,
  providerAuthTokenPath,
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
}) {
  const [providerHealth, capabilities, identity, runtimeHealth] = await Promise.all([
    frogletPublicRequest(providerUrl, requestTimeoutMs, "/health"),
    frogletRequest(providerUrl, providerAuthTokenPath, requestTimeoutMs, "GET", "/v1/node/capabilities"),
    frogletRequest(providerUrl, providerAuthTokenPath, requestTimeoutMs, "GET", "/v1/node/identity"),
    frogletPublicRequest(runtimeUrl, requestTimeoutMs, "/health"),
  ])
  const providerHealthy = isHealthyResponse(providerHealth)
  const runtimeHealthy = isHealthyResponse(runtimeHealth)
  return {
    healthy: providerHealthy && runtimeHealthy,
    node_id: identity?.node_id ?? identity?.id,
    discovery: identity?.discovery,
    reference_discovery: identity?.reference_discovery,
    compute_offers: capabilities?.compute_offers ?? [],
    raw_compute_offer_ids: capabilities?.compute_offer_ids ?? [],
    raw_compute_offer_id: capabilities?.compute_offer_id,
    provider: { healthy: providerHealthy },
    runtime: { healthy: runtimeHealthy },
    components: {
      provider: { healthy: providerHealthy, health: providerHealth },
      runtime: { healthy: runtimeHealthy, health: runtimeHealth },
    },
    _health: providerHealth,
    _runtime_health: runtimeHealth,
    _capabilities: capabilities,
    _identity: identity,
  }
}

/**
 * Publish an artifact to the provider API.
 *
 * @param {{ providerUrl: string, providerAuthTokenPath: string, requestTimeoutMs: number, request: object }} config
 */
export async function publishArtifact({ providerUrl, providerAuthTokenPath, requestTimeoutMs, request }) {
  return frogletRequest(
    providerUrl,
    providerAuthTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/provider/artifacts/publish",
    {
      jsonBody: request,
      expectedStatuses: [200, 201],
    }
  )
}

/**
 * List services registered with the local provider.
 *
 * @param {{ providerUrl: string, providerAuthTokenPath: string, requestTimeoutMs: number }} config
 */
export async function listLocalServices({ providerUrl, providerAuthTokenPath, requestTimeoutMs }) {
  return frogletRequest(
    providerUrl,
    providerAuthTokenPath,
    requestTimeoutMs,
    "GET",
    "/v1/provider/services"
  )
}

/**
 * Get a single service from the local provider.
 *
 * @param {{ providerUrl: string, providerAuthTokenPath: string, requestTimeoutMs: number, serviceId: string }} config
 */
export async function getLocalService({ providerUrl, providerAuthTokenPath, requestTimeoutMs, serviceId }) {
  return frogletRequest(
    providerUrl,
    providerAuthTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/provider/services/${encodeURIComponent(serviceId)}`
  )
}

/**
 * Search for remote services via the runtime API.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, limit?: number, includeInactive?: boolean, query?: string }} config
 */
export async function discoverServices({ runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs, limit, includeInactive, query }) {
  const response = await frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/runtime/search",
    {
      jsonBody: {
        limit,
        include_inactive: includeInactive,
      },
    }
  )
  return {
    ...response,
    services: flattenMarketplaceProviders(response, { query }),
  }
}

/**
 * Get a specific remote service by resolving the provider via the runtime API
 * and then fetching the canonical public service record from the provider.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, request: { provider_id?: string, provider_url?: string, service_id?: string }, searchLimit?: number }} config
 */
export async function getService({ runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs, request, searchLimit = 100 }) {
  const resolved = await resolveRemoteService({
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    request,
    searchLimit,
  })
  return {
    service: {
      ...resolved.service,
      provider_url: resolved.providerUrl,
    },
  }
}

/**
 * Invoke a named service by building a canonical service-addressed execution
 * workload and submitting it through the runtime deal flow.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, request: { provider_id?: string, provider_url?: string, service_id?: string, input?: unknown }, searchLimit?: number }} config
 */
export async function invokeService({ runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs, request, searchLimit = 100 }) {
  const resolved = await resolveRemoteService({
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    request,
    searchLimit,
  })
  const response = await frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/runtime/deals",
    {
      jsonBody: {
        provider: {
          provider_id: resolved.providerId,
          provider_url: resolved.providerUrl,
        },
        offer_id: resolved.service.offer_id,
        kind: "execution",
        execution: buildServiceAddressedExecution(resolved.service, request?.input),
      },
      expectedStatuses: [200, 201],
    }
  )
  return normalizeRuntimeDealCreation(response)
}

/**
 * Run open-ended compute through the runtime deal flow.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, request: { provider_id?: string, provider_url?: string, input?: unknown, runtime?: string, package_kind?: string, entrypoint_kind?: string, entrypoint?: string, contract_version?: string, mounts?: unknown, artifact_path?: string, wasm_module_hex?: string, inline_source?: string, oci_reference?: string, oci_digest?: string }, searchLimit?: number }} config
 */
export async function runCompute({ runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs, request, searchLimit = 100 }) {
  if (typeof request?.artifact_path === "string" && request.artifact_path.trim().length > 0) {
    throw new Error("run_compute via runtime deals does not support artifact_path; provide inline bytes/source or OCI coordinates")
  }
  const provider = await resolveProviderReference({
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    request,
    searchLimit,
  })
  let spec
  if (typeof request?.wasm_module_hex === "string" && request.wasm_module_hex.trim().length > 0) {
    spec = {
      kind: "wasm",
      submission: buildWasmSubmission({
        moduleBytesHex: request.wasm_module_hex,
        input: request.input,
        contractVersion:
          typeof request?.contract_version === "string" && request.contract_version.trim().length > 0
            ? request.contract_version.trim()
            : WASM_RUN_JSON_ABI_V1,
      }),
    }
  } else if (
    request?.runtime === "wasm" &&
    request?.package_kind === "oci_image" &&
    typeof request?.oci_reference === "string" &&
    typeof request?.oci_digest === "string"
  ) {
    spec = {
      kind: "oci_wasm",
      submission: buildOciWasmSubmission({
        ociReference: request.oci_reference,
        ociDigest: request.oci_digest,
        input: request.input,
        contractVersion:
          typeof request?.contract_version === "string" && request.contract_version.trim().length > 0
            ? request.contract_version.trim()
            : WASM_RUN_JSON_ABI_V1,
      }),
    }
  } else {
    spec = {
      kind: "execution",
      execution: buildExecutionWorkload(request),
    }
  }
  const offerId = spec.kind === "execution" ? "execute.compute.generic" : "execute.compute"
  const response = await frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/runtime/deals",
    {
      jsonBody: {
        provider: {
          ...(provider.providerId ? { provider_id: provider.providerId } : {}),
          provider_url: provider.providerUrl,
        },
        offer_id: offerId,
        ...spec,
      },
      expectedStatuses: [200, 201],
    }
  )
  return normalizeRuntimeDealCreation(response)
}

/**
 * Get a task from runtime requester deals first, then fall back to provider jobs
 * only when provider and runtime share the same API surface.
 *
 * @param {{ providerUrl: string, providerAuthTokenPath: string, runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, taskId: string }} config
 */
export async function getTask({
  providerUrl,
  providerAuthTokenPath,
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  taskId,
}) {
  const runtimeResponse = await frogletRequestWithStatus(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/runtime/deals/${encodeURIComponent(taskId)}`,
    { expectedStatuses: [200, 404] }
  )
  if (runtimeResponse.status === 200) {
    return normalizeRuntimeTaskResponse(runtimeResponse.payload)
  }
  if (!sameApiBaseUrl(providerUrl, runtimeUrl)) {
    throw new Error(missingTaskMessage(runtimeResponse.payload))
  }
  try {
    return await frogletRequest(
      providerUrl,
      providerAuthTokenPath,
      requestTimeoutMs,
      "GET",
      `/v1/node/jobs/${encodeURIComponent(taskId)}`
    )
  } catch (error) {
    throw normalizeProviderJobLookupError(error)
  }
}

/**
 * Poll runtime requester deals first, then fall back to provider jobs on shared-surface
 * deployments, until a terminal state or timeout.
 *
 * @param {{ providerUrl: string, providerAuthTokenPath: string, runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, taskId: string, timeoutSecs?: number, pollIntervalSecs?: number }} config
 */
export async function waitTask({
  providerUrl,
  providerAuthTokenPath,
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  taskId,
  timeoutSecs = 30,
  pollIntervalSecs = 1,
}) {
  const deadlineMs = Date.now() + timeoutSecs * 1000
  const intervalMs = Math.max(100, Math.round(pollIntervalSecs * 1000))

  while (true) {
    const response = await getTask({
      providerUrl,
      providerAuthTokenPath,
      runtimeUrl,
      runtimeAuthTokenPath,
      requestTimeoutMs,
      taskId,
    })
    const state = normalizedTaskState(response)

    if (state && (TERMINAL_TASK_STATES.has(state) || TERMINAL_DEAL_STATES.has(state))) {
      return response
    }

    const remainingMs = deadlineMs - Date.now()
    if (remainingMs <= 0) {
      throw new Error(
        `waitTask timed out after ${timeoutSecs}s waiting for task ${taskId} (last state: ${state ?? "unknown"})`
      )
    }

    await new Promise((resolve) => setTimeout(resolve, Math.min(intervalMs, remainingMs)))
  }
}
