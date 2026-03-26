export function appendRaw(lines, payload, includeRaw) {
  if (!includeRaw) {
    return lines
  }
  return [...lines, "", JSON.stringify(payload, null, 2)]
}

export function formatObject(value) {
  return JSON.stringify(value ?? null)
}

export function normalizeStringField(obj, field, fallback = "unknown") {
  const value = obj?.[field]
  if (typeof value === "string" && value.trim().length > 0) {
    return value
  }
  return fallback
}

export function normalizeRuntime(service) {
  return normalizeStringField(service, "runtime")
}

export function normalizePackageKind(service) {
  return normalizeStringField(service, "package_kind")
}

export function normalizeEntrypointKind(service) {
  const value = normalizeStringField(service, "entrypoint_kind")
  if (value !== "unknown") return value
  if (normalizeRuntime(service) === "builtin") return "builtin"
  return "unknown"
}

export function normalizeContractVersion(service) {
  return normalizeStringField(service, "contract_version")
}

export function normalizeMounts(service) {
  if (service?.mounts !== undefined) {
    return service.mounts
  }
  if (service?.requested_access !== undefined) {
    return service.requested_access
  }
  return []
}

export function summarizeService(service) {
  return [
    `service_id: ${service?.service_id ?? "unknown"}`,
    `offer_id: ${service?.offer_id ?? "unknown"}`,
    `offer_kind: ${service?.offer_kind ?? "unknown"}`,
    `resource_kind: ${service?.resource_kind ?? "unknown"}`,
    `project_id: ${service?.project_id ?? "none"}`,
    `summary: ${service?.summary ?? "none"}`,
    `runtime: ${normalizeRuntime(service)}`,
    `package_kind: ${normalizePackageKind(service)}`,
    `entrypoint_kind: ${normalizeEntrypointKind(service)}`,
    `entrypoint: ${service?.entrypoint ?? "unknown"}`,
    `contract_version: ${normalizeContractVersion(service)}`,
    `mounts: ${formatObject(normalizeMounts(service))}`,
    `mode: ${service?.mode ?? "unknown"}`,
    `price_sats: ${service?.price_sats ?? "unknown"}`,
    `publication_state: ${service?.publication_state ?? "unknown"}`,
    `provider_id: ${service?.provider_id ?? "unknown"}`,
    `input_schema: ${formatObject(service?.input_schema)}`,
    `output_schema: ${formatObject(service?.output_schema)}`
  ]
}

export function summarizeProject(project) {
  return [
    `project_id: ${project?.project_id ?? "unknown"}`,
    `service_id: ${project?.service_id ?? "unknown"}`,
    `offer_id: ${project?.offer_id ?? "unknown"}`,
    `summary: ${project?.summary ?? "none"}`,
    `runtime: ${normalizeRuntime(project)}`,
    `package_kind: ${normalizePackageKind(project)}`,
    `entrypoint_kind: ${normalizeEntrypointKind(project)}`,
    `entrypoint: ${project?.entrypoint ?? "unknown"}`,
    `contract_version: ${normalizeContractVersion(project)}`,
    `mounts: ${formatObject(normalizeMounts(project))}`,
    `mode: ${project?.mode ?? "unknown"}`,
    `price_sats: ${project?.price_sats ?? "unknown"}`,
    `publication_state: ${project?.publication_state ?? "unknown"}`,
    `build_artifact_path: ${project?.build_artifact_path ?? "none"}`,
    `module_hash: ${project?.module_hash ?? "none"}`
  ]
}

export function summarizeTask(task) {
  return [
    `task_id: ${task?.task_id ?? task?.deal_id ?? "unknown"}`,
    `status: ${task?.status ?? "unknown"}`,
    `provider_id: ${task?.provider_id ?? "unknown"}`,
    `result: ${formatObject(task?.result)}`,
    `error: ${task?.error ?? "none"}`
  ]
}

export function serviceAuthorityNotes(service) {
  return [
    service?.input_schema == null
      ? "input_contract: no input_schema is declared; Froglet may forward any JSON input and the service may ignore it."
      : "input_contract: input_schema is declared; stay within that contract when invoking the service.",
    "Only listed fields are authoritative; do not infer behavior beyond offer_kind, resource_kind, runtime, package_kind, entrypoint_kind, entrypoint, contract_version, mounts, input_schema, and output_schema."
  ]
}

export function firstDefined(...values) {
  for (const value of values) {
    if (value !== undefined) {
      return value
    }
  }
  return undefined
}
