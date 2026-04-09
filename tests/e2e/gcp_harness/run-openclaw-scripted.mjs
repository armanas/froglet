import assert from "node:assert/strict"
import { readFileSync } from "node:fs"

import {
  executeTool,
  loadFrogletTool,
  parseCliArgs,
  readJson,
  writeJson,
} from "./common.mjs"

function extractNeedles(value, needles, context) {
  const haystacks = [
    JSON.stringify(value),
    JSON.stringify(context),
  ]
  for (const needle of needles ?? []) {
    assert.ok(
      haystacks.some((haystack) => haystack.includes(needle)),
      `missing ${needle}`
    )
  }
}

async function ensureTerminalTask(tool, response, context) {
  if (response?.raw?.terminal !== false) {
    return response
  }
  const taskId = response?.raw?.task?.task_id ?? response?.raw?.deal?.deal_id
  assert.ok(taskId, "pending response did not expose task_id")
  context.task_id = taskId
  return executeTool(tool, {
    action: "wait_task",
    task_id: taskId,
    timeout_secs: 30,
    poll_interval_secs: 1,
    include_raw: true,
  })
}

async function runRemoteServiceScenario(tool, scenario, context) {
  const providerId = scenario.fixture_injections.provider_id
  const serviceId = scenario.fixture_injections.service_id

  const discovered = await executeTool(tool, {
    action: "discover_services",
    query: serviceId,
    include_raw: true,
  })
  const services = discovered.raw?.services ?? []
  const service = services.find(
    (entry) => entry.service_id === serviceId && entry.provider_id === providerId
  )
  assert.ok(service, `discover_services did not return ${providerId}/${serviceId}`)

  const detail = await executeTool(tool, {
    action: "get_service",
    service_id: serviceId,
    provider_id: providerId,
    provider_url: service.provider_url,
    include_raw: true,
  })
  assert.equal(detail.raw?.service?.service_id, serviceId)

  const invoked = await executeTool(tool, {
    action: "invoke_service",
    service_id: serviceId,
    provider_id: providerId,
    provider_url: service.provider_url,
    input: { delay_ms: 25, marker: "openclaw-remote" },
    include_raw: true,
  })
  const terminal = await ensureTerminalTask(tool, invoked, context)
  const result = terminal.raw?.result ?? terminal.raw?.task?.result ?? terminal.raw?.deal?.result
  assert.equal(result?.async, true)
  assert.equal(result?.echo?.marker, "openclaw-remote")
  extractNeedles({ discovered: discovered.raw, detail: detail.raw, terminal: terminal.raw }, scenario.result_oracles?.must_contain, context)

  return {
    scenario_id: scenario.scenario_id,
    status: "passed",
    steps: [discovered.raw, detail.raw, invoked.raw, terminal.raw],
  }
}

async function runDirectComputeScenario(tool, scenario, fixtures, context) {
  const providerId = scenario.fixture_injections.provider_id

  const wasm = await executeTool(tool, {
    action: "run_compute",
    provider_id: providerId,
    runtime: "wasm",
    package_kind: "inline_module",
    contract_version: "froglet.wasm.run_json.v1",
    wasm_module_hex: fixtures.validWasmHex,
    input: { value: 42 },
    timeout_secs: scenario.fixture_injections.timeout_secs ?? 15,
    include_raw: true,
  })
  const wasmTerminal = await ensureTerminalTask(tool, wasm, context)
  const wasmResult =
    wasmTerminal.raw?.result ??
    wasmTerminal.raw?.task?.result ??
    wasmTerminal.raw?.deal?.result
  assert.equal(wasmResult, 42)

  const python = await executeTool(tool, {
    action: "run_compute",
    provider_id: providerId,
    runtime: "python",
    package_kind: "inline_source",
    entrypoint_kind: "handler",
    entrypoint: "handler",
    contract_version: "froglet.python.handler_json.v1",
    inline_source:
      "def handler(event, context):\n    return {\"marker\":\"processed by python handler\",\"input\":event}\n",
    input: { value: 7 },
    timeout_secs: scenario.fixture_injections.timeout_secs ?? 15,
    include_raw: true,
  })
  const pythonTerminal = await ensureTerminalTask(tool, python, context)
  const pythonResult =
    pythonTerminal.raw?.result ??
    pythonTerminal.raw?.task?.result ??
    pythonTerminal.raw?.deal?.result
  assert.equal(pythonResult?.marker, "processed by python handler")
  assert.equal(pythonResult?.input?.value, 7)

  extractNeedles(
    { wasm: wasmTerminal.raw, python: pythonTerminal.raw },
    scenario.result_oracles?.must_contain,
    context
  )

  return {
    scenario_id: scenario.scenario_id,
    status: "passed",
    steps: [wasm.raw, wasmTerminal.raw, python.raw, pythonTerminal.raw],
  }
}

async function main() {
  const { values } = parseCliArgs({
    inventory: { type: "string", short: "i" },
    scenarios: { type: "string", short: "s" },
    "provider-url": { type: "string" },
    "runtime-url": { type: "string" },
    "provider-token": { type: "string" },
    "runtime-token": { type: "string" },
    out: { type: "string", short: "o" },
  })
  if (
    !values.inventory ||
    !values.scenarios ||
    !values["provider-url"] ||
    !values["runtime-url"] ||
    !values["provider-token"] ||
    !values["runtime-token"] ||
    !values.out
  ) {
    throw new Error(
      "--inventory, --scenarios, --provider-url, --runtime-url, --provider-token, --runtime-token, and --out are required"
    )
  }

  const [inventory, scenarioSet] = await Promise.all([
    readJson(values.inventory),
    readJson(values.scenarios),
  ])
  const tool = loadFrogletTool({
    providerUrl: values["provider-url"],
    runtimeUrl: values["runtime-url"],
    providerAuthTokenPath: values["provider-token"],
    runtimeAuthTokenPath: values["runtime-token"],
    requestTimeoutMs: 20_000,
  })
  const fixtures = {
    validWasmHex: readFileSync(
      new URL("../../../integrations/openclaw/froglet/test/fixtures/valid-wasm.hex", import.meta.url),
      "utf8"
    ).trim(),
  }
  const context = { run_id: inventory.run_id }
  const results = []

  for (const scenario of scenarioSet.openclaw?.scripted ?? []) {
    try {
      if (scenario.flow === "remote_service") {
        results.push(await runRemoteServiceScenario(tool, scenario, context))
      } else if (scenario.flow === "direct_compute") {
        results.push(await runDirectComputeScenario(tool, scenario, fixtures, context))
      } else {
        throw new Error(`unsupported scripted flow ${scenario.flow}`)
      }
    } catch (error) {
      results.push({
        scenario_id: scenario.scenario_id,
        status: "failed",
        error: String(error.message ?? error),
      })
    }
  }

  const failed = results.filter((entry) => entry.status === "failed")
  await writeJson(values.out, {
    generated_at: new Date().toISOString(),
    run_id: inventory.run_id,
    total: results.length,
    passed: results.length - failed.length,
    failed: failed.length,
    results,
  })
  if (failed.length > 0) {
    for (const failure of failed) {
      console.error(`${failure.scenario_id}: ${failure.error}`)
    }
    process.exitCode = 1
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
