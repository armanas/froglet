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
const defaultProviderUrl = process.env.FROGLET_PROVIDER_URL ?? "http://127.0.0.1:8080"
const validWasmHex =
  "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432"

function getTextResult(result) {
  const text = result?.content?.find?.((item) => item?.type === "text")?.text
  assert.equal(typeof text, "string", "missing text tool result")
  return text
}

function assertContainsAll(text, needles, message = "missing expected text") {
  for (const needle of needles) {
    assert.ok(String(text).includes(needle), `${message}: ${needle}`)
  }
}

function extractField(text, key) {
  const match = String(text).match(new RegExp(`^${key}:\\s+(.+)$`, "m"))
  return match?.[1]
}

async function callToolText(client, name, args = {}) {
  return getTextResult(await client.callTool({ name, arguments: args }))
}

async function waitForDiscovery(client, serviceId, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs
  let lastText = ""
  let lastError = null

  while (Date.now() < deadline) {
    try {
      lastText = await callToolText(client, "froglet_discover", {
        query: serviceId,
        limit: 10
      })
      lastError = null
      if (lastText.includes(`service_id: ${serviceId}`)) {
        return lastText
      }
    } catch (error) {
      lastError = error
      lastText = `error: ${error?.message ?? String(error)}`
    }
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }

  throw new Error(
    `service ${serviceId} did not appear in discovery\n${lastText}${lastError ? `\nlast_error: ${lastError.message}` : ""}`
  )
}

async function waitForHealthyStatus(client, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs
  let lastText = ""
  let lastError = null

  while (Date.now() < deadline) {
    try {
      lastText = await callToolText(client, "froglet_status")
      lastError = null
      if (
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
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [serverPath],
    cwd: packageDir,
    env: {
      FROGLET_BASE_URL: process.env.FROGLET_BASE_URL ?? "http://127.0.0.1:9191",
      FROGLET_AUTH_TOKEN_PATH:
        process.env.FROGLET_AUTH_TOKEN_PATH ??
        path.join(repoRoot, "data/runtime/froglet-control.token"),
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
    assert.deepEqual(names, [
      "froglet_status",
      "froglet_logs",
      "froglet_discover",
      "froglet_get_service",
      "froglet_invoke",
      "froglet_local_services",
      "froglet_project",
      "froglet_task",
      "froglet_compute"
    ])

    const statusText = await waitForHealthyStatus(client)
    assertContainsAll(statusText, ["discovery_mode:"], "missing discovery state")

    const discoveryEnabled = statusText.includes("reference_discovery_enabled: true")
    if (!discoveryEnabled && process.env.FROGLET_ALLOW_DISCOVERY_DISABLED !== "1") {
      throw new Error(
        `compose smoke requires reference discovery enabled\n${statusText}`
      )
    }

    const serviceId = `mcp-compose-smoke-ping-${Date.now()}`
    const createText = await callToolText(client, "froglet_project", {
      action: "create",
      name: serviceId,
      summary: "Returns pong for the MCP compose smoke",
      price_sats: 0,
      publication_state: "active",
      result_json: { message: "pong" }
    })
    assertContainsAll(createText, [
      `project_id: ${serviceId}`,
      `service_id: ${serviceId}`,
      "publication_state: active",
      "published: true",
      `published_service_id: ${serviceId}`
    ])

    const testText = await callToolText(client, "froglet_project", {
      action: "test",
      project_id: serviceId,
      input: { source: "compose-smoke-test" }
    })
    assertContainsAll(
      testText,
      [`project_id: ${serviceId}`, 'output: {"message":"pong"}'],
      "unexpected project test result"
    )

    const localListText = await callToolText(client, "froglet_local_services")
    assertContainsAll(localListText, [`service_id: ${serviceId}`], "missing published local service")

    const localServiceText = await callToolText(client, "froglet_local_services", {
      service_id: serviceId
    })
    assertContainsAll(localServiceText, [
      `service_id: ${serviceId}`,
      "publication_state: active",
      "offer_kind:",
      "resource_kind:"
    ])
    const providerId = extractField(localServiceText, "provider_id")
    assert.equal(typeof providerId, "string", "missing provider_id in local service output")

    if (discoveryEnabled) {
      const discoverText = await waitForDiscovery(client, serviceId)
      assertContainsAll(discoverText, ["provider_nodes_discovered:"], "missing discovery response")
      assertContainsAll(discoverText, [`service_id: ${serviceId}`])
    }

    const invokeText = await callToolText(client, "froglet_invoke", {
      provider_id: providerId,
      service_id: serviceId,
      input: { source: "compose-smoke" }
    })
    if (invokeText.includes("terminal: false")) {
      const taskId = extractField(invokeText, "task_id")
      assert.equal(typeof taskId, "string", "missing invoke task_id")
      const waitText = await callToolText(client, "froglet_task", {
        task_id: taskId,
        wait: true,
        timeout_secs: 30
      })
      assertContainsAll(waitText, ['status: succeeded', 'result: {"message":"pong"}'], "unexpected invoke task result")
    } else {
      assertContainsAll(
        invokeText,
        ['status: succeeded', 'result: {"message":"pong"}'],
        "unexpected invoke result"
      )
    }

    const computeText = await callToolText(client, "froglet_compute", {
      provider_url: defaultProviderUrl,
      runtime: "wasm",
      package_kind: "inline_module",
      wasm_module_hex: validWasmHex
    })
    if (computeText.includes("terminal: false")) {
      const taskId = extractField(computeText, "task_id")
      assert.equal(typeof taskId, "string", "missing compute task_id")
      const waitText = await callToolText(client, "froglet_task", {
        task_id: taskId,
        wait: true,
        timeout_secs: 30
      })
      assertContainsAll(waitText, ["result: 42"], "unexpected compute task result")
    } else {
      assertContainsAll(
        computeText,
        ["status: succeeded", "result: 42"],
        "unexpected compute result"
      )
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
