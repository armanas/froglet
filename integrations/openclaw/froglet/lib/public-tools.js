import { formatTimestamp } from "./shared.js"

export function formatServicePrice(service) {
  const serviceId = service?.service_id ?? "unknown"
  if (!service?.payment_required) {
    return `${serviceId}=free`
  }
  return `${serviceId}=${service?.price_sats ?? "?"} sats`
}

function formatTransportEndpoint(endpoint) {
  const transport = endpoint?.transport ?? "unknown"
  const uri = endpoint?.uri ?? "unknown"
  if (typeof uri === "string" && uri.startsWith(`${transport}:`)) {
    return uri
  }
  return `${transport} ${uri}`
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
    `confidential_profile_hash=${payload.confidential_profile_hash ?? "none"}`,
    `settlement_method=${payload.settlement_method ?? "unknown"}`,
    `quote_ttl_secs=${payload.quote_ttl_secs ?? "unknown"}`,
    `base_fee_msat=${price.base_fee_msat ?? "unknown"}`,
    `success_fee_msat=${price.success_fee_msat ?? "unknown"}`
  ].join(" ")
}
