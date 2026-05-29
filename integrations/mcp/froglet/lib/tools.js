import { dispatchFrogletAction } from "../../../shared/froglet-lib/tool-dispatch.js"

function errorResult(error) {
  return {
    content: [{ type: "text", text: `Error: ${error?.message ?? String(error)}` }],
    isError: true
  }
}

const frogletToolDescription =
  "Authoritative Froglet MCP tool for a local or self-hosted Froglet node. Use exact Froglet actions instead of guessing. Start with status to verify provider/runtime reachability. For local services use list_local_services or get_local_service. For marketplace-backed remote services use discover_services or get_service. For named service execution use invoke_service and prefer provider_id from discovery results; provider_url is an optional override. Use run_compute for open-ended compute through the runtime deal flow. For ONE-CALL agent-grade publishing (user says \"publish a Froglet service that does X\") use marketplace_publish — it shells out to `froglet-node publish` which builds, signs, registers, and verifies in one call; pass {name, source_inline, hosting:{kind:'tor'|'local'|'self'}}. publish_artifact still exists for the lower-level path of publishing a pre-built artifact to a local provider only. For settlement visibility use get_wallet_balance (current funds snapshot), list_settlement_activity (recent deals), get_payment_intent (per-deal intent), or get_invoice_bundle (per-deal bundle). For the marketplace: marketplace_register (self-register a public provider), marketplace_search (find providers + offers), marketplace_provider (one provider's details), marketplace_receipts (one provider's receipts), marketplace_stake (stake into a provider), marketplace_topup (add to existing stake), marketplace_file_complaint (file an arbiter complaint), marketplace_get_complaint (read complaint status). When the user asks whether or how to install Froglet locally, call plan_install first to collect choices; once the profile is confirmed, call get_install_guide to retrieve the canonical shell commands and run them through your host agent's shell — do NOT route install commands through the Froglet runtime. After install, call plan_use_case before implementing consumer, provider, evidence, payments, batch, or GPU workflows so unsupported boundaries are named before execution. The no-install public demo lives at https://froglet.dev/llms.txt and is intentionally not exposed as an MCP action."

function frogletToolInputSchema(config) {
  return {
    type: "object",
    required: ["action"],
    properties: {
      action: {
        type: "string",
        description:
          "Exact Froglet action name. Do not invent actions. Use status first to verify local provider/runtime reachability. Use list_local_services for local listings, discover_services for remote marketplace listings, get_local_service/get_service for authoritative details, invoke_service for named execution, publish_artifact to publish a built artifact to a local provider, run_compute for open-ended compute. For one-call agent-grade publishing (HEADLINE action) use marketplace_publish — it scaffolds manifests, builds the artifact, signs, registers, and verifies in one MCP call. Settlement visibility: get_wallet_balance, list_settlement_activity, get_payment_intent, get_invoice_bundle. Marketplace wrappers: marketplace_register, marketplace_search, marketplace_provider, marketplace_receipts, marketplace_stake, marketplace_topup, marketplace_file_complaint, marketplace_get_complaint — prefer these over invoke_service when targeting the marketplace. plan_install returns the decision tree, prerequisites, required secrets, validation checks, and post-install playbooks. get_install_guide returns canonical shell commands for a confirmed profile — execute those through your own shell, not the Froglet runtime. plan_use_case returns the post-install implementation plan for consumer/provider/evidence/payments/batch/GPU workflows.",
        enum: [
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
          "plan_use_case",
          "marketplace_register",
          "marketplace_domain_claim",
          "marketplace_domain_complete",
          "marketplace_search",
          "marketplace_provider",
          "marketplace_receipts",
          "marketplace_stake",
          "marketplace_topup",
          "marketplace_file_complaint",
          "marketplace_get_complaint",
          "marketplace_publish"
        ]
      },
      service_id: {
        type: "string",
        description:
          "Service identifier. Required for publish_artifact, get_local_service, get_service, and invoke_service."
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
      template: {
        type: "string",
        enum: ["demo.add"],
        description:
          "Optional publish_artifact template. demo.add publishes a free local Python add service for first-run verification."
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
      capabilities: {
        type: "array",
        items: { type: "string" },
        description:
          "Optional provider-required capability strings for publish_artifact, for example compute.gpu. GPU capabilities require a GPU-enabled provider."
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
        pattern: "^(https://[^\\s]+|http://[a-z2-7]{56}\\.onion)$",
        description:
          "Optional provider base URL override. Must be public https except marketplace_register may use a Tor v3 http://*.onion URL with registration_transport=tor. Usually discovered automatically from provider_id or service_id."
      },
      registration_transport: {
        type: "string",
        enum: ["clearnet", "tor"],
        description:
          "Transport for marketplace_register. Defaults from provider_url; use tor only for http://*.onion provider registration."
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
        enum: ["none", "lightning-mock", "lightning-lnd-rest", "stripe-test", "stripe-live", "x402"],
        description:
          "Explicit payment rail for plan_install/get_install_guide. Required before commands are generated; use none for the first free demo service."
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
      marketplace_arbiter_url: {
        type: "string",
        description:
          "Optional marketplace arbiter URL for marketplace_file_complaint / marketplace_get_complaint."
      },
      use_case: {
        type: "string",
        description:
          "The user's first intended Froglet use case after install, used by plan_install to choose a post-install playbook."
      },
      workload_profile: {
        type: "string",
        enum: ["consumer", "provider", "evidence", "payments", "batch", "gpu"],
        description:
          "Optional profile for plan_use_case. If omitted, Froglet infers it from use_case."
      },
      marketplace_provider_id: {
        type: "string",
        description:
          "Provider id the marketplace_* actions target. Distinct from `provider_id`, which routes the invoke_service call itself. Required for marketplace_file_complaint."
      },
      complaint_id: {
        type: "string",
        description: "Complaint id returned by marketplace_file_complaint."
      },
      claim_id: {
        type: "string",
        description: "Domain claim id returned by marketplace_domain_claim."
      },
      requested_slug: {
        type: "string",
        description: "Optional providers.froglet.dev slug for marketplace_domain_claim."
      },
      public_ip: {
        type: "string",
        description: "Public IPv4 or IPv6 address for a Froglet-managed provider subdomain claim."
      },
      signing_message: {
        type: "string",
        description: "Signing message returned by marketplace_domain_claim; marketplace_domain_complete signs it with the local provider identity."
      },
      reason: {
        type: "string",
        description: "Human-readable reason for marketplace_file_complaint."
      },
      receipt_hash: {
        type: "string",
        description: "Optional receipt artifact hash for marketplace_file_complaint."
      },
      complainant_id: {
        type: "string",
        description: "Optional requester or complainant id for marketplace_file_complaint."
      },
      evidence: {
        description: "Optional JSON object or array of evidence for marketplace_file_complaint."
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
      oci_digest: { type: "string" },
      // marketplace_publish input shape (Phase 3 of agent-grade publish).
      // The handler shells out to `froglet-node publish --json` after
      // materialising froglet.toml + froglet-service.toml + handler.py from
      // these fields, so they mirror the manifest layout directly.
      name: {
        type: "string",
        description:
          "Service name (project_id + service_id + scaffold directory). Lowercase ASCII letters, digits, or interior hyphens. Required for marketplace_publish."
      },
      source_inline: {
        type: "string",
        description:
          "Python source code (full handler.py contents) for marketplace_publish. Required when package_kind defaults to inline_source (the v1A only supported shape)."
      },
      hosting: {
        type: "object",
        description:
          "Hosting backend for marketplace_publish. kind ∈ {local|tor|self}. local = private dev, tor = auto hidden service (requires daemon FROGLET_NETWORK_MODE=tor), self = user-supplied URL.",
        properties: {
          kind: { type: "string", enum: ["local", "tor", "self"] },
          url: { type: "string", description: "Required when kind = 'self'." }
        }
      },
      settlement: {
        type: "object",
        description:
          "Settlement method for marketplace_publish. Supported: 'none' (free) or 'lightning' (hold-invoice escrow, settled by the node's Lightning wallet; requires a Lightning backend configured). Stripe + x402 are not yet exposed on the publish path.",
        properties: {
          method: { type: "string", enum: ["none", "lightning"] }
        }
      },
      marketplace_url: {
        type: "string",
        description:
          "Marketplace URL for marketplace_publish (and for marketplace_register / domain_claim). Defaults to https://marketplace.froglet.dev."
      }
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
