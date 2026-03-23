import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import path from "node:path"
import test from "node:test"

import register from "../index.js"

const pluginRoot = path.resolve(import.meta.dirname, "..")
const exampleFiles = [
  "openclaw.config.example.json",
  "openclaw.config.nemoclaw.example.json",
  "openclaw.config.nemoclaw.hosted.example.json"
]
const expectedTools = [
  "froglet_accept_result",
  "froglet_buy",
  "froglet_events_query",
  "froglet_get_provider",
  "froglet_mock_pay",
  "froglet_payment_intent",
  "froglet_search",
  "froglet_wait_deal",
  "froglet_wallet_balance"
]

function buildToolList(config) {
  const tools = new Map()
  register({
    config,
    registerTool(definition, options) {
      tools.set(definition.name, { definition, options: options ?? {} })
    },
    logger: { info() {} }
  })
  return [...tools.keys()].sort()
}

function extractFrogletConfig(document) {
  return document.plugins.entries.froglet.config
}

test("all checked-in profile examples use the same Froglet plugin key schema", async () => {
  const schema = JSON.parse(
    await readFile(path.join(pluginRoot, "openclaw.plugin.json"), "utf8")
  )
  const requiredKeys = [...schema.configSchema.required].sort()
  const allowedKeys = Object.keys(schema.configSchema.properties).sort()
  const seenKeys = []

  for (const fileName of exampleFiles) {
    const document = JSON.parse(
      await readFile(path.join(pluginRoot, "examples", fileName), "utf8")
    )
    const pluginConfig = extractFrogletConfig(document)
    const keys = Object.keys(pluginConfig).sort()
    seenKeys.push(keys)
    assert.deepEqual(
      requiredKeys.filter((key) => !(key in pluginConfig)),
      [],
      `${fileName} is missing required Froglet plugin keys`
    )
    assert.deepEqual(
      keys.filter((key) => !allowedKeys.includes(key)),
      [],
      `${fileName} contains unsupported Froglet plugin keys`
    )
  }

  for (const keys of seenKeys) {
    assert.deepEqual(keys, seenKeys[0], "profile examples must use the same Froglet-owned keys")
  }
})

test("all checked-in profile examples register the same Froglet tool inventory", async () => {
  for (const fileName of exampleFiles) {
    const document = JSON.parse(
      await readFile(path.join(pluginRoot, "examples", fileName), "utf8")
    )
    assert.deepEqual(
      buildToolList(extractFrogletConfig(document)),
      expectedTools,
      `${fileName} must register the shared Froglet tool inventory`
    )
  }
})
