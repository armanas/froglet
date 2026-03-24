import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

import register from "../index.js"

const pluginRoot = fileURLToPath(new URL("..", import.meta.url))
const exampleFiles = [
  "openclaw.config.example.json",
  "openclaw.config.nemoclaw.example.json",
  "openclaw.config.nemoclaw.hosted.example.json"
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

test("checked-in examples parse and register the single froglet tool", async () => {
  for (const fileName of exampleFiles) {
    const document = JSON.parse(
      await readFile(path.join(pluginRoot, "examples", fileName), "utf8")
    )
    const pluginConfig = extractFrogletConfig(document)
    assert.deepEqual(
      buildToolList(pluginConfig),
      ["froglet"],
      `${fileName} must register only the froglet tool`
    )
  }
})
