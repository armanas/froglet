import { createHash } from "node:crypto"
import { readFile, stat } from "node:fs/promises"

import {
  DEFAULT_MAX_JSON_RESPONSE_BYTES,
  pinnedJsonRequest,
  validateProviderUrl
} from "./url-safety.js"

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
const DEFAULT_PROVIDER_DOMAIN_SUFFIX = "providers.froglet.dev"
const TERMINAL_DEAL_STATES = new Set(["succeeded", "failed", "rejected", "cancelled", "completed", "done", "error"])
const TERMINAL_TASK_STATES = new Set(["succeeded", "failed", "rejected", "cancelled", "completed", "done", "error"])

/** @type {Map<string, { token: string, mtimeMs: number }>} */
const tokenCache = new Map()

/**
 * Per-process cache of DNS-pinned address + family for operator-configured
 * URLs, populated lazily the first time an operator URL is used under
 * FROGLET_EGRESS_MODE=strict. Cache key is the normalized base URL.
 *
 * @type {Map<string, Promise<{ pinnedAddress: string, family: number } | null>>}
 */
const operatorPinCache = new Map()

/**
 * True when the operator has opted into applying the SSRF/rebind-resistant
 * pinned dispatcher to operator-configured URLs (runtimeUrl, providerUrl) in
 * addition to LLM-controlled URLs. Re-read per call so tests can flip it
 * without a module reload.
 */
export function isStrictEgressMode() {
  return process.env.FROGLET_EGRESS_MODE === "strict"
}

/**
 * Reset the operator-URL pin cache. Tests should call this between cases so
 * a previous strict-mode invocation does not leak a pin into a lenient one.
 */
export function __resetOperatorPinCacheForTests() {
  operatorPinCache.clear()
}

/**
 * Resolve a pin for an operator-configured base URL. Returns `null` when
 * strict mode is off, when the URL is missing, or when the URL does not
 * pass the same SSRF validator the LLM-controlled path uses.
 *
 * Validation is cached per normalized base URL for the lifetime of the
 * process, so a caller-facing `frogletRequest` does not re-resolve DNS on
 * every call. That's the point: the pin is the snapshot of the address at
 * first-use, and subsequent calls reuse it even if DNS is later rebound.
 */
export async function resolveOperatorPin(baseUrl, label = "operator_url") {
  if (!isStrictEgressMode()) {
    return null
  }
  if (typeof baseUrl !== "string" || baseUrl.trim().length === 0) {
    return null
  }
  const key = baseUrl.trim().replace(/\/$/, "")
  const cached = operatorPinCache.get(key)
  if (cached) {
    return cached
  }
  const resolve = (async () => {
    const validated = await validateProviderUrl(baseUrl, label)
    return {
      pinnedAddress: validated.pinnedAddress,
      family: validated.family,
    }
  })()
  operatorPinCache.set(key, resolve)
  try {
    return await resolve
  } catch (error) {
    // Don't cache failures — the operator may correct their config and
    // re-invoke. A persistent bad pin here is worse than an occasional
    // re-resolution.
    operatorPinCache.delete(key)
    throw error
  }
}

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

function normalizeProviderDomainClaimProviderId(value) {
  if (typeof value !== "string") {
    throw new Error("provider_id is required for marketplace_domain_claim")
  }
  const providerId = value.trim().toLowerCase()
  if (!/^[0-9a-f]{64}$/.test(providerId)) {
    throw new Error("provider_id must be a 64-character hex public key")
  }
  return providerId
}

function normalizeProviderDomainClaimSlug(value, providerId) {
  if (typeof value !== "string" || value.trim().length === 0) {
    return providerId.slice(0, 16)
  }
  const slug = value.trim().toLowerCase()
  if (!/^[a-z0-9](?:[a-z0-9-]{4,61}[a-z0-9])$/.test(slug)) {
    throw new Error("requested_slug must be 6-63 chars of lowercase letters, digits, or interior hyphens")
  }
  return slug
}

function normalizeProviderDomainSuffix(value) {
  if (typeof value !== "string" || value.trim().length === 0) {
    return DEFAULT_PROVIDER_DOMAIN_SUFFIX
  }
  const suffix = value.trim().replace(/\.$/, "").toLowerCase()
  if (
    suffix.length > 253 ||
    suffix.split(".").length < 2 ||
    suffix.split(".").some((label) => !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label))
  ) {
    throw new Error("provider_domain_suffix must be a valid DNS suffix")
  }
  return suffix
}

function providerDomainClaimIntent({ providerId, requestedSlug, publicIp, providerDomainSuffix }) {
  const normalizedProviderId = normalizeProviderDomainClaimProviderId(providerId)
  const slug = normalizeProviderDomainClaimSlug(requestedSlug, normalizedProviderId)
  const suffix = normalizeProviderDomainSuffix(providerDomainSuffix)
  const normalizedPublicIp =
    typeof publicIp === "string" && publicIp.trim().length > 0
      ? publicIp.trim()
      : null
  if (!normalizedPublicIp) {
    throw new Error("public_ip is required for marketplace_domain_claim")
  }
  const hostname = `${slug}.${suffix}`
  return {
    providerId: normalizedProviderId,
    slug,
    hostname,
    publicIp: normalizedPublicIp,
    signingMessage:
      `froglet-provider-domain-claim-intent-v1\n` +
      `provider_id:${normalizedProviderId}\n` +
      `slug:${slug}\n` +
      `hostname:${hostname}\n` +
      `public_ip:${normalizedPublicIp}`,
  }
}

export async function validateMarketplaceUrl(value, label = "marketplace_url", opts = {}) {
  try {
    return await validateProviderUrl(value, label, opts)
  } catch (error) {
    throw new Error(`${label} is not a safe public HTTPS URL: ${error.message}`)
  }
}

async function marketplaceJsonRequest(validatedBaseUrl, path, requestOptions, deps = {}) {
  const requestFn = deps.marketplaceJsonRequest ?? jsonRequest
  return requestFn(`${validatedBaseUrl.normalizedUrl}${path}`, {
    ...requestOptions,
    pin: validatedBaseUrl,
  })
}

function providerUrlLookupDeps(deps) {
  return deps?.providerUrl ? { _deps: deps.providerUrl } : {}
}

function pinFromValidatedUrl(validated) {
  return {
    pinnedAddress: validated.pinnedAddress,
    family: validated.family,
  }
}

async function validateRemoteProviderUrl(value, label, deps = {}) {
  const validated = await validateProviderUrl(value, label, providerUrlLookupDeps(deps))
  return {
    providerUrl: validated.normalizedUrl,
    pin: pinFromValidatedUrl(validated),
  }
}

async function readJsonResponseTextBounded(response, url, maxBytes = DEFAULT_MAX_JSON_RESPONSE_BYTES) {
  const contentLength = Number.parseInt(String(response.headers?.get?.("content-length") ?? ""), 10)
  if (Number.isFinite(contentLength) && contentLength > maxBytes) {
    throw new Error(`Response from ${url} exceeded maximum JSON body size of ${maxBytes} bytes`)
  }
  if (!response.body) {
    return ""
  }
  const reader = response.body.getReader()
  const chunks = []
  let totalBytes = 0
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) {
        break
      }
      const chunk = Buffer.from(value)
      totalBytes += chunk.length
      if (totalBytes > maxBytes) {
        await reader.cancel().catch(() => {})
        throw new Error(`Response from ${url} exceeded maximum JSON body size of ${maxBytes} bytes`)
      }
      chunks.push(chunk)
    }
  } finally {
    reader.releaseLock()
  }
  return Buffer.concat(chunks, totalBytes).toString("utf8")
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
  if (typeof tokenPath !== "string" || tokenPath.trim().length === 0) {
    throw new Error(
      "froglet auth token path is not configured; set FROGLET_PROVIDER_AUTH_TOKEN_PATH/FROGLET_RUNTIME_AUTH_TOKEN_PATH for local actions. Use https://froglet.dev/llms.txt for the no-install hosted demo."
    )
  }
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
    pin,
    maxResponseBytes = DEFAULT_MAX_JSON_RESPONSE_BYTES,
  } = {}
) {
  // `pin` is produced by `validateProviderUrl`. When present, route through a
  // DNS-pinned https.request so rebinding between validation and fetch cannot
  // redirect the connection to a local or metadata address.
  if (pin) {
    return pinnedJsonRequest(url, {
      method,
      timeoutMs,
      headers,
      jsonBody,
      expectedStatuses,
      pinnedAddress: pin.pinnedAddress,
      family: pin.family,
      maxResponseBytes,
    })
  }
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
    const body = await readJsonResponseTextBounded(response, url, maxResponseBytes)
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
async function frogletRequest(baseUrl, tokenPath, timeoutMs, method, path, { jsonBody, expectedStatuses, pin } = {}) {
  const token = await readAuthToken(tokenPath)
  // When the caller supplies a pin (LLM-controlled URL path), honor it. When
  // they don't, strict-egress mode opportunistically resolves one for the
  // operator-configured base URL; lenient mode leaves pin null and uses stock
  // fetch, preserving prior behavior.
  const effectivePin = pin ?? (await resolveOperatorPin(baseUrl))
  const { payload } = await jsonRequest(`${baseUrl}${path}`, {
    method,
    timeoutMs,
    headers: {
      Authorization: `Bearer ${token}`,
    },
    jsonBody,
    expectedStatuses,
    pin: effectivePin,
  })
  return payload
}

async function frogletRequestWithStatus(
  baseUrl,
  tokenPath,
  timeoutMs,
  method,
  path,
  { jsonBody, expectedStatuses, pin } = {}
) {
  const token = await readAuthToken(tokenPath)
  const effectivePin = pin ?? (await resolveOperatorPin(baseUrl))
  return jsonRequest(`${baseUrl}${path}`, {
    method,
    timeoutMs,
    headers: {
      Authorization: `Bearer ${token}`,
    },
    jsonBody,
    expectedStatuses,
    pin: effectivePin,
  })
}

async function frogletPublicRequest(baseUrl, timeoutMs, path, { expectedStatuses, pin } = {}) {
  const effectivePin = pin ?? (await resolveOperatorPin(baseUrl))
  const { payload } = await jsonRequest(`${baseUrl}${path}`, {
    method: "GET",
    timeoutMs,
    expectedStatuses,
    pin: effectivePin,
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

function requestedAccessFromService(service, mounts) {
  const capabilities = Array.isArray(service?.capabilities) ? service.capabilities : []
  return [...new Set([...requestedAccessFromMounts(mounts), ...capabilities])]
    .filter((capability) => typeof capability === "string" && capability.trim().length > 0)
    .map((capability) => capability.trim())
    .sort()
}

function requestedAccessFromRequest(request, mounts) {
  const capabilities = Array.isArray(request?.capabilities) ? request.capabilities : []
  return [...new Set([...requestedAccessFromMounts(mounts), ...capabilities])]
    .filter((capability) => typeof capability === "string" && capability.trim().length > 0)
    .map((capability) => capability.trim())
    .sort()
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

function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : null
}

function builtinServiceName(service, normalizedEntrypoint) {
  return nonEmptyString(service?.entrypoint) ?? nonEmptyString(service?.service_id) ?? normalizedEntrypoint
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
    requested_access: requestedAccessFromRequest(request, mounts),
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
  if (packageKind !== "builtin" && !bindingHash) {
    throw new Error(`service ${service.service_id ?? "unknown"} does not expose a binding hash`)
  }
  const builtinName = packageKind === "builtin" ? builtinServiceName(service, entrypoint) : null
  const effectiveEntrypoint = builtinName ?? entrypoint
  const security =
    packageKind === "builtin"
      ? { mode: "standard" }
      : {
          mode: "standard",
          service_id: service.service_id,
        }
  const execution = {
    schema_version: FROGLET_SCHEMA_V1,
    workload_kind: builtinName ?? WORKLOAD_KIND_EXECUTION_V1,
    runtime,
    package_kind: packageKind,
    entrypoint: {
      kind: entrypointKind,
      value: effectiveEntrypoint,
    },
    contract_version:
      packageKind === "builtin" && builtinName && contractVersion === CONTRACT_BUILTIN_EVENTS_QUERY_V1 && builtinName !== "events.query"
        ? `froglet.builtin.${builtinName}.v1`
        : contractVersion,
    input_format: JCS_JSON_FORMAT,
    input_hash: inputHash(normalized),
    requested_access: requestedAccessFromService(service, mounts),
    security,
    mounts,
    input: normalized,
  }
  if (packageKind === "inline_source") {
    execution.source_hash = bindingHash
  } else if (packageKind === "inline_module" || packageKind === "oci_image") {
    execution.module_hash = bindingHash
  } else if (packageKind === "builtin") {
    execution.builtin_name = builtinName
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

async function fetchPublicProviderService({ providerUrl, requestTimeoutMs, serviceId, pin, _deps = {} }) {
  if (pin && typeof _deps.providerJsonRequest === "function") {
    const { payload } = await _deps.providerJsonRequest(
      `${providerUrl}/v1/provider/services/${encodeURIComponent(serviceId)}`,
      {
        method: "GET",
        timeoutMs: requestTimeoutMs,
        expectedStatuses: [200],
        pin,
      }
    )
    return payload
  }
  return frogletPublicRequest(
    providerUrl,
    requestTimeoutMs,
    `/v1/provider/services/${encodeURIComponent(serviceId)}`,
    { pin }
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
  trustedProviderUrl = null,
  preferTrustedProviderUrl = false,
  _deps = {},
}) {
  // The operator-configured `runtimeUrl` goes through the strict
  // `normalizeBaseUrl` helper at config-load time and is trusted thereafter.
  // The `request.provider_url` field, in contrast, is LLM-controlled and must
  // pass the SSRF validator: https-only, no private/loopback/metadata
  // addresses, IP-pinned at the socket layer to prevent DNS rebinding.
  let explicitProviderUrl = null
  let explicitPin = null
  if (typeof request?.provider_url === "string" && request.provider_url.trim().length > 0) {
    const validated = await validateProviderUrl(
      request.provider_url,
      "request.provider_url",
      providerUrlLookupDeps(_deps)
    )
    explicitProviderUrl = validated.normalizedUrl
    explicitPin = pinFromValidatedUrl(validated)
  }
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
      pin: explicitPin,
      matchSource: "provider_url",
    }
  }

  if (explicitProviderId) {
    const hasTrustedProviderUrl =
      typeof trustedProviderUrl === "string" && trustedProviderUrl.trim().length > 0
    if (hasTrustedProviderUrl && preferTrustedProviderUrl) {
      return {
        providerId: explicitProviderId,
        providerUrl: trustedProviderUrl.trim(),
        matchSource: "trusted_provider_url",
      }
    }
    let providerResponse
    try {
      providerResponse = await runtimeProviderDetails({
        runtimeUrl,
        runtimeAuthTokenPath,
        requestTimeoutMs,
        providerId: explicitProviderId,
      })
    } catch (error) {
      if (hasTrustedProviderUrl) {
        return {
          providerId: explicitProviderId,
          providerUrl: trustedProviderUrl.trim(),
          matchSource: "trusted_provider_url_fallback",
        }
      }
      throw error
    }
    const detail = providerResponse?.provider
    if (!detail) {
      if (hasTrustedProviderUrl) {
        return {
          providerId: explicitProviderId,
          providerUrl: trustedProviderUrl.trim(),
          matchSource: "trusted_provider_url_fallback",
        }
      }
      throw new Error(`provider ${explicitProviderId} not found`)
    }
    const validated = await validateRemoteProviderUrl(
      providerUrlFromRuntimeDetail(detail, explicitProviderId),
      `provider ${explicitProviderId} provider_url`,
      _deps
    )
    return {
      providerId: explicitProviderId,
      providerUrl: validated.providerUrl,
      pin: validated.pin,
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
  const validated = await validateRemoteProviderUrl(
    match.provider_url,
    `service ${serviceId} provider_url`,
    _deps
  )
  return {
    providerId: match.provider_id,
    providerUrl: validated.providerUrl,
    pin: validated.pin,
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
  trustedProviderUrl = null,
  _deps = {},
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
    trustedProviderUrl,
    _deps,
  })
  const serviceResponse = await fetchPublicProviderService({
    providerUrl: provider.providerUrl,
    requestTimeoutMs,
    serviceId,
    pin: provider.pin,
    _deps,
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

function clearnetUrlFromCapabilities(capabilities) {
  const nested = capabilities?.transports?.clearnet?.url
  if (typeof nested === "string" && nested.trim().length > 0) {
    return nested.trim()
  }
  const legacy = capabilities?.transports?.clearnet_url
  if (typeof legacy === "string" && legacy.trim().length > 0) {
    return legacy.trim()
  }
  return null
}

function onionUrlFromCapabilities(capabilities) {
  const nested = capabilities?.transports?.tor?.url ?? capabilities?.transports?.tor?.onion_url
  if (typeof nested === "string" && nested.trim().length > 0) {
    return nested.trim()
  }
  const legacy = capabilities?.transports?.tor_onion_url
  if (typeof legacy === "string" && legacy.trim().length > 0) {
    return legacy.trim()
  }
  return null
}

function normalizeOnionProviderUrl(value, label = "provider_url") {
  const normalized = normalizeUrl(value)
  if (!normalized) {
    throw new Error(`${label} must be a non-empty URL`)
  }
  let parsed
  try {
    parsed = new URL(normalized)
  } catch (error) {
    throw new Error(`${label} is not a valid URL: ${error.message}`)
  }
  if (parsed.protocol !== "http:") {
    throw new Error(`${label} must use http:// for Tor onion registration`)
  }
  if (parsed.username !== "" || parsed.password !== "") {
    throw new Error(`${label} must not contain credentials`)
  }
  if (parsed.pathname !== "/" && parsed.pathname !== "") {
    throw new Error(`${label} must be an origin URL without a path`)
  }
  if (parsed.search || parsed.hash) {
    throw new Error(`${label} must not include query or fragment`)
  }
  if (!/^[a-z2-7]{56}\.onion$/.test(parsed.hostname)) {
    throw new Error(`${label} must be a Tor v3 .onion hostname`)
  }
  return parsed.toString().replace(/\/$/, "")
}

/**
 * Register a provider with a marketplace's frictionless registration API.
 *
 * @param {{ marketplaceUrl: string, providerUrl: string, requestTimeoutMs: number, request?: { provider_url?: string, registration_transport?: string } }} config
 */
export async function registerProviderOnMarketplace({
  marketplaceUrl,
  providerUrl,
  requestTimeoutMs,
  request = {},
  _deps = {},
}) {
  let candidateProviderUrl = normalizeUrl(request.provider_url)
  let transport =
    typeof request.registration_transport === "string" && request.registration_transport.trim().length > 0
      ? request.registration_transport.trim().toLowerCase()
      : null
  if (transport && transport !== "clearnet" && transport !== "tor") {
    throw new Error("registration_transport must be clearnet or tor")
  }
  if (!candidateProviderUrl) {
    const capabilities = await frogletPublicRequest(
      providerUrl,
      requestTimeoutMs,
      "/v1/node/capabilities"
    )
    candidateProviderUrl =
      transport === "tor" ? onionUrlFromCapabilities(capabilities) : clearnetUrlFromCapabilities(capabilities)
    if (!candidateProviderUrl) {
      throw new Error(
        transport === "tor"
          ? "provider_url was omitted and the configured provider did not advertise transports.tor.url. Start the provider in FROGLET_NETWORK_MODE=tor or dual, wait for the onion URL, then retry marketplace_register with registration_transport=tor."
          : "provider_url was omitted and the configured provider did not advertise transports.clearnet.url. Set FROGLET_PUBLIC_BASE_URL on the provider, restart it, then retry marketplace_register."
      )
    }
  }

  if (!transport) {
    transport = /\.onion(?::\d+)?$/i.test(new URL(candidateProviderUrl).hostname) ? "tor" : "clearnet"
  }

  let normalizedProviderUrl
  if (transport === "tor") {
    try {
      normalizedProviderUrl = normalizeOnionProviderUrl(candidateProviderUrl, "provider_url")
    } catch (error) {
      throw new Error(
        `provider_url is not marketplace-registerable: ${error.message}. Tor marketplace registration requires a public http://<v3>.onion origin advertised by the provider.`
      )
    }
  } else {
    try {
      normalizedProviderUrl = (await validateProviderUrl(candidateProviderUrl, "provider_url")).normalizedUrl
    } catch (error) {
      throw new Error(
        `provider_url is not marketplace-registerable: ${error.message}. Marketplace registration requires a public https URL; set FROGLET_PUBLIC_BASE_URL to the provider's public HTTPS origin and retry.`
      )
    }
  }

  const marketplace = await validateMarketplaceUrl(marketplaceUrl, "marketplace_url", {
    _deps: _deps.marketplaceUrl,
  })
  const { status, payload } = await marketplaceJsonRequest(
    marketplace,
    "/v1/registrations",
    {
      method: "POST",
      timeoutMs: requestTimeoutMs,
      jsonBody: {
        provider_url: normalizedProviderUrl,
        transport,
      },
      expectedStatuses: [200, 201],
    },
    _deps
  )

  return {
    ...payload,
    http_status: status,
    provider_url: payload?.provider_url ?? normalizedProviderUrl,
  }
}

/**
 * Create a Froglet-managed providers.froglet.dev domain claim.
 *
 * @param {{ marketplaceUrl: string, providerUrl: string, providerAuthTokenPath: string, requestTimeoutMs: number, request: { provider_id: string, requested_slug?: string, public_ip: string, provider_domain_suffix?: string } }} config
 */
export async function createProviderDomainClaim({
  marketplaceUrl,
  providerUrl,
  providerAuthTokenPath,
  requestTimeoutMs,
  request,
  _deps = {},
}) {
  const intent = providerDomainClaimIntent({
    providerId: request?.provider_id,
    requestedSlug: request?.requested_slug,
    publicIp: request?.public_ip,
    providerDomainSuffix: request?.provider_domain_suffix,
  })
  const signResponse = await frogletRequest(
    providerUrl,
    providerAuthTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/provider/domain-claims/sign",
    {
      jsonBody: { signing_message: intent.signingMessage },
      expectedStatuses: [200],
    }
  )
  const marketplace = await validateMarketplaceUrl(marketplaceUrl, "marketplace_url", {
    _deps: _deps.marketplaceUrl,
  })
  const { status, payload } = await marketplaceJsonRequest(
    marketplace,
    "/v1/provider-domains/claims",
    {
      method: "POST",
      timeoutMs: requestTimeoutMs,
      jsonBody: {
        provider_id: intent.providerId,
        requested_slug: intent.slug,
        public_ip: intent.publicIp,
        intent_signature: signResponse.signature,
      },
      expectedStatuses: [200],
    },
    _deps
  )
  return { ...payload, http_status: status }
}

/**
 * Sign and complete a Froglet-managed domain claim.
 *
 * @param {{ marketplaceUrl: string, providerUrl: string, providerAuthTokenPath: string, requestTimeoutMs: number, claimId: string, signingMessage: string }} config
 */
export async function completeProviderDomainClaim({
  marketplaceUrl,
  providerUrl,
  providerAuthTokenPath,
  requestTimeoutMs,
  claimId,
  signingMessage,
  _deps = {},
}) {
  const signResponse = await frogletRequest(
    providerUrl,
    providerAuthTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/provider/domain-claims/sign",
    {
      jsonBody: { signing_message: signingMessage },
      expectedStatuses: [200],
    }
  )
  const marketplace = await validateMarketplaceUrl(marketplaceUrl, "marketplace_url", {
    _deps: _deps.marketplaceUrl,
  })
  const { status, payload } = await marketplaceJsonRequest(
    marketplace,
    `/v1/provider-domains/claims/${encodeURIComponent(claimId)}/complete`,
    {
      method: "POST",
      timeoutMs: requestTimeoutMs,
      jsonBody: {
        signature: signResponse.signature,
      },
      expectedStatuses: [200],
    },
    _deps
  )
  return {
    ...payload,
    http_status: status,
    signed_provider_id: signResponse.provider_id,
  }
}

/**
 * File an MVP marketplace complaint with the operator-run arbiter service.
 *
 * @param {{ arbiterUrl: string, requestTimeoutMs: number, request: object }} config
 */
export async function fileMarketplaceComplaint({
  arbiterUrl,
  requestTimeoutMs,
  request,
  _deps = {},
}) {
  const arbiter = await validateMarketplaceUrl(arbiterUrl, "marketplace_arbiter_url", {
    _deps: _deps.marketplaceUrl,
  })
  const { status, payload } = await marketplaceJsonRequest(
    arbiter,
    "/v1/complaints",
    {
      method: "POST",
      timeoutMs: requestTimeoutMs,
      jsonBody: request,
      expectedStatuses: [201],
    },
    _deps
  )
  return {
    ...payload,
    http_status: status,
  }
}

/**
 * Read an MVP marketplace complaint from the operator-run arbiter service.
 *
 * @param {{ arbiterUrl: string, requestTimeoutMs: number, complaintId: string }} config
 */
export async function getMarketplaceComplaint({
  arbiterUrl,
  requestTimeoutMs,
  complaintId,
  _deps = {},
}) {
  const arbiter = await validateMarketplaceUrl(arbiterUrl, "marketplace_arbiter_url", {
    _deps: _deps.marketplaceUrl,
  })
  const encoded = encodeURIComponent(complaintId)
  const { status, payload } = await marketplaceJsonRequest(
    arbiter,
    `/v1/complaints/${encoded}`,
    {
      method: "GET",
      timeoutMs: requestTimeoutMs,
      expectedStatuses: [200],
    },
    _deps
  )
  return {
    ...payload,
    http_status: status,
  }
}

/**
 * Get a specific remote service by resolving the provider via the runtime API
 * and then fetching the canonical public service record from the provider.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, request: { provider_id?: string, provider_url?: string, service_id?: string }, searchLimit?: number }} config
 */
export async function getService({ runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs, request, searchLimit = 100, _deps = {} }) {
  const resolved = await resolveRemoteService({
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    request,
    searchLimit,
    _deps,
  })
  return {
    service: {
      ...resolved.service,
      provider_url: resolved.providerUrl,
    },
  }
}

/**
 * POST /v1/runtime/deals with requester-spend-policy awareness.
 *
 * The daemon refuses paid deals that violate the node's spend policy with a
 * 402 carrying a stable `code` (`spend_budget_unconfigured`,
 * `spend_cap_exceeded`, `spend_budget_exceeded`). Surface those as actionable
 * errors so the calling agent learns the remediation (env var to set, or the
 * reset endpoint) instead of a generic HTTP failure. Any other unexpected
 * status keeps the original generic error shape.
 *
 * @param {string} runtimeUrl
 * @param {string} runtimeAuthTokenPath
 * @param {number} requestTimeoutMs
 * @param {unknown} jsonBody
 */
async function createRuntimeDeal(runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs, jsonBody) {
  const { status, payload } = await frogletRequestWithStatus(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/runtime/deals",
    {
      jsonBody,
      expectedStatuses: [200, 201, 402],
    }
  )
  if (status === 402) {
    const code = typeof payload?.code === "string" ? payload.code : null
    if (code && code.startsWith("spend_")) {
      const detail = typeof payload?.error === "string" ? payload.error : JSON.stringify(payload)
      const remaining =
        typeof payload?.remaining_msat === "number" ? ` remaining_msat=${payload.remaining_msat}.` : ""
      const error = new Error(`Deal refused by the requester spend policy (${code}): ${detail}.${remaining}`)
      error.code = code
      error.payload = payload
      throw error
    }
    // Non-spend 402s (e.g. missing buyer wallet) keep the legacy error shape.
    throw new Error(`Request to ${runtimeUrl}/v1/runtime/deals failed with 402: ${JSON.stringify(payload)}`)
  }
  return payload
}

/**
 * Invoke a named service by building a canonical service-addressed execution
 * workload and submitting it through the runtime deal flow.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, request: { provider_id?: string, provider_url?: string, service_id?: string, input?: unknown }, searchLimit?: number, trustedProviderUrl?: string | null }} config
 */
export async function invokeService({
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  request,
  searchLimit = 100,
  trustedProviderUrl = null,
  _deps = {},
}) {
  const resolved = await resolveRemoteService({
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    request,
    searchLimit,
    trustedProviderUrl,
    _deps,
  })
  const response = await createRuntimeDeal(runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs, {
    provider: {
      provider_id: resolved.providerId,
      provider_url: resolved.providerUrl,
    },
    offer_id: resolved.service.offer_id,
    kind: "execution",
    execution: buildServiceAddressedExecution(resolved.service, request?.input),
  })
  return normalizeRuntimeDealCreation(response)
}

/**
 * Run open-ended compute through the runtime deal flow.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, request: { provider_id?: string, provider_url?: string, input?: unknown, runtime?: string, package_kind?: string, entrypoint_kind?: string, entrypoint?: string, contract_version?: string, mounts?: unknown, artifact_path?: string, wasm_module_hex?: string, inline_source?: string, oci_reference?: string, oci_digest?: string }, searchLimit?: number, trustedProviderUrl?: string | null }} config
 */
export async function runCompute({
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  request,
  searchLimit = 100,
  trustedProviderUrl = null,
  _deps = {},
}) {
  if (typeof request?.artifact_path === "string" && request.artifact_path.trim().length > 0) {
    throw new Error("run_compute via runtime deals does not support artifact_path; provide inline bytes/source or OCI coordinates")
  }
  const provider = await resolveProviderReference({
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    request,
    searchLimit,
    trustedProviderUrl,
    preferTrustedProviderUrl: true,
    _deps,
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
  const response = await createRuntimeDeal(runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs, {
    provider: {
      ...(provider.providerId ? { provider_id: provider.providerId } : {}),
      provider_url: provider.providerUrl,
    },
    offer_id: offerId,
    ...spec,
  })
  return normalizeRuntimeDealCreation(response)
}

/**
 * Settlement visibility: wallet balance snapshot from the runtime's configured
 * settlement driver. Wraps GET /v1/runtime/wallet/balance.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number }} config
 */
export async function getWalletBalance({ runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs }) {
  return frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "GET",
    "/v1/runtime/wallet/balance"
  )
}

/**
 * Requester spend policy and ledger totals (fail-closed budget headroom).
 * Wraps GET /v1/runtime/spend.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number }} config
 */
export async function getSpendStatus({ runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs }) {
  return frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "GET",
    "/v1/runtime/spend"
  )
}

/**
 * Archive committed spend, restoring cumulative budget headroom. In-flight
 * reservations are left untouched. Wraps POST /v1/runtime/spend/reset.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number }} config
 */
export async function resetSpend({ runtimeUrl, runtimeAuthTokenPath, requestTimeoutMs }) {
  return frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "POST",
    "/v1/runtime/spend/reset"
  )
}

/**
 * List recent requester-side deals with compact settlement state. Wraps GET
 * /v1/runtime/settlement/activity.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, limit?: number }} config
 */
export async function listSettlementActivity({
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  limit,
}) {
  const query = typeof limit === "number" ? `?limit=${encodeURIComponent(limit)}` : ""
  return frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/runtime/settlement/activity${query}`
  )
}

/**
 * Get the payment-intent payload for a specific requester deal. Wraps GET
 * /v1/runtime/deals/:deal_id/payment-intent.
 *
 * @param {{ runtimeUrl: string, runtimeAuthTokenPath: string, requestTimeoutMs: number, dealId: string }} config
 */
export async function getDealPaymentIntent({
  runtimeUrl,
  runtimeAuthTokenPath,
  requestTimeoutMs,
  dealId,
}) {
  return frogletRequest(
    runtimeUrl,
    runtimeAuthTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/runtime/deals/${encodeURIComponent(dealId)}/payment-intent`
  )
}

/**
 * Get the invoice bundle for a specific provider-side deal. Wraps GET
 * /v1/provider/deals/:deal_id/invoice-bundle.
 *
 * @param {{ providerUrl: string, providerAuthTokenPath: string, requestTimeoutMs: number, dealId: string }} config
 */
export async function getDealInvoiceBundle({
  providerUrl,
  providerAuthTokenPath,
  requestTimeoutMs,
  dealId,
}) {
  return frogletRequest(
    providerUrl,
    providerAuthTokenPath,
    requestTimeoutMs,
    "GET",
    `/v1/provider/deals/${encodeURIComponent(dealId)}/invoice-bundle`
  )
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
