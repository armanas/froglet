import { dispatchFrogletAction } from "../../../shared/froglet-lib/tool-dispatch.js"

function errorResult(error) {
  return {
    content: [{ type: "text", text: `Error: ${error?.message ?? String(error)}` }],
    isError: true
  }
}

const frogletToolDescription =
  "Authoritative Froglet MCP tool. Use exact Froglet actions instead of guessing. First-run/zero-config users should call run_hosted_proof to execute the hosted demo.add plus optional witness/hash proof before recommending local install. For local services use list_local_services or get_local_service. For marketplace-backed remote services use discover_services or get_service. For named service execution use invoke_service and prefer provider_id from discovery results; provider_url is an optional override. Use run_compute for open-ended compute through the runtime deal flow. Use publish_artifact to publish a built artifact to the local provider. For settlement visibility use get_wallet_balance (current funds snapshot), list_settlement_activity (recent deals), get_payment_intent (per-deal intent), or get_invoice_bundle (per-deal bundle). For the marketplace: marketplace_search (find providers + offers), marketplace_provider (one provider's details), marketplace_receipts (one provider's receipts), marketplace_stake (stake into a provider), marketplace_topup (add to existing stake). When the user asks whether or how to install Froglet locally, call plan_install first to collect choices; once the profile is confirmed, call get_install_guide to retrieve the canonical shell commands and run them through your host agent's shell — do NOT route install commands through the Froglet runtime."

function frogletToolInputSchema(config) {
  return {
    type: "object",
    required: ["action"],
    properties: {
      action: {
        type: "string",
        description:
          "Exact Froglet action name. Do not invent actions. Use run_hosted_proof for the zero-config hosted proof. Use list_local_services for local listings, discover_services for remote marketplace listings, get_local_service/get_service for authoritative details, invoke_service for named execution, publish_artifact to publish a built artifact, run_compute for open-ended compute. Settlement visibility: get_wallet_balance, list_settlement_activity, get_payment_intent, get_invoice_bundle. Marketplace wrappers: marketplace_search, marketplace_provider, marketplace_receipts, marketplace_stake, marketplace_topup — prefer these over invoke_service when targeting the marketplace. plan_install returns the decision tree, prerequisites, required secrets, validation checks, and post-install playbooks. get_install_guide returns canonical shell commands for a confirmed profile — execute those through your own shell, not the Froglet runtime.",
        enum: [
          "run_hosted_proof",
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
          "list_settlement_activity",
          "get_payment_intent",
          "get_invoice_bundle",
          "plan_install",
          "get_install_guide",
          "marketplace_search",
          "marketplace_provider",
          "marketplace_receipts",
          "marketplace_stake",
          "marketplace_topup"
        ]
      },
      service_id: {
        type: "string",
        description:
          "Service identifier. Required for publish_artifact, get_local_service, get_service, and invoke_service."
      },
      hosted_url: {
        type: "string",
        format: "uri",
        pattern: "^https://[^\\s]+$",
        description:
          "Hosted trial base URL for run_hosted_proof. Defaults to https://try.froglet.dev."
      },
      followup: {
        type: "string",
        enum: ["none", "demo.fetch-witness", "demo.hash-verify"],
        description:
          "Optional hosted proof follow-up after demo.add. Defaults to demo.fetch-witness."
      },
      offer_id: { type: "string" },
      summary: {
        type: "string",
        description: "Descriptive metadata for publish_artifact."
      },
      starter: {
        type: "string",
        description: "Optional compact JSON example input for publish_artifact; example only, not a stronger contract than input_schema."
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
          "Optional inline Wasm module bytes in hex. Low-level escape hatch for direct inline Wasm compute or publish_artifact. Prefer artifact_path instead."
      },
      inline_source: {
        type: "string",
        description:
          "Optional inline source for a compute request. Use this when you want to run explicit source text, typically for runtime=python package_kind=inline_source."
      },
      input: {},
      result_json: {
        description:
          "Optional static JSON result. Used with publish_artifact for constant-return services."
      },
      output_schema: {},
      input_schema: {},
      price_sats: { type: "integer", minimum: 0 },
      publication_state: {
        type: "string",
        enum: ["active", "hidden"]
      },
      mode: { type: "string", enum: ["sync", "async"] },
      provider_id: {
        type: "string",
        description:
          "Target provider node ID. Preferred for marketplace-backed get_service, invoke_service, and run_compute calls."
      },
      provider_url: {
        type: "string",
        format: "uri",
        pattern: "^https://[^\\s]+$",
        description:
          "Optional provider base URL override. Must be https (Tor onion providers must be accessed via the runtime, not this override). Usually discovered automatically from provider_id or service_id."
      },
      limit: {
        type: "integer",
        minimum: 1,
        maximum: config.maxSearchLimit
      },
      include_inactive: { type: "boolean" },
      query: { type: "string" },
      task_id: { type: "string" },
      deal_id: {
        type: "string",
        description: "Target deal id. Required for get_payment_intent and get_invoice_bundle."
      },
      target_agent: {
        type: "string",
        enum: ["claude-code", "codex", "openclaw", "manual"],
        description:
          "Agent target for plan_install/get_install_guide. Defaults to claude-code; use manual when the user will configure MCP themselves."
      },
      payment_rail: {
        type: "string",
        enum: ["none", "lightning", "stripe", "x402"],
        description:
          "Payment rail for plan_install/get_install_guide. Defaults to lightning in mock mode; use lightning_mode=lnd_rest only when the user already has LND REST credentials."
      },
      lightning_mode: {
        type: "string",
        enum: ["mock", "lnd_rest"],
        description:
          "Lightning mode for plan_install/get_install_guide. mock requires no wallet; lnd_rest requires an LND REST URL and macaroon path."
      },
      footprint: {
        type: "string",
        enum: ["docker", "binary", "source"],
        description:
          "Install footprint for plan_install/get_install_guide. docker is the full local provider+runtime stack; binary installs only froglet-node; source builds from the cloned repo."
      },
      role: {
        type: "string",
        enum: ["consumer", "provider", "both"],
        description:
          "User intent for plan_install/get_install_guide. Docker defaults to both provider and runtime; split roles are a direct froglet-node concern."
      },
      network_mode: {
        type: "string",
        enum: ["clearnet", "tor", "dual"],
        description:
          "Network mode for local/self-hosted Froglet. Keep clearnet/loopback first; use tor or dual only after local health checks pass."
      },
      marketplace_url: {
        type: "string",
        description:
          "Optional marketplace URL to export before starting the local stack."
      },
      use_case: {
        type: "string",
        description:
          "The user's first intended Froglet use case after install, used by plan_install to choose a post-install playbook."
      },
      marketplace_provider_id: {
        type: "string",
        description:
          "Provider id the marketplace_* actions target. Distinct from `provider_id`, which routes the invoke_service call itself."
      },
      amount_msat: {
        type: "integer",
        minimum: 1,
        description:
          "Amount in millisatoshis for marketplace_stake / marketplace_topup. Must be positive."
      },
      offer_kind: {
        type: "string",
        description: "Offer-kind filter for marketplace_search (e.g. \"named.v1\")."
      },
      max_price_sats: {
        type: "integer",
        minimum: 0,
        description: "Upper price bound in sats for marketplace_search results."
      },
      status: {
        type: "string",
        description: "Status filter for marketplace_receipts (e.g. \"succeeded\")."
      },
      cursor: {
        type: "string",
        description: "Opaque pagination cursor for marketplace_search / marketplace_receipts."
      },
      timeout_secs: { type: "integer", minimum: 1, maximum: 600 },
      poll_interval_secs: { type: "number", minimum: 0.1, maximum: 10 },
      artifact_path: { type: "string" },
      oci_reference: { type: "string" },
      oci_digest: { type: "string" }
    }
  }
}

export function buildToolDefinitions(config) {
  return [
    {
      name: "froglet",
      description: frogletToolDescription,
      inputSchema: frogletToolInputSchema(config)
    }
  ]
}

export async function handleToolCall(name, args, config) {
  if (name !== "froglet") {
    return errorResult(new Error(`Unknown tool: ${name}`))
  }
  try {
    return await dispatchFrogletAction(args ?? {}, config)
  } catch (error) {
    return errorResult(error)
  }
}
