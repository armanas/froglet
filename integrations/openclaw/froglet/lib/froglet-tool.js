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

function appendRaw(lines, payload, includeRaw) {
  if (!includeRaw) {
    return lines
  }
  return [...lines, "", JSON.stringify(payload, null, 2)]
}

function formatObject(value) {
  return JSON.stringify(value ?? null)
}

function summarizeService(service) {
  return [
    `service_id: ${service?.service_id ?? "unknown"}`,
    `offer_id: ${service?.offer_id ?? "unknown"}`,
    `project_id: ${service?.project_id ?? "none"}`,
    `summary: ${service?.summary ?? "none"}`,
    `execution_kind: ${service?.execution_kind ?? "unknown"}`,
    `abi_version: ${service?.abi_version ?? "unknown"}`,
    `mode: ${service?.mode ?? "unknown"}`,
    `price_sats: ${service?.price_sats ?? "unknown"}`,
    `publication_state: ${service?.publication_state ?? "unknown"}`,
    `provider_id: ${service?.provider_id ?? "unknown"}`
  ]
}

function summarizeProject(project) {
  return [
    `project_id: ${project?.project_id ?? "unknown"}`,
    `service_id: ${project?.service_id ?? "unknown"}`,
    `offer_id: ${project?.offer_id ?? "unknown"}`,
    `summary: ${project?.summary ?? "none"}`,
    `execution_kind: ${project?.execution_kind ?? "unknown"}`,
    `mode: ${project?.mode ?? "unknown"}`,
    `price_sats: ${project?.price_sats ?? "unknown"}`,
    `publication_state: ${project?.publication_state ?? "unknown"}`,
    `entrypoint: ${project?.entrypoint ?? "unknown"}`,
    `build_artifact_path: ${project?.build_artifact_path ?? "none"}`,
    `module_hash: ${project?.module_hash ?? "none"}`
  ]
}

function summarizeTask(task) {
  return [
    `task_id: ${task?.task_id ?? task?.deal_id ?? "unknown"}`,
    `status: ${task?.status ?? "unknown"}`,
    `provider_id: ${task?.provider_id ?? "unknown"}`,
    `result: ${formatObject(task?.result)}`,
    `error: ${task?.error ?? "none"}`
  ]
}

function context(config) {
  return {
    baseUrl: config.baseUrl,
    authTokenPath: config.authTokenPath,
    requestTimeoutMs: config.requestTimeoutMs
  }
}

function firstDefined(...values) {
  for (const value of values) {
    if (value !== undefined) {
      return value
    }
  }
  return undefined
}

export function registerFrogletTool(api, config) {
  api.registerTool(
    {
      name: "froglet",
      description:
        "Authoritative Froglet tool. Use it for Froglet-specific questions instead of guessing. It can list and inspect services, invoke a named service, create and publish local service projects, or run expert raw compute.",
      parameters: {
        type: "object",
        additionalProperties: true,
        required: ["action"],
        properties: {
          action: {
            type: "string",
            enum: [
              "discover_services",
              "get_service",
              "invoke_service",
              "list_local_services",
              "get_local_service",
              "create_project",
              "list_projects",
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
          service_id: { type: "string" },
          project_id: { type: "string" },
          offer_id: { type: "string" },
          summary: { type: "string" },
          starter: {
            type: "string",
            description:
              "Optional starter code scaffold. This is only initial code scaffolding, not a publish mode."
          },
          path: { type: "string" },
          contents: { type: "string" },
          input: {},
          result_json: {
            description:
              "Optional static JSON result to scaffold into a new project. Use this to create simple services like a service that always returns \"lol\"."
          },
          output_schema: {},
          input_schema: {},
          price_sats: { type: "integer", minimum: 0 },
          publication_state: { type: "string", enum: ["active", "hidden"] },
          mode: { type: "string", enum: ["sync", "async"] },
          target: { type: "string", enum: ["runtime", "provider", "all"] },
          lines: { type: "integer", minimum: 1, maximum: 500 },
          provider_id: { type: "string" },
          provider_url: { type: "string" },
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
          wasm_module_hex: { type: "string" },
          oci_reference: { type: "string" },
          oci_digest: { type: "string" },
          execution_kind: {
            type: "string",
            enum: ["wasm_inline", "wasm_oci"]
          },
          abi_version: { type: "string" },
          include_raw: { type: "boolean" }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const clientContext = context(config)

        switch (args.action) {
          case "status": {
            const response = await frogletStatus(clientContext)
            const lines = [
              `node_id: ${response.node_id ?? "unknown"}`,
              `runtime_healthy: ${response.runtime?.healthy === true}`,
              `provider_healthy: ${response.provider?.healthy === true}`,
              `projects_root: ${response.projects_root ?? "unknown"}`,
              `raw_compute_offer_id: ${response.raw_compute_offer_id ?? "execute.wasm"}`
            ]
            return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
          }
          case "tail_logs": {
            const response = await frogletTailLogs({
              ...clientContext,
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
            return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
          }
          case "restart": {
            const response = await frogletRestart({
              ...clientContext,
              target: args.target
            })
            const results = Array.isArray(response.results) ? response.results : []
            const lines = results.flatMap((entry) => [
              `${entry.target}: ${entry.status ?? "unknown"}`,
              `stdout_preview: ${entry.stdout_preview ?? "none"}`,
              `stderr_preview: ${entry.stderr_preview ?? "none"}`,
              ""
            ])
            return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
          }
          case "list_projects": {
            const response = await listProjects(clientContext)
            const projects = Array.isArray(response.projects) ? response.projects : []
            const lines = [
              `projects: ${projects.length}`,
              "",
              ...(projects.length > 0
                ? projects.flatMap((project, index) => [`${index + 1}.`, ...summarizeProject(project), ""])
                : ["no projects"])
            ]
            return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
          }
          case "create_project": {
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
              ...clientContext,
              request: {
                project_id: args.project_id,
                service_id: args.service_id,
                offer_id: args.offer_id,
                name,
                summary,
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
            let finalPayload = response
            if (project.project_id && project.publication_state === "active") {
              const publication = await publishProject({
                ...clientContext,
                projectId: project.project_id
              })
              lines.push("")
              lines.push("published: true")
              lines.push(`request_id: ${publication.request_id ?? "unknown"}`)
              lines.push(`publish_status: ${publication.status ?? "unknown"}`)
              lines.push(`published_service_id: ${publication.evidence?.service_id ?? project.service_id ?? "unknown"}`)
              lines.push(`published_offer_id: ${publication.evidence?.offer_id ?? project.offer_id ?? "unknown"}`)
              finalPayload = {
                project: response.project,
                publication
              }
            } else {
              lines.push("")
              lines.push("published: false")
              lines.push("next_step: call publish_project to make this service available to other Froglet nodes")
            }
            return toolTextResult(
              appendRaw(lines, finalPayload, includeRaw).join("\n")
            )
          }
          case "get_project": {
            const response = await getProject({
              ...clientContext,
              projectId: args.project_id
            })
            return toolTextResult(
              appendRaw(summarizeProject(response.project ?? {}), response, includeRaw).join("\n")
            )
          }
          case "read_file": {
            const response = await readProjectFile({
              ...clientContext,
              projectId: args.project_id,
              path: args.path
            })
            return toolTextResult(
              appendRaw(
                [
                  `project_id: ${response.project_id ?? args.project_id ?? "unknown"}`,
                  `path: ${response.path ?? args.path ?? "unknown"}`,
                  "",
                  response.contents ?? ""
                ],
                response,
                includeRaw
              ).join("\n")
            )
          }
          case "write_file": {
            const response = await writeProjectFile({
              ...clientContext,
              projectId: args.project_id,
              path: args.path,
              contents: args.contents
            })
            return toolTextResult(
              appendRaw(
                [
                  `status: ${response.status ?? "unknown"}`,
                  `project_id: ${response.project_id ?? args.project_id ?? "unknown"}`,
                  `path: ${response.path ?? args.path ?? "unknown"}`
                ],
                response,
                includeRaw
              ).join("\n")
            )
          }
          case "build_project": {
            const response = await buildProject({
              ...clientContext,
              projectId: args.project_id
            })
            return toolTextResult(
              appendRaw(summarizeProject(response.project ?? {}), response, includeRaw).join("\n")
            )
          }
          case "test_project": {
            const response = await testProject({
              ...clientContext,
              projectId: args.project_id,
              input: args.input
            })
            return toolTextResult(
              appendRaw(
                [...summarizeProject(response.project ?? {}), `output: ${formatObject(response.output)}`],
                response,
                includeRaw
              ).join("\n")
            )
          }
          case "publish_project": {
            const response = await publishProject({
              ...clientContext,
              projectId: args.project_id
            })
            return toolTextResult(
              appendRaw(
                [
                  `status: ${response.status ?? "unknown"}`,
                  ...summarizeService(response.service ?? {}),
                  `offer_hash: ${response.offer_hash ?? "none"}`
                ],
                response,
                includeRaw
              ).join("\n")
            )
          }
          case "publish_artifact": {
            const response = await publishArtifact({
              ...clientContext,
              request: {
                service_id: args.service_id,
                offer_id: args.offer_id,
                summary: args.summary,
                artifact_path: args.artifact_path,
                wasm_module_hex: args.wasm_module_hex,
                oci_reference: args.oci_reference,
                oci_digest: args.oci_digest,
                execution_kind: args.execution_kind,
                abi_version: args.abi_version,
                mode: args.mode,
                price_sats: args.price_sats,
                publication_state: args.publication_state,
                input_schema: args.input_schema,
                output_schema: args.output_schema
              }
            })
            return toolTextResult(
              appendRaw(
                [
                  `status: ${response.status ?? "unknown"}`,
                  ...summarizeService(response.service ?? {}),
                  `offer_hash: ${response.offer_hash ?? "none"}`
                ],
                response,
                includeRaw
              ).join("\n")
            )
          }
          case "list_local_services": {
            const response = await listLocalServices(clientContext)
            const services = Array.isArray(response.services) ? response.services : []
            const lines = [
              `services: ${services.length}`,
              "",
              ...(services.length > 0
                ? services.flatMap((service, index) => [`${index + 1}.`, ...summarizeService(service), ""])
                : ["no local services"])
            ]
            return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
          }
          case "get_local_service": {
            const response = await getLocalService({
              ...clientContext,
              serviceId: args.service_id
            })
            return toolTextResult(
              appendRaw(
                [
                  ...summarizeService(response.service ?? {}),
                  `input_schema: ${formatObject(response.service?.input_schema)}`,
                  `output_schema: ${formatObject(response.service?.output_schema)}`,
                  "Only the listed summary/schema fields are authoritative."
                ],
                response,
                includeRaw
              ).join("\n")
            )
          }
          case "discover_services": {
            const response = await discoverServices({
              ...clientContext,
              limit: args.limit ?? config.defaultSearchLimit,
              includeInactive: args.include_inactive === true,
              query: args.query
            })
            const services = Array.isArray(response.services) ? response.services : []
            const lines = [
              `services: ${services.length}`,
              "",
              ...(services.length > 0
                ? services.flatMap((service, index) => [`${index + 1}.`, ...summarizeService(service), ""])
                : ["no remote services"])
            ]
            return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
          }
          case "get_service": {
            const response = await getService({
              ...clientContext,
              request: {
                provider_id: args.provider_id,
                provider_url: args.provider_url,
                service_id: args.service_id
              }
            })
            return toolTextResult(
              appendRaw(
                [
                  ...summarizeService(response.service ?? {}),
                  `input_schema: ${formatObject(response.service?.input_schema)}`,
                  `output_schema: ${formatObject(response.service?.output_schema)}`,
                  "Only the listed summary/schema fields are authoritative."
                ],
                response,
                includeRaw
              ).join("\n")
            )
          }
          case "invoke_service": {
            const response = await invokeService({
              ...clientContext,
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
                    : ["pending: use wait_task with the returned task_id if you need the final result"])
                ]
              : [`status: ${response.status ?? "unknown"}`, `result: ${formatObject(effectiveResult)}`]
            return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
          }
          case "run_compute": {
            const response = await runCompute({
              ...clientContext,
              request: {
                provider_id: args.provider_id,
                provider_url: args.provider_url,
                input: args.input,
                wasm_module_hex: args.wasm_module_hex,
                oci_reference: args.oci_reference,
                oci_digest: args.oci_digest,
                execution_kind: args.execution_kind,
                abi_version: args.abi_version,
                timeout_secs: args.timeout_secs
              }
            })
            const lines = response.task
              ? [...summarizeTask(response.task), `terminal: ${response.terminal === true}`]
              : [`status: ${response.status ?? "unknown"}`, `result: ${formatObject(response.result)}`]
            return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
          }
          case "get_task": {
            const response = await getTask({
              ...clientContext,
              taskId: args.task_id
            })
            return toolTextResult(
              appendRaw(summarizeTask(response.task ?? {}), response, includeRaw).join("\n")
            )
          }
          case "wait_task": {
            const response = await waitTask({
              ...clientContext,
              taskId: args.task_id,
              timeoutSecs: args.timeout_secs,
              pollIntervalSecs: args.poll_interval_secs
            })
            return toolTextResult(
              appendRaw(summarizeTask(response.task ?? {}), response, includeRaw).join("\n")
            )
          }
          default:
            throw new Error(`Unsupported froglet action: ${args.action}`)
        }
      }
    },
    { optional: true }
  )
}
