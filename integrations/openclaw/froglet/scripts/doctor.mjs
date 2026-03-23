import { access, readFile } from "node:fs/promises"
import { constants as fsConstants } from "node:fs"
import path from "node:path"
import process from "node:process"
import { fileURLToPath } from "node:url"
import { parseArgs } from "node:util"

import {
  ABSOLUTE_MAX_SEARCH_LIMIT,
  DEFAULT_MAX_SEARCH_LIMIT,
  DEFAULT_SEARCH_LIMIT,
  DEFAULT_TIMEOUT_MS,
  MAX_TIMEOUT_MS,
  MIN_SEARCH_LIMIT,
  MIN_TIMEOUT_MS,
  normalizeBaseUrl,
  requestJson
} from "../lib/shared.js"

const LOOPBACK_HTTP_HOSTS = new Set(["127.0.0.1", "localhost", "::1", "[::1]"])
const ALLOWED_PLUGIN_CONFIG_KEYS = new Set([
  "runtimeUrl",
  "runtimeAuthTokenPath",
  "requestTimeoutMs",
  "defaultSearchLimit",
  "maxSearchLimit"
])

function usage() {
  return [
    "Usage:",
    "  node scripts/doctor.mjs --config <path> --target openclaw|nemoclaw [--check-runtime]",
    "",
    "This validates the Froglet-owned portion of a complete OpenClaw or NemoClaw JSON config."
  ].join("\n")
}

function buildCheck(id, status, summary, details = {}) {
  return { id, status, summary, details }
}

function hasErrors(checks) {
  return checks.some((check) => check.status === "error")
}

function overallStatus(checks) {
  if (checks.some((check) => check.status === "error")) {
    return "error"
  }
  if (checks.some((check) => check.status === "warning")) {
    return "warning"
  }
  return "ok"
}

function validateIntegerField(name, value, minimum, maximum) {
  if (value === undefined) {
    return
  }
  if (!Number.isInteger(value)) {
    throw new Error(`${name} must be an integer`)
  }
  if (value < minimum || value > maximum) {
    throw new Error(`${name} must be between ${minimum} and ${maximum}`)
  }
}

function validateRuntimeUrl(value) {
  const normalized = normalizeBaseUrl(value, "runtimeUrl")
  const parsed = new URL(normalized)
  if (parsed.protocol === "http:" && !LOOPBACK_HTTP_HOSTS.has(parsed.hostname)) {
    throw new Error("runtimeUrl must use https:// or loopback http://")
  }
  return normalized
}

function validatePluginConfig(pluginConfig) {
  if (!pluginConfig || typeof pluginConfig !== "object" || Array.isArray(pluginConfig)) {
    throw new Error("plugins.entries.froglet.config must be an object")
  }

  const unknownKeys = Object.keys(pluginConfig).filter((key) => !ALLOWED_PLUGIN_CONFIG_KEYS.has(key))
  if (unknownKeys.length > 0) {
    throw new Error(`Unknown Froglet plugin config keys: ${unknownKeys.join(", ")}`)
  }

  if (
    typeof pluginConfig.runtimeAuthTokenPath !== "string" ||
    pluginConfig.runtimeAuthTokenPath.trim().length === 0
  ) {
    throw new Error("runtimeAuthTokenPath must be a non-empty filesystem path")
  }
  if (!path.isAbsolute(pluginConfig.runtimeAuthTokenPath.trim())) {
    throw new Error("runtimeAuthTokenPath must be an absolute filesystem path")
  }

  const runtimeUrl = validateRuntimeUrl(pluginConfig.runtimeUrl)
  validateIntegerField("requestTimeoutMs", pluginConfig.requestTimeoutMs, MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)
  validateIntegerField("defaultSearchLimit", pluginConfig.defaultSearchLimit, MIN_SEARCH_LIMIT, ABSOLUTE_MAX_SEARCH_LIMIT)
  validateIntegerField("maxSearchLimit", pluginConfig.maxSearchLimit, MIN_SEARCH_LIMIT, ABSOLUTE_MAX_SEARCH_LIMIT)

  const effectiveMaxSearchLimit = pluginConfig.maxSearchLimit ?? DEFAULT_MAX_SEARCH_LIMIT
  const effectiveDefaultSearchLimit = pluginConfig.defaultSearchLimit ?? DEFAULT_SEARCH_LIMIT
  if (effectiveDefaultSearchLimit > effectiveMaxSearchLimit) {
    throw new Error(
      `defaultSearchLimit must be less than or equal to maxSearchLimit (${effectiveMaxSearchLimit})`
    )
  }

  return {
    runtimeUrl,
    runtimeAuthTokenPath: pluginConfig.runtimeAuthTokenPath.trim(),
    requestTimeoutMs: pluginConfig.requestTimeoutMs ?? DEFAULT_TIMEOUT_MS
  }
}

async function pathExists(targetPath) {
  try {
    await access(targetPath, fsConstants.F_OK)
    return true
  } catch {
    return false
  }
}

async function pathReadable(targetPath) {
  try {
    await access(targetPath, fsConstants.R_OK)
    return true
  } catch {
    return false
  }
}

function getPluginLoadPath(document) {
  const paths = document?.plugins?.load?.paths
  if (!Array.isArray(paths) || paths.length === 0) {
    throw new Error("plugins.load.paths must contain at least one plugin path")
  }
  if (typeof paths[0] !== "string" || paths[0].trim().length === 0) {
    throw new Error("plugins.load.paths[0] must be a non-empty filesystem path")
  }
  if (!path.isAbsolute(paths[0].trim())) {
    throw new Error("plugins.load.paths[0] must be an absolute filesystem path")
  }
  return paths[0].trim()
}

function getFrogletEntry(document) {
  const entry = document?.plugins?.entries?.froglet
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
    throw new Error("plugins.entries.froglet must be present in the complete config")
  }
  if (entry.enabled !== true) {
    throw new Error("plugins.entries.froglet.enabled must be true")
  }
  return entry
}

function nemoclawManualCommands({ loadPath, runtimeUrl, runtimeAuthTokenPath }) {
  return [
    "nemoclaw <sandbox-name> status",
    "openshell sandbox get <sandbox-name>",
    `openshell sandbox upload <sandbox-name> /absolute/path/to/froglet/integrations/openclaw/froglet ${loadPath}`,
    `openshell sandbox upload <sandbox-name> /absolute/path/to/froglet-runtime.token ${runtimeAuthTokenPath}`,
    "openshell sandbox upload <sandbox-name> /absolute/path/to/your-ca.pem /sandbox/froglet/runtime-ca.pem   # optional",
    "nemoclaw <sandbox-name> connect",
    `TOKEN=$(cat ${runtimeAuthTokenPath})`,
    `curl -H "Authorization: Bearer $TOKEN" ${runtimeUrl}/v1/runtime/wallet/balance`
  ]
}

export async function collectDoctorResults({ configPath, target, checkRuntime = false }) {
  const checks = []
  const normalizedTarget = target === "openclaw" || target === "nemoclaw" ? target : null
  if (normalizedTarget === null) {
    throw new Error(`target must be 'openclaw' or 'nemoclaw'; got ${target}`)
  }

  let document
  try {
    document = JSON.parse(await readFile(configPath, "utf8"))
    checks.push(buildCheck("config", "ok", `Parsed ${configPath}`))
  } catch (error) {
    checks.push(buildCheck("config", "error", `Failed to parse ${configPath}: ${error.message}`))
    return { overallStatus: overallStatus(checks), checks }
  }

  let loadPath
  let frogletEntry
  let pluginConfig
  try {
    loadPath = getPluginLoadPath(document)
    frogletEntry = getFrogletEntry(document)
    pluginConfig = validatePluginConfig(frogletEntry.config)
    checks.push(
      buildCheck(
        "froglet_config",
        "ok",
        `Validated Froglet plugin contract for ${normalizedTarget}`,
        {
          runtimeUrl: pluginConfig.runtimeUrl,
          runtimeAuthTokenPath: pluginConfig.runtimeAuthTokenPath
        }
      )
    )
  } catch (error) {
    checks.push(buildCheck("froglet_config", "error", error.message))
    return { overallStatus: overallStatus(checks), checks }
  }

  if (normalizedTarget === "openclaw") {
    if (await pathExists(loadPath)) {
      checks.push(buildCheck("plugin_load_path", "ok", `Plugin path exists: ${loadPath}`))
    } else {
      checks.push(
        buildCheck(
          "plugin_load_path",
          "error",
          `OpenClaw plugin path does not exist on this machine: ${loadPath}`
        )
      )
    }
  } else if (await pathExists(loadPath)) {
    checks.push(buildCheck("plugin_load_path", "ok", `Plugin path exists locally: ${loadPath}`))
  } else if (loadPath.startsWith("/sandbox/")) {
    checks.push(
      buildCheck(
        "plugin_load_path",
        "warning",
        `Sandbox plugin path must be verified after staging: ${loadPath}`
      )
    )
  } else {
    checks.push(
      buildCheck(
        "plugin_load_path",
        "error",
        `NemoClaw plugin path must either exist locally or point inside /sandbox: ${loadPath}`
      )
    )
  }

  if (checkRuntime) {
    const tokenReadable = await pathReadable(pluginConfig.runtimeAuthTokenPath)
    if (!tokenReadable) {
      const status = normalizedTarget === "openclaw" ? "error" : "warning"
      checks.push(
        buildCheck(
          "runtime_health",
          status,
          `Skipped runtime probe because the token is not readable on this machine: ${pluginConfig.runtimeAuthTokenPath}`
        )
      )
    } else {
      try {
        const token = (await readFile(pluginConfig.runtimeAuthTokenPath, "utf8")).trim()
        if (token.length === 0) {
          throw new Error(`Runtime auth token file is empty: ${pluginConfig.runtimeAuthTokenPath}`)
        }
        await requestJson(`${pluginConfig.runtimeUrl}/v1/runtime/wallet/balance`, {
          timeoutMs: pluginConfig.requestTimeoutMs,
          headers: { Authorization: `Bearer ${token}` },
          expectedStatuses: [200]
        })
        checks.push(
          buildCheck(
            "runtime_health",
            "ok",
            `Runtime responded successfully at ${pluginConfig.runtimeUrl}/v1/runtime/wallet/balance`
          )
        )
      } catch (error) {
        checks.push(buildCheck("runtime_health", "error", error.message))
      }
    }
  }

  if (normalizedTarget === "nemoclaw") {
    checks.push(
      buildCheck(
        "manual_nemoclaw_checks",
        "warning",
        "Run native NemoClaw/OpenShell staging and in-sandbox verification commands",
        {
          commands: nemoclawManualCommands({
            loadPath,
            runtimeUrl: pluginConfig.runtimeUrl,
            runtimeAuthTokenPath: pluginConfig.runtimeAuthTokenPath
          })
        }
      )
    )
  }

  return { overallStatus: overallStatus(checks), checks }
}

function formatOutput(result) {
  const lines = result.checks.map((check) => `[${check.status}] ${check.id}: ${check.summary}`)
  const manualCheck = result.checks.find((check) => check.id === "manual_nemoclaw_checks")
  if (manualCheck?.details?.commands) {
    lines.push("")
    lines.push("Native NemoClaw follow-up:")
    for (const command of manualCheck.details.commands) {
      lines.push(`  ${command}`)
    }
  }
  lines.push("")
  lines.push(`overall_status=${result.overallStatus}`)
  return `${lines.join("\n")}\n`
}

export async function runDoctorCli(argv = process.argv.slice(2), stdout = process.stdout, stderr = process.stderr) {
  const parsed = parseArgs({
    args: argv,
    options: {
      config: { type: "string" },
      target: { type: "string" },
      "check-runtime": { type: "boolean" },
      help: { type: "boolean", short: "h" }
    },
    allowPositionals: false
  })

  if (parsed.values.help) {
    stdout.write(`${usage()}\n`)
    return 0
  }

  if (!parsed.values.config || !parsed.values.target) {
    stderr.write(`${usage()}\n`)
    return 2
  }

  try {
    const result = await collectDoctorResults({
      configPath: parsed.values.config,
      target: parsed.values.target,
      checkRuntime: parsed.values["check-runtime"] === true
    })
    stdout.write(formatOutput(result))
    return hasErrors(result.checks) ? 1 : 0
  } catch (error) {
    stderr.write(`${error.message}\n`)
    return 1
  }
}

const isEntrypoint =
  process.argv[1] !== undefined && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])

if (isEntrypoint) {
  const exitCode = await runDoctorCli()
  process.exitCode = exitCode
}
