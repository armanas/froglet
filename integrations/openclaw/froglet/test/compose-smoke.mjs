import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import { setTimeout as delay } from "node:timers/promises"

const testDir = fileURLToPath(new URL("./", import.meta.url))
const pluginDir = path.resolve(testDir, "..")
const repoRoot = path.resolve(pluginDir, "../../..")

function extractAppendedJson(text) {
  const start = Math.max(text.lastIndexOf("\n{"), text.lastIndexOf("\n["))
  assert.notEqual(start, -1, "missing appended JSON payload in tool output")
  return JSON.parse(text.slice(start + 1))
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
  pluginConfig.authTokenPath = path.resolve(
    pluginConfig.authTokenPath.replace("/absolute/path/to/froglet", repoRoot)
  )
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

  const status = await froglet.definition.execute("status", {
    action: "status",
    include_raw: true
  })
  const statusRaw = extractAppendedJson(status.content[0].text)
  assert.equal(statusRaw.runtime?.healthy, true)
  assert.equal(statusRaw.provider?.healthy, true)
  assert.equal(statusRaw.reference_discovery?.connected, true)

  let discoverRaw = null
  let providerId = null
  for (let attempt = 0; attempt < 15; attempt += 1) {
    const discover = await froglet.definition.execute("discover", {
      action: "discover_services",
      limit: 10,
      include_inactive: false,
      include_raw: true
    })
    discoverRaw = extractAppendedJson(discover.content[0].text)
    const services = Array.isArray(discoverRaw.services) ? discoverRaw.services : []
    const executeCompute = services.find((service) => service?.service_id === "execute.compute")
    providerId = executeCompute?.provider_id ?? null
    if (providerId) {
      break
    }
    await delay(1000)
  }

  assert.ok(discoverRaw, "missing discovery response")
  assert.equal(Number(discoverRaw.provider_nodes_discovered ?? 0) >= 1, true)
  assert.ok(providerId, "discovery did not return execute.compute")

  const service = await froglet.definition.execute("service", {
    action: "get_service",
    provider_id: providerId,
    service_id: "execute.compute",
    include_raw: true
  })
  const serviceRaw = extractAppendedJson(service.content[0].text)
  assert.equal(serviceRaw.service?.service_id, "execute.compute")
  assert.equal(serviceRaw.service?.provider_id, providerId)
  assert.equal(serviceRaw.service?.publication_state, "active")

  const invoke = await froglet.definition.execute("invoke", {
    action: "invoke_service",
    provider_id: providerId,
    service_id: "events.query",
    input: { kinds: [], limit: 1 },
    include_raw: true
  })
  const invokeRaw = extractAppendedJson(invoke.content[0].text)
  const effectiveResult = invokeRaw.result ?? invokeRaw.task?.result
  assert.notEqual(effectiveResult, undefined)
  assert.equal(typeof effectiveResult, "object")
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
