import { resolveRuntimeAuthTokenPath, resolveRuntimeUrl } from "./config.js"
import { summarizeDescriptor, summarizeNode, summarizeOffer } from "./public-tools.js"
import { toolTextResult } from "./shared.js"
import {
  DEFAULT_WAIT_STATUSES,
  acceptResultForDeal,
  buyWithRuntime,
  eventsQueryWithRuntime,
  getProvider,
  mockPayForDeal,
  paymentIntentForDeal,
  searchRuntime,
  waitForDeal,
  walletBalance
} from "./runtime-client.js"

const DEFAULT_WAIT_TIMEOUT_SECS = 15
const DEFAULT_WAIT_POLL_INTERVAL_SECS = 0.2

function buildRuntimeContext(config, args = {}) {
  return {
    runtimeUrl: resolveRuntimeUrl(config, args.runtime_url),
    runtimeAuthTokenPath: resolveRuntimeAuthTokenPath(
      config,
      args.runtime_auth_token_path
    )
  }
}

function summarizeDeal(deal) {
  return [
    `deal_id: ${deal?.deal_id ?? "unknown"}`,
    `provider_id: ${deal?.provider_id ?? "unknown"}`,
    `provider_url: ${deal?.provider_url ?? "unknown"}`,
    `status: ${deal?.status ?? "unknown"}`,
    `result_hash: ${deal?.result_hash ?? "none"}`,
    `receipt_hash: ${deal?.receipt?.hash ?? "none"}`
  ]
}

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
  const mockAction = intent.mock_action ?? null
  const releaseAction = intent.release_action ?? null
  return [
    `payment_backend: ${intent.backend ?? "unknown"}`,
    `payment_mode: ${intent.mode ?? "unknown"}`,
    `payment_session_id: ${intent.session_id ?? "unknown"}`,
    `deal_status: ${intent.deal_status ?? "unknown"}`,
    `admission_ready: ${intent.admission_ready === true}`,
    `result_ready: ${intent.result_ready === true}`,
    `can_release_preimage: ${intent.can_release_preimage === true}`,
    `payment_requests: ${summarizePaymentRequests(intent)}`,
    `mock_payment_endpoint: ${mockAction?.endpoint_path ?? "none"}`,
    `release_endpoint: ${releaseAction?.endpoint_path ?? "none"}`,
    `release_expected_result_hash: ${releaseAction?.expected_result_hash ?? "none"}`
  ]
}

function summarizeEventsQueryResult(result) {
  if (result === null || typeof result !== "object") {
    return ["events_returned: unknown", "cursor: none"]
  }
  const events = Array.isArray(result.events) ? result.events : []
  return [
    `events_returned: ${events.length}`,
    `cursor: ${result.cursor ?? "none"}`
  ]
}

function appendRaw(lines, label, payload, includeRaw) {
  if (!includeRaw) {
    return lines
  }
  return [...lines, "", label, JSON.stringify(payload, null, 2)]
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

export function registerRuntimeTools(api, config) {
  api.registerTool(
    {
      name: "froglet_events_query",
      description:
        "Run an `events.query` workload through the authenticated local runtime using the provider's advertised events offer.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["kinds"],
        properties: {
          provider_id: {
            type: "string",
            description: "Froglet provider_id to query."
          },
          provider_url: {
            type: "string",
            description: "Optional direct provider_url override."
          },
          kinds: {
            type: "array",
            minItems: 1,
            items: { type: "string" },
            description: "Event kinds to query."
          },
          limit: {
            type: "integer",
            minimum: 1,
            description: "Optional result limit."
          },
          max_price_sats: {
            type: "integer",
            minimum: 0,
            description: "Optional maximum acceptable price in sats."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw runtime events-query response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        if (typeof args.provider_id !== "string" && typeof args.provider_url !== "string") {
          throw new Error("froglet_events_query requires either provider_id or provider_url")
        }
        const includeRaw = args.include_raw === true
        const provider = {}
        if (typeof args.provider_id === "string") {
          provider.provider_id = args.provider_id
        }
        if (typeof args.provider_url === "string") {
          provider.provider_url = args.provider_url
        }
        const response = await eventsQueryWithRuntime({
          ...buildRuntimeContext(config, args),
          provider,
          kinds: args.kinds,
          limit: args.limit,
          maxPriceSats: args.max_price_sats,
          requestTimeoutMs: config.requestTimeoutMs
        })
        const lines = [
          `runtime_url: ${response.runtime_url}`,
          ...summarizeDeal(response.deal),
          `terminal: ${response.terminal === true}`,
          ...summarizeEventsQueryResult(response.deal?.result),
          `payment_intent_path: ${response.payment_intent_path ?? "none"}`,
          ...summarizePaymentIntent(response.payment_intent)
        ]
        return toolTextResult(
          appendRaw(lines, "events_query_response_json:", response, includeRaw).join("\n")
        )
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_search",
      description:
        "Search Froglet discovery through the authenticated local runtime.",
      parameters: {
        type: "object",
        additionalProperties: false,
        properties: {
          limit: {
            type: "integer",
            minimum: 1,
            maximum: config.maxSearchLimit,
            description: "Maximum number of providers to return."
          },
          include_inactive: {
            type: "boolean",
            description: "Include inactive discovery records."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw runtime search response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await searchRuntime({
          ...buildRuntimeContext(config, args),
          limit: args.limit ?? config.defaultSearchLimit,
          includeInactive: args.include_inactive === true,
          requestTimeoutMs: config.requestTimeoutMs
        })
        const nodes = Array.isArray(response?.nodes) ? response.nodes : []
        const lines = [
          `runtime_url: ${response.runtime_url}`,
          `returned_nodes: ${nodes.length}`,
          "",
          ...(nodes.length > 0
            ? nodes.map((node, index) => `${index + 1}.\n${summarizeNode(node)}`)
            : ["No Froglet providers matched the requested search."])
        ]
        return toolTextResult(
          appendRaw(lines, "search_response_json:", response, includeRaw).join("\n")
        )
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_get_provider",
      description:
        "Fetch provider discovery, descriptor, and offers through the authenticated local runtime.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["provider_id"],
        properties: {
          provider_id: {
            type: "string",
            description: "Froglet provider_id to resolve through the local runtime."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw runtime provider response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await getProvider({
          ...buildRuntimeContext(config, args),
          providerId: args.provider_id,
          requestTimeoutMs: config.requestTimeoutMs
        })
        const offers = Array.isArray(response.offers) ? response.offers : []
        const lines = [
          `runtime_url: ${response.runtime_url}`,
          "",
          summarizeNode(response.discovery),
          "",
          summarizeDescriptor(response.descriptor),
          "",
          `offers_returned: ${offers.length}`,
          ...(offers.length > 0 ? offers.map(summarizeOffer) : ["no offers"])
        ]
        return toolTextResult(
          appendRaw(lines, "provider_response_json:", response, includeRaw).join("\n")
        )
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_buy",
      description:
        "Create a Froglet deal through the authenticated local runtime. The runtime owns requester identity, deal signing, and payment preimage management.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["request"],
        properties: {
          request: {
            type: "object",
            additionalProperties: true,
            description:
              "Runtime deal request. It must include a provider reference, offer_id, and workload fields."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw runtime buy response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await buyWithRuntime({
          ...buildRuntimeContext(config, args),
          request: args.request,
          requestTimeoutMs: config.requestTimeoutMs
        })
        const lines = [
          `runtime_url: ${response.runtime_url}`,
          ...summarizeDeal(response.deal),
          `terminal: ${response.terminal === true}`,
          `payment_intent_path: ${response.payment_intent_path ?? "none"}`,
          ...summarizePaymentIntent(response.payment_intent)
        ]
        return toolTextResult(
          appendRaw(lines, "buy_response_json:", response, includeRaw).join("\n")
        )
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_wait_deal",
      description:
        "Poll the authenticated local runtime until a Froglet deal reaches one of the requested statuses.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["deal_id"],
        properties: {
          deal_id: {
            type: "string",
            description: "Runtime deal_id to poll."
          },
          wait_statuses: {
            type: "array",
            items: { type: "string" },
            description: "Statuses that should stop polling."
          },
          timeout_secs: {
            type: "number",
            minimum: 0.5,
            description: "Maximum wait time in seconds."
          },
          poll_interval_secs: {
            type: "number",
            minimum: 0.05,
            description: "Polling interval in seconds."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw wait response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await waitForDeal({
          ...buildRuntimeContext(config, args),
          dealId: args.deal_id,
          waitStatuses: normalizeStatuses(args.wait_statuses),
          timeoutSecs: args.timeout_secs ?? DEFAULT_WAIT_TIMEOUT_SECS,
          pollIntervalSecs: args.poll_interval_secs ?? DEFAULT_WAIT_POLL_INTERVAL_SECS,
          requestTimeoutMs: config.requestTimeoutMs
        })
        const lines = [
          `runtime_url: ${response.runtime_url}`,
          `wait_statuses: ${response.wait_statuses.join(", ")}`,
          ...summarizeDeal(response.deal)
        ]
        return toolTextResult(
          appendRaw(lines, "wait_response_json:", response, includeRaw).join("\n")
        )
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_payment_intent",
      description:
        "Fetch the current payment intent for a Froglet runtime deal.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["deal_id"],
        properties: {
          deal_id: {
            type: "string",
            description: "Runtime deal_id to inspect."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw payment intent response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await paymentIntentForDeal({
          ...buildRuntimeContext(config, args),
          dealId: args.deal_id,
          requestTimeoutMs: config.requestTimeoutMs
        })
        const lines = [
          `runtime_url: ${response.runtime_url}`,
          `deal_id: ${response.deal_id}`,
          ...summarizePaymentIntent(response.payment_intent)
        ]
        return toolTextResult(
          appendRaw(lines, "payment_intent_response_json:", response, includeRaw).join("\n")
        )
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_mock_pay",
      description:
        "Advance a mock-Lightning Froglet deal through the authenticated local runtime. This is only valid when the runtime payment backend is lightning mock mode.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["deal_id"],
        properties: {
          deal_id: {
            type: "string",
            description: "Runtime deal_id to mark as mock-funded."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw mock-pay response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await mockPayForDeal({
          ...buildRuntimeContext(config, args),
          dealId: args.deal_id,
          requestTimeoutMs: config.requestTimeoutMs
        })
        const lines = [
          `runtime_url: ${response.runtime_url}`,
          `deal_id: ${response.deal_id}`,
          ...summarizeDeal(response.deal),
          `payment_intent_path: ${response.payment_intent_path ?? "none"}`,
          ...summarizePaymentIntent(response.payment_intent)
        ]
        return toolTextResult(
          appendRaw(lines, "mock_pay_response_json:", response, includeRaw).join("\n")
        )
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_accept_result",
      description:
        "Release the managed success preimage for a runtime deal through the authenticated local runtime.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["deal_id"],
        properties: {
          deal_id: {
            type: "string",
            description: "Runtime deal_id to accept."
          },
          expected_result_hash: {
            type: "string",
            description: "Optional expected result hash override."
          },
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw accept response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await acceptResultForDeal({
          ...buildRuntimeContext(config, args),
          dealId: args.deal_id,
          expectedResultHash: args.expected_result_hash,
          requestTimeoutMs: config.requestTimeoutMs
        })
        const lines = [
          `runtime_url: ${response.runtime_url}`,
          ...summarizeDeal(response.deal)
        ]
        return toolTextResult(
          appendRaw(lines, "accept_response_json:", response, includeRaw).join("\n")
        )
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_wallet_balance",
      description:
        "Inspect the local Froglet requester runtime wallet balance and payment backend status.",
      parameters: {
        type: "object",
        additionalProperties: false,
        properties: {
          runtime_url: {
            type: "string",
            description: "Optional runtime base URL override."
          },
          runtime_auth_token_path: {
            type: "string",
            description: "Optional runtime auth token path override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw wallet balance response JSON."
          }
        }
      },
      async execute(_id, args = {}) {
        const includeRaw = args.include_raw === true
        const response = await walletBalance({
          ...buildRuntimeContext(config, args),
          requestTimeoutMs: config.requestTimeoutMs
        })
        const lines = [
          `runtime_url: ${response.runtime_url}`,
          `backend: ${response.backend ?? "unknown"}`,
          `mode: ${response.mode ?? "unknown"}`,
          `balance_known: ${response.balance_known === true}`,
          `balance_sats: ${response.balance_sats ?? "unknown"}`,
          `accepted_payment_methods: ${Array.isArray(response.accepted_payment_methods) ? response.accepted_payment_methods.join(", ") : "none"}`
        ]
        return toolTextResult(
          appendRaw(lines, "wallet_balance_response_json:", response, includeRaw).join("\n")
        )
      }
    },
    { optional: true }
  )
}
