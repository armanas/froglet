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
} from "../../../shared/froglet-lib/shared.js"

const LOCAL_PROFILE = "local"
const DEFAULT_LOCAL_PROVIDER_URL = "http://127.0.0.1:8080"
const DEFAULT_LOCAL_RUNTIME_URL = "http://127.0.0.1:8081"
const SUPPORTED_PROFILES = new Set([LOCAL_PROFILE])

function nonEmptyEnv(name) {
  const value = process.env[name]
  return typeof value === "string" && value.trim().length > 0 ? value : null
}

function resolveProfile() {
  const profile = nonEmptyEnv("FROGLET_PROFILE") ?? LOCAL_PROFILE
  if (!SUPPORTED_PROFILES.has(profile)) {
    throw new Error(
      `FROGLET_PROFILE must be one of: ${[...SUPPORTED_PROFILES].join(", ")}`
    )
  }
  return profile
}

/**
 * Resolve the provider URL.
 *
 * Priority order:
 *   1. FROGLET_PROVIDER_URL
 *   2. FROGLET_BASE_URL  (legacy fallback — sets both provider and runtime URLs)
 *   3. local loopback default
 */
function resolveProviderUrl() {
  const explicit = nonEmptyEnv("FROGLET_PROVIDER_URL")
  if (explicit) {
    return normalizeBaseUrl(explicit, "FROGLET_PROVIDER_URL", { allowInsecure: true })
  }
  const fallback = nonEmptyEnv("FROGLET_BASE_URL")
  if (fallback) {
    return normalizeBaseUrl(fallback, "FROGLET_BASE_URL / FROGLET_PROVIDER_URL", {
      allowInsecure: true
    })
  }
  return normalizeBaseUrl(DEFAULT_LOCAL_PROVIDER_URL, "FROGLET_PROVIDER_URL default", {
    allowInsecure: true
  })
}

/**
 * Resolve the runtime URL.
 *
 * Priority order:
 *   1. FROGLET_RUNTIME_URL
 *   2. FROGLET_BASE_URL  (legacy fallback — sets both provider and runtime URLs)
 *   3. local loopback default
 */
function resolveRuntimeUrl() {
  const explicit = nonEmptyEnv("FROGLET_RUNTIME_URL")
  if (explicit) {
    return normalizeBaseUrl(explicit, "FROGLET_RUNTIME_URL", { allowInsecure: true })
  }
  const fallback = nonEmptyEnv("FROGLET_BASE_URL")
  if (fallback) {
    return normalizeBaseUrl(fallback, "FROGLET_BASE_URL / FROGLET_RUNTIME_URL", {
      allowInsecure: true
    })
  }
  return normalizeBaseUrl(DEFAULT_LOCAL_RUNTIME_URL, "FROGLET_RUNTIME_URL default", {
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
  const explicit = nonEmptyEnv("FROGLET_PROVIDER_AUTH_TOKEN_PATH")
  if (explicit) {
    return normalizeFilesystemPath(explicit, "FROGLET_PROVIDER_AUTH_TOKEN_PATH")
  }
  const fallback = nonEmptyEnv("FROGLET_AUTH_TOKEN_PATH")
  return fallback
    ? normalizeFilesystemPath(
        fallback,
        "FROGLET_AUTH_TOKEN_PATH / FROGLET_PROVIDER_AUTH_TOKEN_PATH"
      )
    : null
}

/**
 * Resolve the runtime auth token path.
 *
 * Priority order:
 *   1. FROGLET_RUNTIME_AUTH_TOKEN_PATH
 *   2. FROGLET_AUTH_TOKEN_PATH  (legacy fallback)
 */
function resolveRuntimeAuthTokenPath() {
  const explicit = nonEmptyEnv("FROGLET_RUNTIME_AUTH_TOKEN_PATH")
  if (explicit) {
    return normalizeFilesystemPath(explicit, "FROGLET_RUNTIME_AUTH_TOKEN_PATH")
  }
  const fallback = nonEmptyEnv("FROGLET_AUTH_TOKEN_PATH")
  return fallback
    ? normalizeFilesystemPath(
        fallback,
        "FROGLET_AUTH_TOKEN_PATH / FROGLET_RUNTIME_AUTH_TOKEN_PATH"
      )
    : null
}

function resolveMarketplaceUrl() {
  const explicit = nonEmptyEnv("FROGLET_MARKETPLACE_URL")
  return normalizeBaseUrl(
    explicit ?? DEFAULT_MARKETPLACE_URL,
    explicit ? "FROGLET_MARKETPLACE_URL" : "FROGLET_MARKETPLACE_URL default"
  )
}

function resolveMarketplaceArbiterUrl() {
  const explicit = nonEmptyEnv("FROGLET_MARKETPLACE_ARBITER_URL")
  return normalizeBaseUrl(
    explicit ?? DEFAULT_MARKETPLACE_ARBITER_URL,
    explicit
      ? "FROGLET_MARKETPLACE_ARBITER_URL"
      : "FROGLET_MARKETPLACE_ARBITER_URL default"
  )
}

export function readConfig() {
  const profile = resolveProfile()
  const providerUrl = resolveProviderUrl()
  const runtimeUrl = resolveRuntimeUrl()
  const providerAuthTokenPath = resolveProviderAuthTokenPath()
  const runtimeAuthTokenPath = resolveRuntimeAuthTokenPath()
  const marketplaceUrl = resolveMarketplaceUrl()
  const marketplaceArbiterUrl = resolveMarketplaceArbiterUrl()

  const maxSearchLimit = clampInteger(
    process.env.FROGLET_MAX_SEARCH_LIMIT,
    DEFAULT_MAX_SEARCH_LIMIT,
    MIN_SEARCH_LIMIT,
    ABSOLUTE_MAX_SEARCH_LIMIT
  )

  return {
    profile,
    providerUrl,
    runtimeUrl,
    providerAuthTokenPath,
    runtimeAuthTokenPath,
    marketplaceUrl,
    marketplaceArbiterUrl,
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
