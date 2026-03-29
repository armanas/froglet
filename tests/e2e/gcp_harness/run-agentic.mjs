import assert from "node:assert/strict"
import { readFileSync } from "node:fs"

import {
  callResponses,
  defaultModel,
  ensureFinalText,
  loadFrogletTool,
  parseCliArgs,
  readJson,
  requireApiKey,
  writeJson,
} from "./common.mjs"

function resolveValue(value, fixtures) {
  if (Array.isArray(value)) {
    return value.map((entry) => resolveValue(entry, fixtures))
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, resolveValue(entry, fixtures)]))
  }
  if (value === "__fixture_valid_wasm_hex") {
    return fixtures.validWasmHex
  }
  return value
}

function mergeMissing(target, source) {
  for (const [key, value] of Object.entries(source ?? {})) {
    if (key === "discover_service_id") {
      continue
    }
    if (target[key] === undefined) {
      target[key] = value
    }
  }
}

async function runDeterministicScenario(tool, scenario, fixtures) {
  const toolCalls = []
  const toolOutputs = []
  let previousResponseId = null
  let input = scenario.prompt
  try {
    for (let step = 0; step < 16; step += 1) {
      const body = {
        model: defaultModel,
        input,
        tools: [
          {
            type: "function",
            name: tool.name,
            description: tool.description,
            parameters: tool.parameters,
          },
        ],
        max_output_tokens: 1200,
      }
      if (previousResponseId) {
        body.previous_response_id = previousResponseId
      }

      const response = await callResponses(body)
      previousResponseId = response.id
      const calls = (response.output ?? []).filter((item) => item.type === "function_call")
      if (calls.length === 0) {
        let finalText = response.output_text ?? ""
        if (finalText.trim().length === 0) {
          const finalized = await ensureFinalText(
            previousResponseId,
            "The required tool work is complete. Reply with the final plain-text answer now."
          )
          previousResponseId = finalized.responseId
          finalText = finalized.finalText
        }
        const executed = new Set(toolCalls.map((call) => call.action).filter(Boolean))
        for (const action of scenario.required_tool_actions ?? []) {
          assert.ok(executed.has(action), `${scenario.scenario_id} did not execute required action ${action}`)
        }
        for (const needle of scenario.result_oracles?.must_contain ?? []) {
          assert.ok(finalText.includes(needle) || toolOutputs.some((output) => output.includes(needle)), `${scenario.scenario_id} missing ${needle}`)
        }
        return {
          scenario_id: scenario.scenario_id,
          status: "passed",
          response_id: previousResponseId,
          tool_calls: toolCalls,
          tool_outputs: toolOutputs,
          final_text: finalText,
        }
      }

      const outputs = []
      for (const call of calls) {
        const args = JSON.parse(call.arguments || "{}")
        toolCalls.push({ action: args.action ?? null })
        mergeMissing(args, resolveValue(scenario.fixture_injections, fixtures))
        try {
          const result = await tool.execute(call.name, args)
          const output = result.content?.[0]?.text ?? ""
          toolOutputs.push(output)
          outputs.push({
            type: "function_call_output",
            call_id: call.call_id,
            output,
          })
        } catch (error) {
          const output = `ERROR: ${error.message}`
          toolOutputs.push(output)
          outputs.push({
            type: "function_call_output",
            call_id: call.call_id,
            output,
          })
        }
      }
      input = outputs
    }
    throw new Error(`scenario ${scenario.scenario_id} exceeded the tool-call limit`)
  } catch (error) {
    return {
      scenario_id: scenario.scenario_id,
      status: "failed",
      response_id: previousResponseId,
      tool_calls: toolCalls,
      tool_outputs: toolOutputs,
      error: String(error.message ?? error),
    }
  }
}

async function runExploratorySession(tool, scenarioSet, fixtures) {
  const anomalies = []
  const toolCalls = []
  let previousResponseId = null
  let input = [
    "You are a senior QA engineer exploring Froglet through the OpenClaw froglet tool.",
    "Cover the full action surface below, starting with safe happy paths and then boundary cases.",
    `Required action coverage: ${(scenarioSet.agentic?.exploratory?.must_cover_actions ?? []).join(", ")}`,
    "Look for security regressions, data leaks, missing validation, inconsistent summaries vs raw payloads, and bad recovery behaviour.",
    "When you finish, report anomalies with severity critical/high/medium/low/info and reproduction steps.",
  ].join("\n")

  for (let step = 0; step < (scenarioSet.agentic?.exploratory?.max_steps ?? 40); step += 1) {
    const body = {
      model: defaultModel,
      input,
      tools: [
        {
          type: "function",
          name: tool.name,
          description: tool.description,
          parameters: tool.parameters,
        },
      ],
      max_output_tokens: 1800,
    }
    if (previousResponseId) {
      body.previous_response_id = previousResponseId
    }
    const response = await callResponses(body)
    previousResponseId = response.id
    const calls = (response.output ?? []).filter((item) => item.type === "function_call")
    if (calls.length === 0) {
      let finalText = response.output_text ?? ""
      if (finalText.trim().length === 0) {
        const finalized = await ensureFinalText(
          previousResponseId,
          "Exploration is complete. Return the structured final assessment now."
        )
        previousResponseId = finalized.responseId
        finalText = finalized.finalText
      }
      const executed = new Set(toolCalls.map((call) => call.action).filter(Boolean))
      for (const action of scenarioSet.agentic?.exploratory?.must_cover_actions ?? []) {
        if (!executed.has(action)) {
          anomalies.push({
            severity: "high",
            context: `exploratory session did not cover required action ${action}`,
          })
        }
      }
      const severityPattern = /(?:severity|level):\s*(critical|high|medium|low|info)/gi
      let match
      while ((match = severityPattern.exec(finalText)) !== null) {
        anomalies.push({
          severity: match[1].toLowerCase(),
          context: finalText.slice(Math.max(0, match.index - 160), Math.min(finalText.length, match.index + 240)).trim(),
        })
      }
      return {
        response_id: previousResponseId,
        tool_calls: toolCalls,
        anomalies,
        model_assessment: finalText,
      }
    }

    const outputs = []
    for (const call of calls) {
      const args = JSON.parse(call.arguments || "{}")
      mergeMissing(args, resolveValue(fixtures.exploratoryDefaults ?? {}, fixtures))
      toolCalls.push({ action: args.action ?? null })
      try {
        const result = await tool.execute(call.name, args)
        outputs.push({
          type: "function_call_output",
          call_id: call.call_id,
          output: result.content?.[0]?.text ?? "",
        })
      } catch (error) {
        anomalies.push({
          severity: "high",
          context: `tool call failed for ${JSON.stringify(args)} -> ${error.message}`,
        })
        outputs.push({
          type: "function_call_output",
          call_id: call.call_id,
          output: `ERROR: ${error.message}`,
        })
      }
    }
    input = outputs
  }

  return {
    response_id: previousResponseId,
    tool_calls: toolCalls,
    anomalies: [...anomalies, { severity: "info", context: "Reached max exploratory steps" }],
    model_assessment: "Exploratory session reached the maximum step limit.",
  }
}

async function main() {
  const { values } = parseCliArgs({
    inventory: { type: "string", short: "i" },
    scenarios: { type: "string", short: "s" },
    "base-url": { type: "string" },
    "auth-token-path": { type: "string" },
    out: { type: "string", short: "o" },
  })
  if (!values.inventory || !values.scenarios || !values["base-url"] || !values["auth-token-path"] || !values.out) {
    throw new Error("--inventory, --scenarios, --base-url, --auth-token-path, and --out are required")
  }

  requireApiKey()
  const scenarioSet = await readJson(values.scenarios)
  await readJson(values.inventory)

  const tool = loadFrogletTool({
    baseUrl: values["base-url"],
    authTokenPath: values["auth-token-path"],
    requestTimeoutMs: 20_000,
  })
  const fixtures = {
    validWasmHex: readFileSync(
      new URL("../../../integrations/openclaw/froglet/test/fixtures/valid-wasm.hex", import.meta.url),
      "utf8"
    ).trim(),
    exploratoryDefaults: {
      free_provider_id: scenarioSet.seeds?.free?.provider_id,
      paid_provider_id: scenarioSet.seeds?.paid?.provider_id,
      paid_provider_url: scenarioSet.seeds?.paid?.provider_public_url,
      free_service_id: scenarioSet.seeds?.free?.services?.free_static?.service_id,
      async_service_id: scenarioSet.seeds?.paid?.services?.async_echo?.service_id,
      wasm_module_hex: "__fixture_valid_wasm_hex",
    },
  }

  const deterministic = []
  for (const scenario of scenarioSet.agentic?.deterministic ?? []) {
    deterministic.push(await runDeterministicScenario(tool, scenario, fixtures))
  }
  const exploratory = await runExploratorySession(tool, scenarioSet, fixtures)
  const severe = exploratory.anomalies.filter((entry) => entry.severity === "critical" || entry.severity === "high")
  const deterministicFailures = deterministic.filter((entry) => entry.status === "failed")

  await writeJson(values.out, {
    generated_at: new Date().toISOString(),
    model: defaultModel,
    deterministic,
    exploratory,
  })

  if (severe.length > 0) {
    process.exitCode = 1
  }
  if (deterministicFailures.length > 0) {
    process.exitCode = 1
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
