import {
  buildProject,
  createProject,
  discoverServices,
  frogletStatus,
  frogletTailLogs,
  getLocalService,
  getProject,
  getService,
  getTask,
  invokeService,
  listLocalServices,
  listProjects,
  publishArtifact,
  publishProject,
  readProjectFile,
  runCompute,
  testProject,
  waitTask,
  writeProjectFile
} from "../../../shared/froglet-lib/froglet-client.js"
import { toolTextResult } from "../../../shared/froglet-lib/shared.js"
import {
  firstDefined,
  formatObject,
  serviceAuthorityNotes,
  summarizeProject,
  summarizeService,
  summarizeTask
} from "../../../shared/froglet-lib/summarize.js"

function errorResult(error) {
  return {
    content: [{ type: "text", text: `Error: ${error?.message ?? String(error)}` }],
    isError: true
  }
}

function ctx(config) {
  return {
    baseUrl: config.baseUrl,
    authTokenPath: config.authTokenPath,
    requestTimeoutMs: config.requestTimeoutMs
  }
}

// --- Tool: froglet_status ---

async function handleStatus(args, config) {
  const response = await frogletStatus(ctx(config))
  const lines = [
    `node_id: ${response.node_id ?? "unknown"}`,
    `runtime_healthy: ${response.runtime?.healthy === true}`,
    `provider_healthy: ${response.provider?.healthy === true}`,
    `discovery_mode: ${response.discovery?.mode ?? "unknown"}`,
    `reference_discovery_enabled: ${response.reference_discovery?.enabled === true}`,
    `reference_discovery_publish_enabled: ${response.reference_discovery?.publish_enabled === true}`,
    `reference_discovery_connected: ${response.reference_discovery?.connected === true}`,
    `reference_discovery_url: ${response.reference_discovery?.url ?? "none"}`,
    `reference_discovery_last_error: ${response.reference_discovery?.last_error ?? "none"}`,
    `projects_root: ${response.projects_root ?? "unknown"}`,
    `compute_offer_id: ${response.raw_compute_offer_id ?? "execute.compute"}`
  ]
  return toolTextResult(lines.join("\n"))
}

// --- Tool: froglet_logs ---

async function handleLogs(args, config) {
  const response = await frogletTailLogs({
    ...ctx(config),
    target: args.target,
    lines: args.lines
  })
  const logs = Array.isArray(response.logs) ? response.logs : []
  const lines = [
    `targets: ${logs.map((entry) => entry.target).join(", ") || "none"}`,
    "",
    ...logs.flatMap((entry) => [
      `${entry.target}:`,
      ...(Array.isArray(entry.lines) && entry.lines.length > 0 ? entry.lines : ["no lines"]),
      ""
    ])
  ]
  return toolTextResult(lines.join("\n"))
}

// --- Tool: froglet_discover ---

async function handleDiscover(args, config) {
  const response = await discoverServices({
    ...ctx(config),
    limit: args.limit ?? config.defaultSearchLimit,
    includeInactive: args.include_inactive === true,
    query: args.query
  })
  const services = Array.isArray(response.services) ? response.services : []
  const failures = Array.isArray(response.provider_fetch_failures)
    ? response.provider_fetch_failures
    : []
  const lines = [
    `services: ${services.length}`,
    `provider_nodes_discovered: ${response.provider_nodes_discovered ?? 0}`,
    `provider_fetch_failures: ${failures.length}`,
    "",
    ...(services.length > 0
      ? services.flatMap((service, index) => [`${index + 1}.`, ...summarizeService(service), ""])
      : ["no remote services discovered"]),
    ...(failures.length > 0
      ? [
          "provider fetch failures:",
          ...failures.flatMap((failure) => [
            `- provider_url: ${failure?.provider_url ?? "unknown"}`,
            `  status: ${failure?.status ?? "none"}`,
            `  error: ${failure?.error ?? "unknown"}`,
            ""
          ])
        ]
      : []),
    "Only listed fields are authoritative. Use froglet_get_service for one service at a time."
  ]
  return toolTextResult(lines.join("\n"))
}

// --- Tool: froglet_get_service ---

async function handleGetService(args, config) {
  const response = await getService({
    ...ctx(config),
    request: {
      provider_id: args.provider_id,
      provider_url: args.provider_url,
      service_id: args.service_id
    }
  })
  const lines = [
    ...summarizeService(response.service ?? {}),
    ...serviceAuthorityNotes(response.service ?? {})
  ]
  return toolTextResult(lines.join("\n"))
}

// --- Tool: froglet_invoke ---

async function handleInvoke(args, config) {
  const response = await invokeService({
    ...ctx(config),
    request: {
      provider_id: args.provider_id,
      provider_url: args.provider_url,
      service_id: args.service_id,
      input: args.input,
      timeout_secs: args.timeout_secs
    }
  })
  const effectiveResult =
    response.result !== undefined ? response.result : response.task?.result
  const lines = response.task
    ? [
        ...summarizeTask(response.task),
        `terminal: ${response.terminal === true}`,
        `result: ${formatObject(effectiveResult)}`,
        ...(response.terminal === true
          ? []
          : ["pending: use froglet_task with wait=true if you need the final result"])
      ]
    : [`status: ${response.status ?? "unknown"}`, `result: ${formatObject(effectiveResult)}`]
  return toolTextResult(lines.join("\n"))
}

// --- Tool: froglet_local_services ---

async function handleLocalServices(args, config) {
  if (args.service_id) {
    const response = await getLocalService({
      ...ctx(config),
      serviceId: args.service_id
    })
    const lines = [
      ...summarizeService(response.service ?? {}),
      ...serviceAuthorityNotes(response.service ?? {})
    ]
    return toolTextResult(lines.join("\n"))
  }

  const response = await listLocalServices(ctx(config))
  const services = Array.isArray(response.services) ? response.services : []
  const lines = [
    `services: ${services.length}`,
    "",
    ...(services.length > 0
      ? services.flatMap((service, index) => [`${index + 1}.`, ...summarizeService(service), ""])
      : ["no local services"]),
    "",
    "Only listed fields are authoritative. Pass service_id for one service at a time."
  ]
  return toolTextResult(lines.join("\n"))
}

// --- Tool: froglet_project ---

async function handleProject(args, config) {
  const c = ctx(config)

  switch (args.action) {
    case "list": {
      const response = await listProjects(c)
      const projects = Array.isArray(response.projects) ? response.projects : []
      const lines = [
        `projects: ${projects.length}`,
        "",
        ...(projects.length > 0
          ? projects.flatMap((project, index) => [`${index + 1}.`, ...summarizeProject(project), ""])
          : ["no projects"])
      ]
      return toolTextResult(lines.join("\n"))
    }
    case "create": {
      const name = firstDefined(args.name, args.project_name, args.service_name, args.title)
      const summary = firstDefined(args.summary, args.description)
      const resultJson = firstDefined(
        args.result_json,
        args.result,
        args.returns,
        args.response,
        args.return_value,
        args.output
      )
      const response = await createProject({
        ...c,
        request: {
          project_id: args.project_id,
          service_id: args.service_id,
          offer_id: args.offer_id,
          name,
          summary,
          runtime: args.runtime,
          package_kind: args.package_kind,
          entrypoint_kind: args.entrypoint_kind,
          entrypoint: args.entrypoint,
          contract_version: args.contract_version,
          mounts: args.mounts,
          inline_source: args.inline_source,
          starter: args.starter,
          result_json: resultJson,
          price_sats: args.price_sats,
          publication_state: args.publication_state,
          mode: args.mode,
          input_schema: args.input_schema,
          output_schema: args.output_schema
        }
      })
      const project = response.project ?? {}
      const lines = [...summarizeProject(project)]
      if (project.project_id && project.publication_state === "active") {
        lines.push("")
        lines.push("published: true")
        lines.push("publish_status: already_published")
        lines.push(`published_service_id: ${project.service_id ?? "unknown"}`)
        lines.push(`published_offer_id: ${project.offer_id ?? "unknown"}`)
      } else {
        lines.push("")
        lines.push("published: false")
        lines.push("next_step: use write_file, build, test, then publish when the service is ready")
      }
      return toolTextResult(lines.join("\n"))
    }
    case "get": {
      const response = await getProject({
        ...c,
        projectId: args.project_id
      })
      return toolTextResult(summarizeProject(response.project ?? {}).join("\n"))
    }
    case "read_file": {
      const response = await readProjectFile({
        ...c,
        projectId: args.project_id,
        path: args.path
      })
      const lines = [
        `project_id: ${response.project_id ?? args.project_id ?? "unknown"}`,
        `path: ${response.path ?? args.path ?? "unknown"}`,
        "",
        response.contents ?? ""
      ]
      return toolTextResult(lines.join("\n"))
    }
    case "write_file": {
      const response = await writeProjectFile({
        ...c,
        projectId: args.project_id,
        path: args.path,
        contents: args.contents
      })
      const lines = [
        `status: ${response.status ?? "unknown"}`,
        `project_id: ${response.project_id ?? args.project_id ?? "unknown"}`,
        `path: ${response.path ?? args.path ?? "unknown"}`
      ]
      return toolTextResult(lines.join("\n"))
    }
    case "build": {
      const response = await buildProject({
        ...c,
        projectId: args.project_id
      })
      return toolTextResult(summarizeProject(response.project ?? {}).join("\n"))
    }
    case "test": {
      const response = await testProject({
        ...c,
        projectId: args.project_id,
        input: args.input
      })
      const lines = [
        ...summarizeProject(response.project ?? {}),
        `output: ${formatObject(response.output)}`
      ]
      return toolTextResult(lines.join("\n"))
    }
    case "publish": {
      const response = await publishProject({
        ...c,
        projectId: args.project_id
      })
      const lines = [
        `status: ${response.status ?? "unknown"}`,
        ...summarizeService(response.service ?? {}),
        ...serviceAuthorityNotes(response.service ?? {}),
        `offer_hash: ${response.offer_hash ?? "none"}`
      ]
      return toolTextResult(lines.join("\n"))
    }
    case "publish_artifact": {
      const response = await publishArtifact({
        ...c,
        request: {
          service_id: args.service_id,
          offer_id: args.offer_id,
          summary: args.summary,
          artifact_path: args.artifact_path,
          wasm_module_hex: args.wasm_module_hex,
          inline_source: args.inline_source,
          oci_reference: args.oci_reference,
          oci_digest: args.oci_digest,
          runtime: args.runtime,
          package_kind: args.package_kind,
          entrypoint_kind: args.entrypoint_kind,
          entrypoint: args.entrypoint,
          contract_version: args.contract_version,
          mounts: args.mounts,
          mode: args.mode,
          price_sats: args.price_sats,
          publication_state: args.publication_state,
          input_schema: args.input_schema,
          output_schema: args.output_schema
        }
      })
      const lines = [
        `status: ${response.status ?? "unknown"}`,
        ...summarizeService(response.service ?? {}),
        ...serviceAuthorityNotes(response.service ?? {}),
        `offer_hash: ${response.offer_hash ?? "none"}`
      ]
      return toolTextResult(lines.join("\n"))
    }
    default:
      throw new Error(`Unknown project action: ${args.action}`)
  }
}

// --- Tool: froglet_task ---

async function handleTask(args, config) {
  if (args.wait) {
    const response = await waitTask({
      ...ctx(config),
      taskId: args.task_id,
      timeoutSecs: args.timeout_secs,
      pollIntervalSecs: args.poll_interval_secs
    })
    return toolTextResult(summarizeTask(response.task ?? {}).join("\n"))
  }

  const response = await getTask({
    ...ctx(config),
    taskId: args.task_id
  })
  return toolTextResult(summarizeTask(response.task ?? {}).join("\n"))
}

// --- Tool: froglet_compute ---

async function handleCompute(args, config) {
  const response = await runCompute({
    ...ctx(config),
    request: {
      provider_id: args.provider_id,
      provider_url: args.provider_url,
      input: args.input,
      wasm_module_hex: args.wasm_module_hex,
      inline_source: args.inline_source,
      oci_reference: args.oci_reference,
      oci_digest: args.oci_digest,
      runtime: args.runtime,
      package_kind: args.package_kind,
      entrypoint_kind: args.entrypoint_kind,
      entrypoint: args.entrypoint,
      contract_version: args.contract_version,
      mounts: args.mounts,
      timeout_secs: args.timeout_secs
    }
  })
  const lines = response.task
    ? [...summarizeTask(response.task), `terminal: ${response.terminal === true}`]
    : [`status: ${response.status ?? "unknown"}`, `result: ${formatObject(response.result)}`]
  return toolTextResult(lines.join("\n"))
}

// --- Tool definitions ---

export function buildToolDefinitions(config) {
  return [
    {
      name: "froglet_status",
      description: "Check Froglet node health, runtime/provider/discovery status, and configuration.",
      inputSchema: {
        type: "object",
        properties: {}
      }
    },
    {
      name: "froglet_logs",
      description: "Tail runtime or provider logs from the Froglet node.",
      inputSchema: {
        type: "object",
        properties: {
          target: {
            type: "string",
            enum: ["runtime", "provider", "all"],
            description: "Which log target to tail."
          },
          lines: {
            type: "integer",
            minimum: 1,
            maximum: 500,
            description: "Number of log lines to return."
          }
        }
      }
    },
    {
      name: "froglet_discover",
      description:
        "Search for remote Froglet services across the discovery network. Returns matching services with metadata, schemas, and pricing.",
      inputSchema: {
        type: "object",
        properties: {
          query: { type: "string", description: "Free-text search query to filter services." },
          limit: {
            type: "integer",
            minimum: 1,
            maximum: config.maxSearchLimit,
            description: "Maximum number of results."
          },
          include_inactive: { type: "boolean", description: "Include inactive/hidden services." }
        }
      }
    },
    {
      name: "froglet_get_service",
      description:
        "Get authoritative details for a specific remote Froglet service or data-service binding. Returns full metadata including offer_kind, resource_kind, and input/output schemas.",
      inputSchema: {
        type: "object",
        required: ["service_id"],
        properties: {
          service_id: { type: "string" },
          provider_id: { type: "string", description: "Provider node ID (optional, for disambiguation)." },
          provider_url: { type: "string", description: "Direct provider URL (optional)." }
        }
      }
    },
    {
      name: "froglet_invoke",
      description:
        "Invoke a remote Froglet named service or data-service binding by service_id. May return a result synchronously or a task_id for async polling via froglet_task. Use froglet_compute for open-ended compute instead of inventing a service call.",
      inputSchema: {
        type: "object",
        required: ["service_id"],
        properties: {
          service_id: { type: "string" },
          input: { description: "Input payload matching the service's input_schema." },
          provider_id: { type: "string" },
          provider_url: { type: "string" },
          timeout_secs: { type: "integer", minimum: 1, maximum: 600 }
        }
      }
    },
    {
      name: "froglet_local_services",
      description:
        "List or inspect local Froglet named/data service bindings on this node. Without service_id lists all; with service_id returns authoritative details including offer_kind and resource_kind. Direct compute offers are not listed here; use froglet_compute.",
      inputSchema: {
        type: "object",
        properties: {
          service_id: {
            type: "string",
            description: "If provided, get details for this specific local service."
          }
        }
      }
    },
    {
      name: "froglet_project",
      description:
        "Manage Froglet service authoring projects. Actions: list, create, get, read_file, write_file, build, test, publish, publish_artifact. Typical workflow: create -> write_file -> build -> test -> publish. Projects currently cover project-backed WAT->Wasm and inline-source Python authoring. Use publish_artifact for prebuilt Wasm modules or OCI-backed/container profiles. For simple fixed-response services use create with result_json and publication_state=active.",
      inputSchema: {
        type: "object",
        required: ["action"],
        properties: {
          action: {
            type: "string",
            enum: ["list", "create", "get", "read_file", "write_file", "build", "test", "publish", "publish_artifact"],
            description: "Project sub-action."
          },
          project_id: { type: "string" },
          name: { type: "string", description: "Friendly name; derives IDs if explicit IDs are omitted." },
          summary: { type: "string" },
          runtime: { type: "string", description: "Execution runtime such as wasm, python, or container." },
          package_kind: { type: "string", description: "Execution package shape such as inline_module, inline_source, or oci_image." },
          entrypoint_kind: { type: "string", description: "Entrypoint style such as handler, script, or builtin." },
          entrypoint: { type: "string" },
          contract_version: { type: "string" },
          mounts: { description: "Mount handles or bindings." },
          wasm_module_hex: { type: "string", description: "Inline Wasm module bytes in hex for publish_artifact." },
          inline_source: { type: "string", description: "Inline source text for Python-backed authored services." },
          starter: { type: "string" },
          result_json: { description: "Static JSON result for simple constant-return services." },
          price_sats: { type: "integer", minimum: 0 },
          publication_state: { type: "string", enum: ["active", "hidden"] },
          mode: { type: "string", enum: ["sync", "async"] },
          input_schema: {},
          output_schema: {},
          path: { type: "string", description: "File path within the project (for read_file/write_file)." },
          contents: { type: "string", description: "File contents (for write_file)." },
          input: { description: "Test input (for test action)." },
          service_id: { type: "string" },
          offer_id: { type: "string" },
          artifact_path: { type: "string" },
          oci_reference: { type: "string" },
          oci_digest: { type: "string" }
        }
      }
    },
    {
      name: "froglet_task",
      description:
        "Get status of or wait for an async Froglet task. Use wait=true to poll until completion.",
      inputSchema: {
        type: "object",
        required: ["task_id"],
        properties: {
          task_id: { type: "string" },
          wait: { type: "boolean", description: "If true, poll until task completes." },
          timeout_secs: { type: "integer", minimum: 1, maximum: 600 },
          poll_interval_secs: { type: "number", minimum: 0.1, maximum: 10 }
        }
      }
    },
    {
      name: "froglet_compute",
      description:
        "Execute open-ended compute on a Froglet provider without a registered service. You must target a provider with provider_id or provider_url because service discovery/listing does not expose this path automatically; it uses the provider's direct compute offer. Current supported inputs are inline Wasm via wasm_module_hex, inline Python via inline_source, OCI-backed Wasm, and OCI image execution for python/container profiles. Zip archives are not yet supported.",
      inputSchema: {
        type: "object",
        properties: {
          input: { description: "Input payload for the compute workload." },
          wasm_module_hex: { type: "string", description: "Inline Wasm module bytes in hex for runtime=wasm package_kind=inline_module." },
          inline_source: { type: "string", description: "Inline Python source for runtime=python package_kind=inline_source." },
          oci_reference: { type: "string" },
          oci_digest: { type: "string" },
          runtime: { type: "string", description: "Required runtime selector such as wasm, python, or container." },
          package_kind: { type: "string", description: "Required package selector such as inline_module, inline_source, or oci_image." },
          entrypoint_kind: { type: "string", description: "Entrypoint style such as handler or script." },
          entrypoint: { type: "string" },
          contract_version: { type: "string" },
          mounts: {},
          provider_id: {
            type: "string",
            description: "Target provider node ID. Provide this or provider_url for direct compute."
          },
          provider_url: {
            type: "string",
            description: "Target provider base URL. Provide this or provider_id for direct compute."
          },
          timeout_secs: { type: "integer", minimum: 1, maximum: 600 }
        }
      }
    }
  ]
}

const handlers = {
  froglet_status: handleStatus,
  froglet_logs: handleLogs,
  froglet_discover: handleDiscover,
  froglet_get_service: handleGetService,
  froglet_invoke: handleInvoke,
  froglet_local_services: handleLocalServices,
  froglet_project: handleProject,
  froglet_task: handleTask,
  froglet_compute: handleCompute
}

export async function handleToolCall(name, args, config) {
  const handler = handlers[name]
  if (!handler) {
    return errorResult(new Error(`Unknown tool: ${name}`))
  }
  try {
    return await handler(args ?? {}, config)
  } catch (error) {
    return errorResult(error)
  }
}
