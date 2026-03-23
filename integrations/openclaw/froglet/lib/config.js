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
} from "./shared.js"

const HOST_PRODUCTS = new Set(["openclaw", "nemoclaw"])

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

export function readPluginConfig(api) {
  const config = api?.config ?? {}
  const hostProduct = normalizeHostProduct(
    resolveConfigValue(config.hostProduct, "FROGLET_HOST_PRODUCT")
  )

  const baseUrl = normalizeBaseUrl(
    resolveConfigValue(config.baseUrl, "FROGLET_BASE_URL"),
    "baseUrl"
  )
  const authTokenPath = normalizeFilesystemPath(
    resolveConfigValue(config.authTokenPath, "FROGLET_AUTH_TOKEN_PATH"),
    "authTokenPath"
  )
  const maxSearchLimit = clampInteger(
    config.maxSearchLimit ?? process.env.FROGLET_MAX_SEARCH_LIMIT,
    DEFAULT_MAX_SEARCH_LIMIT,
    MIN_SEARCH_LIMIT,
    ABSOLUTE_MAX_SEARCH_LIMIT
  )

  return {
    hostProduct,
    baseUrl,
    authTokenPath,
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
