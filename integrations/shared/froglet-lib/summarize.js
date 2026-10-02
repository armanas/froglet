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
    ...(service?.starter !== undefined ? [`starter: ${formatObject(service.starter)}`] : []),
    `runtime: ${normalizeRuntime(service)}`,
    `package_kind: ${normalizePackageKind(service)}`,
    `entrypoint_kind: ${normalizeEntrypointKind(service)}`,
    `entrypoint: ${service?.entrypoint ?? "unknown"}`,
    `contract_version: ${normalizeContractVersion(service)}`,
    `mounts: ${formatObject(normalizeMounts(service))}`,
    `capabilities: ${formatObject(service?.capabilities ?? [])}`,
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
    `capabilities: ${formatObject(project?.capabilities ?? [])}`,
    `mode: ${project?.mode ?? "unknown"}`,
    `price_sats: ${project?.price_sats ?? "unknown"}`,
    `publication_state: ${project?.publication_state ?? "unknown"}`,
    `build_artifact_path: ${project?.build_artifact_path ?? "none"}`,
    `module_hash: ${project?.module_hash ?? "none"}`
  ]
}

export function summarizeTask(task) {
  return summarizeExecutionResult({ task })
}

export function executionResultDetails(response) {
  const task = response?.task ?? response?.deal ?? response ?? {}
  const receipt = task.receipt ?? response?.receipt ?? null
  const receiptHash = receipt?.hash ?? task.receipt_hash ?? response?.receipt_hash ?? null
  const details = {
    task_id: task.task_id ?? task.deal_id ?? response?.deal_id ?? null,
    status: task.status ?? response?.status ?? "unknown",
    provider_id: task.provider_id ?? response?.provider_id ?? null,
    result: firstDefined(response?.result, task.result) ?? null,
    result_hash: task.result_hash ?? response?.result_hash ?? receipt?.payload?.result_hash ?? null,
    error: firstDefined(response?.error, task.error) ?? null,
    receipt_hash: receiptHash,
    execution_state: task.execution_state ?? receipt?.payload?.execution_state ?? null,
    settlement_state: task.settlement_state ?? receipt?.payload?.settlement_state ?? null,
    signed_artifacts: {
      quote: response?.quote ?? task.quote ?? null,
      deal: task.deal ?? null,
      receipt,
    },
    evidence: response?.evidence ?? task.evidence ?? null,
    // A receipt, result hash or terminal status is not a cryptographic check.
    // Preserve an upstream report verbatim; this JS adapter runs no verifier.
    receipt_verification: response?.receipt_verification ?? task.receipt_verification ?? {
      status: "not_available",
      verified: false,
      receipt_hash: receiptHash,
      checks: [],
      boundary: "This JavaScript adapter has not independently verified the signed receipt or result binding.",
    },
  }
  for (const field of ["terminal", "idempotency_key", "next_action", "payment_intent_path", "verification"]) {
    const value = firstDefined(response?.[field], task[field])
    if (value !== undefined) details[field] = value
  }
  return details
}

export function summarizeExecutionResult(response) {
  const task = executionResultDetails(response)
  return [
    `task_id: ${task?.task_id ?? task?.deal_id ?? "unknown"}`,
    `status: ${task?.status ?? "unknown"}`,
    `provider_id: ${task?.provider_id ?? "unknown"}`,
    `result: ${formatObject(task?.result)}`,
    `result_hash: ${task.result_hash ?? "none"}`,
    `receipt_hash: ${task.receipt_hash ?? "none"}`,
    `execution_state: ${task.execution_state ?? "unknown"}`,
    `settlement_state: ${task.settlement_state ?? "unknown"}`,
    `receipt_verification: ${formatObject(task.receipt_verification)}`,
    `error: ${task?.error ?? "none"}`
  ]
}

export function serviceAuthorityNotes(service) {
  return [
    service?.input_schema == null
      ? "input_contract: no input_schema is declared; Froglet may forward any JSON input and the service may ignore it."
      : "input_contract: input_schema is declared; stay within that contract when invoking the service.",
    "starter_example: service.starter, when present, is only an example request shape; input_schema and output_schema remain authoritative.",
    "Only listed fields are authoritative; do not infer behavior beyond offer_kind, resource_kind, runtime, package_kind, entrypoint_kind, entrypoint, contract_version, mounts, capabilities, input_schema, and output_schema."
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
