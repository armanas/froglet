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

export function readPluginConfig(api) {
  const config = api?.config ?? {}
  const maxSearchLimit = clampInteger(
    config.maxSearchLimit,
    DEFAULT_MAX_SEARCH_LIMIT,
    MIN_SEARCH_LIMIT,
    ABSOLUTE_MAX_SEARCH_LIMIT
  )

  return {
    runtimeUrl: normalizeBaseUrl(config.runtimeUrl, "runtimeUrl"),
    runtimeAuthTokenPath: normalizeFilesystemPath(
      config.runtimeAuthTokenPath,
      "runtimeAuthTokenPath"
    ),
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

export function resolveRuntimeUrl(config, overrideUrl, options = {}) {
  const fieldName = options.fieldName ?? "runtime_url"
  if (overrideUrl !== undefined) {
    return normalizeBaseUrl(overrideUrl, fieldName)
  }
  return config.runtimeUrl
}

export function resolveRuntimeAuthTokenPath(config, overridePath, options = {}) {
  const fieldName = options.fieldName ?? "runtime_auth_token_path"
  if (overridePath !== undefined) {
    return normalizeFilesystemPath(overridePath, fieldName)
  }
  return config.runtimeAuthTokenPath
}
