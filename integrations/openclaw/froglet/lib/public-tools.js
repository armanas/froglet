import {
  ABSOLUTE_MAX_SEARCH_LIMIT,
  MIN_SEARCH_LIMIT,
  clampInteger,
  fetchJson,
  formatTimestamp,
  toolTextResult
} from "./shared.js"
import { resolveMarketplaceUrl, resolveProviderUrl } from "./config.js"

export function formatServicePrice(service) {
  const serviceId = service?.service_id ?? "unknown"
  if (!service?.payment_required) {
    return `${serviceId}=free`
  }
  return `${serviceId}=${service?.price_sats ?? "?"} sats`
}

export function summarizeNode(record) {
  const descriptor = record?.descriptor ?? {}
  const transports = descriptor.transports ?? {}
  const services = Array.isArray(descriptor.services) ? descriptor.services : []
  const serviceSummary =
    services.length > 0 ? services.map(formatServicePrice).join(", ") : "none"

  return [
    `node_id: ${descriptor.node_id ?? "unknown"}`,
    `status: ${record?.status ?? "unknown"}`,
    `version: ${descriptor.version ?? "unknown"}`,
    `clearnet_url: ${transports.clearnet_url ?? "none"}`,
    `onion_url: ${transports.onion_url ?? "none"}`,
    `tor_status: ${transports.tor_status ?? "unknown"}`,
    `services: ${serviceSummary}`,
    `last_seen_at: ${formatTimestamp(record?.last_seen_at)}`
  ].join("\n")
}

function formatTransportEndpoint(endpoint) {
  const transport = endpoint?.transport ?? "unknown"
  const uri = endpoint?.uri ?? "unknown"
  if (typeof uri === "string" && uri.startsWith(`${transport}:`)) {
    return uri
  }
  return `${transport} ${uri}`
}

export function summarizeDescriptor(descriptor) {
  const payload = descriptor?.payload ?? {}
  const capabilities = payload.capabilities ?? {}
  const transportEndpoints = Array.isArray(payload.transport_endpoints)
    ? payload.transport_endpoints
    : []
  const linkedIdentities = Array.isArray(payload.linked_identities)
    ? payload.linked_identities
    : []

  const transports =
    transportEndpoints.length > 0
      ? transportEndpoints.map((endpoint) => formatTransportEndpoint(endpoint)).join(", ")
      : "none"
  const identities =
    linkedIdentities.length > 0
      ? linkedIdentities
          .map((identity) => `${identity.identity_kind}:${identity.identity}`)
          .join(", ")
      : "none"

  return [
    `provider_id: ${payload.provider_id ?? "unknown"}`,
    `protocol_version: ${payload.protocol_version ?? "unknown"}`,
    `descriptor_seq: ${payload.descriptor_seq ?? "unknown"}`,
    `service_kinds: ${Array.isArray(capabilities.service_kinds) ? capabilities.service_kinds.join(", ") || "none" : "none"}`,
    `execution_runtimes: ${Array.isArray(capabilities.execution_runtimes) ? capabilities.execution_runtimes.join(", ") || "none" : "none"}`,
    `max_concurrent_deals: ${capabilities.max_concurrent_deals ?? "unset"}`,
    `transport_endpoints: ${transports}`,
    `linked_identities: ${identities}`
  ].join("\n")
}

export function summarizeOffer(offer) {
  const payload = offer?.payload ?? {}
  const price = payload.price_schedule ?? {}
  return [
    `offer_id=${payload.offer_id ?? "unknown"}`,
    `offer_kind=${payload.offer_kind ?? "unknown"}`,
    `settlement_method=${payload.settlement_method ?? "unknown"}`,
    `quote_ttl_secs=${payload.quote_ttl_secs ?? "unknown"}`,
    `base_fee_msat=${price.base_fee_msat ?? "unknown"}`,
    `success_fee_msat=${price.success_fee_msat ?? "unknown"}`
  ].join(" ")
}

function formatNodeList(nodes) {
  if (!Array.isArray(nodes) || nodes.length === 0) {
    return "No marketplace nodes matched the requested filter."
  }

  return nodes
    .map((node, index) => {
      const services = Array.isArray(node?.descriptor?.services)
        ? node.descriptor.services.map(formatServicePrice).join(", ")
        : "none"
      return [
        `${index + 1}. ${node?.descriptor?.node_id ?? "unknown"} (${node?.status ?? "unknown"})`,
        `   clearnet: ${node?.descriptor?.transports?.clearnet_url ?? "none"}`,
        `   onion: ${node?.descriptor?.transports?.onion_url ?? "none"}`,
        `   services: ${services}`,
        `   last_seen_at: ${formatTimestamp(node?.last_seen_at)}`
      ].join("\n")
    })
    .join("\n")
}

export function registerPublicTools(api, config) {
  api.registerTool(
    {
      name: "froglet_marketplace_search",
      description:
        "List Froglet marketplace nodes using the public discovery API. This is recency-ordered discovery, not keyword search.",
      parameters: {
        type: "object",
        additionalProperties: false,
        properties: {
          limit: {
            type: "integer",
            minimum: MIN_SEARCH_LIMIT,
            maximum: ABSOLUTE_MAX_SEARCH_LIMIT,
            description: "Maximum number of nodes to return."
          },
          include_inactive: {
            type: "boolean",
            description: "Include inactive marketplace listings."
          },
          marketplace_url: {
            type: "string",
            description: "Optional marketplace base URL override."
          },
          include_raw: {
            type: "boolean",
            description: "Include the raw marketplace response JSON in the response."
          }
        }
      },
      async execute(_id, args = {}) {
        const limit = clampInteger(
          args.limit,
          config.defaultSearchLimit,
          MIN_SEARCH_LIMIT,
          config.maxSearchLimit
        )
        const includeInactive = args.include_inactive === true
        const includeRaw = args.include_raw === true
        const marketplaceUrl = resolveMarketplaceUrl(config, args.marketplace_url)
        const query = new URLSearchParams({
          limit: String(limit),
          include_inactive: includeInactive ? "true" : "false"
        })
        const response = await fetchJson(
          `${marketplaceUrl}/v1/marketplace/search?${query.toString()}`,
          config.requestTimeoutMs
        )
        const nodes = Array.isArray(response?.nodes) ? response.nodes : []

        const lines = [
          `marketplace: ${marketplaceUrl}`,
          `returned_nodes: ${nodes.length}`,
          `include_inactive: ${includeInactive}`,
          "",
          formatNodeList(nodes)
        ]
        if (includeRaw) {
          lines.push("", "search_response_json:", JSON.stringify(response, null, 2))
        }

        return toolTextResult(lines.join("\n"))
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_marketplace_node",
      description: "Fetch a single Froglet marketplace node record by node_id.",
      parameters: {
        type: "object",
        additionalProperties: false,
        required: ["node_id"],
        properties: {
          node_id: {
            type: "string",
            description: "The Froglet node_id to fetch from the marketplace."
          },
          marketplace_url: {
            type: "string",
            description: "Optional marketplace base URL override."
          },
          include_raw: {
            type: "boolean",
            description: "Include raw node JSON in the response."
          }
        }
      },
      async execute(_id, args = {}) {
        const marketplaceUrl = resolveMarketplaceUrl(config, args.marketplace_url)
        const nodeId = typeof args.node_id === "string" ? args.node_id.trim() : ""
        const includeRaw = args.include_raw === true
        if (nodeId.length === 0) {
          throw new Error("node_id must be a non-empty string")
        }

        const record = await fetchJson(
          `${marketplaceUrl}/v1/marketplace/nodes/${encodeURIComponent(nodeId)}`,
          config.requestTimeoutMs
        )

        const lines = [`marketplace: ${marketplaceUrl}`, "", summarizeNode(record)]
        if (includeRaw) {
          lines.push("", "raw_record_json:", JSON.stringify(record, null, 2))
        }

        return toolTextResult(lines.join("\n"))
      }
    },
    { optional: true }
  )

  api.registerTool(
    {
      name: "froglet_provider_surface",
      description:
        "Fetch a Froglet provider's public descriptor and offers from its public API base URL.",
      parameters: {
        type: "object",
        additionalProperties: false,
        properties: {
          provider_url: {
            type: "string",
            description:
              "Optional public Froglet provider base URL override, for example http://127.0.0.1:8080."
          },
          include_raw: {
            type: "boolean",
            description: "Include raw descriptor and offers JSON in the response."
          }
        }
      },
      async execute(_id, args = {}) {
        const providerUrl = resolveProviderUrl(config, args.provider_url)
        const includeRaw = args.include_raw === true
        const [descriptor, offersResponse] = await Promise.all([
          fetchJson(`${providerUrl}/v1/descriptor`, config.requestTimeoutMs),
          fetchJson(`${providerUrl}/v1/offers`, config.requestTimeoutMs)
        ])
        const offers = Array.isArray(offersResponse?.offers) ? offersResponse.offers : []

        const lines = [
          `provider_url: ${providerUrl}`,
          "",
          summarizeDescriptor(descriptor),
          "",
          `offers_returned: ${offers.length}`,
          ...(offers.length > 0 ? offers.map(summarizeOffer) : ["no offers published"])
        ]

        if (includeRaw) {
          lines.push(
            "",
            "descriptor_json:",
            JSON.stringify(descriptor, null, 2),
            "",
            "offers_json:",
            JSON.stringify(offers, null, 2)
          )
        }

        return toolTextResult(lines.join("\n"))
      }
    },
    { optional: true }
  )
}
