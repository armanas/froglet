import path from "node:path"
import { fileURLToPath } from "node:url"

import {
  parseCliArgs,
  readJson,
  writeJson,
} from "./common.mjs"

function shortRunId(inventory) {
  const source = inventory.run_id ?? "gcp-harness"
  return source.replace(/[^a-zA-Z0-9-]/g, "-").slice(0, 12)
}

function shortExecutionSuffix(value) {
  if (typeof value !== "string" || value.trim().length === 0) {
    return ""
  }
  return value.replace(/[^a-zA-Z0-9-]/g, "-").replace(/^-+|-+$/g, "").slice(0, 12)
}

function toolScenario({
  scenarioId,
  action,
  caseName,
  authProfile,
  fixtureInjections,
  resultOracles,
  requiredArtifacts = [],
  timeoutSecs = 30,
  storeContext = null,
}) {
  return {
    scenario_id: scenarioId,
    runner: "tool",
    action,
    case: caseName,
    target_node: "froglet-marketplace",
    auth_profile: authProfile,
    required_tool_actions: [action],
    fixture_injections: fixtureInjections,
    expected_protocol_artifacts: requiredArtifacts,
    failure_budget: {
      timeout_secs: timeoutSecs,
      max_failures: 0,
    },
    result_oracles: resultOracles,
    ...(storeContext ? { store_context: storeContext } : {}),
  }
}

function protocolScenario({
  scenarioId,
  targetNode,
  expectedArtifacts,
  description,
}) {
  return {
    scenario_id: scenarioId,
    runner: "protocol",
    target_node: targetNode,
    required_tool_actions: [],
    fixture_injections: {},
    expected_protocol_artifacts: expectedArtifacts,
    failure_budget: {
      timeout_secs: 60,
      max_failures: 0,
    },
    result_oracles: {
      description,
    },
  }
}

function agenticScenario({
  scenarioId,
  prompt,
  requiredActions,
  fixtureInjections,
  resultOracles,
  timeoutSecs = 120,
}) {
  return {
    scenario_id: scenarioId,
    runner: "agentic",
    target_node: "froglet-marketplace",
    required_tool_actions: requiredActions,
    fixture_injections: fixtureInjections,
    expected_protocol_artifacts: [],
    failure_budget: {
      timeout_secs: timeoutSecs,
      max_failures: 0,
    },
    result_oracles: resultOracles,
    prompt,
  }
}

function marketplaceBootstrap(prefix, executionSuffix = "") {
  const resolvedPrefix = executionSuffix ? `${prefix}-${executionSuffix}` : prefix
  return {
    build_project_id: `${resolvedPrefix}-market-build`,
    invalid_build_project_id: `${resolvedPrefix}-market-invalid`,
    blank_publish_project_id: `${resolvedPrefix}-market-blank`,
    publish_ready_project_id: `${resolvedPrefix}-market-ready`,
    local_static_service_id: `${resolvedPrefix}-market-static`,
    local_hidden_service_id: `${resolvedPrefix}-market-hidden`,
    create_project_service_id: `${resolvedPrefix}-create-happy`,
    create_inline_project_id: `${resolvedPrefix}-create-inline`,
    create_inline_service_id: `${resolvedPrefix}-create-inline`,
    publish_artifact_inline_service_id: `${resolvedPrefix}-artifact-inline`,
    publish_artifact_hidden_service_id: `${resolvedPrefix}-artifact-hidden`,
  }
}

export function buildScenarioSet(inventory, freeSeed, paidSeed, options = {}) {
  const prefix = shortRunId(inventory)
  const executionSuffix = shortExecutionSuffix(options.executionSuffix)
  const bootstrap = marketplaceBootstrap(prefix, executionSuffix)
  const scenarios = []

  const freeStatic = freeSeed.services.free_static
  const freePython = freeSeed.services.free_python_inline
  const freeWat = freeSeed.services.wat_project
  const freeHidden = freeSeed.services.hidden
  const freeData = freeSeed.services.data_echo
  const paidAsync = paidSeed.services.async_echo
  const paidPriced = paidSeed.services.priced
  const paidOciWasm = paidSeed.services.oci_wasm
  const paidOciContainer = paidSeed.services.oci_container
  const duplicateServiceId = freeSeed.services.shared_collision.service_id

  scenarios.push(
    toolScenario({
      scenarioId: "tool.status.happy",
      action: "status",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: { action: "status", include_raw: true },
      resultOracles: {
        text_contains: ["healthy: true", "runtime_healthy: true", "provider_healthy: true"],
        raw_assertions: [
          { path: "healthy", equals: true },
          { path: "reference_discovery.connected", equals: true },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.status.consumer",
      action: "status",
      caseName: "boundary",
      authProfile: "consumer_control",
      fixtureInjections: { action: "status", include_raw: true },
      resultOracles: {
        text_contains: ["healthy: true"],
        raw_assertions: [{ path: "healthy", equals: true }],
      },
    }),
    toolScenario({
      scenarioId: "tool.status.invalid_auth",
      action: "status",
      caseName: "failure",
      authProfile: "bogus",
      fixtureInjections: { action: "status" },
      resultOracles: {
        expect_error: true,
        error_contains: ["401"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.create_project.happy",
      action: "create_project",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "create_project",
        service_id: bootstrap.create_project_service_id,
        name: bootstrap.create_project_service_id,
        summary: "Marketplace-created result_json service",
        result_json: { message: "created" },
        price_sats: 0,
        publication_state: "active",
        include_raw: true,
      },
      resultOracles: {
        text_contains: [
          `project_id: ${bootstrap.create_project_service_id}`,
          "published: true",
        ],
        raw_assertions: [
          { path: "project.project_id", equals: bootstrap.create_project_service_id },
          { path: "project.publication_state", equals: "active" },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.create_project.inline_boundary",
      action: "create_project",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "create_project",
        project_id: bootstrap.create_inline_project_id,
        service_id: bootstrap.create_inline_service_id,
        name: bootstrap.create_inline_project_id,
        summary: "Marketplace-created inline Python echo project",
        runtime: "python",
        package_kind: "inline_source",
        entrypoint_kind: "handler",
        entrypoint: "source/main.py",
        contract_version: "froglet.python.handler_json.v1",
        inline_source: "def handler(event, context):\n    return {\"echo\": event, \"boundary\": True}\n",
        price_sats: 0,
        publication_state: "active",
        include_raw: true,
      },
      resultOracles: {
        text_contains: [
          `project_id: ${bootstrap.create_inline_project_id}`,
          "published: true",
        ],
        raw_assertions: [
          { path: "project.project_id", equals: bootstrap.create_inline_project_id },
          { path: "project.runtime", equals: "python" },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.create_project.blank_failure",
      action: "create_project",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "create_project",
        name: `${prefix}-blank-rejected`,
        summary: "This should be rejected",
        publication_state: "active",
        price_sats: 0,
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["active publication requires an explicit runnable scaffold"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.list_projects.happy",
      action: "list_projects",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: { action: "list_projects", include_raw: true },
      resultOracles: {
        text_contains: [
          `project_id: ${bootstrap.build_project_id}`,
          `project_id: ${bootstrap.create_inline_project_id}`,
        ],
        raw_assertions: [
          { path: "projects.0.project_id", exists: true },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.list_projects.invalid_auth",
      action: "list_projects",
      caseName: "failure",
      authProfile: "consumer_control",
      fixtureInjections: { action: "list_projects" },
      resultOracles: {
        expect_error: true,
        error_contains: ["401"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.get_project.happy",
      action: "get_project",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "get_project",
        project_id: bootstrap.build_project_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`project_id: ${bootstrap.build_project_id}`],
        raw_assertions: [{ path: "project.project_id", equals: bootstrap.build_project_id }],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_project.boundary",
      action: "get_project",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "get_project",
        project_id: bootstrap.create_inline_project_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [
          `project_id: ${bootstrap.create_inline_project_id}`,
          "runtime: python",
        ],
        raw_assertions: [{ path: "project.runtime", equals: "python" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_project.missing_failure",
      action: "get_project",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: { action: "get_project", project_id: `${prefix}-missing-project` },
      resultOracles: {
        expect_error: true,
        error_contains: ["project not found"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.read_file.happy",
      action: "read_file",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "read_file",
        project_id: bootstrap.build_project_id,
        path: "source/main.wat",
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`project_id: ${bootstrap.build_project_id}`, "(module"],
        raw_assertions: [{ path: "path", equals: "source/main.wat" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.read_file.boundary",
      action: "read_file",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "read_file",
        project_id: bootstrap.create_inline_project_id,
        path: "source/main.py",
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["path: source/main.py", "def handler"],
        raw_assertions: [{ path: "path", equals: "source/main.py" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.read_file.missing_failure",
      action: "read_file",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "read_file",
        project_id: bootstrap.build_project_id,
        path: "source/does-not-exist.wat",
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["failed to read"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.write_file.happy",
      action: "write_file",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "write_file",
        project_id: bootstrap.build_project_id,
        path: "source/main.wat",
        contents: `(module
  (func (export "run_json") (result i32)
    i32.const 11)
)`,
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: written", `project_id: ${bootstrap.build_project_id}`],
        raw_assertions: [{ path: "status", equals: "written" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.write_file.boundary",
      action: "write_file",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "write_file",
        project_id: bootstrap.build_project_id,
        path: "notes.txt",
        contents: "gcp harness notes\n",
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["path: notes.txt"],
        raw_assertions: [{ path: "path", equals: "notes.txt" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.write_file.missing_failure",
      action: "write_file",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "write_file",
        project_id: bootstrap.build_project_id,
        path: "../escape.wat",
        contents: "(module)",
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["must not traverse parent directories"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.build_project.happy",
      action: "build_project",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "build_project",
        project_id: bootstrap.build_project_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [
          `project_id: ${bootstrap.build_project_id}`,
          "build_artifact_path:",
        ],
        raw_assertions: [{ path: "project.project_id", equals: bootstrap.build_project_id }],
      },
    }),
    toolScenario({
      scenarioId: "tool.build_project.rebuild_boundary",
      action: "build_project",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "build_project",
        project_id: bootstrap.build_project_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`project_id: ${bootstrap.build_project_id}`],
        raw_assertions: [{ path: "project.module_hash", exists: true }],
      },
    }),
    toolScenario({
      scenarioId: "tool.build_project.invalid_failure",
      action: "build_project",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "build_project",
        project_id: `${prefix}-absent-build-project`,
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["project not found"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.test_project.happy",
      action: "test_project",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "test_project",
        project_id: bootstrap.build_project_id,
        input: { source: "matrix" },
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`project_id: ${bootstrap.build_project_id}`],
        raw_assertions: [{ path: "output", equals: 11 }],
      },
    }),
    toolScenario({
      scenarioId: "tool.test_project.inline_boundary",
      action: "test_project",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "test_project",
        project_id: bootstrap.create_inline_project_id,
        input: { ok: true },
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`project_id: ${bootstrap.create_inline_project_id}`],
        raw_assertions: [{ path: "output.echo.ok", equals: true }],
      },
    }),
    toolScenario({
      scenarioId: "tool.test_project.invalid_failure",
      action: "test_project",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "test_project",
        project_id: `${prefix}-absent-test-project`,
        input: { should_fail: true },
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["project not found"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.publish_project.happy",
      action: "publish_project",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "publish_project",
        project_id: bootstrap.build_project_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: created", `service_id: ${bootstrap.build_project_id}`],
        raw_assertions: [{ path: "service.service_id", equals: bootstrap.build_project_id }],
      },
    }),
    toolScenario({
      scenarioId: "tool.publish_project.boundary",
      action: "publish_project",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "publish_project",
        project_id: bootstrap.publish_ready_project_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: created", `service_id: ${bootstrap.publish_ready_project_id}`],
        raw_assertions: [
          { path: "service.publication_state", equals: "active" },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.publish_project.blank_failure",
      action: "publish_project",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "publish_project",
        project_id: bootstrap.blank_publish_project_id,
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["blank projects are scaffolds only"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.publish_artifact.happy",
      action: "publish_artifact",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "publish_artifact",
        service_id: bootstrap.publish_artifact_inline_service_id,
        runtime: "python",
        package_kind: "inline_source",
        entrypoint_kind: "handler",
        entrypoint: "handler",
        contract_version: "froglet.python.handler_json.v1",
        inline_source: "def handler(event, context):\n    return {\"artifact\": True, \"input\": event}\n",
        price_sats: 0,
        publication_state: "active",
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`service_id: ${bootstrap.publish_artifact_inline_service_id}`],
        raw_assertions: [
          { path: "service.runtime", equals: "python" },
          { path: "service.binding_hash", exists: true },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.publish_artifact.boundary",
      action: "publish_artifact",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "publish_artifact",
        service_id: bootstrap.publish_artifact_hidden_service_id,
        runtime: "wasm",
        package_kind: "inline_module",
        publication_state: "hidden",
        price_sats: 0,
        wasm_module_hex: "__fixture_valid_wasm_hex",
        include_raw: true,
      },
      resultOracles: {
        text_contains: [
          `service_id: ${bootstrap.publish_artifact_hidden_service_id}`,
          "publication_state: hidden",
        ],
        raw_assertions: [{ path: "service.publication_state", equals: "hidden" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.publish_artifact.failure",
      action: "publish_artifact",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "publish_artifact",
        service_id: `${prefix}-artifact-invalid`,
        price_sats: 0,
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["runtime is required"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.list_local_services.happy",
      action: "list_local_services",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: { action: "list_local_services", include_raw: true },
      resultOracles: {
        text_contains: [
          `service_id: ${bootstrap.local_static_service_id}`,
          `service_id: ${bootstrap.local_hidden_service_id}`,
        ],
        raw_assertions: [{ path: "services.0.service_id", exists: true }],
      },
    }),
    toolScenario({
      scenarioId: "tool.list_local_services.boundary",
      action: "list_local_services",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: { action: "list_local_services", include_raw: true },
      resultOracles: {
        text_contains: ["Only listed fields are authoritative."],
        raw_assertions: [
          {
            path: "services",
            contains: { service_id: bootstrap.local_hidden_service_id },
          },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.list_local_services.failure",
      action: "list_local_services",
      caseName: "failure",
      authProfile: "consumer_control",
      fixtureInjections: { action: "list_local_services" },
      resultOracles: {
        expect_error: true,
        error_contains: ["401"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.get_local_service.happy",
      action: "get_local_service",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "get_local_service",
        service_id: bootstrap.local_static_service_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`service_id: ${bootstrap.local_static_service_id}`],
        raw_assertions: [{ path: "service.service_id", equals: bootstrap.local_static_service_id }],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_local_service.boundary",
      action: "get_local_service",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "get_local_service",
        service_id: bootstrap.local_hidden_service_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [
          `service_id: ${bootstrap.local_hidden_service_id}`,
          "publication_state: hidden",
        ],
        raw_assertions: [{ path: "service.publication_state", equals: "hidden" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_local_service.failure",
      action: "get_local_service",
      caseName: "failure",
      authProfile: "consumer_control",
      fixtureInjections: {
        action: "get_local_service",
        service_id: bootstrap.local_static_service_id,
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["401"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.discover_services.happy",
      action: "discover_services",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "discover_services",
        limit: 20,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [
          `service_id: ${freeStatic.service_id}`,
          `service_id: ${paidAsync.service_id}`,
        ],
        text_not_contains: [freeHidden.service_id],
        raw_assertions: [
          { path: "services", contains: { service_id: freeStatic.service_id } },
          { path: "services", contains: { service_id: paidAsync.service_id } },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.discover_services.boundary",
      action: "discover_services",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "discover_services",
        query: duplicateServiceId,
        limit: 10,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`service_id: ${duplicateServiceId}`],
        raw_assertions: [{ path: "services.1.service_id", equals: duplicateServiceId }],
      },
    }),
    toolScenario({
      scenarioId: "tool.discover_services.failure",
      action: "discover_services",
      caseName: "failure",
      authProfile: "bogus",
      fixtureInjections: { action: "discover_services", limit: 5 },
      resultOracles: {
        expect_error: true,
        error_contains: ["401"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.get_service.happy",
      action: "get_service",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "get_service",
        provider_id: freeSeed.provider_id,
        service_id: freePython.service_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`service_id: ${freePython.service_id}`, "runtime: python"],
        raw_assertions: [
          { path: "service.service_id", equals: freePython.service_id },
          { path: "service.binding_hash", exists: true },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_service.boundary",
      action: "get_service",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "get_service",
        service_id: duplicateServiceId,
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["matched multiple providers"],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_service.hidden_failure",
      action: "get_service",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "get_service",
        provider_id: freeSeed.provider_id,
        service_id: freeHidden.service_id,
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["service not found"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.invoke_service.happy",
      action: "invoke_service",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "invoke_service",
        provider_id: freeSeed.provider_id,
        service_id: freeStatic.service_id,
        input: { caller: "matrix" },
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: succeeded", "\"message\":\"pong\""],
        raw_assertions: [{ path: "status", equals: "succeeded" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.invoke_service.async_boundary",
      action: "invoke_service",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "invoke_service",
        provider_id: paidSeed.provider_id,
        service_id: paidAsync.service_id,
        input: { delay_ms: 1500, value: "async" },
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["terminal: false", "pending: use wait_task"],
        raw_assertions: [{ path: "task.deal_id", exists: true }],
      },
      storeContext: {
        path: "task.deal_id",
        key: "async_task_id",
      },
    }),
    toolScenario({
      scenarioId: "tool.invoke_service.hidden_failure",
      action: "invoke_service",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "invoke_service",
        provider_id: freeSeed.provider_id,
        service_id: freeHidden.service_id,
        input: { caller: "matrix" },
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["service not found"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.get_task.happy",
      action: "get_task",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: { action: "get_task", task_id: "__context_async_task_id", include_raw: true },
      resultOracles: {
        text_contains: ["task_id:"],
        raw_assertions: [{ path: "task.deal_id", equals_context: "async_task_id" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_task.failure",
      action: "get_task",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: { action: "get_task", task_id: `${prefix}-missing-task` },
      resultOracles: {
        expect_error: true,
        error_contains: ["deal not found"],
      },
    }),
    toolScenario({
      scenarioId: "tool.wait_task.happy",
      action: "wait_task",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "wait_task",
        task_id: "__context_async_task_id",
        timeout_secs: 30,
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: succeeded"],
        raw_assertions: [{ path: "task.status", equals: "succeeded" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.wait_task.boundary",
      action: "wait_task",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "wait_task",
        task_id: "__context_async_task_id",
        timeout_secs: 1,
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: succeeded"],
        raw_assertions: [{ path: "task.status", equals: "succeeded" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.wait_task.failure",
      action: "wait_task",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: { action: "wait_task", task_id: `${prefix}-missing-task`, timeout_secs: 1 },
      resultOracles: {
        expect_error: true,
        error_contains: ["deal not found"],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_task.terminal_boundary",
      action: "get_task",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: { action: "get_task", task_id: "__context_async_task_id", include_raw: true },
      resultOracles: {
        text_contains: ["status: succeeded"],
        raw_assertions: [{ path: "task.status", equals: "succeeded" }],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.tail_logs.happy",
      action: "tail_logs",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "tail_logs",
        target: "all",
        lines: 20,
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["scope: node", "runtime:", "provider:"],
        raw_assertions: [{ path: "logs.0.component", exists: true }],
      },
    }),
    toolScenario({
      scenarioId: "tool.tail_logs.boundary",
      action: "tail_logs",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "tail_logs",
        target: "runtime",
        lines: 5,
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["components: runtime"],
        raw_assertions: [{ path: "logs.0.component", equals: "runtime" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.tail_logs.failure",
      action: "tail_logs",
      caseName: "failure",
      authProfile: "consumer_control",
      fixtureInjections: {
        action: "tail_logs",
        target: "runtime",
        lines: 5,
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["401"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.restart.happy",
      action: "restart",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "restart",
        target: "runtime",
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["runtime: restarted"],
        raw_assertions: [{ path: "results.0.status", equals: "restarted" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.restart.boundary",
      action: "restart",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "restart",
        target: "all",
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["runtime: restarted", "provider: restarted"],
        raw_assertions: [{ path: "results.1.status", equals: "restarted" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.restart.failure",
      action: "restart",
      caseName: "failure",
      authProfile: "consumer_control",
      fixtureInjections: { action: "restart", target: "runtime" },
      resultOracles: {
        expect_error: true,
        error_contains: ["401"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.run_compute.happy",
      action: "run_compute",
      caseName: "happy",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "run_compute",
        provider_id: freeSeed.provider_id,
        runtime: "wasm",
        package_kind: "inline_module",
        wasm_module_hex: "__fixture_valid_wasm_hex",
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: succeeded", "42"],
        raw_assertions: [{ path: "status", equals: "succeeded" }],
      },
      requiredArtifacts: ["quote", "deal", "receipt"],
    }),
    toolScenario({
      scenarioId: "tool.run_compute.inline_python_boundary",
      action: "run_compute",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "run_compute",
        provider_id: freeSeed.provider_id,
        runtime: "python",
        package_kind: "inline_source",
        entrypoint_kind: "handler",
        entrypoint: "handler",
        contract_version: "froglet.python.handler_json.v1",
        inline_source:
          "def handler(event, context):\n    return {\"via\": \"inline-python\", \"input\": event}\n",
        input: { via: "matrix" },
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: succeeded", "\"via\":\"inline-python\""],
        raw_assertions: [{ path: "status", equals: "succeeded" }],
      },
      requiredArtifacts: ["quote", "deal", "receipt"],
    }),
    toolScenario({
      scenarioId: "tool.run_compute.failure",
      action: "run_compute",
      caseName: "failure",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "run_compute",
        runtime: "wasm",
        package_kind: "inline_module",
        wasm_module_hex: "__fixture_valid_wasm_hex",
      },
      resultOracles: {
        expect_error: true,
        error_contains: ["provider_id or provider_url is required"],
      },
    }),
  )

  scenarios.push(
    toolScenario({
      scenarioId: "tool.invoke_service.data_boundary",
      action: "invoke_service",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "invoke_service",
        provider_id: freeSeed.provider_id,
        service_id: freeData.service_id,
        input: { hello: "world" },
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: succeeded"],
        raw_assertions: [{ path: "result.echo.hello", equals: "world" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.invoke_service.project_wasm_boundary",
      action: "invoke_service",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "invoke_service",
        provider_id: freeSeed.provider_id,
        service_id: freeWat.service_id,
        input: { source: "wat" },
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: succeeded"],
        raw_assertions: [{ path: "status", equals: "succeeded" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.invoke_service.oci_boundary",
      action: "invoke_service",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "invoke_service",
        provider_id: paidSeed.provider_id,
        service_id: paidOciContainer.service_id,
        input: { hello: "oci" },
        timeout_secs: 20,
        include_raw: true,
      },
      resultOracles: {
        text_contains: ["status: succeeded"],
        raw_assertions: [{ path: "result.input.hello", equals: "oci" }],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_service.oci_wasm_boundary",
      action: "get_service",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "get_service",
        provider_id: paidSeed.provider_id,
        service_id: paidOciWasm.service_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`service_id: ${paidOciWasm.service_id}`, "package_kind: oci_image"],
        raw_assertions: [
          { path: "service.package_kind", equals: "oci_image" },
          { path: "service.binding_hash", exists: true },
        ],
      },
    }),
    toolScenario({
      scenarioId: "tool.get_service.priced_boundary",
      action: "get_service",
      caseName: "boundary",
      authProfile: "provider_control",
      fixtureInjections: {
        action: "get_service",
        provider_id: paidSeed.provider_id,
        service_id: paidPriced.service_id,
        include_raw: true,
      },
      resultOracles: {
        text_contains: [`service_id: ${paidPriced.service_id}`, "price_sats:"],
        raw_assertions: [{ path: "service.price_sats", equals: paidPriced.price_sats }],
      },
    }),
  )

  const protocolScenarios = [
    protocolScenario({
      scenarioId: "protocol.discovery.visibility_and_hidden_exclusion",
      targetNode: "froglet-discovery",
      expectedArtifacts: ["descriptor", "offer"],
      description:
        "Discovery search returns active services, excludes hidden services, and recovers after provider restarts.",
    }),
    protocolScenario({
      scenarioId: "protocol.public_service_redaction",
      targetNode: "froglet-provider-free",
      expectedArtifacts: ["offer"],
      description:
        "Public provider service detail redacts bindings and rejects hidden-service detail fetches.",
    }),
    protocolScenario({
      scenarioId: "protocol.free_compute_artifact_chain",
      targetNode: "froglet-provider-free",
      expectedArtifacts: ["descriptor", "offer", "quote", "deal", "receipt"],
      description:
        "Free direct compute yields a valid descriptor/offer/quote/deal/receipt chain and tampered artifacts are rejected.",
    }),
    protocolScenario({
      scenarioId: "protocol.mock_lightning_bundle_and_receipt",
      targetNode: "froglet-provider-paid",
      expectedArtifacts: ["descriptor", "offer", "quote", "deal", "invoice_bundle", "receipt"],
      description:
        "Mock-lightning flow emits a valid bundle, settles after provider-side state promotion, and rejects tampered bundle and receipt hashes.",
    }),
    protocolScenario({
      scenarioId: "protocol.runtime_security_and_ssrf",
      targetNode: "froglet-marketplace",
      expectedArtifacts: ["quote", "deal", "receipt"],
      description:
        "Runtime rejects provider URL mismatches and SSRF-style local/private targets while preserving successful free remote execution.",
    }),
    protocolScenario({
      scenarioId: "protocol.real_lnd_regtest",
      targetNode: "froglet-settlement-lab",
      expectedArtifacts: ["quote", "deal", "invoice_bundle", "receipt"],
      description:
        "Existing LND regtest integration runs on the settlement lab VM for hold-invoice release, expiry, and restart recovery.",
    }),
  ]

  const agenticScenarios = [
    agenticScenario({
      scenarioId: "agentic.remote_service_matrix",
      prompt:
        `Use the froglet tool to discover services, inspect ${freeStatic.service_id}, invoke it, then inspect ${paidAsync.service_id}. ` +
        "Return a compact report with the discovered provider ids, the free invocation result, and whether the async service exposed a task id.",
      requiredActions: ["discover_services", "get_service", "invoke_service"],
      fixtureInjections: {
        discover_service_id: freeStatic.service_id,
        free_service_id: freeStatic.service_id,
        free_provider_id: freeSeed.provider_id,
        async_service_id: paidAsync.service_id,
        async_provider_id: paidSeed.provider_id,
      },
      resultOracles: {
        must_contain: [freeStatic.service_id, paidAsync.service_id, "pong"],
      },
    }),
    agenticScenario({
      scenarioId: "agentic.local_project_lifecycle",
      prompt:
        `Use the froglet tool to create a new hidden project named ${prefix}-agentic-local, read its main source file, write runnable source, build it, test it, publish it, then confirm it appears in local services. ` +
        "Return the project_id, service_id, and test output.",
      requiredActions: [
        "create_project",
        "read_file",
        "write_file",
        "build_project",
        "test_project",
        "publish_project",
        "list_local_services",
      ],
      fixtureInjections: {
        project_name: `${prefix}-agentic-local`,
      },
      resultOracles: {
        must_contain: [`${prefix}-agentic-local`, "service_id", "output"],
      },
    }),
    agenticScenario({
      scenarioId: "agentic.direct_compute",
      prompt:
        `Use the froglet tool to run direct compute twice against provider ${freeSeed.provider_id}: once using inline Wasm, and once using a tiny inline Python handler that returns the input with a marker. ` +
        "Return both results in plain text.",
      requiredActions: ["run_compute"],
      fixtureInjections: {
        free_provider_id: freeSeed.provider_id,
        wasm_module_hex: "__fixture_valid_wasm_hex",
      },
      resultOracles: {
        must_contain: ["42", "marker"],
      },
    }),
    agenticScenario({
      scenarioId: "agentic.observability_and_recovery",
      prompt:
        "Use the froglet tool to check status, tail runtime logs, restart runtime, and check status again. Return a concise recovery summary.",
      requiredActions: ["status", "tail_logs", "restart"],
      fixtureInjections: {},
      resultOracles: {
        must_contain: ["healthy", "runtime"],
      },
    }),
  ]

  return {
    version: 1,
    generated_at: new Date().toISOString(),
    inventory_ref: path.basename(inventory.inventory_path ?? "inventory.json"),
    run_id: inventory.run_id,
    bootstrap,
    seeds: {
      free: freeSeed,
      paid: paidSeed,
    },
    scenarios: [...scenarios, ...protocolScenarios],
    agentic: {
      deterministic: agenticScenarios,
      exploratory: {
        max_steps: 40,
        must_cover_actions: [
          "status",
          "discover_services",
          "get_service",
          "invoke_service",
          "list_local_services",
          "get_local_service",
          "create_project",
          "list_projects",
          "get_project",
          "read_file",
          "write_file",
          "build_project",
          "test_project",
          "publish_project",
          "publish_artifact",
          "tail_logs",
          "restart",
          "get_task",
          "wait_task",
          "run_compute",
        ],
      },
    },
  }
}

export async function main() {
  const { values } = parseCliArgs({
    inventory: { type: "string", short: "i" },
    "seed-free": { type: "string" },
    "seed-paid": { type: "string" },
    "execution-suffix": { type: "string" },
    out: { type: "string", short: "o" },
  })
  if (!values.inventory || !values["seed-free"] || !values["seed-paid"] || !values.out) {
    throw new Error("--inventory, --seed-free, --seed-paid, and --out are required")
  }

  const [inventory, freeSeed, paidSeed] = await Promise.all([
    readJson(values.inventory),
    readJson(values["seed-free"]),
    readJson(values["seed-paid"]),
  ])
  inventory.inventory_path = values.inventory
  const scenarioSet = buildScenarioSet(inventory, freeSeed, paidSeed, {
    executionSuffix: values["execution-suffix"],
  })
  await writeJson(values.out, scenarioSet)
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error)
    process.exitCode = 1
  })
}
