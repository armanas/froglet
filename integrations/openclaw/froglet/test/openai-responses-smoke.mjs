import path from "node:path"
import { readFileSync } from "node:fs"
import { fileURLToPath, pathToFileURL } from "node:url"

import {
  executeTool,
  loadFrogletTool,
  parseCliArgs,
  requireApiKey,
  writeJson,
} from "../../../../tests/e2e/gcp_harness/common.mjs"
import { runCuratedSuite } from "../../../../tests/e2e/gcp_harness/openclaw-llm-runner.mjs"

const testDir = fileURLToPath(new URL("./", import.meta.url))
const pluginDir = path.resolve(testDir, "..")
const repoRoot = path.resolve(pluginDir, "../../..")
const defaultResultsDir = path.join(repoRoot, "_tmp", "test-results")

export function localResultsPath(fileName) {
  return path.join(
    process.env.FROGLET_TEST_RESULTS_DIR ?? defaultResultsDir,
    fileName
  )
}

function localToolConfig() {
  return {
    providerUrl: process.env.FROGLET_PROVIDER_URL ?? process.env.FROGLET_BASE_URL ?? "http://127.0.0.1:8080",
    runtimeUrl: process.env.FROGLET_RUNTIME_URL ?? "http://127.0.0.1:8081",
    providerAuthTokenPath:
      process.env.FROGLET_PROVIDER_AUTH_TOKEN_PATH ??
      process.env.FROGLET_AUTH_TOKEN_PATH ??
      path.join(repoRoot, "data", "runtime", "froglet-control.token"),
    runtimeAuthTokenPath:
      process.env.FROGLET_RUNTIME_AUTH_TOKEN_PATH ??
      process.env.FROGLET_AUTH_TOKEN_PATH ??
      path.join(repoRoot, "data", "runtime", "auth.token"),
    requestTimeoutMs: Number.parseInt(process.env.FROGLET_REQUEST_TIMEOUT_MS ?? "15000", 10),
    defaultSearchLimit: Number.parseInt(process.env.FROGLET_DEFAULT_SEARCH_LIMIT ?? "10", 10),
    maxSearchLimit: Number.parseInt(process.env.FROGLET_MAX_SEARCH_LIMIT ?? "50", 10),
  }
}

export function loadLocalFrogletTool() {
  return loadFrogletTool(localToolConfig())
}

export function localValidWasmHex() {
  return readFileSync(new URL("./fixtures/valid-wasm.hex", import.meta.url), "utf8").trim()
}

export async function waitForHealthyStatus(tool, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs
  let lastRaw = null
  let lastError = null

  while (Date.now() < deadline) {
    try {
      const status = await executeTool(tool, {
        action: "status",
        include_raw: true,
      })
      lastRaw = status.raw
      lastError = null
      if (
        lastRaw?.healthy === true &&
        lastRaw?.provider?.healthy === true &&
        lastRaw?.runtime?.healthy === true
      ) {
        return lastRaw
      }
    } catch (error) {
      lastError = error
    }
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }

  throw new Error(
    `local OpenClaw stack did not become healthy: ${JSON.stringify(lastRaw)}${lastError ? `; last_error=${lastError.message}` : ""}`
  )
}

export async function waitForDiscovery(tool, serviceId, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs
  let lastRaw = null
  let lastError = null

  while (Date.now() < deadline) {
    try {
      const discover = await executeTool(tool, {
        action: "discover_services",
        query: serviceId,
        limit: 25,
        include_raw: true,
      })
      lastRaw = discover.raw
      lastError = null
      if ((lastRaw?.services ?? []).some((service) => service?.service_id === serviceId)) {
        return lastRaw
      }
    } catch (error) {
      lastError = error
    }
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }

  throw new Error(
    `service ${serviceId} did not appear in marketplace discovery: ${JSON.stringify(lastRaw)}${lastError ? `; last_error=${lastError.message}` : ""}`
  )
}

async function ensurePublished(tool, request) {
  try {
    await executeTool(tool, {
      action: "publish_artifact",
      ...request,
      include_raw: true,
    })
  } catch (error) {
    const message = String(error.message ?? error)
    if (!message.includes("409") && !message.includes("already exists")) {
      throw error
    }
  }
}

export async function bootstrapLocalFixtures(tool, { prefix } = {}) {
  const status = await waitForHealthyStatus(tool)
  const providerId = status.node_id
  const fixturePrefix = prefix ?? `oa-llm-${Date.now()}`
  const localVisibleServiceId = `${fixturePrefix}-visible`
  const localHiddenServiceId = `${fixturePrefix}-hidden`

  await ensurePublished(tool, {
    service_id: localVisibleServiceId,
    offer_id: localVisibleServiceId,
    summary: "Local curated local-only visible service",
    runtime: "python",
    package_kind: "inline_source",
    entrypoint_kind: "handler",
    entrypoint: "handler",
    contract_version: "froglet.python.handler_json.v1",
    inline_source:
      "def handler(event, context):\n    return {\"message\":\"pong\",\"provider\":\"local\",\"input\":event}\n",
    price_sats: 0,
    publication_state: "active",
  })
  await ensurePublished(tool, {
    service_id: localHiddenServiceId,
    offer_id: localHiddenServiceId,
    summary: "Local curated local-only hidden service",
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

  return {
    providerId,
    computeServiceId: "execute.compute",
    queryServiceId: "events.query",
    localVisibleServiceId,
    localHiddenServiceId,
  }
}

export function buildLocalCuratedScenarios(fixtures) {
  return [
    {
      scenario_id: "openclaw.curated.local.status",
      prompt:
        "Use froglet exactly once with action status. Report whether the provider and runtime are healthy and include the node_id.",
      required_tool_actions: ["status"],
      fixture_injections: { action: "status" },
      result_oracles: {
        final_text_contains: ["provider", "runtime"],
        tool_output_assertions: [
          { action: "status", path: "healthy", equals: true },
          { action: "status", path: "provider.healthy", equals: true },
          { action: "status", path: "runtime.healthy", equals: true },
          { action: "status", path: "node_id", equals: fixtures.providerId },
        ],
      },
    },
    {
      scenario_id: "openclaw.curated.local.discovery_visibility",
      prompt:
        `Use froglet to discover services. Confirm that built-in service ${fixtures.computeServiceId} is visible for provider ${fixtures.providerId}. Return the visible service_id and provider_id.`,
      required_tool_actions: ["discover_services"],
      fixture_injections: {
        action: "discover_services",
        limit: 50,
      },
      result_oracles: {
        final_text_contains: [fixtures.computeServiceId, fixtures.providerId],
        tool_output_assertions: [
          {
            action: "discover_services",
            path: "services",
            contains: {
              service_id: fixtures.computeServiceId,
              provider_id: fixtures.providerId,
            },
          },
        ],
      },
    },
    {
      scenario_id: "openclaw.curated.local.local_service_visibility",
      prompt:
        `Use froglet to list local services. Confirm that local service ${fixtures.localVisibleServiceId} is present and hidden service ${fixtures.localHiddenServiceId} is not listed. Return the visible service_id and explain that the hidden service is absent from the listing.`,
      required_tool_actions: ["list_local_services"],
      fixture_injections: {
        action: "list_local_services",
      },
      result_oracles: {
        final_text_contains: [fixtures.localVisibleServiceId],
        tool_output_assertions: [
          {
            action: "list_local_services",
            path: "services",
            contains: {
              service_id: fixtures.localVisibleServiceId,
              publication_state: "active",
            },
          },
          {
            action: "list_local_services",
            path: "services",
            not_contains: {
              service_id: fixtures.localHiddenServiceId,
            },
          },
        ],
      },
    },
    {
      scenario_id: "openclaw.curated.local.get_local_service_detail",
      prompt:
        `Use froglet to fetch the local service detail for ${fixtures.localVisibleServiceId}. Return the service_id, provider_id, runtime, package kind, and publication_state.`,
      required_tool_actions: ["get_local_service"],
      fixture_injections: {
        action: "get_local_service",
        service_id: fixtures.localVisibleServiceId,
      },
      result_oracles: {
        final_text_contains: [fixtures.localVisibleServiceId, fixtures.providerId, "python"],
        tool_output_assertions: [
          {
            action: "get_local_service",
            path: "service.package_kind",
            equals: "inline_source",
          },
          {
            action: "get_local_service",
            path: "service.service_id",
            equals: fixtures.localVisibleServiceId,
          },
          {
            action: "get_local_service",
            path: "service.provider_id",
            equals: fixtures.providerId,
          },
          {
            action: "get_local_service",
            path: "service.runtime",
            equals: "python",
          },
          {
            action: "get_local_service",
            path: "service.publication_state",
            equals: "active",
          },
        ],
      },
    },
    {
      scenario_id: "openclaw.curated.local.run_wasm_compute",
      prompt:
        `Use froglet to run a Wasm compute job on provider ${fixtures.providerId}. If the job is pending, wait for the final result. Return only the final numeric result.`,
      required_tool_actions: ["run_compute"],
      fixture_injections: {
        action: "run_compute",
        provider_id: fixtures.providerId,
        runtime: "wasm",
        package_kind: "inline_module",
        contract_version: "froglet.wasm.run_json.v1",
        wasm_module_hex: "__fixture_valid_wasm_hex",
        input: { value: 42 },
        timeout_secs: 15,
      },
      require_wait_on_pending_actions: ["run_compute"],
      result_oracles: {
        final_text_contains: ["42"],
      },
    },
    {
      scenario_id: "openclaw.curated.local.run_python_compute",
      prompt:
        `Use froglet to run inline Python compute on provider ${fixtures.providerId}. If the job is pending, wait for the final result. Return the marker from the final output.`,
      required_tool_actions: ["run_compute"],
      fixture_injections: {
        action: "run_compute",
        provider_id: fixtures.providerId,
        runtime: "python",
        package_kind: "inline_source",
        entrypoint_kind: "handler",
        entrypoint: "handler",
        contract_version: "froglet.python.handler_json.v1",
        inline_source:
          "def handler(event, context):\n    return {\"marker\":\"processed by python handler\",\"input\":event}\n",
        input: { value: 7 },
        timeout_secs: 15,
      },
      require_wait_on_pending_actions: ["run_compute"],
      result_oracles: {
        tool_output_assertions: [
          { action: "run_compute", path: "terminal", equals: false },
          {
            action: "wait_task",
            path: "task.result.marker",
            equals: "processed by python handler",
          },
        ],
      },
    },
    {
      scenario_id: "openclaw.curated.local.invalid_missing_service",
      prompt:
        `Use froglet to invoke missing service ${fixtures.localVisibleServiceId}-missing on provider ${fixtures.providerId}. Return the failure reason.`,
      required_tool_actions: ["invoke_service"],
      fixture_injections: {
        action: "invoke_service",
        service_id: `${fixtures.localVisibleServiceId}-missing`,
        provider_id: fixtures.providerId,
        input: { marker: "missing" },
      },
      result_oracles: {
        expect_error: true,
        error_contains: ["service not found"],
      },
    },
  ]
}

export function buildLocalExploratoryScenarioSet() {
  return {
    agentic: {
      exploratory: {
        max_steps: 40,
        must_cover_actions: [
          "status",
          "discover_services",
          "list_local_services",
          "get_local_service",
          "publish_artifact",
          "run_compute",
        ],
      },
    },
  }
}

export function buildLocalExploratoryFixtures(fixtures) {
  return {
    validWasmHex: localValidWasmHex(),
    exploratoryDefaults: {
      provider_id: fixtures.providerId,
      free_provider_id: fixtures.providerId,
      paid_provider_id: fixtures.providerId,
      free_service_id: fixtures.computeServiceId,
      wasm_module_hex: "__fixture_valid_wasm_hex",
    },
  }
}

export async function runLocalCuratedSuite({ out } = {}) {
  requireApiKey()
  const tool = loadLocalFrogletTool()
  const fixtures = await bootstrapLocalFixtures(tool)
  const curated = await runCuratedSuite(
    tool,
    buildLocalCuratedScenarios(fixtures),
    { validWasmHex: localValidWasmHex() },
    { scope: "local-openclaw" }
  )
  const outputPath = out ?? localResultsPath("openclaw-curated-local.json")
  await writeJson(outputPath, {
    generated_at: new Date().toISOString(),
    fixtures,
    ...curated,
  })
  return { outputPath, fixtures, curated }
}

async function main() {
  const { values } = parseCliArgs({
    out: { type: "string", short: "o" },
  })
  const { curated } = await runLocalCuratedSuite({ out: values.out })
  if (curated.failed > 0) {
    process.exitCode = 1
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error)
    process.exitCode = 1
  })
}
