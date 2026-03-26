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

export function readConfig() {
  const baseUrl = normalizeBaseUrl(
    process.env.FROGLET_BASE_URL,
    "FROGLET_BASE_URL"
  )
  const authTokenPath = normalizeFilesystemPath(
    process.env.FROGLET_AUTH_TOKEN_PATH,
    "FROGLET_AUTH_TOKEN_PATH"
  )
  const maxSearchLimit = clampInteger(
    process.env.FROGLET_MAX_SEARCH_LIMIT,
    DEFAULT_MAX_SEARCH_LIMIT,
    MIN_SEARCH_LIMIT,
    ABSOLUTE_MAX_SEARCH_LIMIT
  )

  return {
    baseUrl,
    authTokenPath,
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
