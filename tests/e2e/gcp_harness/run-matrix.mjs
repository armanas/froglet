import assert from "node:assert/strict"
import { readFileSync } from "node:fs"

import {
  executeTool,
  getJsonPath,
  loadFrogletTool,
  normalizeResultValue,
  parseCliArgs,
  readJson,
  writeJson,
} from "./common.mjs"

function subsetMatch(candidate, expected) {
  if (expected == null || typeof expected !== "object" || Array.isArray(expected)) {
    return candidate === expected
  }
  if (candidate == null || typeof candidate !== "object" || Array.isArray(candidate)) {
    return false
  }
  return Object.entries(expected).every(([key, value]) => subsetMatch(candidate[key], value))
}

function assertRawAssertions(raw, assertions, context) {
  for (const assertion of assertions ?? []) {
    const value = getJsonPath(raw, assertion.path)
    if (Object.hasOwn(assertion, "exists")) {
      assert.equal(value !== undefined, assertion.exists, `raw path ${assertion.path} existence`)
    }
    if (Object.hasOwn(assertion, "equals")) {
      assert.deepEqual(value, assertion.equals, `raw path ${assertion.path} equality`)
    }
    if (Object.hasOwn(assertion, "equals_context")) {
      assert.deepEqual(
        value,
        context[assertion.equals_context],
        `raw path ${assertion.path} equality against context ${assertion.equals_context}`
      )
    }
    if (Object.hasOwn(assertion, "contains")) {
      assert.ok(Array.isArray(value), `raw path ${assertion.path} must be an array for contains`)
      assert.ok(
        value.some((entry) => subsetMatch(entry, assertion.contains)),
        `raw path ${assertion.path} did not contain ${JSON.stringify(assertion.contains)}`
      )
    }
  }
}

function assertText(text, oracles) {
  for (const needle of oracles.text_contains ?? []) {
    assert.ok(text.includes(needle), `missing text content ${needle}`)
  }
  for (const needle of oracles.text_not_contains ?? []) {
    assert.ok(!text.includes(needle), `unexpected text content ${needle}`)
  }
}

function deepResolve(value, context, fixtures) {
  if (Array.isArray(value)) {
    return value.map((entry) => deepResolve(entry, context, fixtures))
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [key, deepResolve(entry, context, fixtures)])
    )
  }
  if (typeof value !== "string") {
    return value
  }
  if (value === "__fixture_valid_wasm_hex") {
    return fixtures.validWasmHex
  }
  if (value.startsWith("__context_")) {
    const key = value.slice("__context_".length)
    const resolved = context[key]
    if (resolved === undefined) {
      throw new Error(`missing required context value ${key}`)
    }
    return resolved
  }
  return value
}

async function ensurePublishArtifact(tool, request) {
  try {
    await executeTool(tool, request)
  } catch (error) {
    const message = String(error.message)
    if (!message.includes("409") && !message.includes("already exists")) {
      throw error
    }
  }
}

async function bootstrapMarketplaceFixtures(tool, bootstrap) {
  await ensurePublishArtifact(tool, {
    action: "publish_artifact",
    service_id: bootstrap.local_static_service_id,
    summary: "GCP harness local static service",
    runtime: "python",
    package_kind: "inline_source",
    entrypoint_kind: "handler",
    entrypoint: "handler",
    contract_version: "froglet.python.handler_json.v1",
    inline_source:
      "def handler(event, context):\n    return {\"message\": \"market-local\"}\n",
    price_sats: 0,
    publication_state: "active",
  })
  await ensurePublishArtifact(tool, {
    action: "publish_artifact",
    service_id: bootstrap.local_hidden_service_id,
    summary: "GCP harness local hidden service",
    runtime: "python",
    package_kind: "inline_source",
    entrypoint_kind: "handler",
    entrypoint: "handler",
    contract_version: "froglet.python.handler_json.v1",
    inline_source:
      "def handler(event, context):\n    return {\"hidden\": True, \"input\": event}\n",
    price_sats: 0,
    publication_state: "hidden",
  })
}

async function runScenario(toolByProfile, scenario, context, fixtures) {
  const authProfile = scenario.auth_profile ?? "provider_control"
  const tool = toolByProfile[authProfile]
  if (!tool) {
    throw new Error(`missing tool for auth profile ${authProfile}`)
  }

  const startedAt = Date.now()
  try {
    const args = deepResolve(scenario.fixture_injections, context, fixtures)
    const { text, raw } = await executeTool(tool, args)
    if (scenario.result_oracles?.expect_error === true) {
      throw new Error(`scenario ${scenario.scenario_id} unexpectedly succeeded`)
    }
    assertText(text, scenario.result_oracles ?? {})
    assertRawAssertions(raw, scenario.result_oracles?.raw_assertions, context)
    if (scenario.store_context) {
      const storedValue = getJsonPath(raw, scenario.store_context.path)
      if (storedValue === undefined) {
        throw new Error(
          `scenario ${scenario.scenario_id} did not produce context path ${scenario.store_context.path}`
        )
      }
      context[scenario.store_context.key] = normalizeResultValue(storedValue)
    }
    return {
      scenario_id: scenario.scenario_id,
      action: scenario.action,
      case: scenario.case,
      auth_profile: authProfile,
      status: "passed",
      elapsed_ms: Date.now() - startedAt,
      output_text: text,
      raw,
    }
  } catch (error) {
    const message = String(error.message ?? error)
    if (scenario.result_oracles?.expect_error === true) {
      for (const needle of scenario.result_oracles.error_contains ?? []) {
        if (!message.includes(needle)) {
          return {
            scenario_id: scenario.scenario_id,
            action: scenario.action,
            case: scenario.case,
            auth_profile: authProfile,
            status: "failed",
            elapsed_ms: Date.now() - startedAt,
            expected_error: true,
            error: `error message missing ${needle}: ${message}`,
          }
        }
      }
      return {
        scenario_id: scenario.scenario_id,
        action: scenario.action,
        case: scenario.case,
        auth_profile: authProfile,
        status: "passed",
        elapsed_ms: Date.now() - startedAt,
        expected_error: true,
        error: message,
      }
    }
    return {
      scenario_id: scenario.scenario_id,
      action: scenario.action,
      case: scenario.case,
      auth_profile: authProfile,
      status: "failed",
      elapsed_ms: Date.now() - startedAt,
      error: message,
    }
  }
}

async function main() {
  const { values } = parseCliArgs({
    inventory: { type: "string", short: "i" },
    scenarios: { type: "string", short: "s" },
    "provider-url": { type: "string" },
    "runtime-url": { type: "string" },
    "base-url": { type: "string" },
    "provider-token": { type: "string" },
    "runtime-token": { type: "string" },
    "consumer-token": { type: "string" },
    "bogus-token": { type: "string" },
    out: { type: "string", short: "o" },
  })
  const providerUrl = values["provider-url"] ?? values["base-url"]
  const runtimeUrl = values["runtime-url"] ?? values["base-url"]
  if (
    !values.inventory ||
    !values.scenarios ||
    !providerUrl ||
    !runtimeUrl ||
    !values["provider-token"] ||
    !values["runtime-token"] ||
    !values["consumer-token"] ||
    !values["bogus-token"] ||
    !values.out
  ) {
    throw new Error(
      "--inventory, --scenarios, --out, --provider-token, --runtime-token, --consumer-token, --bogus-token, and either split --provider-url/--runtime-url or legacy --base-url are required"
    )
  }

  const [inventory, scenarioSet] = await Promise.all([
    readJson(values.inventory),
    readJson(values.scenarios),
  ])
  const toolByProfile = {
    provider_control: loadFrogletTool({
      providerUrl,
      runtimeUrl,
      providerAuthTokenPath: values["provider-token"],
      runtimeAuthTokenPath: values["runtime-token"],
    }),
    consumer_control: loadFrogletTool({
      providerUrl,
      runtimeUrl,
      providerAuthTokenPath: values["consumer-token"],
      runtimeAuthTokenPath: values["runtime-token"],
    }),
    bogus: loadFrogletTool({
      providerUrl,
      runtimeUrl,
      providerAuthTokenPath: values["bogus-token"],
      runtimeAuthTokenPath: values["bogus-token"],
    }),
  }

  const fixtures = {
    validWasmHex: readFileSync(
      new URL("../../../integrations/openclaw/froglet/test/fixtures/valid-wasm.hex", import.meta.url),
      "utf8"
    ).trim(),
  }
  const context = {
    run_id: inventory.run_id,
  }

  await bootstrapMarketplaceFixtures(toolByProfile.provider_control, scenarioSet.bootstrap)

  const toolScenarios = (scenarioSet.scenarios ?? []).filter((scenario) => scenario.runner === "tool")
  const results = []
  for (const scenario of toolScenarios) {
    results.push(await runScenario(toolByProfile, scenario, context, fixtures))
  }

  const failed = results.filter((result) => result.status === "failed")
  const summary = {
    generated_at: new Date().toISOString(),
    run_id: inventory.run_id,
    provider_url: providerUrl,
    runtime_url: runtimeUrl,
    total: results.length,
    passed: results.length - failed.length,
    failed: failed.length,
    context,
    results,
  }
  await writeJson(values.out, summary)

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
