export const FROGLET_ACTIONS = Object.freeze([
  "discover_services",
  "get_service",
  "invoke_service",
  "list_local_services",
  "get_local_service",
  "publish_artifact",
  "status",
  "get_task",
  "wait_task",
  "run_compute",
  "get_wallet_balance",
  "get_spend_status",
  "reset_spend",
  "list_settlement_activity",
  "get_payment_intent",
  "get_invoice_bundle",
  "plan_install",
  "get_install_guide",
  "plan_use_case",
  "marketplace_register",
  "marketplace_domain_claim",
  "marketplace_domain_complete",
  "marketplace_search",
  "marketplace_provider",
  "marketplace_receipts",
  "marketplace_file_complaint",
  "marketplace_get_complaint",
  "marketplace_publish"
])

export const FROGLET_PAYMENT_RAILS = Object.freeze([
  "none",
  "lightning-mock",
  "lightning-lnd-rest",
  "lightning-phoenixd",
  "stripe-test",
  "stripe-live",
  "x402"
])

export const FROGLET_LIGHTNING_MODES = Object.freeze(["mock", "lnd_rest", "phoenixd"])

export const MARKETPLACE_ATTESTATION_PROPERTIES = Object.freeze({
  attested: {
    type: "boolean",
    description:
      "marketplace_search filter: only providers holding at least one valid identity attestation (docs/IDENTITY_ATTESTATION.md). Attestations are optional; unattested providers are still first-class."
  },
  attestation_kind: {
    type: "string",
    enum: ["dns", "oauth"],
    description: "marketplace_search filter: require an attestation of this kind."
  },
  attestation_dns_zone: {
    type: "string",
    description:
      "marketplace_search filter: require a DNS attestation for this zone (e.g. example.com)."
  }
})
