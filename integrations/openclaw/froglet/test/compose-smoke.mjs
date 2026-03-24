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
  assert.equal(statusRaw.reference_discovery?.enabled, true)

  const serviceId = `compose-smoke-ping-${Date.now()}`
  const create = await froglet.definition.execute("create", {
    action: "create_project",
    service_id: serviceId,
    summary: "Returns pong for the compose smoke",
    price_sats: 0,
    publication_state: "active",
    result_json: { message: "pong" },
    include_raw: true
  })
  const createRaw = extractAppendedJson(create.content[0].text)
  assert.equal(createRaw.project?.service_id, serviceId)
  assert.equal(createRaw.project?.publication_state, "active")

  const build = await froglet.definition.execute("build", {
    action: "build_project",
    project_id: createRaw.project?.project_id ?? serviceId,
    include_raw: true
  })
  const buildRaw = extractAppendedJson(build.content[0].text)
  assert.equal(buildRaw.project?.project_id, createRaw.project?.project_id ?? serviceId)

  await froglet.definition.execute("publish", {
    action: "publish_project",
    project_id: createRaw.project?.project_id ?? serviceId,
    include_raw: true
  })

  const local = await froglet.definition.execute("local", {
    action: "list_local_services",
    include_raw: true
  })
  const localRaw = extractAppendedJson(local.content[0].text)
  const localService = (Array.isArray(localRaw.services) ? localRaw.services : []).find(
    (service) => service?.service_id === serviceId
  )
  assert.ok(localService, "local services did not include the published smoke service")

  const discover = await froglet.definition.execute("discover", {
    action: "discover_services",
    limit: 10,
    include_inactive: false,
    include_raw: true
  })
  const discoverRaw = extractAppendedJson(discover.content[0].text)
  assert.ok(discoverRaw, "missing discovery response")
  assert.equal(Number(discoverRaw.provider_nodes_discovered ?? 0) >= 1, true)
  assert.equal(Array.isArray(discoverRaw.services), true)

  const service = await froglet.definition.execute("service", {
    action: "get_local_service",
    service_id: serviceId,
    include_raw: true
  })
  const serviceRaw = extractAppendedJson(service.content[0].text)
  assert.equal(serviceRaw.service?.service_id, serviceId)
  assert.equal(serviceRaw.service?.publication_state, "active")
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
