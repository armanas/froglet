import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import { setTimeout as delay } from "node:timers/promises"

const testDir = fileURLToPath(new URL("./", import.meta.url))
const pluginDir = path.resolve(testDir, "..")
const repoRoot = path.resolve(pluginDir, "../../..")

function sha256Hex(value) {
  return createHash("sha256").update(value).digest("hex")
}

function canonicalJson(value) {
  if (value === null) {
    return "null"
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalJson(item)).join(",")}]`
  }
  switch (typeof value) {
    case "boolean":
      return value ? "true" : "false"
    case "number":
      if (!Number.isFinite(value)) {
        throw new Error("Canonical JSON does not allow non-finite numbers")
      }
      return JSON.stringify(value)
    case "string":
      return JSON.stringify(value)
    case "object": {
      const keys = Object.keys(value).sort()
      return `{${keys.map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`
    }
    default:
      throw new Error(`Canonical JSON does not support ${typeof value}`)
  }
}

function buildWasmRequest(moduleHex, providerId) {
  const moduleBytes = Buffer.from(moduleHex, "hex")
  const inputValue = null
  return {
    provider: { provider_id: providerId },
    offer_id: "execute.wasm",
    kind: "wasm",
    submission: {
      schema_version: "froglet/v1",
      submission_type: "wasm_submission",
      workload: {
        schema_version: "froglet/v1",
        workload_kind: "compute.wasm.v1",
        abi_version: "froglet.wasm.run_json.v1",
        module_format: "application/wasm",
        module_hash: sha256Hex(moduleBytes),
        input_format: "application/json+jcs",
        input_hash: sha256Hex(Buffer.from(canonicalJson(inputValue), "utf8")),
        requested_capabilities: []
      },
      module_bytes_hex: moduleHex,
      input: inputValue
    }
  }
}

function extractJsonSection(text, label) {
  const marker = `${label}\n`
  const index = text.indexOf(marker)
  assert.notEqual(index, -1, `missing ${label} in tool output`)
  return JSON.parse(text.slice(index + marker.length))
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
  const [packageJson, pluginManifest, exampleConfig, moduleHex] = await Promise.all([
    readFile(path.join(pluginDir, "package.json"), "utf8").then(JSON.parse),
    readFile(path.join(pluginDir, "openclaw.plugin.json"), "utf8").then(JSON.parse),
    readFile(path.join(pluginDir, "examples/openclaw.config.example.json"), "utf8").then(JSON.parse),
    readFile(path.join(testDir, "fixtures/valid-wasm.hex"), "utf8").then((value) => value.trim())
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
  pluginConfig.runtimeAuthTokenPath = path.join(repoRoot, "data/runtime/auth.token")
  for (const [key, value] of Object.entries(pluginConfig)) {
    assert.ok(schemaProperties[key], `unknown plugin config key ${key}`)
    assertConfigValueMatchesSchema(key, value, schemaProperties[key])
  }

  const extension = packageJson.openclaw?.extensions?.[0]
  assert.equal(typeof extension, "string")
  const registerModule = await import(pathToFileURL(path.join(pluginDir, extension)).href)
  assert.equal(typeof registerModule.default, "function")

  return {
    moduleHex,
    pluginConfig,
    register: registerModule.default
  }
}

async function main() {
  const { moduleHex, pluginConfig, register } = await loadPluginFromPackageMetadata()

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

  const expectedTools = [
    "froglet_search",
    "froglet_get_provider",
    "froglet_buy",
    "froglet_mock_pay",
    "froglet_wait_deal",
    "froglet_payment_intent",
    "froglet_accept_result",
    "froglet_wallet_balance"
  ]
  for (const toolName of expectedTools) {
    assert.ok(tools.has(toolName), `missing tool ${toolName}`)
  }

  const invalidTokenDir = await mkdtemp(path.join(os.tmpdir(), "froglet-openclaw-smoke-"))
  try {
    const invalidTokenPath = path.join(invalidTokenDir, "auth.token")
    await writeFile(invalidTokenPath, "invalid-runtime-token\n", "utf8")
    await assert.rejects(
      tools.get("froglet_wallet_balance").definition.execute("wallet", {
        runtime_auth_token_path: invalidTokenPath
      }),
      /failed with 401/
    )
  } finally {
    await rm(invalidTokenDir, { recursive: true, force: true })
  }

  const wallet = await tools.get("froglet_wallet_balance").definition.execute("wallet", {
    include_raw: true
  })
  const walletRaw = extractJsonSection(wallet.content[0].text, "wallet_balance_response_json:")
  assert.equal(walletRaw.backend, "lightning")

  let search
  let providerId = null
  for (let attempt = 0; attempt < 15; attempt += 1) {
    search = await tools.get("froglet_search").definition.execute("search", {
      limit: 5,
      include_raw: true
    })
    const raw = extractJsonSection(search.content[0].text, "search_response_json:")
    const first = Array.isArray(raw.nodes) ? raw.nodes[0] : null
    providerId = first?.descriptor?.node_id ?? null
    if (providerId) {
      break
    }
    await delay(1000)
  }
  assert.ok(providerId, "runtime search did not return a provider")

  const provider = await tools.get("froglet_get_provider").definition.execute("provider", {
    provider_id: providerId,
    include_raw: true
  })
  const providerRaw = extractJsonSection(provider.content[0].text, "provider_response_json:")
  assert.equal(providerRaw.descriptor.payload.provider_id, providerId)
  assert.ok(
    providerRaw.offers.some((offer) => offer?.payload?.offer_id === "execute.wasm"),
    "provider does not advertise execute.wasm"
  )

  const buy = await tools.get("froglet_buy").definition.execute("buy", {
    request: {
      ...buildWasmRequest(moduleHex, providerId),
      idempotency_key: "compose-smoke-runtime-buy"
    },
    include_raw: true
  })
  const buyRaw = extractJsonSection(buy.content[0].text, "buy_response_json:")
  assert.equal(buyRaw.deal.provider_id, providerId)
  assert.equal(buyRaw.deal.status, "payment_pending")
  assert.ok(buyRaw.payment_intent_path)

  const waited = await tools.get("froglet_wait_deal").definition.execute("wait", {
    deal_id: buyRaw.deal.deal_id,
    wait_statuses: ["payment_pending"],
    timeout_secs: 5,
    poll_interval_secs: 0.2,
    include_raw: true
  })
  const waitRaw = extractJsonSection(waited.content[0].text, "wait_response_json:")
  assert.equal(waitRaw.deal.status, "payment_pending")

  const paymentIntent = await tools.get("froglet_payment_intent").definition.execute("payment", {
    deal_id: buyRaw.deal.deal_id,
    include_raw: true
  })
  const paymentIntentRaw = extractJsonSection(
    paymentIntent.content[0].text,
    "payment_intent_response_json:"
  )
  assert.equal(paymentIntentRaw.payment_intent.deal_id, buyRaw.deal.deal_id)
  assert.equal(paymentIntentRaw.payment_intent.backend, "lightning")

  if (paymentIntentRaw.payment_intent.mock_action?.endpoint_path) {
    const mockPay = await tools.get("froglet_mock_pay").definition.execute("mock-pay", {
      deal_id: buyRaw.deal.deal_id,
      include_raw: true
    })
    const mockPayRaw = extractJsonSection(mockPay.content[0].text, "mock_pay_response_json:")
    assert.equal(mockPayRaw.deal.deal_id, buyRaw.deal.deal_id)

    const settled = await tools.get("froglet_wait_deal").definition.execute("wait-after-mock", {
      deal_id: buyRaw.deal.deal_id,
      wait_statuses: ["result_ready", "succeeded", "failed", "rejected"],
      timeout_secs: 15,
      poll_interval_secs: 0.2,
      include_raw: true
    })
    const settledRaw = extractJsonSection(settled.content[0].text, "wait_response_json:")
    assert.ok(
      ["result_ready", "succeeded", "failed", "rejected"].includes(settledRaw.deal.status),
      `unexpected post-mock status ${settledRaw.deal.status}`
    )

    if (settledRaw.deal.status === "result_ready") {
      const accepted = await tools.get("froglet_accept_result").definition.execute("accept", {
        deal_id: buyRaw.deal.deal_id,
        include_raw: true
      })
      const acceptedRaw = extractJsonSection(accepted.content[0].text, "accept_response_json:")
      assert.equal(acceptedRaw.deal.status, "succeeded")
    }
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
