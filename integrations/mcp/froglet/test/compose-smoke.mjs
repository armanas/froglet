import assert from "node:assert/strict"
import path from "node:path"
import process from "node:process"
import { fileURLToPath } from "node:url"

import { Client } from "@modelcontextprotocol/sdk/client/index.js"
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js"

const testDir = fileURLToPath(new URL("./", import.meta.url))
const packageDir = path.resolve(testDir, "..")
const repoRoot = path.resolve(packageDir, "../../..")
const serverPath = path.join(packageDir, "server.js")
const validWasmHex =
  "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432"

function getTextResult(result) {
  const text = result?.content?.find?.((item) => item?.type === "text")?.text
  assert.equal(typeof text, "string", "missing text tool result")
  return text
}

function assertContainsAll(text, needles, message = "missing expected text") {
  for (const needle of needles) {
    if (!String(text).includes(needle)) {
      // Dump the full text on assertion failure so the CI log shows what we
      // actually got. Silent "unexpected X: Y" failures are impossible to
      // debug from logs otherwise.
      console.error(`--- assertContainsAll failed ---\nneedle: ${needle}\nactual text:\n${text}\n--- end ---`)
      assert.ok(false, `${message}: ${needle}`)
    }
  }
}

function extractField(text, key) {
  const match = String(text).match(new RegExp(`^${key}:\\s+(.+)$`, "m"))
  return match?.[1]
}

function parseMaybeJson(value) {
  if (typeof value !== "string") {
    return value
  }
  try {
    return JSON.parse(value)
  } catch {
    return value
  }
}

function assertOptionalResult(text, expected, label) {
  const resultText = extractField(text, "result") ?? extractField(text, "output")
  if (resultText === undefined) {
    assertContainsAll(text, ["status: succeeded"], label)
    return
  }
  assert.deepEqual(parseMaybeJson(resultText), expected, label)
}

async function callToolText(client, name, args = {}) {
  return getTextResult(await client.callTool({ name, arguments: args }))
}

async function waitForHealthyStatus(client, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs
  let lastText = ""
  let lastError = null

  while (Date.now() < deadline) {
    try {
      lastText = await callToolText(client, "froglet", { action: "status" })
      lastError = null
      if (
        lastText.includes("healthy: true") &&
        lastText.includes("runtime_healthy: true") &&
        lastText.includes("provider_healthy: true")
      ) {
        return lastText
      }
    } catch (error) {
      lastError = error
      lastText = `error: ${error?.message ?? String(error)}`
    }
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }

  throw new Error(
    `froglet status did not become healthy\n${lastText}${lastError ? `\nlast_error: ${lastError.message}` : ""}`
  )
}

async function main() {
  const providerUrl = process.env.FROGLET_PROVIDER_URL ?? "http://127.0.0.1:8080"
  const runtimeUrl = process.env.FROGLET_RUNTIME_URL ?? "http://127.0.0.1:8081"
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [serverPath],
    cwd: packageDir,
    env: {
      FROGLET_PROVIDER_URL: providerUrl,
      FROGLET_RUNTIME_URL: runtimeUrl,
      FROGLET_PROVIDER_AUTH_TOKEN_PATH:
        process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH ??
        path.join(repoRoot, "data/runtime/froglet-control.token"),
      FROGLET_RUNTIME_AUTH_TOKEN_PATH:
        process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH ??
        path.join(repoRoot, "data/runtime/auth.token"),
      FROGLET_REQUEST_TIMEOUT_MS: process.env.FROGLET_REQUEST_TIMEOUT_MS ?? "10000"
    },
    stderr: "pipe"
  })
  const stderrChunks = []
  transport.stderr?.setEncoding("utf8")
  transport.stderr?.on("data", (chunk) => {
    stderrChunks.push(chunk)
  })

  const client = new Client(
    { name: "froglet-compose-smoke", version: "0.1.0" },
    { capabilities: {} }
  )

  try {
    await client.connect(transport)

    const tools = await client.listTools()
    const names = tools.tools.map((tool) => tool.name)
    assert.deepEqual(names, ["froglet"])

    const statusText = await waitForHealthyStatus(client)
    assertContainsAll(statusText, ["healthy: true"], "missing healthy state")
    const providerId = extractField(statusText, "node_id")
    assert.equal(typeof providerId, "string", "status should expose node_id")

    // Verify local service listing works
    const localListText = await callToolText(client, "froglet", {
      action: "list_local_services"
    })
    assert.ok(typeof localListText === "string", "list_local_services should return text")

    // Verify run_compute works against the operator-configured local provider.
    // This stack has no marketplace, so provider_id alone would force a
    // runtime lookup that cannot succeed. The MCP server must pair the local
    // provider_id with its trusted operator-configured provider URL instead of
    // requiring an LLM-supplied provider_url override.
    const computeText = await callToolText(client, "froglet", {
      action: "run_compute",
      provider_id: providerId,
      runtime: "wasm",
      package_kind: "inline_module",
      wasm_module_hex: validWasmHex
    })
    if (computeText.includes("terminal: false")) {
      const taskId = extractField(computeText, "task_id")
      assert.equal(typeof taskId, "string", "missing compute task_id")
      const waitText = await callToolText(client, "froglet", {
        action: "wait_task",
        task_id: taskId,
        timeout_secs: 30
      })
      assertContainsAll(waitText, ["result: 42"], "unexpected compute task result")
    } else {
      assertContainsAll(computeText, ["status: succeeded"], "unexpected compute result")
      assertOptionalResult(computeText, 42, "unexpected compute result payload")
    }
  } catch (error) {
    const stderr = stderrChunks.join("").trim()
    if (stderr) {
      console.error(stderr)
    }
    throw error
  } finally {
    await transport.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
