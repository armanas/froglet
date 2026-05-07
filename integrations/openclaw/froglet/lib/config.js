import {
  ABSOLUTE_MAX_SEARCH_LIMIT,
  DEFAULT_MARKETPLACE_ARBITER_URL,
  DEFAULT_MARKETPLACE_URL,
  DEFAULT_MAX_SEARCH_LIMIT,
  DEFAULT_SEARCH_LIMIT,
  DEFAULT_TIMEOUT_MS,
  MAX_TIMEOUT_MS,
  MIN_SEARCH_LIMIT,
  MIN_TIMEOUT_MS,
  clampInteger,
  normalizeBaseUrl,
  normalizeFilesystemPath
} from "./shared.js"

const HOST_PRODUCTS = new Set(["openclaw", "nemoclaw"])
const DEFAULT_LOCAL_PROVIDER_URL = "http://127.0.0.1:8080"
const DEFAULT_LOCAL_RUNTIME_URL = "http://127.0.0.1:8081"

function resolveConfigValue(configValue, envName) {
  if (typeof configValue === "string" && configValue.trim().length > 0) {
    return configValue
  }
  const envValue = process.env[envName]
  if (typeof envValue === "string" && envValue.trim().length > 0) {
    return envValue
  }
  return configValue
}

function normalizeHostProduct(value) {
  const normalized =
    typeof value === "string" && value.trim().length > 0
      ? value.trim().toLowerCase()
      : "openclaw"
  if (!HOST_PRODUCTS.has(normalized)) {
    throw new Error(`hostProduct must be one of ${[...HOST_PRODUCTS].join(", ")}`)
  }
  return normalized
}

function normalizeOptionalFilesystemPath(value, fieldName) {
  if (typeof value !== "string" || value.trim().length === 0) {
    return null
  }
  return normalizeFilesystemPath(value, fieldName)
}

/**
 * Resolve the provider URL.
 *
 * Priority order:
 *   1. config.providerUrl / FROGLET_PROVIDER_URL
 *   2. config.baseUrl / FROGLET_BASE_URL  (legacy fallback — sets both URLs)
 *   3. local loopback default
 */
function resolveProviderUrl(config) {
  const explicit = resolveConfigValue(config.providerUrl, "FROGLET_PROVIDER_URL")
  if (typeof explicit === "string" && explicit.trim().length > 0) {
    return normalizeBaseUrl(explicit, "providerUrl")
  }
  const fallback = resolveConfigValue(config.baseUrl, "FROGLET_BASE_URL")
  if (typeof fallback === "string" && fallback.trim().length > 0) {
    return normalizeBaseUrl(fallback, "FROGLET_BASE_URL / providerUrl")
  }
  return normalizeBaseUrl(DEFAULT_LOCAL_PROVIDER_URL, "providerUrl default")
}

/**
 * Resolve the runtime URL.
 *
 * Priority order:
 *   1. config.runtimeUrl / FROGLET_RUNTIME_URL
 *   2. config.baseUrl / FROGLET_BASE_URL  (legacy fallback — sets both URLs)
 *   3. local loopback default
 */
function resolveRuntimeUrl(config) {
  const explicit = resolveConfigValue(config.runtimeUrl, "FROGLET_RUNTIME_URL")
  if (typeof explicit === "string" && explicit.trim().length > 0) {
    return normalizeBaseUrl(explicit, "runtimeUrl")
  }
  const fallback = resolveConfigValue(config.baseUrl, "FROGLET_BASE_URL")
  if (typeof fallback === "string" && fallback.trim().length > 0) {
    return normalizeBaseUrl(fallback, "FROGLET_BASE_URL / runtimeUrl")
  }
  return normalizeBaseUrl(DEFAULT_LOCAL_RUNTIME_URL, "runtimeUrl default")
}

function resolveMarketplaceUrl(config) {
  const explicit = resolveConfigValue(config.marketplaceUrl, "FROGLET_MARKETPLACE_URL")
  return normalizeBaseUrl(
    typeof explicit === "string" && explicit.trim().length > 0
      ? explicit
      : DEFAULT_MARKETPLACE_URL,
    "marketplaceUrl / FROGLET_MARKETPLACE_URL"
  )
}

function resolveMarketplaceArbiterUrl(config) {
  const explicit = resolveConfigValue(
    config.marketplaceArbiterUrl,
    "FROGLET_MARKETPLACE_ARBITER_URL"
  )
  return normalizeBaseUrl(
    typeof explicit === "string" && explicit.trim().length > 0
      ? explicit
      : DEFAULT_MARKETPLACE_ARBITER_URL,
    "marketplaceArbiterUrl / FROGLET_MARKETPLACE_ARBITER_URL"
  )
}

export function readPluginConfig(api) {
  const config = api?.config ?? {}
  const hostProduct = normalizeHostProduct(
    resolveConfigValue(config.hostProduct, "FROGLET_HOST_PRODUCT")
  )

  const providerUrl = resolveProviderUrl(config)
  const runtimeUrl = resolveRuntimeUrl(config)
  const marketplaceUrl = resolveMarketplaceUrl(config)
  const marketplaceArbiterUrl = resolveMarketplaceArbiterUrl(config)

  const providerAuthTokenPath = normalizeOptionalFilesystemPath(
    resolveConfigValue(config.providerAuthTokenPath ?? config.authTokenPath, "FROGLET_PROVIDER_AUTH_TOKEN_PATH") ??
      process.env.FROGLET_AUTH_TOKEN_PATH,
    "providerAuthTokenPath / FROGLET_PROVIDER_AUTH_TOKEN_PATH"
  )

  const runtimeAuthTokenPath = normalizeOptionalFilesystemPath(
    resolveConfigValue(config.runtimeAuthTokenPath ?? config.authTokenPath, "FROGLET_RUNTIME_AUTH_TOKEN_PATH") ??
      process.env.FROGLET_AUTH_TOKEN_PATH,
    "runtimeAuthTokenPath / FROGLET_RUNTIME_AUTH_TOKEN_PATH"
  )

  const maxSearchLimit = clampInteger(
    config.maxSearchLimit ?? process.env.FROGLET_MAX_SEARCH_LIMIT,
    DEFAULT_MAX_SEARCH_LIMIT,
    MIN_SEARCH_LIMIT,
    ABSOLUTE_MAX_SEARCH_LIMIT
  )

  return {
    hostProduct,
    providerUrl,
    runtimeUrl,
    marketplaceUrl,
    marketplaceArbiterUrl,
    providerAuthTokenPath,
    runtimeAuthTokenPath,
    requestTimeoutMs: clampInteger(
      config.requestTimeoutMs ?? process.env.FROGLET_REQUEST_TIMEOUT_MS,
      DEFAULT_TIMEOUT_MS,
      MIN_TIMEOUT_MS,
      MAX_TIMEOUT_MS
    ),
    defaultSearchLimit: clampInteger(
      config.defaultSearchLimit ?? process.env.FROGLET_DEFAULT_SEARCH_LIMIT,
      DEFAULT_SEARCH_LIMIT,
      MIN_SEARCH_LIMIT,
      maxSearchLimit
    ),
    maxSearchLimit
  }
}
