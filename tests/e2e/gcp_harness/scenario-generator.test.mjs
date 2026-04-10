import assert from "node:assert/strict"
import test from "node:test"

import { buildScenarioSet } from "./generate-scenarios.mjs"

function sampleInventory() {
  return {
    run_id: "20260329-test",
    inventory_path: "/tmp/inventory.json",
    roles: {
      "froglet-marketplace": {},
      "froglet-provider-free": {},
      "froglet-provider-paid": {},
      "froglet-settlement-lab": {},
    },
  }
}

function sampleFreeSeed() {
  return {
    provider_id: "provider-free",
    provider_public_url: "https://free.example",
    services: {
      free_static: { service_id: "free-static", binding_hash: "1".repeat(64) },
      free_python_inline: { service_id: "free-python", binding_hash: "2".repeat(64) },
      wat_project: { service_id: "free-wat", binding_hash: "3".repeat(64) },
      hidden: { service_id: "hidden-service", binding_hash: "4".repeat(64) },
      data_echo: { service_id: "data-echo", binding_hash: "5".repeat(64) },
      shared_collision: { service_id: "shared-service" },
    },
  }
}

function samplePaidSeed() {
  return {
    provider_id: "provider-paid",
    provider_public_url: "https://paid.example",
    services: {
      priced: { service_id: "priced-service", price_sats: 25, binding_hash: "6".repeat(64) },
      async_echo: { service_id: "async-service", binding_hash: "7".repeat(64) },
      oci_wasm: { service_id: "oci-wasm", binding_hash: "8".repeat(64) },
      oci_container: { service_id: "oci-container", binding_hash: "9".repeat(64) },
      shared_collision: { service_id: "shared-service" },
    },
    fixtures: {
      oci_container: {
        reference: "127.0.0.1:5000/froglet/test:latest",
        digest: "a".repeat(64),
      },
      oci_wasm: {
        reference: "http://127.0.0.1:5001/module:latest",
        digest: "b".repeat(64),
      },
    },
  }
}

test("buildScenarioSet emits tool, protocol, and OpenClaw coverage", () => {
  const scenarioSet = buildScenarioSet(sampleInventory(), sampleFreeSeed(), samplePaidSeed(), {
    executionSuffix: "exec-pass-01",
  })

  assert.equal(typeof scenarioSet.bootstrap.build_project_id, "string")
  assert.ok(scenarioSet.bootstrap.build_project_id.includes("exec-pass-01"))
  assert.ok(Array.isArray(scenarioSet.scenarios))
  assert.ok(Array.isArray(scenarioSet.openclaw.scripted))
  assert.ok(Array.isArray(scenarioSet.openclaw.curated))
  assert.equal(
    scenarioSet.seeds.free.services.free_python_inline.binding_hash,
    "2".repeat(64)
  )
  assert.ok(
    scenarioSet.scenarios.every(
      (scenario) =>
        scenario.runner !== "tool" ||
        ![
          "create_project",
          "list_projects",
          "get_project",
          "read_file",
          "write_file",
          "build_project",
          "test_project",
          "publish_project"
        ].includes(scenario.action)
    )
  )

  const runComputeBoundary = scenarioSet.scenarios.find(
    (scenario) => scenario.scenario_id === "tool.run_compute.inline_python_boundary"
  )
  assert.ok(runComputeBoundary)
  assert.equal(runComputeBoundary.fixture_injections.runtime, "python")
  assert.match(runComputeBoundary.fixture_injections.inline_source, /inline-python/)

  const statusHappy = scenarioSet.scenarios.find(
    (scenario) => scenario.scenario_id === "tool.status.happy"
  )
  assert.ok(statusHappy)
  assert.deepEqual(statusHappy.result_oracles.raw_assertions, [
    { path: "healthy", equals: true },
  ])

  const protocolLightning = scenarioSet.scenarios.find(
    (scenario) => scenario.scenario_id === "protocol.mock_lightning_bundle_and_receipt"
  )
  assert.ok(protocolLightning)
  assert.deepEqual(protocolLightning.expected_protocol_artifacts, [
    "descriptor",
    "offer",
    "quote",
    "deal",
    "invoice_bundle",
    "receipt",
  ])

  const openclawDirectCompute = scenarioSet.openclaw.scripted.find(
    (scenario) => scenario.scenario_id === "openclaw.direct_compute_flow"
  )
  assert.ok(openclawDirectCompute)

  const curatedAsyncInvoke = scenarioSet.openclaw.curated.find(
    (scenario) => scenario.scenario_id === "openclaw.curated.invoke_async_wait"
  )
  assert.ok(curatedAsyncInvoke)
  assert.equal(curatedAsyncInvoke.fixture_injections.action, "invoke_service")

  const curatedTaskRoundtrip = scenarioSet.openclaw.curated.find(
    (scenario) => scenario.scenario_id === "openclaw.curated.task_roundtrip"
  )
  assert.ok(curatedTaskRoundtrip)
  assert.equal(curatedTaskRoundtrip.fixture_injections.action, "invoke_service")
  assert.match(curatedTaskRoundtrip.prompt, /call get_task exactly once/i)
  assert.ok(openclawDirectCompute.required_tool_actions.includes("run_compute"))

  const curatedHidden = scenarioSet.openclaw.curated.find(
    (scenario) => scenario.scenario_id === "openclaw.curated.discovery_visibility"
  )
  assert.ok(curatedHidden)
  assert.ok(curatedHidden.result_oracles.tool_output_assertions.some(
    (assertion) =>
      assertion.action === "discover_services" &&
      assertion.path === "services" &&
      assertion.not_contains?.service_id === "hidden-service"
  ))
})
