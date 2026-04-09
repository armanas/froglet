import assert from "node:assert/strict"

import {
  callResponses,
  defaultModel,
  ensureFinalText,
  executeTool,
  getJsonPath,
} from "./common.mjs"

function resolveValue(value, fixtures) {
  if (Array.isArray(value)) {
    return value.map((entry) => resolveValue(entry, fixtures))
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [key, resolveValue(entry, fixtures)])
    )
  }
  if (value === "__fixture_valid_wasm_hex") {
    return fixtures.validWasmHex
  }
  return value
}

function mergeMissing(target, source) {
  for (const [key, value] of Object.entries(source ?? {})) {
    if (target[key] === undefined) {
      target[key] = value
    }
  }
}

function subsetMatch(candidate, expected) {
  if (expected == null || typeof expected !== "object" || Array.isArray(expected)) {
    return candidate === expected
  }
  if (candidate == null || typeof candidate !== "object" || Array.isArray(candidate)) {
    return false
  }
  return Object.entries(expected).every(([key, value]) => subsetMatch(candidate[key], value))
}

function findScenarioOutput(toolOutputs, action, match = "last") {
  const matches = toolOutputs.filter((entry) => entry.action === action)
  if (matches.length === 0) {
    throw new Error(`missing tool output for action ${action}`)
  }
  return match === "first" ? matches[0] : matches[matches.length - 1]
}

function assertToolOutputAssertions(toolOutputs, assertions) {
  for (const assertion of assertions ?? []) {
    const output = findScenarioOutput(toolOutputs, assertion.action, assertion.match)
    const raw = output.raw
    const value = assertion.path ? getJsonPath(raw, assertion.path) : raw
    if (Object.hasOwn(assertion, "exists")) {
      assert.equal(value !== undefined, assertion.exists, `${assertion.action}:${assertion.path} exists`)
    }
    if (Object.hasOwn(assertion, "equals")) {
      assert.deepEqual(value, assertion.equals, `${assertion.action}:${assertion.path} equals`)
    }
    if (Object.hasOwn(assertion, "contains")) {
      assert.ok(Array.isArray(value), `${assertion.action}:${assertion.path} must be an array`)
      assert.ok(
        value.some((entry) => subsetMatch(entry, assertion.contains)),
        `${assertion.action}:${assertion.path} did not contain ${JSON.stringify(assertion.contains)}`
      )
    }
    if (Object.hasOwn(assertion, "not_contains")) {
      assert.ok(Array.isArray(value), `${assertion.action}:${assertion.path} must be an array`)
      assert.ok(
        !value.some((entry) => subsetMatch(entry, assertion.not_contains)),
        `${assertion.action}:${assertion.path} unexpectedly contained ${JSON.stringify(assertion.not_contains)}`
      )
    }
    if (Object.hasOwn(assertion, "json_contains")) {
      const serialized = JSON.stringify(value)
      assert.ok(
        serialized.includes(assertion.json_contains),
        `${assertion.action}:${assertion.path ?? "<root>"} missing ${assertion.json_contains}`
      )
    }
    if (Object.hasOwn(assertion, "json_not_contains")) {
      const serialized = JSON.stringify(value)
      assert.ok(
        !serialized.includes(assertion.json_not_contains),
        `${assertion.action}:${assertion.path ?? "<root>"} unexpectedly contained ${assertion.json_not_contains}`
      )
    }
  }
}

function assertTextContains(text, needles, label) {
  for (const needle of needles ?? []) {
    assert.ok(text.includes(needle), `${label} missing ${needle}`)
  }
}

function assertTextNotContains(text, needles, label) {
  for (const needle of needles ?? []) {
    assert.ok(!text.includes(needle), `${label} unexpectedly contained ${needle}`)
  }
}

export async function runCuratedScenario(tool, scenario, fixtures = {}, context = {}) {
  const toolCalls = []
  const toolOutputs = []
  const toolErrors = []
  const pendingActions = []
  const fixtureArgs = resolveValue(scenario.fixture_injections ?? {}, fixtures)
  let previousResponseId = null
  let input = scenario.prompt

  for (let step = 0; step < (scenario.max_steps ?? 12); step += 1) {
    const response = await callResponses({
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
      ...(previousResponseId ? { previous_response_id: previousResponseId } : {}),
    })
    previousResponseId = response.id
    const calls = (response.output ?? []).filter((item) => item.type === "function_call")
    if (calls.length === 0) {
      let finalText = response.output_text ?? ""
      if (finalText.trim().length === 0) {
        const finalized = await ensureFinalText(
          previousResponseId,
          "The required tool work is complete. Return the requested final answer now."
        )
        previousResponseId = finalized.responseId
        finalText = finalized.finalText
      }

      const executedActions = new Set(toolCalls.map((call) => call.action).filter(Boolean))
      for (const action of scenario.required_tool_actions ?? []) {
        if (!executedActions.has(action)) {
          throw new Error(
            `scenario ${scenario.scenario_id} did not execute required action ${action}; saw ${JSON.stringify([...executedActions])}`
          )
        }
      }

      if ((scenario.require_wait_on_pending_actions ?? []).length > 0 && pendingActions.length > 0) {
        if (!executedActions.has("wait_task")) {
          throw new Error(
            `scenario ${scenario.scenario_id} left pending task(s) without wait_task: ${JSON.stringify(pendingActions)}`
          )
        }
      }

      const expectError = scenario.result_oracles?.expect_error === true
      if (expectError && toolErrors.length === 0) {
        throw new Error(`scenario ${scenario.scenario_id} expected a tool error but none occurred`)
      }
      if (!expectError && toolErrors.length > 0) {
        throw new Error(
          `scenario ${scenario.scenario_id} encountered tool errors: ${JSON.stringify(toolErrors)}`
        )
      }

      const combinedText = [finalText, ...toolOutputs.map((entry) => entry.text)].join("\n")
      assertTextContains(combinedText, scenario.result_oracles?.final_text_contains, "combined_text")
      assertTextNotContains(
        combinedText,
        scenario.result_oracles?.final_text_not_contains,
        "combined_text"
      )
      assertTextContains(
        toolErrors.join("\n"),
        scenario.result_oracles?.error_contains,
        "tool_errors"
      )
      assertToolOutputAssertions(toolOutputs, scenario.result_oracles?.tool_output_assertions)

      return {
        scenario_id: scenario.scenario_id,
        status: "passed",
        response_id: previousResponseId,
        tool_calls: toolCalls,
        tool_outputs: toolOutputs,
        tool_errors: toolErrors,
        final_text: finalText,
        context,
      }
    }

    const outputs = []
    for (const call of calls) {
      const args = JSON.parse(call.arguments || "{}")
      if (!args.action && fixtureArgs.action) {
        Object.assign(args, fixtureArgs)
      } else if (args.action === fixtureArgs.action) {
        Object.assign(args, fixtureArgs)
      }
      if (
        !args.task_id &&
        context.task_id &&
        (args.action === "get_task" || args.action === "wait_task")
      ) {
        args.task_id = context.task_id
      }
      args.include_raw = true
      const action = args.action ?? null
      toolCalls.push({
        step,
        action,
        args_summary: Object.keys(args).filter((key) => key !== "action" && key !== "include_raw"),
      })
      try {
        const executed = await executeTool(tool, args)
        toolOutputs.push({
          action,
          text: executed.text,
          raw: executed.raw,
        })
        if (
          (scenario.require_wait_on_pending_actions ?? []).includes(action) &&
          executed.raw?.terminal === false
        ) {
          pendingActions.push(action)
          if (!context.task_id) {
            context.task_id = executed.raw?.task?.task_id ?? executed.raw?.deal?.deal_id ?? null
          }
        }
        outputs.push({
          type: "function_call_output",
          call_id: call.call_id,
          output: executed.text,
        })
      } catch (error) {
        const message = String(error.message ?? error)
        toolErrors.push(message)
        outputs.push({
          type: "function_call_output",
          call_id: call.call_id,
          output: `ERROR: ${message}`,
        })
      }
    }
    input = outputs
  }

  throw new Error(`scenario ${scenario.scenario_id} exceeded the tool-call step limit`)
}

export async function runCuratedSuite(tool, scenarios, fixtures = {}, context = {}) {
  const results = []
  for (const scenario of scenarios ?? []) {
    try {
      const scenarioContext = { ...context }
      results.push(await runCuratedScenario(tool, scenario, fixtures, scenarioContext))
    } catch (error) {
      results.push({
        scenario_id: scenario.scenario_id,
        status: "failed",
        error: String(error.message ?? error),
      })
    }
  }
  const failed = results.filter((entry) => entry.status === "failed")
  return {
    total: results.length,
    passed: results.length - failed.length,
    failed: failed.length,
    results,
  }
}

export async function runExploratorySession(tool, scenarioSet, fixtures) {
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
          context: finalText
            .slice(Math.max(0, match.index - 160), Math.min(finalText.length, match.index + 240))
            .trim(),
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
