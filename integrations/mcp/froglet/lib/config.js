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

const HOSTED_TRIAL_URL = "https://try.froglet.dev"
const HOSTED_PROOF_PROFILE = "hosted-proof"
const LOCAL_PROFILE = "local"
const SUPPORTED_PROFILES = new Set([HOSTED_PROOF_PROFILE, LOCAL_PROFILE])

function nonEmptyEnv(name) {
  const value = process.env[name]
  return typeof value === "string" && value.trim().length > 0 ? value : null
}

function resolveProfile() {
  const profile = nonEmptyEnv("FROGLET_PROFILE") ?? HOSTED_PROOF_PROFILE
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
 *   3. hosted proof profile default
 */
function resolveProviderUrl(profile) {
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
  if (profile === HOSTED_PROOF_PROFILE) {
    return HOSTED_TRIAL_URL
  }
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
 *   3. hosted proof profile default
 */
function resolveRuntimeUrl(profile) {
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
  if (profile === HOSTED_PROOF_PROFILE) {
    return HOSTED_TRIAL_URL
  }
  return normalizeBaseUrl(fallback, "FROGLET_BASE_URL / FROGLET_RUNTIME_URL", {
    allowInsecure: true
  })
}

function resolveHostedTrialUrl() {
  return normalizeBaseUrl(nonEmptyEnv("FROGLET_HOSTED_TRIAL_URL") ?? HOSTED_TRIAL_URL, "FROGLET_HOSTED_TRIAL_URL", {
    allowInsecure: false
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

export function readConfig() {
  const profile = resolveProfile()
  const providerUrl = resolveProviderUrl(profile)
  const runtimeUrl = resolveRuntimeUrl(profile)
  const providerAuthTokenPath = resolveProviderAuthTokenPath()
  const runtimeAuthTokenPath = resolveRuntimeAuthTokenPath()

  const maxSearchLimit = clampInteger(
    process.env.FROGLET_MAX_SEARCH_LIMIT,
    DEFAULT_MAX_SEARCH_LIMIT,
    MIN_SEARCH_LIMIT,
    ABSOLUTE_MAX_SEARCH_LIMIT
  )

  return {
    profile,
    hostedTrialUrl: resolveHostedTrialUrl(),
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
