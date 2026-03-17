import { spawn } from "node:child_process"
import { fileURLToPath } from "node:url"

import { resolveProviderUrl, resolveRuntimeAuthTokenPath, resolveRuntimeUrl } from "./config.js"
import { clampNumber, toolTextResult } from "./shared.js"
import { summarizeDescriptor, summarizeOffer } from "./public-tools.js"

export const BRIDGE_SCRIPT_PATH = fileURLToPath(new URL("../bridge.py", import.meta.url))

const DEFAULT_WAIT_TIMEOUT_SECS = 15
const DEFAULT_WAIT_POLL_INTERVAL_SECS = 0.2
const DEFAULT_WAIT_STATUSES = ["result_ready", "succeeded", "failed", "rejected"]

function summarizePaymentRequests(intent) {
  const requests = Array.isArray(intent?.payment_requests) ? intent.payment_requests : []
  if (requests.length === 0) {
    return "none"
  }
  return requests
    .map((request) => {
      const role = request?.role ?? "unknown"
      const state = request?.state ?? "unknown"
      const amount = request?.amount_sats ?? request?.amount_msat ?? "?"
      return `${role}:${state}:${amount}`
    })
    .join(", ")
}

function summarizePaymentIntent(intent) {
  if (intent === null || typeof intent !== "object") {
    return ["payment_intent: none"]
  }

  const releaseAction = intent.release_action ?? null
  return [
    `payment_backend: ${intent.backend ?? "unknown"}`,
    `payment_session_id: ${intent.session_id ?? "unknown"}`,
    `payment_deal_status: ${intent.deal_status ?? "unknown"}`,
    `admission_ready: ${intent.admission_ready === true}`,
    `result_ready: ${intent.result_ready === true}`,
    `can_release_preimage: ${intent.can_release_preimage === true}`,
    `payment_requests: ${summarizePaymentRequests(intent)}`,
    `release_endpoint: ${releaseAction?.endpoint_path ?? "none"}`,
    `release_expected_result_hash: ${releaseAction?.expected_result_hash ?? "none"}`
  ]
}

function summarizeDealRecord(deal) {
  if (deal === null || typeof deal !== "object") {
    return ["deal: none"]
  }
  return [
    `deal_id: ${deal.deal_id ?? "unknown"}`,
    `deal_status: ${deal.status ?? "unknown"}`,
    `result_hash: ${deal.result_hash ?? "none"}`,
    `result: ${JSON.stringify(deal.result ?? null)}`
  ]
}

function normalizeStatuses(value) {
  if (!Array.isArray(value) || value.length === 0) {
    return DEFAULT_WAIT_STATUSES
  }
  const statuses = value
    .map((item) => (typeof item === "string" ? item.trim() : ""))
    .filter((item) => item.length > 0)
  return statuses.length > 0 ? [...new Set(statuses)] : DEFAULT_WAIT_STATUSES
}

function runtimeBridgeError(message, details) {
  if (typeof details !== "string" || details.trim().length === 0) {
    return new Error(message)
  }
  return new Error(`${message}: ${details.trim()}`)
}

export async function invokeRuntimeBridge(config, payload, options = {}) {
  const spawnImpl = options.spawnImpl ?? spawn
  const command = config.pythonExecutable

  return await new Promise((resolve, reject) => {
    let stdout = ""
    let stderr = ""
    let child
    try {
      child = spawnImpl(command, [BRIDGE_SCRIPT_PATH], {
        env: {
          ...process.env,
          PYTHONUNBUFFERED: "1"
        },
        stdio: ["pipe", "pipe", "pipe"]
      })
    } catch (error) {
      reject(runtimeBridgeError(`Failed to spawn runtime bridge command ${command}`, error?.message))
      return
    }

    child.stdout.setEncoding("utf8")
    child.stderr.setEncoding("utf8")
    child.stdout.on("data", (chunk) => {
      stdout += chunk
    })
    child.stderr.on("data", (chunk) => {
      stderr += chunk
    })
    child.on("error", (error) => {
      reject(runtimeBridgeError(`Runtime bridge process error for ${command}`, error?.message))
    })
    child.on("close", (code) => {
      if (code !== 0) {
        reject(
          runtimeBridgeError(
            `Runtime bridge exited with code ${code}`,
            stderr || stdout || "unknown error"
          )
        )
        return
      }

      const trimmed = stdout.trim()
      if (trimmed.length === 0) {
        reject(runtimeBridgeError("Runtime bridge returned no JSON output"))
        return
      }

      try {
        resolve(JSON.parse(trimmed))
      } catch (error) {
        reject(
          runtimeBridgeError(
            `Runtime bridge returned invalid JSON`,
            `${error.message}; payload=${trimmed}`
          )
        )
      }
    })

    child.stdin.on("error", (error) => {
      reject(runtimeBridgeError("Failed to write runtime bridge input", error?.message))
    })
    child.stdin.end(`${JSON.stringify(payload)}\n`)
  })
}

function summarizeBuyResponse(response) {
  const lines = [
    `runtime_url: ${response.runtime_url ?? "unknown"}`,
    `provider_url: ${response.provider_url ?? "unknown"}`,
    ...summarizeDealRecord(response.deal),
    `terminal: ${response.terminal === true}`,
    `payment_intent_path: ${response.payment_intent_path ?? "none"}`,
    `managed_preimage: ${response.stored_preimage === true}`,
    `local_state_path: ${response.stored_state_path ?? "none"}`
  ]
  return [...lines, ...summarizePaymentIntent(response.payment_intent)]
}

function summarizeWaitResponse(response) {
  return [
    `provider_url: ${response.provider_url ?? "unknown"}`,
    `wait_statuses: ${Array.isArray(response.wait_statuses) ? response.wait_statuses.join(", ") : "unknown"}`,
    ...summarizeDealRecord(response.deal)
  ]
}

function summarizeAcceptResponse(response) {
  const terminal = response.terminal ?? {}
  const receipt = terminal.receipt ?? {}
  return [
    `runtime_url: ${response.runtime_url ?? "unknown"}`,
    `provider_url: ${response.provider_url ?? "unknown"}`,
    `deal_id: ${response.deal_id ?? terminal.deal_id ?? "unknown"}`,
    `terminal_status: ${terminal.status ?? "unknown"}`,
    `receipt_hash: ${receipt.hash ?? "none"}`,
    `result_hash: ${terminal.result_hash ?? "none"}`,
    `local_state_path: ${response.stored_state_path ?? "none"}`
  ]
}

function summarizePublishResponse(response) {
  const offers = Array.isArray(response.offers) ? response.offers : []
  return [
    `runtime_url: ${response.runtime_url ?? "unknown"}`,
    `provider_url: ${response.provider_url ?? "unknown"}`,
    "",
    summarizeDescriptor(response.descriptor),
    "",
    `offers_returned: ${offers.length}`,
    ...(offers.length > 0 ? offers.map(summarizeOffer) : ["no offers published"])
  ]
}

function summarizePaymentIntentResponse(response) {
  return [
    `runtime_url: ${response.runtime_url ?? "unknown"}`,
    `deal_id: ${response.deal_id ?? "unknown"}`,
    ...summarizePaymentIntent(response.payment_intent)
  ]
}

function appendRaw(lines, label, payload, includeRaw) {
  if (!includeRaw) {
    return lines
  }
  return [...lines, "", label, JSON.stringify(payload, null, 2)]
}

function buildRuntimeContext(config, args = {}) {
  return {
    runtime_url: resolveRuntimeUrl(config, args.runtime_url, { required: false }),
    provider_url: resolveProviderUrl(config, args.provider_url, { required: false }),
    runtime_auth_token_path: resolveRuntimeAuthTokenPath(
      config,
      args.runtime_auth_token_path,
      { required: false }
    )
  }
}

export function registerRuntimeTools(api, config, options = {}) {
  if (!config.enablePrivilegedRuntimeTools) {
    return
  }

  const runBridge = (payload) => invokeRuntimeBridge(config, payload, options)

  api.registerTool(
    {
      name: "froglet_runtime_buy",
      description:
        "Buy a Froglet service through the authenticated local runtime. This uses public provider APIs plus the documented runtime buy flow, and stores local release state so accept_result only needs a deal_id.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["request"],
        properties: {
          request: {
            type: "object",
            additionalProperties: true,
            description:
              "Generic Froglet runtime buy payload. The helper can fill requester seed and success-preimage state automatically for the default local flow."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          provider_url: {
            type: "string",
            description: "Optional provider base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          wait_for_receipt: {
            type: "boolean",
            description: "Wait for a terminal receipt before returning when the runtime flow supports it."
          },
          wait_timeout_secs: {
            type: "integer",
            minimum: 1,
            description: "Optional runtime-side wait budget in seconds."
          },
          include_payment_intent: {
            type: "boolean",
            description: "Include the current payment intent in the bridge response."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw bridge response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await runBridge({
          action: "buy",
          ...buildRuntimeContext(config, args),
          request: args.request,
          wait_for_receipt: args.wait_for_receipt === true,
          wait_timeout_secs: args.wait_timeout_secs,
          include_payment_intent: args.include_payment_intent !== false
        })
        const lines = appendRaw(
          summarizeBuyResponse(response),
          "buy_response_json:",
          response,
          includeRaw
        )
        return toolTextResult(lines.join("\n"))
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_runtime_wait_deal",
      description:
        "Wait for a Froglet deal to reach result_ready or a terminal state through the provider API.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["deal_id"],
        properties: {
          deal_id: {
            type: "string",
            description: "Deal identifier to poll."
          },
          runtime_url: {
            type: "string",
            description:
              "Optional runtime base URL override. This is accepted for parity with the other runtime tools even though waiting uses provider polling."
          },
          provider_url: {
            type: "string",
            description: "Optional provider base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description:
              "Optional runtime auth token path override. Used only to recover stored helper state."
          },
          statuses: {
            type: "array",
            items: {
              type: "string"
            },
            description:
              "Optional status list to stop on. Defaults to result_ready, succeeded, failed, rejected."
          },
          timeout_secs: {
            type: "number",
            minimum: 0.1,
            description: "Maximum time to wait."
          },
          poll_interval_secs: {
            type: "number",
            minimum: 0.05,
            description: "Polling interval."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw bridge response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await runBridge({
          action: "wait_deal",
          runtime_url: resolveRuntimeUrl(config, args.runtime_url, { required: false }),
          provider_url: resolveProviderUrl(config, args.provider_url, { required: false }),
          runtime_auth_token_path: resolveRuntimeAuthTokenPath(
            config,
            args.runtime_auth_token_path,
            { required: false }
          ),
          deal_id: typeof args.deal_id === "string" ? args.deal_id.trim() : "",
          wait_statuses: normalizeStatuses(args.statuses),
          timeout_secs: clampNumber(
            args.timeout_secs,
            DEFAULT_WAIT_TIMEOUT_SECS,
            0.1,
            3600
          ),
          poll_interval_secs: clampNumber(
            args.poll_interval_secs,
            DEFAULT_WAIT_POLL_INTERVAL_SECS,
            0.05,
            60
          )
        })
        const lines = appendRaw(
          summarizeWaitResponse(response),
          "wait_response_json:",
          response,
          includeRaw
        )
        return toolTextResult(lines.join("\n"))
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_runtime_payment_intent",
      description: "Inspect the current Froglet runtime payment intent for a deal.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["deal_id"],
        properties: {
          deal_id: {
            type: "string",
            description: "Deal identifier to inspect."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          provider_url: {
            type: "string",
            description: "Optional provider base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw bridge response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await runBridge({
          action: "payment_intent",
          ...buildRuntimeContext(config, args),
          deal_id: typeof args.deal_id === "string" ? args.deal_id.trim() : ""
        })
        const lines = appendRaw(
          summarizePaymentIntentResponse(response),
          "payment_intent_response_json:",
          response,
          includeRaw
        )
        return toolTextResult(lines.join("\n"))
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_runtime_accept_result",
      description:
        "Accept a Froglet result by releasing the locally stored success preimage for a deal created by this helper.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["deal_id"],
        properties: {
          deal_id: {
            type: "string",
            description: "Deal identifier to accept."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          provider_url: {
            type: "string",
            description: "Optional provider base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          expected_result_hash: {
            type: "string",
            description: "Optional expected result hash override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw bridge response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await runBridge({
          action: "accept_result",
          ...buildRuntimeContext(config, args),
          deal_id: typeof args.deal_id === "string" ? args.deal_id.trim() : "",
          expected_result_hash:
            typeof args.expected_result_hash === "string" &&
            args.expected_result_hash.trim().length > 0
              ? args.expected_result_hash.trim()
              : null
        })
        const lines = appendRaw(
          summarizeAcceptResponse(response),
          "accept_response_json:",
          response,
          includeRaw
        )
        return toolTextResult(lines.join("\n"))
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_runtime_publish_services",
      description:
        "Publish the current Froglet provider surface through the authenticated local runtime.",
      parameters: {
        type: "object",
        additionalProperties: false,
        properties: {
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          provider_url: {
            type: "string",
            description: "Optional provider base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw bridge response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await runBridge({
          action: "publish_services",
          ...buildRuntimeContext(config, args)
        })
        const lines = appendRaw(
          summarizePublishResponse(response),
          "publish_response_json:",
          response,
          includeRaw
        )
        return toolTextResult(lines.join("\n"))
      }
    },
    { optional: true }
  )
}
