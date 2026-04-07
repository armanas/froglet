import { dispatchFrogletAction } from "../../../shared/froglet-lib/tool-dispatch.js"

function errorResult(error) {
  return {
    content: [{ type: "text", text: `Error: ${error?.message ?? String(error)}` }],
    isError: true
  }
}

const frogletToolDescription =
  "Authoritative Froglet MCP tool. Use exact Froglet actions instead of guessing. For local services use list_local_services or get_local_service. For remote discovery-backed services use discover_services or get_service. For named or data-service bindings use invoke_service. For simple fixed-response services, use create_project with result_json, price_sats, and publication_state=active. For authored services use create_project, write_file, build_project, test_project, and publish_project. Use run_compute for open-ended compute through the provider's direct compute offer, and include provider_id or provider_url."

function frogletToolInputSchema(config) {
  return {
    type: "object",
    required: ["action"],
    properties: {
      action: {
        type: "string",
        description:
          "Exact Froglet action name. Do not invent actions. Use list_local_services for local listings, discover_services for remote discovery-backed listings, get_local_service/get_service for authoritative details, invoke_service for named or data-service execution, create_project plus explicit result_json/inline_source or the project build flow for authoring, and run_compute for open-ended compute with an explicit provider_id or provider_url.",
        enum: [
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
          "status",
          "tail_logs",
          "restart",
          "get_task",
          "wait_task",
          "run_compute"
        ]
      },
      name: {
        type: "string",
        description:
          "Friendly service/project name. For create_project, Froglet will derive project_id, service_id, and offer_id from this if explicit ids are omitted."
      },
      service_id: {
        type: "string",
        description:
          "Service identifier. Required for publish_artifact, get_local_service, get_service, and invoke_service."
      },
      project_id: { type: "string" },
      offer_id: { type: "string" },
      summary: {
        type: "string",
        description:
          "Descriptive metadata only. Summary never generates code and is never enough to auto-publish a runnable service."
      },
      runtime: {
        type: "string",
        description: "Execution runtime for the service or compute request, for example wasm, python, or container."
      },
      package_kind: {
        type: "string",
        description: "Execution package kind for the workload, for example inline_module, inline_source, or oci_image."
      },
      entrypoint_kind: {
        type: "string",
        description: "Entrypoint shape for the workload, for example handler, script, or builtin."
      },
      entrypoint: {
        type: "string",
        description: "Entrypoint identifier or path for the workload."
      },
      contract_version: {
        type: "string",
        description: "Contract version for the execution payload."
      },
      mounts: {
        description:
          "Optional mount handles or bindings required by the workload. Keep this as the provider-defined mount payload."
      },
      wasm_module_hex: {
        type: "string",
        description:
          "Optional inline Wasm module bytes in hex. Low-level escape hatch for direct inline Wasm compute or publish_artifact. Prefer artifact_path instead."
      },
      inline_source: {
        type: "string",
        description:
          "Optional inline source for a new project or compute request. Use this when you want Froglet to author or run explicit source text, typically for runtime=python package_kind=inline_source."
      },
      starter: {
        type: "string",
        description:
          "Optional starter code scaffold. This is only initial code scaffolding, not a publish mode. Use starter only when you want Froglet to scaffold starter code explicitly."
      },
      path: { type: "string" },
      contents: { type: "string" },
      input: {},
      result_json: {
        description:
          "Optional static JSON result to scaffold into a new project. Use this for simple constant-return services. Summary is metadata only and does not generate code."
      },
      output_schema: {},
      input_schema: {},
      price_sats: { type: "integer", minimum: 0 },
      publication_state: {
        type: "string",
        enum: ["active", "hidden"],
        description:
          "Use active only when the request also includes starter, result_json, or inline_source, or when a built project is already ready to publish. Blank projects should remain hidden."
      },
      mode: { type: "string", enum: ["sync", "async"] },
      target: { type: "string", enum: ["node", "runtime", "provider", "all"] },
      lines: { type: "integer", minimum: 1, maximum: 500 },
      provider_id: {
        type: "string",
        description:
          "Target provider node ID. Required for run_compute unless provider_url is supplied."
      },
      provider_url: {
        type: "string",
        description:
          "Target provider base URL. Required for run_compute unless provider_id is supplied."
      },
      limit: {
        type: "integer",
        minimum: 1,
        maximum: config.maxSearchLimit
      },
      include_inactive: { type: "boolean" },
      query: { type: "string" },
      task_id: { type: "string" },
      timeout_secs: { type: "integer", minimum: 1, maximum: 600 },
      poll_interval_secs: { type: "number", minimum: 0.1, maximum: 10 },
      artifact_path: { type: "string" },
      oci_reference: { type: "string" },
      oci_digest: { type: "string" }
    }
  }
}

export function buildToolDefinitions(config) {
  return [
    {
      name: "froglet",
      description: frogletToolDescription,
      inputSchema: frogletToolInputSchema(config)
    }
  ]
}

export async function handleToolCall(name, args, config) {
  if (name !== "froglet") {
    return errorResult(new Error(`Unknown tool: ${name}`))
  }
  try {
    return await dispatchFrogletAction(args ?? {}, config)
  } catch (error) {
    return errorResult(error)
  }
}
