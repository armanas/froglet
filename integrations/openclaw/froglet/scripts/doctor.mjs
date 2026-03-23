import { access, readFile } from "node:fs/promises"
import { constants as fsConstants } from "node:fs"
import path from "node:path"
import process from "node:process"
import { parseArgs } from "node:util"

import {
  ABSOLUTE_MAX_SEARCH_LIMIT,
  MAX_TIMEOUT_MS,
  MIN_SEARCH_LIMIT,
  MIN_TIMEOUT_MS,
  normalizeBaseUrl,
  requestJson
} from "../lib/shared.js"
import { readPluginConfig } from "../lib/config.js"

const LOOPBACK_HTTP_HOSTS = new Set(["127.0.0.1", "localhost", "::1", "[::1]"])

function usage() {
  return [
    "Usage:",
    "  node scripts/doctor.mjs --config <path> --target openclaw|nemoclaw [--check-runtime]",
    "",
    "This validates the Froglet plugin section inside a complete OpenClaw or NemoClaw config."
  ].join("\n")
}

function buildCheck(id, status, summary, details = {}) {
  return { id, status, summary, details }
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

function validateInteger(name, value, minimum, maximum) {
  if (!Number.isInteger(value)) {
    throw new Error(`${name} must be an integer`)
  }
  if (value < minimum || value > maximum) {
    throw new Error(`${name} must be between ${minimum} and ${maximum}`)
  }
}

function validateBaseUrl(name, value) {
  const normalized = normalizeBaseUrl(value, name)
  const parsed = new URL(normalized)
  if (parsed.protocol === "http:" && !LOOPBACK_HTTP_HOSTS.has(parsed.hostname)) {
    throw new Error(`${name} must use https:// or loopback http://`)
  }
  return normalized
}

async function pathReadable(targetPath) {
  try {
    await access(targetPath, fsConstants.R_OK)
    return true
  } catch {
    return false
  }
}

function getPluginPath(document) {
  const paths = document?.plugins?.load?.paths
  if (!Array.isArray(paths) || paths.length === 0) {
    throw new Error("plugins.load.paths must contain at least one plugin path")
  }
  const pluginPath = String(paths[0] ?? "").trim()
  if (!path.isAbsolute(pluginPath)) {
    throw new Error("plugins.load.paths[0] must be an absolute filesystem path")
  }
  return pluginPath
}

function getPluginConfig(document) {
  const entry = document?.plugins?.entries?.froglet
  if (!entry || entry.enabled !== true || typeof entry.config !== "object") {
    throw new Error("plugins.entries.froglet.enabled=true with a config object is required")
  }
  return readPluginConfig({ config: entry.config })
}

async function checkRuntime(baseUrl, authTokenPath, requestTimeoutMs) {
  const token = (await readFile(authTokenPath, "utf8")).trim()
  return requestJson(`${baseUrl}/v1/froglet/status`, {
    method: "GET",
    timeoutMs: requestTimeoutMs,
    headers: {
      Authorization: `Bearer ${token}`
    }
  })
}

function manualCommands(target, config) {
  if (target === "nemoclaw") {
    return [
      "nemoclaw <sandbox-name> status",
      "openshell sandbox get <sandbox-name>",
      "openshell sandbox upload <sandbox-name> /absolute/path/to/froglet /sandbox/froglet/integrations/openclaw/froglet",
      `openshell sandbox upload <sandbox-name> /absolute/path/to/froglet-control.token ${config.authTokenPath}`,
      "nemoclaw <sandbox-name> connect",
      `TOKEN=$(cat ${config.authTokenPath})`,
      `curl -H "Authorization: Bearer $TOKEN" ${config.baseUrl}/v1/froglet/status`
    ]
  }
  return [
    `TOKEN=$(cat ${config.authTokenPath})`,
    `curl -H "Authorization: Bearer $TOKEN" ${config.baseUrl}/v1/froglet/status`
  ]
}

async function main() {
  const parsed = parseArgs({
    allowPositionals: false,
    options: {
      config: { type: "string" },
      target: { type: "string" },
      "check-runtime": { type: "boolean", default: false },
      help: { type: "boolean", default: false }
    }
  })

  if (parsed.values.help) {
    process.stdout.write(`${usage()}\n`)
    process.exit(0)
  }

  const configPath = parsed.values.config
  const target = parsed.values.target
  if (!configPath || !target || (target !== "openclaw" && target !== "nemoclaw")) {
    process.stderr.write(`${usage()}\n`)
    process.exit(1)
  }

  const document = JSON.parse(await readFile(configPath, "utf8"))
  const pluginPath = getPluginPath(document)
  const pluginConfig = getPluginConfig(document)

  if (pluginConfig.hostProduct !== target) {
    throw new Error(`doctor target ${target} requires hostProduct=${target}`)
  }

  validateBaseUrl("baseUrl", pluginConfig.baseUrl)
  validateInteger("requestTimeoutMs", pluginConfig.requestTimeoutMs, MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)
  validateInteger("defaultSearchLimit", pluginConfig.defaultSearchLimit, MIN_SEARCH_LIMIT, ABSOLUTE_MAX_SEARCH_LIMIT)
  validateInteger("maxSearchLimit", pluginConfig.maxSearchLimit, MIN_SEARCH_LIMIT, ABSOLUTE_MAX_SEARCH_LIMIT)
  if (pluginConfig.defaultSearchLimit > pluginConfig.maxSearchLimit) {
    throw new Error("defaultSearchLimit must be less than or equal to maxSearchLimit")
  }
  if (!path.isAbsolute(pluginConfig.authTokenPath)) {
    throw new Error("authTokenPath must be an absolute filesystem path")
  }

  const checks = []
  checks.push(buildCheck("config", "ok", "plugin config parsed", { hostProduct: pluginConfig.hostProduct }))
  const pluginExists = await pathReadable(pluginPath)
  checks.push(
    buildCheck(
      "plugin_source",
      pluginExists ? "ok" : "error",
      pluginExists
        ? `Plugin path exists: ${pluginPath}`
        : `Plugin path is missing: ${pluginPath}`
    )
  )
  const tokenReadable = await pathReadable(pluginConfig.authTokenPath)
  checks.push(
    buildCheck(
      "auth_token",
      tokenReadable ? "ok" : "error",
      tokenReadable
        ? `Auth token is readable: ${pluginConfig.authTokenPath}`
        : `Auth token is missing or unreadable: ${pluginConfig.authTokenPath}`
    )
  )

  if (parsed.values["check-runtime"] && tokenReadable) {
    try {
      const status = await checkRuntime(
        pluginConfig.baseUrl,
        pluginConfig.authTokenPath,
        pluginConfig.requestTimeoutMs
      )
      checks.push(
        buildCheck("runtime_health", "ok", "Froglet control API responded", {
          node_id: status.node_id,
          runtime_healthy: status.runtime?.healthy,
          provider_healthy: status.provider?.healthy
        })
      )
    } catch (error) {
      checks.push(buildCheck("runtime_health", "error", `Runtime check failed: ${error.message}`))
    }
  } else {
    checks.push(
      buildCheck(
        "runtime_health",
        "warning",
        "Runtime check skipped",
        { manual_commands: manualCommands(target, pluginConfig) }
      )
    )
  }

  process.stdout.write(
    `${JSON.stringify(
      {
        target,
        overall_status: overallStatus(checks),
        checks
      },
      null,
      2
    )}\n`
  )
}

main().catch((error) => {
  process.stderr.write(`${error.message}\n`)
  process.exit(1)
})
