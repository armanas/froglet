/**
 * AI-driven exploratory testing — uses OpenAI API to drive an agent that
 * explores the Froglet API creatively and reports anomalies.
 *
 * Extends the pattern from openai-responses-smoke.mjs.
 *
 * Env vars:
 *   OPENCLAW_API_KEY  – preferred (falls back to OPENAI_API_KEY)
 *   OPENAI_MODEL      – model to use (default: gpt-4.1-mini)
 */
import { readFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const testDir = fileURLToPath(new URL("./", import.meta.url))
const repoRoot = path.resolve(testDir, "../..")
const pluginDir = path.resolve(repoRoot, "integrations/openclaw/froglet")

// Dynamically import the plugin register function
const { default: register } = await import(path.join(pluginDir, "index.js"))

const API_KEY = process.env.OPENCLAW_API_KEY || process.env.OPENAI_API_KEY
if (!API_KEY) {
  console.error("OPENCLAW_API_KEY or OPENAI_API_KEY is required")
  process.exit(1)
}

const MODEL = process.env.OPENAI_MODEL ?? "gpt-4.1-mini"
const MAX_STEPS = 30

function loadFrogletTool() {
  const tools = new Map()
  register({
    config: {
      hostProduct: "openclaw",
      baseUrl: process.env.FROGLET_BASE_URL ?? "http://127.0.0.1:9191",
      authTokenPath:
        process.env.FROGLET_AUTH_TOKEN_PATH ??
        path.resolve(repoRoot, "data/runtime/froglet-control.token"),
      requestTimeoutMs: Number.parseInt(process.env.FROGLET_REQUEST_TIMEOUT_MS ?? "15000", 10),
      defaultSearchLimit: 10,
      maxSearchLimit: 50,
    },
    registerTool(definition) {
      tools.set(definition.name, definition)
    },
    logger: { info() {}, error() {} },
  })

  const froglet = tools.get("froglet")
  if (!froglet) throw new Error("froglet tool was not registered")
  return froglet
}

async function callResponses(body) {
  const response = await fetch("https://api.openai.com/v1/responses", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${API_KEY}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
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

async function runExploratorySession(froglet) {
  const toolCalls = []
  const anomalies = []
  let previousResponseId = null

  const systemPrompt = `You are a senior QA engineer performing exploratory testing on a resource protocol called Froglet.
You have access to one tool called "froglet" with multiple actions.

Your goals:
1. Systematically explore ALL available actions (status, list_projects, create_project, discover_services, list_local_services, invoke_service, get_local_service, tail_logs, run_compute, etc.)
2. Try normal flows first, then edge cases
3. Try invalid inputs, missing parameters, extreme values
4. Look for: unexpected errors, inconsistent responses, missing error messages, slow responses, data that shouldn't be exposed
5. After exploration, provide a structured assessment

For each anomaly found, note:
- Severity: critical / high / medium / low / info
- Description of the issue
- Steps to reproduce

Start by checking status, then systematically explore each action.`

  let input = systemPrompt

  for (let step = 0; step < MAX_STEPS; step += 1) {
    const body = {
      model: MODEL,
      input,
      tools: [
        {
          type: "function",
          name: froglet.name,
          description: froglet.description,
          parameters: froglet.parameters,
        },
      ],
      max_output_tokens: 1500,
    }
    if (previousResponseId) {
      body.previous_response_id = previousResponseId
    }

    const response = await callResponses(body)
    previousResponseId = response.id

    const calls = (response.output ?? []).filter((item) => item.type === "function_call")
    if (calls.length === 0) {
      // Model is done — extract final assessment
      const finalText = response.output_text ?? ""

      // Parse anomalies from the model's response
      const severityPattern = /(?:severity|level):\s*(critical|high|medium|low|info)/gi
      let match
      while ((match = severityPattern.exec(finalText)) !== null) {
        const contextStart = Math.max(0, match.index - 200)
        const contextEnd = Math.min(finalText.length, match.index + 200)
        anomalies.push({
          severity: match[1].toLowerCase(),
          context: finalText.slice(contextStart, contextEnd).trim(),
        })
      }

      return {
        scenarios_explored: toolCalls.length,
        tool_calls: toolCalls,
        anomalies,
        model_assessment: finalText,
      }
    }

    // Execute tool calls
    const outputs = []
    for (const call of calls) {
      const args = JSON.parse(call.arguments || "{}")
      toolCalls.push({
        step,
        action: args.action ?? null,
        args_summary: Object.keys(args).filter((k) => k !== "action").join(","),
      })

      try {
        const result = await froglet.execute(call.name, args)
        const output = result.content?.[0]?.text ?? ""
        outputs.push({
          type: "function_call_output",
          call_id: call.call_id,
          output,
        })
      } catch (error) {
        const output = `ERROR: ${error.message}`
        outputs.push({
          type: "function_call_output",
          call_id: call.call_id,
          output,
        })
      }
    }
    input = outputs
  }

  return {
    scenarios_explored: toolCalls.length,
    tool_calls: toolCalls,
    anomalies: [{ severity: "info", context: "Reached maximum step limit" }],
    model_assessment: "Session ended due to step limit.",
  }
}

async function main() {
  const froglet = loadFrogletTool()
  console.error(`Starting exploratory testing with model=${MODEL}, max_steps=${MAX_STEPS}`)

  const result = await runExploratorySession(froglet)

  // Output structured JSON result
  console.log(JSON.stringify(result, null, 2))

  // Print summary to stderr
  console.error(`\nExploratory testing complete:`)
  console.error(`  Scenarios explored: ${result.scenarios_explored}`)
  console.error(`  Anomalies found: ${result.anomalies.length}`)
  for (const a of result.anomalies) {
    console.error(`    [${a.severity}] ${a.context.slice(0, 100)}`)
  }

  // Exit with error if critical/high anomalies found
  const severe = result.anomalies.filter(
    (a) => a.severity === "critical" || a.severity === "high"
  )
  if (severe.length > 0) {
    console.error(`\n${severe.length} critical/high anomalies found — failing.`)
    process.exitCode = 1
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
