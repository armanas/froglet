import {
  ABSOLUTE_MAX_SEARCH_LIMIT,
  DEFAULT_MAX_SEARCH_LIMIT,
  DEFAULT_SEARCH_LIMIT,
  DEFAULT_TIMEOUT_MS,
  MAX_TIMEOUT_MS,
  MIN_SEARCH_LIMIT,
  MIN_TIMEOUT_MS,
  clampInteger,
  normalizeBaseUrl,
  normalizeFilesystemPath
} from "../../../shared/froglet-lib/shared.js"

/**
 * Resolve the provider URL.
 *
 * Priority order:
 *   1. FROGLET_PROVIDER_URL
 *   2. FROGLET_BASE_URL  (legacy fallback — sets both provider and runtime URLs)
 */
function resolveProviderUrl() {
  const explicit = process.env.FROGLET_PROVIDER_URL
  if (typeof explicit === "string" && explicit.trim().length > 0) {
    return normalizeBaseUrl(explicit, "FROGLET_PROVIDER_URL", { allowInsecure: true })
  }
  const fallback = process.env.FROGLET_BASE_URL
  return normalizeBaseUrl(fallback, "FROGLET_BASE_URL / FROGLET_PROVIDER_URL", {
    allowInsecure: true
  })
}

/**
 * Resolve the runtime URL.
 *
 * Priority order:
 *   1. FROGLET_RUNTIME_URL
 *   2. FROGLET_BASE_URL  (legacy fallback — sets both provider and runtime URLs)
 */
function resolveRuntimeUrl() {
  const explicit = process.env.FROGLET_RUNTIME_URL
  if (typeof explicit === "string" && explicit.trim().length > 0) {
    return normalizeBaseUrl(explicit, "FROGLET_RUNTIME_URL", { allowInsecure: true })
  }
  const fallback = process.env.FROGLET_BASE_URL
  return normalizeBaseUrl(fallback, "FROGLET_BASE_URL / FROGLET_RUNTIME_URL", {
    allowInsecure: true
  })
}

/**
 * Resolve the provider auth token path.
 *
 * Priority order:
 *   1. FROGLET_PROVIDER_AUTH_TOKEN_PATH
 *   2. FROGLET_AUTH_TOKEN_PATH  (legacy fallback)
 */
function resolveProviderAuthTokenPath() {
  const explicit = process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH
  if (typeof explicit === "string" && explicit.trim().length > 0) {
    return normalizeFilesystemPath(explicit, "FROGLET_PROVIDER_AUTH_TOKEN_PATH")
  }
  return normalizeFilesystemPath(
    process.env.FROGLET_AUTH_TOKEN_PATH,
    "FROGLET_AUTH_TOKEN_PATH / FROGLET_PROVIDER_AUTH_TOKEN_PATH"
  )
}

/**
 * Resolve the runtime auth token path.
 *
 * Priority order:
 *   1. FROGLET_RUNTIME_AUTH_TOKEN_PATH
 *   2. FROGLET_AUTH_TOKEN_PATH  (legacy fallback)
 */
function resolveRuntimeAuthTokenPath() {
  const explicit = process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH
  if (typeof explicit === "string" && explicit.trim().length > 0) {
    return normalizeFilesystemPath(explicit, "FROGLET_RUNTIME_AUTH_TOKEN_PATH")
  }
  return normalizeFilesystemPath(
    process.env.FROGLET_AUTH_TOKEN_PATH,
    "FROGLET_AUTH_TOKEN_PATH / FROGLET_RUNTIME_AUTH_TOKEN_PATH"
  )
}

export function readConfig() {
  const providerUrl = resolveProviderUrl()
  const runtimeUrl = resolveRuntimeUrl()
  const providerAuthTokenPath = resolveProviderAuthTokenPath()
  const runtimeAuthTokenPath = resolveRuntimeAuthTokenPath()

  const maxSearchLimit = clampInteger(
    process.env.FROGLET_MAX_SEARCH_LIMIT,
    DEFAULT_MAX_SEARCH_LIMIT,
    MIN_SEARCH_LIMIT,
    ABSOLUTE_MAX_SEARCH_LIMIT
  )

  return {
    providerUrl,
    runtimeUrl,
    providerAuthTokenPath,
    runtimeAuthTokenPath,
    requestTimeoutMs: clampInteger(
      process.env.FROGLET_REQUEST_TIMEOUT_MS,
      DEFAULT_TIMEOUT_MS,
      MIN_TIMEOUT_MS,
      MAX_TIMEOUT_MS
    ),
    defaultSearchLimit: clampInteger(
      process.env.FROGLET_DEFAULT_SEARCH_LIMIT,
      DEFAULT_SEARCH_LIMIT,
      MIN_SEARCH_LIMIT,
      maxSearchLimit
    ),
    maxSearchLimit
  }
}
