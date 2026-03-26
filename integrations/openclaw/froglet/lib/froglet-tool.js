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
} from "../../../shared/froglet-lib/summarize.js"

function context(config) {
  return {
    baseUrl: config.baseUrl,
    authTokenPath: config.authTokenPath,
    requestTimeoutMs: config.requestTimeoutMs
  }
}

export function registerFrogletTool(api, config) {
  api.registerTool(
    {
      name: "froglet",
      description:
        "Authoritative Froglet tool. Use exact Froglet actions instead of guessing. For local services use list_local_services or get_local_service. For remote discovery-backed services use discover_services or get_service. For named or data-service bindings use invoke_service. For simple fixed-response services, use create_project with result_json, price_sats, and publication_state=active. Example: if the user says create a service called ping which just returns \"pong\" for free, use action=create_project, name=ping, result_json=\"pong\", price_sats=0, publication_state=active. For authored services use create_project, write_file, build_project, test_project, and publish_project. Prefer runtime, package_kind, entrypoint_kind, entrypoint, contract_version, mounts, and explicit artifact fields when the user asks for execution metadata. Use run_compute for open-ended compute through the provider's direct compute offer, and include provider_id or provider_url.",
      parameters: {
        type: "object",
        additionalProperties: true,
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
              "Optional inline Wasm module bytes in hex. Use this for direct inline Wasm compute or publish_artifact with runtime=wasm package_kind=inline_module."
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
              "Optional static JSON result to scaffold into a new project. Use this for simple constant-return services. Example: for a service that just returns \"pong\", set result_json to \"pong\". Summary is metadata only and does not generate code."
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
          target: { type: "string", enum: ["runtime", "provider", "all"] },
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
          oci_digest: { type: "string" },
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
              `discovery_mode: ${response.discovery?.mode ?? "unknown"}`,
              `reference_discovery_enabled: ${response.reference_discovery?.enabled === true}`,
              `reference_discovery_publish_enabled: ${response.reference_discovery?.publish_enabled === true}`,
              `reference_discovery_connected: ${response.reference_discovery?.connected === true}`,
              `reference_discovery_url: ${response.reference_discovery?.url ?? "none"}`,
              `reference_discovery_last_error: ${response.reference_discovery?.last_error ?? "none"}`,
              `projects_root: ${response.projects_root ?? "unknown"}`,
              `compute_offer_id: ${response.raw_compute_offer_id ?? "execute.compute"}`
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
              lines.push("next_step: write_file, build_project, test_project, then publish_project when the service is ready")
            }
            return toolTextResult(
              appendRaw(lines, response, includeRaw).join("\n")
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
                  ...serviceAuthorityNotes(response.service ?? {}),
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
            return toolTextResult(
              appendRaw(
                [
                  `status: ${response.status ?? "unknown"}`,
                  ...summarizeService(response.service ?? {}),
                  ...serviceAuthorityNotes(response.service ?? {}),
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
                : ["no local services"]),
              "",
              "Only listed fields are authoritative. Use get_local_service for one service at a time."
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
                  ...serviceAuthorityNotes(response.service ?? {})
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
                  ...serviceAuthorityNotes(response.service ?? {})
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
