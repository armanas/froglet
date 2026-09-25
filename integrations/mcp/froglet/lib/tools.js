import { dispatchFrogletAction } from "../../../shared/froglet-lib/tool-dispatch.js"
import {
  FROGLET_ACTIONS,
  FROGLET_LIGHTNING_MODES,
  FROGLET_PAYMENT_RAILS,
  MARKETPLACE_ATTESTATION_PROPERTIES
} from "../../../shared/froglet-lib/tool-contract.js"

function errorResult(error) {
  return {
    content: [{ type: "text", text: `Error: ${error?.message ?? String(error)}` }],
    isError: true
  }
}

const frogletToolDescription =
  "Authoritative Froglet MCP tool for a local or self-hosted Froglet node. Use exact Froglet actions instead of guessing. Start with status to verify provider/runtime reachability. For local services use list_local_services or get_local_service. For marketplace-backed remote services use discover_services or get_service. For named service execution use invoke_service and prefer provider_id from discovery results; provider_url is an optional override. Use run_compute for open-ended compute through the runtime deal flow. For agent-grade public publishing use marketplace_publish twice: the first call is non-mutating and returns the exact relay/plaintext, quota, backup, limits, and price consent; present it to the user, then repeat with its consent_hash to publish and verify. publish_artifact remains the lower-level local-only path. For settlement visibility use get_wallet_balance, get_spend_status, reset_spend, list_settlement_activity, get_payment_intent, or get_invoice_bundle. For marketplace operations use the named marketplace actions. When asked to install, call plan_install, present its immutable release and exact impact, then pass the returned release_tag and install_approval_hash unchanged to get_install_guide only after user approval. Run approved commands through the host shell. The no-install public demo lives at https://froglet.dev/llms.txt."

export function frogletToolInputSchema(config) {
  return {
    type: "object",
    required: ["action"],
    properties: {
      action: {
        type: "string",
        description:
          "Exact Froglet action name. Do not invent actions. Use status first to verify local provider/runtime reachability. Use list_local_services for local listings, discover_services for remote marketplace listings, get_local_service/get_service for authoritative details, invoke_service for named execution, publish_artifact to publish a built artifact to a local provider, and run_compute for open-ended compute. Public marketplace_publish is a two-call flow: omit consent_hash to get the exact non-mutating disclosure, present it to the user, then repeat with the returned consent_hash to publish and verify. Use the named settlement, marketplace, install, and use-case planning actions for those workflows.",
        enum: [...FROGLET_ACTIONS]
      },
      service_id: {
        type: "string",
        description:
          "Service identifier. Required for publish_artifact, get_local_service, get_service, and invoke_service."
      },
      offer_id: { type: "string" },
      project_id: {
        type: "string",
        description: "Optional project identity preserved by publish_artifact."
      },
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
        type: "array",
        items: {
          type: "object",
          required: ["handle", "kind"],
          additionalProperties: false,
          properties: {
            handle: { type: "string", pattern: "^[a-z0-9_]{1,64}$" },
            kind: {
              type: "string",
              enum: ["postgres", "sqlite", "object_store", "s3", "redis"],
              description: "Use object_store for new authoring; s3 is a compatibility alias normalized before publication."
            },
            read_only: { type: "boolean", default: true }
          }
        },
        description:
          "Optional workload mounts. For publication, provide only handle, kind, and read_only (default true); provider-owned binding values are rejected. New object-store authoring uses kind=object_store."
      },
      capabilities: {
        type: "array",
        items: { type: "string" },
        description:
          "Optional provider-required capability strings for publish_artifact or marketplace_publish, for example compute.gpu. GPU capabilities require a GPU-enabled provider."
      },
      limits: {
        type: "object",
        additionalProperties: false,
        properties: {
          max_input_bytes: { type: "integer", minimum: 1 },
          max_runtime_ms: { type: "integer", minimum: 1 },
          max_memory_bytes: { type: "integer", minimum: 1 },
          max_output_bytes: { type: "integer", minimum: 1 },
          fuel_limit: { type: "integer", minimum: 0 }
        },
        description: "Optional exact execution limits for marketplace_publish."
      },
      verification: {
        type: "object",
        required: ["input"],
        additionalProperties: false,
        properties: { input: {}, expected_output: {} },
        description:
          "Provider-private local-canary fixture for marketplace_publish; required for relay, Tor, and self-hosted publication, optional for local-only publication, and never signed or listed publicly."
      },
      wasm_module_hex: {
        type: "string",
        description:
          "Optional inline Wasm module bytes in hex for direct inline Wasm compute or publish_artifact. Prefer inline bytes or inline_source for agent-driven publication."
      },
      inline_source: {
        type: "string",
        description:
          "Optional inline source for a compute request. Use this when you want to run explicit source text, typically for runtime=python package_kind=inline_source."
      },
      source_kind: {
        type: "string",
        description: "Optional authoring source classification preserved by publish_artifact."
      },
      input: {},
      result_json: {
        description:
          "Optional static JSON result. Used with publish_artifact for constant-return services."
      },
      output_schema: {},
      input_schema: {},
      price_sats: { type: "integer", minimum: 0 },
      base_fee_msat: { type: "integer", minimum: 0 },
      success_fee_msat: { type: "integer", minimum: 0 },
      settlement_method: {
        type: "string",
        enum: ["none", "lightning", "stripe"],
        description: "Explicit settlement rail for publish_artifact. Paid publications must set this."
      },
      price_currency: {
        type: "string",
        enum: ["sat", "usd"],
        description: "Explicit price currency for publish_artifact."
      },
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
        enum: [...FROGLET_PAYMENT_RAILS],
        description:
          "Explicit payment rail for plan_install/get_install_guide. Required before commands are generated; use none for the first free demo service."
      },
      lightning_mode: {
        type: "string",
        enum: [...FROGLET_LIGHTNING_MODES],
        description:
          "Lightning mode for plan_install/get_install_guide. mock requires no wallet; lnd_rest requires an LND REST URL and macaroon path; phoenixd requires a running phoenixd daemon URL and its http-password (prepaid rail, no channel management)."
      },
      footprint: {
        type: "string",
        enum: ["auto", "native", "docker", "binary", "source"],
        description:
          "Install footprint for plan_install/get_install_guide. auto is the native-first no-clone default with a digest-pinned Docker fallback; native requires launchd/user-systemd; docker explicitly selects the immutable-image fallback; binary installs only froglet-node; source builds from the cloned repo."
      },
      role: {
        type: "string",
        enum: ["consumer", "provider", "both"],
        description:
          "User intent for plan_install/get_install_guide. No-clone bootstrap footprints start the dual-role node; split roles are a direct froglet-node concern."
      },
      network_mode: {
        type: "string",
        enum: ["clearnet", "tor", "dual"],
        description:
          "Network mode for local/self-hosted Froglet. Keep clearnet/loopback first; use tor or dual only after local health checks pass."
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
      release_tag: {
        type: "string",
        pattern: "^v[0-9]+\\.[0-9]+\\.[0-9]+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$",
        description:
          "Immutable GitHub release tag returned by plan_install. Omit on the first plan to resolve the latest immutable release; pass the exact returned tag to get_install_guide."
      },
      install_approval_hash: {
        type: "string",
        pattern: "^[0-9a-f]{64}$",
        description:
          "Exact approval hash returned by plan_install after it binds the immutable release/manifest/bootstrap, profile, persistent paths, process-manager impact, and command preview. Pass unchanged to get_install_guide only after user approval."
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
      offer_kind: {
        type: "string",
        description: "Offer-kind filter for marketplace_search (e.g. \"named.v1\")."
      },
      max_price_sats: {
        type: "integer",
        minimum: 0,
        description: "Upper price bound in sats for marketplace_search results."
      },
      ...MARKETPLACE_ATTESTATION_PROPERTIES,
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
      artifact_path: {
        type: "string",
        description:
          "Daemon-local artifact path for publish_artifact. Only use when the provider is configured with FROGLET_PROVIDER_ARTIFACT_ROOT and the path is known to canonicalize under that root."
      },
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
          "Python source code (full handler.py contents defining handler(event, context)) for marketplace_publish. Required when package_kind defaults to inline_source (the v1A only supported shape)."
      },
      hosting: {
        type: "object",
        description:
          "Hosting backend for marketplace_publish. kind ∈ {local|relay|tor|self}. relay = default outbound WSS reachability with relay TLS/plaintext disclosure; local = private dev; tor = hidden service; self = user-supplied URL.",
        properties: {
          kind: { type: "string", enum: ["local", "relay", "tor", "self"] },
          url: { type: "string", description: "Required when kind = 'self'." }
        }
      },
      settlement: {
        type: "object",
        description:
          "Settlement method for marketplace_publish. Supported: 'none' (free), 'lightning' (hold-invoice escrow, settled by the node's Lightning wallet; requires Lightning backend + price.currency='sat'), or 'stripe' (Stripe MPP / Shared Payment Tokens; requires Stripe backend + price.currency='usd').",
        properties: {
          method: { type: "string", enum: ["none", "lightning", "stripe"] }
        }
      },
      marketplace_url: {
        type: "string",
        description:
          "Marketplace URL for marketplace_publish, marketplace_register, and domain_claim (defaults to https://marketplace.froglet.dev), and the URL plan_install/get_install_guide export before starting the local stack."
      },
      consent_hash: {
        type: "string",
        pattern: "^[0-9a-f]{64}$",
        description:
          "Approval token returned by the prior marketplace_publish plan. Omit on the first call; Froglet returns the exact relay/plaintext, quota, identity-backup, limits, and price disclosure without opening a tunnel. After the user approves it, repeat with this hash."
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
