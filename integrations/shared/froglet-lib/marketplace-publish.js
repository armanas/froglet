// marketplace_publish MCP action.
//
// Phase 3 of the agent-grade publish plan. Wraps the `froglet-node publish`
// CLI subcommand (Phase 2) so an LLM can take a service from a one-sentence
// user intent to a live marketplace offer in one MCP call.
//
// Architectural rule: this handler delegates to the SAME `froglet-node publish`
// surface a human would type, by writing manifests to a temp directory and
// shelling out. That way the MCP path can never diverge from the CLI path —
// one bug, one fix. The shelling-out is intentional, not a shortcut.

import { execFile } from "node:child_process"
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, isAbsolute, join, resolve, sep } from "node:path"
import { promisify } from "node:util"

import { validateProviderUrl } from "./url-safety.js"

const execFileAsync = promisify(execFile)

const PUBLISH_TIMEOUT_MS = 5 * 60 * 1000 // 5 min covers indexer-wait worst case
const VALID_NAME = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/
const VALID_RUNTIMES = new Set(["python"])
const VALID_PACKAGE_KINDS = new Set(["inline_source"])
const VALID_HOSTING = new Set(["local", "relay", "tor", "self"])
const VALID_SETTLEMENT = new Set(["none", "lightning", "stripe"])
const VALID_ENTRYPOINT = /^[A-Za-z0-9._/-]+$/
const VALID_MOUNT_HANDLE = /^[a-z0-9_]{1,64}$/
const VALID_MOUNT_KIND = new Set(["postgres", "sqlite", "object_store", "s3", "redis"])
const VALID_MODE = new Set(["sync", "async"])
const VALID_PUBLICATION_STATE = new Set(["active", "hidden"])
const LIMIT_FIELDS = [
  "max_input_bytes",
  "max_runtime_ms",
  "max_memory_bytes",
  "max_output_bytes",
  "fuel_limit"
]

function tomlString(value) {
  return JSON.stringify(String(value))
}

function tomlKey(value) {
  return /^[A-Za-z0-9_-]+$/.test(value) ? value : tomlString(value)
}

function tomlInlineValue(value, field) {
  if (typeof value === "string") return tomlString(value)
  if (typeof value === "boolean") return String(value)
  if (typeof value === "number" && Number.isFinite(value)) return String(value)
  if (Array.isArray(value)) {
    return `[${value.map((item, index) => tomlInlineValue(item, `${field}[${index}]`)).join(", ")}]`
  }
  if (value !== null && typeof value === "object") {
    return `{ ${Object.entries(value)
      .map(([key, item]) => `${tomlKey(key)} = ${tomlInlineValue(item, `${field}.${key}`)}`)
      .join(", ")} }`
  }
  throw new Error(
    `marketplace_publish: ${field} contains a value that cannot be represented in TOML`
  )
}

function tomlJsonString(value, field) {
  let encoded
  try {
    encoded = JSON.stringify(value)
  } catch (error) {
    throw new Error(`marketplace_publish: ${field} is not JSON-serializable: ${error.message}`)
  }
  if (encoded === undefined) {
    throw new Error(`marketplace_publish: ${field} is not JSON-serializable`)
  }
  return tomlString(encoded)
}

function normalizeStringArray(value, field) {
  if (value === undefined) return []
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string" || item.trim() === "")) {
    throw new Error(`marketplace_publish: ${field} must be an array of non-empty strings`)
  }
  return [...new Set(value.map((item) => item.trim()))]
}

function normalizeCapabilities(value) {
  return [...new Set(
    normalizeStringArray(value, "capabilities")
      .map((capability) => capability.toLowerCase())
      .map((capability) => capability.startsWith("mount.s3.")
        ? `mount.object_store.${capability.slice("mount.s3.".length)}`
        : capability)
  )].sort()
}

function normalizeMounts(value) {
  if (value === undefined) return []
  if (!Array.isArray(value)) {
    throw new Error("marketplace_publish: mounts must be an array")
  }
  return value.map((mount, index) => {
    if (mount === null || typeof mount !== "object" || Array.isArray(mount)) {
      throw new Error(`marketplace_publish: mounts[${index}] must be an object`)
    }
    const unsupported = Object.keys(mount).filter(
      (key) => !["handle", "kind", "read_only"].includes(key)
    )
    if (unsupported.length > 0) {
      throw new Error(
        `marketplace_publish: mounts[${index}] has unsupported fields: ${unsupported.join(", ")}`
      )
    }
    if (!VALID_MOUNT_HANDLE.test(mount.handle ?? "")) {
      throw new Error(
        `marketplace_publish: mounts[${index}].handle must contain 1-64 lowercase letters, digits, or underscores`
      )
    }
    if (!VALID_MOUNT_KIND.has(mount.kind)) {
      throw new Error(
        `marketplace_publish: mounts[${index}].kind must be postgres, sqlite, object_store, or redis (legacy alias: s3)`
      )
    }
    if (mount.read_only !== undefined && typeof mount.read_only !== "boolean") {
      throw new Error(`marketplace_publish: mounts[${index}].read_only must be a boolean`)
    }
    return {
      handle: mount.handle,
      kind: mount.kind === "s3" ? "object_store" : mount.kind,
      read_only: mount.read_only ?? true
    }
  })
}

function normalizeLimits(value) {
  if (value === undefined) return undefined
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("marketplace_publish: limits must be an object")
  }
  const unsupported = Object.keys(value).filter((key) => !LIMIT_FIELDS.includes(key))
  if (unsupported.length > 0) {
    throw new Error(`marketplace_publish: limits has unsupported fields: ${unsupported.join(", ")}`)
  }
  const normalized = {}
  for (const field of LIMIT_FIELDS) {
    if (value[field] === undefined) continue
    const minimum = field === "fuel_limit" ? 0 : 1
    if (!Number.isSafeInteger(value[field]) || value[field] < minimum) {
      throw new Error(
        `marketplace_publish: limits.${field} must be an integer >= ${minimum}`
      )
    }
    normalized[field] = value[field]
  }
  return normalized
}

function normalizeVerification(value) {
  if (value === undefined) return undefined
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("marketplace_publish: verification must be an object")
  }
  const unsupported = Object.keys(value).filter(
    (key) => !["input", "expected_output"].includes(key)
  )
  if (unsupported.length > 0) {
    throw new Error(
      `marketplace_publish: verification has unsupported fields: ${unsupported.join(", ")}`
    )
  }
  if (!Object.prototype.hasOwnProperty.call(value, "input")) {
    throw new Error("marketplace_publish: verification.input is required")
  }
  tomlJsonString(value.input, "verification.input")
  if (Object.prototype.hasOwnProperty.call(value, "expected_output")) {
    tomlJsonString(value.expected_output, "verification.expected_output")
  }
  return {
    input: value.input,
    ...(Object.prototype.hasOwnProperty.call(value, "expected_output")
      ? { expected_output: value.expected_output }
      : {})
  }
}

function normalizeEntrypoint(value) {
  const raw = typeof value === "string" ? value.trim() : "handler.py"
  if (raw.length === 0) {
    throw new Error("marketplace_publish: entrypoint must be a non-empty relative path")
  }
  if (isAbsolute(raw) || /^[A-Za-z]:[\\/]/.test(raw)) {
    throw new Error("marketplace_publish: entrypoint must be a relative path")
  }
  if (!VALID_ENTRYPOINT.test(raw)) {
    throw new Error("marketplace_publish: entrypoint contains unsupported characters")
  }
  const segments = raw.split(/[\\/]+/)
  if (segments.some((segment) => segment.length === 0 || segment === "." || segment === "..")) {
    throw new Error("marketplace_publish: entrypoint must not contain path traversal")
  }
  return segments.join("/")
}

function normalizeHttpUrlSyntax(value, label) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`marketplace_publish: ${label} must be a non-empty https:// URL`)
  }
  const raw = value.trim()
  if (/\s/.test(raw)) {
    throw new Error(`marketplace_publish: ${label} is not a valid URL: contains whitespace`)
  }
  let parsed
  try {
    parsed = new URL(raw)
  } catch (error) {
    throw new Error(`marketplace_publish: ${label} is not a valid URL: ${error.message}`)
  }
  if (parsed.protocol !== "https:") {
    throw new Error(`marketplace_publish: ${label} must use https://`)
  }
  if (parsed.username !== "" || parsed.password !== "") {
    throw new Error(`marketplace_publish: ${label} must not contain credentials`)
  }
  return parsed.toString().replace(/\/$/, "")
}

async function validatePublicHttpsUrl(value, label, deps) {
  try {
    return (await validateProviderUrl(value, label, deps ? { _deps: deps } : {})).normalizedUrl
  } catch (error) {
    throw new Error(`marketplace_publish: ${label} must be a public https:// URL: ${error.message}`)
  }
}

/**
 * Validate the MCP-shaped publish input. Throws on the first problem with a
 * message the LLM can act on directly ("set X to Y") rather than a generic
 * "invalid input".
 */
function validatePublishInputShape(args) {
  const name = (args.name ?? "").trim()
  if (!VALID_NAME.test(name)) {
    throw new Error(
      `marketplace_publish: name ${JSON.stringify(args.name)} is invalid; ` +
        "must be 1-63 lowercase ASCII letters, digits, or interior hyphens"
    )
  }

  const offerId = typeof args.offer_id === "string" ? args.offer_id.trim() : undefined
  if (offerId !== undefined && !VALID_NAME.test(offerId)) {
    throw new Error(
      `marketplace_publish: offer_id ${JSON.stringify(args.offer_id)} is invalid; ` +
        "must be 1-63 lowercase ASCII letters, digits, or interior hyphens"
    )
  }

  const runtime = args.runtime ?? "python"
  if (!VALID_RUNTIMES.has(runtime)) {
    throw new Error(
      `marketplace_publish: runtime ${JSON.stringify(runtime)} not supported in v1A; ` +
        "use 'python' (WASM + OCI runtimes land in Phase 1B)"
    )
  }

  const packageKind = args.package_kind ?? "inline_source"
  if (!VALID_PACKAGE_KINDS.has(packageKind)) {
    throw new Error(
      `marketplace_publish: package_kind ${JSON.stringify(packageKind)} not supported in v1A; ` +
        "use 'inline_source'"
    )
  }

  if (typeof args.source_inline !== "string" || args.source_inline.length === 0) {
    throw new Error(
      "marketplace_publish: source_inline is required (Python source defining handler(event, context)); " +
        "WASM + OCI source forms are Phase 1B"
    )
  }

  const hostingKind = args.hosting?.kind ?? "relay"
  if (!VALID_HOSTING.has(hostingKind)) {
    throw new Error(
      `marketplace_publish: hosting.kind ${JSON.stringify(hostingKind)} not supported in v1A; ` +
        "use 'local' | 'relay' | 'tor' | 'self'"
    )
  }
  if (hostingKind === "self" && typeof args.hosting?.url !== "string") {
    throw new Error(
      "marketplace_publish: hosting.url is required when hosting.kind = 'self'"
    )
  }
  if (
    args.consent_hash !== undefined &&
    (typeof args.consent_hash !== "string" || !/^[0-9a-f]{64}$/.test(args.consent_hash))
  ) {
    throw new Error(
      "marketplace_publish: consent_hash must be the 64-character lowercase hash returned by the prior approval_required plan"
    )
  }

  const settlementMethod = args.settlement?.method ?? "none"
  if (!VALID_SETTLEMENT.has(settlementMethod)) {
    throw new Error(
      `marketplace_publish: settlement.method ${JSON.stringify(settlementMethod)} not supported; ` +
        "use 'none' (free), 'lightning' (paid via Lightning), or 'stripe' (paid via Stripe MPP)"
    )
  }

  if (
    args.price_sats !== undefined &&
    (!Number.isSafeInteger(args.price_sats) || args.price_sats < 0)
  ) {
    throw new Error("marketplace_publish: price_sats must be a non-negative safe integer")
  }
  const priceSats = args.price_sats ?? 0
  if (priceSats > 0 && settlementMethod === "none") {
    throw new Error(
      "marketplace_publish: paid publication requires settlement.method = 'lightning' or 'stripe'"
    )
  }
  // The manifest validator requires currency="usd" for stripe settlement and
  // "sat"/absent for lightning/none. Stripe "sats" are therefore USD cents.
  const currency = settlementMethod === "stripe" ? "usd" : "sat"
  const mode = args.mode ?? "sync"
  if (!VALID_MODE.has(mode)) {
    throw new Error("marketplace_publish: mode must be 'sync' or 'async'")
  }
  const publicationState = args.publication_state ?? "active"
  if (!VALID_PUBLICATION_STATE.has(publicationState)) {
    throw new Error("marketplace_publish: publication_state must be 'active' or 'hidden'")
  }

  for (const field of ["input_schema", "output_schema"]) {
    if (args[field] !== undefined) tomlJsonString(args[field], field)
  }
  const verification = normalizeVerification(args.verification)
  if (hostingKind !== "local" && verification === undefined) {
    throw new Error(
      "marketplace_publish: verification with an input fixture is required for relay, tor, and self-hosted publication; " +
        "the fixture runs privately before any public candidate is submitted"
    )
  }

  return {
    name,
    offerId,
    summary: typeof args.summary === "string" ? args.summary : `Froglet service ${name}`,
    starter: typeof args.starter === "string" ? args.starter : undefined,
    runtime,
    packageKind,
    entrypointKind: "handler",
    entrypoint: normalizeEntrypoint(args.entrypoint),
    contractVersion: "froglet.python.handler_json.v1",
    mode,
    publicationState,
    mounts: normalizeMounts(args.mounts),
    capabilities: normalizeCapabilities(args.capabilities),
    limits: normalizeLimits(args.limits),
    inputSchema: args.input_schema,
    outputSchema: args.output_schema,
    verification,
    sourceInline: args.source_inline,
    hosting: {
      kind: hostingKind,
      url: hostingKind === "self" ? normalizeHttpUrlSyntax(args.hosting?.url, "hosting.url") : undefined,
    },
    consentHash: args.consent_hash,
    settlement: { method: settlementMethod },
    priceSats,
    currency,
    marketplaceUrl: normalizeHttpUrlSyntax(
      typeof args.marketplace_url === "string"
        ? args.marketplace_url
        : "https://marketplace.froglet.dev",
      "marketplace_url"
    ),
  }
}

export async function validatePublishInput(args, opts = {}) {
  const input = validatePublishInputShape(args)
  const deps = opts?._deps ?? {}
  const marketplaceUrl = await validatePublicHttpsUrl(
    input.marketplaceUrl,
    "marketplace_url",
    deps.marketplaceUrl
  )
  const selfUrl = input.hosting.kind === "self"
    ? await validatePublicHttpsUrl(input.hosting.url, "hosting.url", deps.selfUrl)
    : undefined
  return {
    ...input,
    marketplaceUrl,
    hosting: {
      ...input.hosting,
      url: selfUrl,
    },
  }
}

function projectToml({ name, marketplaceUrl }) {
  return [
    `schema_version = "froglet/v1"`,
    ``,
    `[project]`,
    `name = ${tomlString(name)}`,
    ``,
    `[project.marketplace]`,
    `url = ${tomlString(marketplaceUrl)}`,
    ``,
  ].join("\n")
}

export function serviceToml(input) {
  const {
    name,
    offerId,
    summary,
    starter,
    runtime,
    packageKind,
    entrypointKind,
    entrypoint,
    contractVersion,
    mode,
    publicationState,
    mounts,
    capabilities,
    limits,
    inputSchema,
    outputSchema,
    verification,
    hosting,
    settlement,
    priceSats,
    currency,
    marketplaceUrl
  } = input
  const lines = [
    `schema_version = "froglet-service/v4"`,
    ``,
    `project_id = ${tomlString(name)}`,
    `service_id = ${tomlString(name)}`,
    ...(offerId === undefined ? [] : [`offer_id = ${tomlString(offerId)}`]),
    `summary = ${tomlString(summary)}`,
    ...(starter === undefined ? [] : [`starter = ${tomlString(starter)}`]),
    ``,
    `runtime = ${tomlString(runtime)}`,
    `package_kind = ${tomlString(packageKind)}`,
    `entrypoint_kind = ${tomlString(entrypointKind)}`,
    `entrypoint = ${tomlString(entrypoint)}`,
    `contract_version = ${tomlString(contractVersion)}`,
    `mode = ${tomlString(mode)}`,
    `publication_state = ${tomlString(publicationState)}`,
    ...(mounts.length === 0 ? [] : [`mounts = ${tomlInlineValue(mounts, "mounts")}`]),
    ...(capabilities.length === 0
      ? []
      : [`capabilities = ${tomlInlineValue(capabilities, "capabilities")}`]),
    ...(limits === undefined ? [] : [`limits = ${tomlInlineValue(limits, "limits")}`]),
    ...(inputSchema === undefined
      ? []
      : [`input_schema_json = ${tomlJsonString(inputSchema, "input_schema")}`]),
    ...(outputSchema === undefined
      ? []
      : [`output_schema_json = ${tomlJsonString(outputSchema, "output_schema")}`]),
    ...(verification === undefined
      ? []
      : [
          `verification = { input_json = ${tomlJsonString(verification.input, "verification.input")}${Object.prototype.hasOwnProperty.call(verification, "expected_output") ? `, expected_output_json = ${tomlJsonString(verification.expected_output, "verification.expected_output")}` : ""} }`
        ]),
    ``,
    `[hosting]`,
    `default = ${tomlString(hosting.kind)}`,
  ]
  if (hosting.kind === "self") {
    lines.push(``, `[hosting.self]`, `url = ${tomlString(hosting.url)}`)
  }
  lines.push(``, `[settlement]`, `method = ${tomlString(settlement.method)}`)
  lines.push(``, `[marketplace]`, `url = ${tomlString(marketplaceUrl)}`)
  lines.push(``, `[price]`, `sats = ${priceSats ?? 0}`, `currency = ${tomlString(currency ?? "sat")}`, ``)
  return lines.join("\n")
}

/**
 * Run `froglet-node publish --json` against a freshly-materialised service
 * directory and return the parsed JSON output. Cleans up the temp dir on
 * success and failure; the source code is preserved in the daemon's offer
 * artifact regardless.
 *
 * `frogletNodeBinary` defaults to "froglet-node" so the caller's PATH wins.
 * Override via the FROGLET_NODE_BIN env var for tests / non-PATH installs.
 */
export async function runMarketplacePublish(args, options = {}) {
  const input = await validatePublishInput(args, { _deps: options._deps })
  const binary = options.frogletNodeBinary || process.env.FROGLET_NODE_BIN || "froglet-node"

  const workDir = await mkdtemp(join(tmpdir(), "froglet-publish-"))
  try {
    await writeFile(join(workDir, "froglet.toml"), projectToml(input))
    await writeFile(join(workDir, "froglet-service.toml"), serviceToml(input))
    const workDirRoot = `${resolve(workDir)}${sep}`
    const entrypointPath = resolve(workDir, input.entrypoint)
    if (!entrypointPath.startsWith(workDirRoot)) {
      throw new Error("marketplace_publish: entrypoint resolved outside the temporary service directory")
    }
    await mkdir(dirname(entrypointPath), { recursive: true })
    await writeFile(entrypointPath, input.sourceInline)

    const flags = ["publish", "--json"]
    if (input.hosting.kind !== "local") {
      // The default is taken from the manifest; only pass --host when the
      // MCP caller wants to override (we always set it explicitly here so
      // the behaviour is deterministic).
      flags.push("--host", input.hosting.kind)
    } else {
      flags.push("--host", "local")
    }
    flags.push("--marketplace", input.marketplaceUrl)
    if (input.hosting.kind !== "local") {
      if (input.consentHash === undefined) {
        flags.push("--plan")
      } else {
        flags.push("--approve-consent", input.consentHash)
      }
    }

    const { stdout, stderr } = await execFileAsync(binary, flags, {
      cwd: workDir,
      timeout: PUBLISH_TIMEOUT_MS,
      maxBuffer: 4 * 1024 * 1024,
      env: process.env,
    })
    if (stderr && stderr.length > 0) {
      // The CLI uses stderr for human-readable warnings; surface them so the
      // LLM can mention them in its response without parsing the JSON.
      // eslint-disable-next-line no-console
      console.error(`marketplace_publish stderr: ${stderr.trim()}`)
    }
    return validateMarketplacePublishResponse(JSON.parse(stdout))
  } catch (error) {
    const exitCode = error?.code
    const stderr = error?.stderr ? String(error.stderr).trim() : ""
    const stdout = error?.stdout ? String(error.stdout).trim() : ""
    const detail = stderr || stdout || error?.message || "unknown"
    const wrapped = new Error(
      `marketplace_publish: froglet-node publish failed (exit=${exitCode}): ${detail}`
    )
    wrapped.cause = error
    throw wrapped
  } finally {
    await rm(workDir, { recursive: true, force: true }).catch(() => {})
  }
}

export function validateMarketplacePublishResponse(response) {
  if (!response || typeof response !== "object" || Array.isArray(response)) {
    throw new Error("marketplace_publish: froglet-node returned a non-object JSON response")
  }
  if (response.status === "approval_required") {
    if (!/^[0-9a-f]{64}$/.test(response.consent_hash ?? "")) {
      throw new Error("marketplace_publish: approval response is missing a canonical consent_hash")
    }
    if (!response.summary || typeof response.summary !== "object" || Array.isArray(response.summary)) {
      throw new Error("marketplace_publish: approval response is missing its consent summary")
    }
    return response
  }
  if (response.status && ![
    "published", "active", "succeeded", // older CLI responses
    "local_published", "local_verified", "pending_review",
    "marketplace_active", "healthy"
  ].includes(response.status)) {
    throw new Error(`marketplace_publish: unexpected publication status ${response.status}`)
  }
  for (const field of ["provider_id", "public_url", "offer_hash"]) {
    if (typeof response[field] !== "string" || response[field].trim().length === 0) {
      throw new Error(`marketplace_publish: publication response is missing ${field}`)
    }
  }
  if (response.warnings !== undefined && !Array.isArray(response.warnings)) {
    throw new Error("marketplace_publish: publication response warnings must be an array")
  }
  return response
}
