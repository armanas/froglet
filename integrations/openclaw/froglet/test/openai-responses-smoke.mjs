import register from "../index.js"
import { readFileSync } from "node:fs"

function loadFrogletTool() {
  const tools = new Map()
  register({
    config: {
      hostProduct: "openclaw",
      baseUrl: process.env.FROGLET_BASE_URL ?? "http://127.0.0.1:9191",
      authTokenPath:
        process.env.FROGLET_AUTH_TOKEN_PATH ??
        "/home/armanas/froglet-e2e/data/runtime/froglet-control.token",
      requestTimeoutMs: Number.parseInt(process.env.FROGLET_REQUEST_TIMEOUT_MS ?? "15000", 10),
      defaultSearchLimit: Number.parseInt(
        process.env.FROGLET_DEFAULT_SEARCH_LIMIT ?? "10",
        10
      ),
      maxSearchLimit: Number.parseInt(process.env.FROGLET_MAX_SEARCH_LIMIT ?? "50", 10)
    },
    registerTool(definition) {
      tools.set(definition.name, definition)
    },
    logger: {
      info() {},
      error() {}
    }
  })

  const froglet = tools.get("froglet")
  if (!froglet) {
    throw new Error("froglet tool was not registered")
  }
  return froglet
}

async function callResponses(body) {
  const response = await fetch("https://api.openai.com/v1/responses", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${process.env.OPENAI_API_KEY}`,
      "Content-Type": "application/json"
    },
    body: JSON.stringify(body)
  })
  const text = await response.text()
  let json
  try {
    json = JSON.parse(text)
  } catch {
    throw new Error(`non-JSON OpenAI response (${response.status}): ${text}`)
  }
  if (!response.ok) {
    throw new Error(`OpenAI error (${response.status}): ${JSON.stringify(json)}`)
  }
  return json
}

async function runScenario(froglet, name, prompt, requiredActions = [], { injectBeforeExecute } = {}) {
  const toolCalls = []
  const toolOutputs = []
  let previousResponseId = null
  let input = prompt

  for (let step = 0; step < 12; step += 1) {
    const body = {
      model: process.env.OPENAI_MODEL ?? "gpt-4.1-mini",
      input,
      tools: [
        {
          type: "function",
          name: froglet.name,
          description: froglet.description,
          parameters: froglet.parameters
        }
      ],
      max_output_tokens: 700
    }
    if (previousResponseId) {
      body.previous_response_id = previousResponseId
    }

    const response = await callResponses(body)
    previousResponseId = response.id
    const calls = (response.output ?? []).filter((item) => item.type === "function_call")
    if (calls.length === 0) {
      const actions = new Set(toolCalls.map((call) => call.action).filter(Boolean))
      for (const action of requiredActions) {
        if (!actions.has(action)) {
          throw new Error(
            `scenario ${name} did not execute required action ${action}; saw ${JSON.stringify([...actions])}`
          )
        }
      }
      return {
        name,
        response_id: response.id,
        tool_calls: toolCalls,
        tool_outputs: toolOutputs,
        final_text: response.output_text ?? ""
      }
    }

    const outputs = []
    for (const call of calls) {
      const args = JSON.parse(call.arguments || "{}")
      toolCalls.push({
        action: args.action ?? null,
        service_id: args.service_id ?? null,
        project_id: args.project_id ?? null
      })
      // Hook may mutate args in-place before the tool call, e.g. to inject
      // fixtures the model should not supply (called once per tool call per step).
      if (injectBeforeExecute) {
        injectBeforeExecute(args)
      }
      try {
        const result = await froglet.execute(call.name, args)
        const output = result.content?.[0]?.text ?? ""
        toolOutputs.push(output)
        outputs.push({
          type: "function_call_output",
          call_id: call.call_id,
          output
        })
      } catch (error) {
        const output = `ERROR: ${error.message}`
        toolOutputs.push(output)
        outputs.push({
          type: "function_call_output",
          call_id: call.call_id,
          output
        })
      }
    }
    input = outputs
  }

  throw new Error(`scenario ${name} exceeded the tool-call step limit`)
}

async function main() {
  if (!process.env.OPENAI_API_KEY) {
    throw new Error("OPENAI_API_KEY is required")
  }

  const froglet = loadFrogletTool()
  const suffix = Date.now()
  const validWasmHex = readFileSync(new URL("./fixtures/valid-wasm.hex", import.meta.url), "utf8")
    .trim()
  const scenarios = [
    {
      name: "status_create_publish_discover_invoke",
      prompt:
        `Use the froglet tool to inspect status, create a free active service named oa-smoke-ping-${suffix} ` +
        `that returns {"message":"pong"}, list local services, discover services, and invoke the new service. ` +
        `Return the service_id and final invocation result.`,
      requiredActions: [
        "status",
        "create_project",
        "list_local_services",
        "discover_services",
        "invoke_service"
      ]
    },
    {
      name: "project_and_service_details",
      prompt:
        `Use the froglet tool to list projects, then call get_local_service with service_id=oa-smoke-ping-${suffix}. ` +
        `After that, tail the last 5 runtime log lines. Return only a compact summary.`,
      requiredActions: ["list_projects", "get_local_service", "tail_logs"]
    },
    {
      name: "direct_compute_wasm",
      prompt:
        'Call froglet exactly once with action "run_compute", provider_url "http://127.0.0.1:8080", ' +
        'runtime "wasm", and package_kind "inline_module". Do NOT supply wasm_module_hex — the harness ' +
        "will inject it. Return the compute result.",
      requiredActions: ["run_compute"],
      injectBeforeExecute(args) {
        if (!args.wasm_module_hex) {
          args.wasm_module_hex = validWasmHex
        }
      }
    },
    {
      name: "expected_missing_service_error",
      prompt:
        `Use the froglet tool to invoke a definitely missing service id missing-service-${suffix}. ` +
        "Return the exact error you get.",
      requiredActions: ["invoke_service"]
    }
  ]

  const results = []
  for (const scenario of scenarios) {
    results.push(
      await runScenario(froglet, scenario.name, scenario.prompt, scenario.requiredActions, {
        injectBeforeExecute: scenario.injectBeforeExecute
      })
    )
  }

  console.log(JSON.stringify(results, null, 2))
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
