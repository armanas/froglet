import {
  discoverServices,
  frogletStatus,
  getLocalService,
  getService,
  getTask,
  invokeService,
  listLocalServices,
  publishArtifact,
  runCompute,
  waitTask
} from "./froglet-client.js"
import { toolTextResult } from "./shared.js"
import {
  appendRaw,
  firstDefined,
  formatObject,
  serviceAuthorityNotes,
  summarizeService,
  summarizeTask
} from "./summarize.js"

/**
 * Extract the subset of config fields needed for provider API calls.
 *
 * @param {object} config
 */
function providerCtx(config) {
  return {
    providerUrl: config.providerUrl,
    providerAuthTokenPath: config.providerAuthTokenPath,
    requestTimeoutMs: config.requestTimeoutMs
  }
}

/**
 * Extract the subset of config fields needed for runtime API calls.
 *
 * @param {object} config
 */
function runtimeCtx(config) {
  return {
    runtimeUrl: config.runtimeUrl,
    runtimeAuthTokenPath: config.runtimeAuthTokenPath,
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
  const response = await frogletStatus({
    ...providerCtx(config),
    ...runtimeCtx(config)
  })
  const offerIds = computeOfferIds(response)
  const identity = response._identity ?? {}
  const lines = [
    `healthy: ${response.healthy === true}`,
    `node_id: ${response.node_id ?? "unknown"}`,
    `discovery_mode: ${identity.discovery?.mode ?? response.discovery?.mode ?? "unknown"}`,
    `reference_discovery_enabled: ${(identity.reference_discovery ?? response.reference_discovery)?.enabled === true}`,
    `reference_discovery_publish_enabled: ${(identity.reference_discovery ?? response.reference_discovery)?.publish_enabled === true}`,
    `reference_discovery_connected: ${(identity.reference_discovery ?? response.reference_discovery)?.connected === true}`,
    `reference_discovery_url: ${(identity.reference_discovery ?? response.reference_discovery)?.url ?? "none"}`,
    `reference_discovery_last_error: ${(identity.reference_discovery ?? response.reference_discovery)?.last_error ?? "none"}`,
    `compute_offer_ids: ${offerIds.join(", ")}`,
    "",
    `provider_healthy: ${response.provider?.healthy === true}`,
    `runtime_healthy: ${response.runtime?.healthy === true}`
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleDiscover(args, config, includeRaw) {
  const response = await discoverServices({
    ...runtimeCtx(config),
    limit: args.limit ?? config.defaultSearchLimit,
    includeInactive: args.include_inactive === true,
    query: args.query
  })
  const providers = Array.isArray(response.providers) ? response.providers : []
  const services = Array.isArray(response.services) ? response.services : []
  const lines = [
    `providers: ${providers.length}`,
    `services: ${services.length}`,
    "",
    ...(services.length > 0
      ? services.flatMap((service, index) => [`${index + 1}.`, ...summarizeService(service), ""])
      : ["no remote services discovered"]),
    "Only listed fields are authoritative. Use get_service for one service at a time."
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleGetService(args, config, includeRaw) {
  const response = await getService({
    ...runtimeCtx(config),
    searchLimit: args.limit ?? config.defaultSearchLimit,
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
    ...runtimeCtx(config),
    searchLimit: args.limit ?? config.defaultSearchLimit,
    request: {
      provider_id: resolvedProviderId(args),
      provider_url: resolvedProviderUrl(args),
      service_id: resolvedServiceId(args),
      input: args.input
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
      ...providerCtx(config),
      serviceId
    })
    const lines = [
      ...summarizeService(response.service ?? {}),
      ...serviceAuthorityNotes(response.service ?? {})
    ]
    return renderResult(lines, response, includeRaw)
  }

  const response = await listLocalServices(providerCtx(config))
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

async function handlePublishArtifact(args, config, includeRaw) {
  const response = await publishArtifact({
    ...providerCtx(config),
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

async function handleTask(args, config, includeRaw) {
  if (args.wait) {
    const response = await waitTask({
      ...providerCtx(config),
      ...runtimeCtx(config),
      taskId: args.task_id,
      timeoutSecs: args.timeout_secs,
      pollIntervalSecs: args.poll_interval_secs
    })
    return renderResult(summarizeTask(response.task ?? {}), response, includeRaw)
  }

  const response = await getTask({
    ...providerCtx(config),
    ...runtimeCtx(config),
    taskId: args.task_id
  })
  return renderResult(summarizeTask(response.task ?? {}), response, includeRaw)
}

async function handleCompute(args, config, includeRaw) {
  const response = await runCompute({
    ...runtimeCtx(config),
    searchLimit: args.limit ?? config.defaultSearchLimit,
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
    case "publish_artifact":
      return handlePublishArtifact(args, config, includeRaw)
    case "get_task":
      return handleTask({ ...args, wait: false }, config, includeRaw)
    case "wait_task":
      return handleTask({ ...args, wait: true }, config, includeRaw)
    case "run_compute":
      return handleCompute(args, config, includeRaw)
    // Removed actions — return clear error messages
    case "tail_logs":
      throw new Error("Log tailing removed; use systemd journal directly")
    case "restart":
      throw new Error("Restart removed; use systemctl directly")
    case "list_projects":
    case "create_project":
    case "get_project":
    case "read_file":
    case "write_file":
    case "build_project":
    case "test_project":
    case "publish_project":
      throw new Error("Project authoring not available in current API")
    default:
      throw new Error(`Unknown Froglet action: ${args.action}`)
  }
}
