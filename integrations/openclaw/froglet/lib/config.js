import {
  ABSOLUTE_MAX_SEARCH_LIMIT,
  DEFAULT_MAX_SEARCH_LIMIT,
  DEFAULT_PYTHON_EXECUTABLE,
  DEFAULT_SEARCH_LIMIT,
  DEFAULT_TIMEOUT_MS,
  MAX_TIMEOUT_MS,
  MIN_SEARCH_LIMIT,
  MIN_TIMEOUT_MS,
  clampInteger,
  normalizeBaseUrl,
  normalizeCommand,
  normalizeFilesystemPath
} from "./shared.js"

function readOptionalBaseUrl(value, fieldName) {
  if (typeof value !== "string" || value.trim().length === 0) {
    return null
  }
  return normalizeBaseUrl(value, fieldName)
}

function readOptionalPath(value, fieldName) {
  if (typeof value !== "string" || value.trim().length === 0) {
    return null
  }
  return normalizeFilesystemPath(value, fieldName)
}

export function readPluginConfig(api) {
  const config = api?.config ?? {}
  const maxSearchLimit = clampInteger(
    config.maxSearchLimit,
    DEFAULT_MAX_SEARCH_LIMIT,
    MIN_SEARCH_LIMIT,
    ABSOLUTE_MAX_SEARCH_LIMIT
  )

  return {
    marketplaceUrl: readOptionalBaseUrl(config.marketplaceUrl, "marketplaceUrl"),
    providerUrl: readOptionalBaseUrl(config.providerUrl, "providerUrl"),
    runtimeUrl: readOptionalBaseUrl(config.runtimeUrl, "runtimeUrl"),
    runtimeAuthTokenPath: readOptionalPath(
      config.runtimeAuthTokenPath,
      "runtimeAuthTokenPath"
    ),
    pythonExecutable:
      typeof config.pythonExecutable === "string" && config.pythonExecutable.trim().length > 0
        ? normalizeCommand(config.pythonExecutable, "pythonExecutable")
        : DEFAULT_PYTHON_EXECUTABLE,
    enablePrivilegedRuntimeTools: config.enablePrivilegedRuntimeTools === true,
    requestTimeoutMs: clampInteger(
      config.requestTimeoutMs,
      DEFAULT_TIMEOUT_MS,
      MIN_TIMEOUT_MS,
      MAX_TIMEOUT_MS
    ),
    defaultSearchLimit: clampInteger(
      config.defaultSearchLimit,
      DEFAULT_SEARCH_LIMIT,
      MIN_SEARCH_LIMIT,
      maxSearchLimit
    ),
    maxSearchLimit
  }
}

export function resolveMarketplaceUrl(config, overrideUrl) {
  if (overrideUrl !== undefined) {
    return normalizeBaseUrl(overrideUrl, "marketplace_url")
  }
  if (config.marketplaceUrl === null) {
    throw new Error(
      "marketplace_url is required when plugin config.marketplaceUrl is not set"
    )
  }
  return config.marketplaceUrl
}

export function resolveProviderUrl(config, overrideUrl, options = {}) {
  const fieldName = options.fieldName ?? "provider_url"
  const required = options.required ?? true
  if (overrideUrl !== undefined) {
    return normalizeBaseUrl(overrideUrl, fieldName)
  }
  if (config.providerUrl !== null) {
    return config.providerUrl
  }
  if (required) {
    throw new Error(`${fieldName} is required when plugin config.providerUrl is not set`)
  }
  return null
}

export function resolveRuntimeUrl(config, overrideUrl, options = {}) {
  const fieldName = options.fieldName ?? "runtime_url"
  const required = options.required ?? true
  if (overrideUrl !== undefined) {
    return normalizeBaseUrl(overrideUrl, fieldName)
  }
  if (config.runtimeUrl !== null) {
    return config.runtimeUrl
  }
  if (required) {
    throw new Error(`${fieldName} is required when plugin config.runtimeUrl is not set`)
  }
  return null
}

export function resolveRuntimeAuthTokenPath(config, overridePath, options = {}) {
  const fieldName = options.fieldName ?? "runtime_auth_token_path"
  const required = options.required ?? true
  if (overridePath !== undefined) {
    return normalizeFilesystemPath(overridePath, fieldName)
  }
  if (config.runtimeAuthTokenPath !== null) {
    return config.runtimeAuthTokenPath
  }
  if (required) {
    throw new Error(
      `${fieldName} is required when plugin config.runtimeAuthTokenPath is not set`
    )
  }
  return null
}
