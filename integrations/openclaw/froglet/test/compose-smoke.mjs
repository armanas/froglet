import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

const testDir = fileURLToPath(new URL("./", import.meta.url))
const pluginDir = path.resolve(testDir, "..")
const repoRoot = path.resolve(pluginDir, "../../..")

function extractAppendedJson(text) {
  const start = Math.max(text.lastIndexOf("\n{"), text.lastIndexOf("\n["))
  assert.notEqual(start, -1, "missing appended JSON payload in tool output")
  return JSON.parse(text.slice(start + 1))
}

function normalizeResultValue(value) {
  if (typeof value === "string") {
    try {
      return JSON.parse(value)
    } catch {
      return value
    }
  }
  return value
}

async function waitForHealthyStatus(froglet, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs
  let lastRaw = null
  let lastError = null
  while (Date.now() < deadline) {
    try {
      const status = await froglet.definition.execute("status", {
        action: "status",
        include_raw: true
      })
      lastRaw = extractAppendedJson(status.content[0].text)
      lastError = null
      if (
        lastRaw.healthy === true &&
        (lastRaw.components?.runtime?.healthy ?? lastRaw.runtime?.healthy) === true &&
        (lastRaw.components?.provider?.healthy ?? lastRaw.provider?.healthy) === true
      ) {
        return lastRaw
      }
    } catch (error) {
      lastError = error
    }
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }

  throw new Error(
    `compose smoke requires a healthy stack: ${JSON.stringify(lastRaw)}${lastError ? `; last_error=${lastError.message}` : ""}`
  )
}

async function waitForDiscovery(froglet, serviceId, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs
  let lastText = ""
  let lastError = null

  while (Date.now() < deadline) {
    try {
      const discover = await froglet.definition.execute("discover", {
        action: "discover_services",
        limit: 10,
        include_inactive: false
      })
      lastText = discover.content[0].text
      lastError = null
      if (lastText.includes(`service_id: ${serviceId}`)) {
        return lastText
      }
    } catch (error) {
      lastError = error
    }
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }

  throw new Error(
    `service ${serviceId} did not appear in discovery: ${lastText}${lastError ? `; last_error=${lastError.message}` : ""}`
  )
}

function assertConfigValueMatchesSchema(key, value, schema) {
  const expectedType = schema?.type
  switch (expectedType) {
    case "integer":
      assert.equal(Number.isInteger(value), true, `${key} should be integer`)
      break
    case "string":
      assert.equal(typeof value, "string", `${key} should be string`)
      break
    default:
      throw new Error(`Unsupported schema type for ${key}: ${expectedType}`)
  }
}

async function loadPluginFromPackageMetadata() {
  const [packageJson, pluginManifest, exampleConfig] = await Promise.all([
    readFile(path.join(pluginDir, "package.json"), "utf8").then(JSON.parse),
    readFile(path.join(pluginDir, "openclaw.plugin.json"), "utf8").then(JSON.parse),
    readFile(path.join(pluginDir, "examples/openclaw.config.example.json"), "utf8").then(JSON.parse)
  ])

  const pluginId = pluginManifest.id
  const configuredPlugin = exampleConfig.plugins?.entries?.[pluginId]
  assert.ok(configuredPlugin, `missing plugins.entries.${pluginId} in example config`)
  assert.equal(configuredPlugin.enabled, true)

  const expectedLoadPath = path.resolve(
    exampleConfig.plugins.load.paths[0].replace("/absolute/path/to/froglet", repoRoot)
  )
  assert.equal(expectedLoadPath, pluginDir)

  const schemaProperties = pluginManifest.configSchema?.properties ?? {}
  const pluginConfig = structuredClone(configuredPlugin.config)
  for (const tokenKey of ["authTokenPath", "providerAuthTokenPath", "runtimeAuthTokenPath"]) {
    if (typeof pluginConfig[tokenKey] === "string") {
      pluginConfig[tokenKey] = path.resolve(
        pluginConfig[tokenKey].replace("/absolute/path/to/froglet", repoRoot)
      )
    }
  }
  if (process.env.FROGLET_BASE_URL) {
    pluginConfig.baseUrl = process.env.FROGLET_BASE_URL
  }
  if (process.env.FROGLET_PROVIDER_URL) {
    pluginConfig.providerUrl = process.env.FROGLET_PROVIDER_URL
  }
  if (process.env.FROGLET_RUNTIME_URL) {
    pluginConfig.runtimeUrl = process.env.FROGLET_RUNTIME_URL
  }
  if (process.env.FROGLET_AUTH_TOKEN_PATH) {
    pluginConfig.authTokenPath = process.env.FROGLET_AUTH_TOKEN_PATH
  }
  if (process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH) {
    pluginConfig.providerAuthTokenPath = process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH
  }
  if (process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH) {
    pluginConfig.runtimeAuthTokenPath = process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH
  }
  for (const [key, value] of Object.entries(pluginConfig)) {
    assert.ok(schemaProperties[key], `unknown plugin config key ${key}`)
    assertConfigValueMatchesSchema(key, value, schemaProperties[key])
  }

  const extension = packageJson.openclaw?.extensions?.[0]
  assert.equal(typeof extension, "string")
  const registerModule = await import(pathToFileURL(path.join(pluginDir, extension)).href)
  assert.equal(typeof registerModule.default, "function")

  return {
    pluginConfig,
    register: registerModule.default
  }
}

async function main() {
  const { pluginConfig, register } = await loadPluginFromPackageMetadata()

  const tools = new Map()
  register({
    config: pluginConfig,
    registerTool(definition, options = {}) {
      tools.set(definition.name, { definition, options })
    },
    logger: {
      info() {}
    }
  })

  assert.deepEqual([...tools.keys()], ["froglet"])

  const froglet = tools.get("froglet")

  await waitForHealthyStatus(froglet)

  // Verify local service listing works
  const local = await froglet.definition.execute("local", {
    action: "list_local_services"
  })
  assert.equal(typeof local.content[0].text, "string")

  // Verify marketplace discovery works
  const discoverText = await waitForDiscovery(froglet, "execute.compute")
  assert.ok(discoverText.includes("service_id: execute.compute"))
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
