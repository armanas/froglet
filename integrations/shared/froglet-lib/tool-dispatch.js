import {
  buildProject,
  createProject,
  discoverServices,
  frogletRestart,
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
} from "./froglet-client.js"
import { toolTextResult } from "./shared.js"
import {
  appendRaw,
  firstDefined,
  formatObject,
  serviceAuthorityNotes,
  summarizeProject,
  summarizeService,
  summarizeTask
} from "./summarize.js"

function ctx(config) {
  return {
    baseUrl: config.baseUrl,
    authTokenPath: config.authTokenPath,
    requestTimeoutMs: config.requestTimeoutMs
  }
}

function renderResult(lines, response, includeRaw) {
  return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
}

function resolvedProviderId(args) {
  return firstDefined(args.provider_id, args.free_provider_id, args.paid_provider_id)
}

function resolvedProviderUrl(args) {
  return firstDefined(args.provider_url, args.free_provider_url, args.paid_provider_url)
}

function resolvedServiceId(args) {
  return firstDefined(args.service_id, args.free_service_id, args.async_service_id)
}

function computeOfferIds(response) {
  if (Array.isArray(response.raw_compute_offer_ids) && response.raw_compute_offer_ids.length > 0) {
    return response.raw_compute_offer_ids
  }
  if (typeof response.raw_compute_offer_id === "string" && response.raw_compute_offer_id.length > 0) {
    return [response.raw_compute_offer_id]
  }
  return ["execute.compute"]
}

function resolveProjectFileTarget(args) {
  let projectId = args.project_id
  let filePath = args.path
  if (typeof filePath === "string") {
    const match = filePath.match(/\/projects\/([^/]+)\/(.+)$/)
    if (match) {
      if (!projectId) {
        projectId = match[1]
      }
      filePath = match[2]
    } else if (filePath.startsWith("/")) {
      filePath = filePath.replace(/^\/+/, "")
    }
  }
  return { projectId, path: filePath }
}

function summarizeMutationResponse(response) {
  const offer = response.offer ?? {}
  const payload = offer.offer?.payload ?? {}
  const service = {
    service_id: offer.service_id ?? response.evidence?.service_id ?? "unknown",
    offer_id: payload.offer_id ?? response.evidence?.offer_id ?? "unknown",
    offer_kind: payload.offer_kind ?? "unknown",
    resource_kind: "service",
    project_id: offer.project_id ?? "none",
    summary: offer.summary ?? response.summary ?? "none",
    runtime: offer.runtime ?? "unknown",
    package_kind: offer.package_kind ?? "unknown",
    entrypoint_kind: offer.entrypoint_kind ?? "unknown",
    entrypoint: offer.entrypoint ?? "unknown",
    contract_version: offer.contract_version ?? "unknown",
    mounts: offer.mounts ?? [],
    mode: offer.mode ?? "unknown",
    price_sats: payload.price_sats ?? "unknown",
    publication_state: offer.publication_state ?? "unknown",
    provider_id: response.evidence?.provider_id ?? payload.provider_id ?? "unknown",
    input_schema: offer.input_schema,
    output_schema: offer.output_schema
  }
  return [
    `status: ${response.status ?? "unknown"}`,
    ...summarizeService(service),
    ...serviceAuthorityNotes(service),
    `offer_hash: ${response.evidence?.offer_hash ?? response.offer_hash ?? "none"}`
  ]
}

async function handleStatus(args, config, includeRaw) {
  const response = await frogletStatus(ctx(config))
  const runtimeHealthy = response.components?.runtime?.healthy ?? response.runtime?.healthy
  const providerHealthy = response.components?.provider?.healthy ?? response.provider?.healthy
  const offerIds = computeOfferIds(response)
  const lines = [
    `service: ${response.service ?? "froglet"}`,
    `healthy: ${response.healthy === true}`,
    `node_id: ${response.node_id ?? "unknown"}`,
    `discovery_mode: ${response.discovery?.mode ?? "unknown"}`,
    `reference_discovery_enabled: ${response.reference_discovery?.enabled === true}`,
    `reference_discovery_publish_enabled: ${response.reference_discovery?.publish_enabled === true}`,
    `reference_discovery_connected: ${response.reference_discovery?.connected === true}`,
    `reference_discovery_url: ${response.reference_discovery?.url ?? "none"}`,
    `reference_discovery_last_error: ${response.reference_discovery?.last_error ?? "none"}`,
    `projects_root: ${response.projects_root ?? "unknown"}`,
    `compute_offer_ids: ${offerIds.join(", ")}`,
    "",
    `runtime_healthy: ${runtimeHealthy === true}`,
    `provider_healthy: ${providerHealthy === true}`
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleLogs(args, config, includeRaw) {
  const response = await frogletTailLogs({
    ...ctx(config),
    target: args.target,
    lines: args.lines
  })
  const logs = Array.isArray(response.logs) ? response.logs : []
  const components = logs
    .map((entry) => entry.component ?? entry.target)
    .filter((value) => typeof value === "string" && value.length > 0)
  const lines = [
    `service: ${response.service ?? "froglet"}`,
    `scope: ${response.scope ?? "node"}`,
    `components: ${components.join(", ") || "none"}`,
    "",
    ...logs.flatMap((entry) => [
      `${entry.component ?? entry.target ?? "unknown"}:`,
      ...(Array.isArray(entry.lines) && entry.lines.length > 0 ? entry.lines : ["no lines"]),
      ""
    ])
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleRestart(args, config, includeRaw) {
  const response = await frogletRestart({
    ...ctx(config),
    target: args.target
  })
  const results = Array.isArray(response.results) ? response.results : []
  const lines = [
    `service: ${response.service ?? "froglet"}`,
    `scope: ${response.scope ?? "node"}`,
    "",
    ...results.flatMap((entry) => [
      `${entry.component ?? entry.target ?? "unknown"}: ${entry.status ?? "unknown"}`,
      `stdout_preview: ${entry.stdout_preview ?? "none"}`,
      `stderr_preview: ${entry.stderr_preview ?? "none"}`,
      ""
    ])
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleDiscover(args, config, includeRaw) {
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
    "Only listed fields are authoritative. Use get_service for one service at a time."
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleGetService(args, config, includeRaw) {
  const response = await getService({
    ...ctx(config),
    request: {
      provider_id: resolvedProviderId(args),
      provider_url: resolvedProviderUrl(args),
      service_id: resolvedServiceId(args)
    }
  })
  const lines = [
    ...summarizeService(response.service ?? {}),
    ...serviceAuthorityNotes(response.service ?? {})
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleInvoke(args, config, includeRaw) {
  const response = await invokeService({
    ...ctx(config),
    request: {
      provider_id: resolvedProviderId(args),
      provider_url: resolvedProviderUrl(args),
      service_id: resolvedServiceId(args),
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
          : ["pending: use wait_task with the returned task_id if you need the final result"])
      ]
    : [`status: ${response.status ?? "unknown"}`, `result: ${formatObject(effectiveResult)}`]
  return renderResult(lines, response, includeRaw)
}

async function handleLocalServices(args, config, includeRaw) {
  const serviceId = resolvedServiceId(args)
  if (serviceId) {
    const response = await getLocalService({
      ...ctx(config),
      serviceId
    })
    const lines = [
      ...summarizeService(response.service ?? {}),
      ...serviceAuthorityNotes(response.service ?? {})
    ]
    return renderResult(lines, response, includeRaw)
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
    "Only listed fields are authoritative. Use get_local_service for one service at a time."
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleProject(args, config, includeRaw) {
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
      return renderResult(lines, response, includeRaw)
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
      return renderResult(lines, response, includeRaw)
    }
    case "get": {
      const response = await getProject({
        ...c,
        projectId: args.project_id
      })
      return renderResult(summarizeProject(response.project ?? {}), response, includeRaw)
    }
    case "read_file": {
      const target = resolveProjectFileTarget(args)
      const response = await readProjectFile({
        ...c,
        projectId: target.projectId,
        path: target.path
      })
      const lines = [
        `project_id: ${response.project_id ?? target.projectId ?? "unknown"}`,
        `path: ${response.path ?? target.path ?? "unknown"}`,
        "",
        response.contents ?? ""
      ]
      return renderResult(lines, response, includeRaw)
    }
    case "write_file": {
      const target = resolveProjectFileTarget(args)
      const response = await writeProjectFile({
        ...c,
        projectId: target.projectId,
        path: target.path,
        contents: args.contents
      })
      const lines = [
        `status: ${response.status ?? "unknown"}`,
        `project_id: ${response.project_id ?? target.projectId ?? "unknown"}`,
        `path: ${response.path ?? target.path ?? "unknown"}`
      ]
      return renderResult(lines, response, includeRaw)
    }
    case "build": {
      const response = await buildProject({
        ...c,
        projectId: args.project_id
      })
      return renderResult(summarizeProject(response.project ?? {}), response, includeRaw)
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
      return renderResult(lines, response, includeRaw)
    }
    case "publish": {
      const response = await publishProject({
        ...c,
        projectId: args.project_id
      })
      return renderResult(summarizeMutationResponse(response), response, includeRaw)
    }
    case "publish_artifact": {
      const response = await publishArtifact({
        ...c,
        request: {
          service_id: resolvedServiceId(args),
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
      return renderResult(summarizeMutationResponse(response), response, includeRaw)
    }
    default:
      throw new Error(`Unknown project action: ${args.action}`)
  }
}

async function handleTask(args, config, includeRaw) {
  if (args.wait) {
    const response = await waitTask({
      ...ctx(config),
      taskId: args.task_id,
      timeoutSecs: args.timeout_secs,
      pollIntervalSecs: args.poll_interval_secs
    })
    return renderResult(summarizeTask(response.task ?? {}), response, includeRaw)
  }

  const response = await getTask({
    ...ctx(config),
    taskId: args.task_id
  })
  return renderResult(summarizeTask(response.task ?? {}), response, includeRaw)
}

async function handleCompute(args, config, includeRaw) {
  const response = await runCompute({
    ...ctx(config),
    request: {
      provider_id: resolvedProviderId(args),
      provider_url: resolvedProviderUrl(args),
      input: args.input,
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
      timeout_secs: args.timeout_secs ?? 15
    }
  })
  const lines = response.task
    ? [...summarizeTask(response.task), `terminal: ${response.terminal === true}`]
    : [`status: ${response.status ?? "unknown"}`, `result: ${formatObject(response.result)}`]
  return renderResult(lines, response, includeRaw)
}

export async function dispatchFrogletAction(args, config, { includeRaw = false } = {}) {
  switch (args.action) {
    case "status":
      return handleStatus(args, config, includeRaw)
    case "tail_logs":
      return handleLogs(args, config, includeRaw)
    case "restart":
      return handleRestart(args, config, includeRaw)
    case "discover_services":
      return handleDiscover(args, config, includeRaw)
    case "get_service":
      return handleGetService(args, config, includeRaw)
    case "invoke_service":
      return handleInvoke(args, config, includeRaw)
    case "list_local_services":
      return handleLocalServices(args, config, includeRaw)
    case "get_local_service":
      return handleLocalServices(args, config, includeRaw)
    case "list_projects":
      return handleProject({ ...args, action: "list" }, config, includeRaw)
    case "get_project":
      return handleProject({ ...args, action: "get" }, config, includeRaw)
    case "create_project":
      return handleProject({ ...args, action: "create" }, config, includeRaw)
    case "read_file":
      return handleProject({ ...args, action: "read_file" }, config, includeRaw)
    case "write_file":
      return handleProject({ ...args, action: "write_file" }, config, includeRaw)
    case "build_project":
      return handleProject({ ...args, action: "build" }, config, includeRaw)
    case "test_project":
      return handleProject({ ...args, action: "test" }, config, includeRaw)
    case "publish_project":
      return handleProject({ ...args, action: "publish" }, config, includeRaw)
    case "publish_artifact":
      return handleProject({ ...args, action: "publish_artifact" }, config, includeRaw)
    case "get_task":
      return handleTask({ ...args, wait: false }, config, includeRaw)
    case "wait_task":
      return handleTask({ ...args, wait: true }, config, includeRaw)
    case "run_compute":
      return handleCompute(args, config, includeRaw)
    default:
      throw new Error(`Unknown Froglet action: ${args.action}`)
  }
}
