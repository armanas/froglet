import {
  discoverServices,
  frogletStatus,
  getDealInvoiceBundle,
  getDealPaymentIntent,
  getLocalService,
  getService,
  getTask,
  getWalletBalance,
  invokeService,
  listLocalServices,
  listSettlementActivity,
  publishArtifact,
  runCompute,
  waitTask
} from "./froglet-client.js"
import { toolTextResult } from "./shared.js"
import {
  appendRaw,
  firstDefined,
  formatObject,
  serviceAuthorityNotes,
  summarizeService,
  summarizeTask
} from "./summarize.js"

/**
 * Extract the subset of config fields needed for provider API calls.
 *
 * @param {object} config
 */
function providerCtx(config) {
  return {
    providerUrl: config.providerUrl,
    providerAuthTokenPath: config.providerAuthTokenPath,
    requestTimeoutMs: config.requestTimeoutMs
  }
}

/**
 * Extract the subset of config fields needed for runtime API calls.
 *
 * @param {object} config
 */
function runtimeCtx(config) {
  return {
    runtimeUrl: config.runtimeUrl,
    runtimeAuthTokenPath: config.runtimeAuthTokenPath,
    requestTimeoutMs: config.requestTimeoutMs
  }
}

function renderResult(lines, response, includeRaw) {
  return toolTextResult(appendRaw(lines, response, includeRaw).join("\n"))
}

function resolvedProviderId(args) {
  return firstDefined(args.provider_id, args.free_provider_id, args.paid_provider_id)
}

function resolvedProviderUrl(args) {
  return firstDefined(args.provider_url, args.free_provider_url, args.paid_provider_url)
}

function resolvedServiceId(args) {
  return firstDefined(args.service_id, args.free_service_id, args.async_service_id)
}

function computeOfferIds(response) {
  if (Array.isArray(response.raw_compute_offer_ids) && response.raw_compute_offer_ids.length > 0) {
    return response.raw_compute_offer_ids
  }
  if (typeof response.raw_compute_offer_id === "string" && response.raw_compute_offer_id.length > 0) {
    return [response.raw_compute_offer_id]
  }
  return ["execute.compute"]
}

function summarizeMutationResponse(response) {
  const offer = response.offer ?? {}
  const payload = offer.offer?.payload ?? {}
  const service = {
    service_id: offer.service_id ?? response.evidence?.service_id ?? "unknown",
    offer_id: payload.offer_id ?? response.evidence?.offer_id ?? "unknown",
    offer_kind: payload.offer_kind ?? "unknown",
    resource_kind: "service",
    project_id: offer.project_id ?? "none",
    summary: offer.summary ?? response.summary ?? "none",
    runtime: offer.runtime ?? "unknown",
    package_kind: offer.package_kind ?? "unknown",
    entrypoint_kind: offer.entrypoint_kind ?? "unknown",
    entrypoint: offer.entrypoint ?? "unknown",
    contract_version: offer.contract_version ?? "unknown",
    mounts: offer.mounts ?? [],
    mode: offer.mode ?? "unknown",
    price_sats: payload.price_sats ?? "unknown",
    publication_state: offer.publication_state ?? "unknown",
    provider_id: response.evidence?.provider_id ?? payload.provider_id ?? "unknown",
    input_schema: offer.input_schema,
    output_schema: offer.output_schema
  }
  return [
    `status: ${response.status ?? "unknown"}`,
    ...summarizeService(service),
    ...serviceAuthorityNotes(service),
    `offer_hash: ${response.evidence?.offer_hash ?? response.offer_hash ?? "none"}`
  ]
}

async function handleStatus(args, config, includeRaw) {
  const response = await frogletStatus({
    ...providerCtx(config),
    ...runtimeCtx(config)
  })
  const offerIds = computeOfferIds(response)
  const identity = response._identity ?? {}
  const lines = [
    `healthy: ${response.healthy === true}`,
    `node_id: ${response.node_id ?? "unknown"}`,
    `discovery_mode: ${identity.discovery?.mode ?? response.discovery?.mode ?? "unknown"}`,
    `reference_discovery_enabled: ${(identity.reference_discovery ?? response.reference_discovery)?.enabled === true}`,
    `reference_discovery_publish_enabled: ${(identity.reference_discovery ?? response.reference_discovery)?.publish_enabled === true}`,
    `reference_discovery_connected: ${(identity.reference_discovery ?? response.reference_discovery)?.connected === true}`,
    `reference_discovery_url: ${(identity.reference_discovery ?? response.reference_discovery)?.url ?? "none"}`,
    `reference_discovery_last_error: ${(identity.reference_discovery ?? response.reference_discovery)?.last_error ?? "none"}`,
    `compute_offer_ids: ${offerIds.join(", ")}`,
    "",
    `provider_healthy: ${response.provider?.healthy === true}`,
    `runtime_healthy: ${response.runtime?.healthy === true}`
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleDiscover(args, config, includeRaw) {
  const response = await discoverServices({
    ...runtimeCtx(config),
    limit: args.limit ?? config.defaultSearchLimit,
    includeInactive: args.include_inactive === true,
    query: args.query
  })
  const providers = Array.isArray(response.providers) ? response.providers : []
  const services = Array.isArray(response.services) ? response.services : []
  const lines = [
    `providers: ${providers.length}`,
    `services: ${services.length}`,
    "",
    ...(services.length > 0
      ? services.flatMap((service, index) => [`${index + 1}.`, ...summarizeService(service), ""])
      : ["no remote services discovered"]),
    "Only listed fields are authoritative. Use get_service for one service at a time."
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleGetService(args, config, includeRaw) {
  const response = await getService({
    ...runtimeCtx(config),
    searchLimit: args.limit ?? config.defaultSearchLimit,
    request: {
      provider_id: resolvedProviderId(args),
      provider_url: resolvedProviderUrl(args),
      service_id: resolvedServiceId(args)
    }
  })
  const lines = [
    ...summarizeService(response.service ?? {}),
    ...serviceAuthorityNotes(response.service ?? {})
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleInvoke(args, config, includeRaw) {
  const response = await invokeService({
    ...runtimeCtx(config),
    searchLimit: args.limit ?? config.defaultSearchLimit,
    request: {
      provider_id: resolvedProviderId(args),
      provider_url: resolvedProviderUrl(args),
      service_id: resolvedServiceId(args),
      input: args.input
    }
  })
  const effectiveResult =
    response.result !== undefined ? response.result : response.task?.result
  const lines = response.task
    ? [
        ...summarizeTask(response.task),
        `terminal: ${response.terminal === true}`,
        `result: ${formatObject(effectiveResult)}`,
        ...(response.terminal === true
          ? []
          : ["pending: use wait_task with the returned task_id if you need the final result"])
      ]
    : [`status: ${response.status ?? "unknown"}`, `result: ${formatObject(effectiveResult)}`]
  return renderResult(lines, response, includeRaw)
}

async function handleLocalServices(args, config, includeRaw) {
  const serviceId = resolvedServiceId(args)
  if (serviceId) {
    const response = await getLocalService({
      ...providerCtx(config),
      serviceId
    })
    const lines = [
      ...summarizeService(response.service ?? {}),
      ...serviceAuthorityNotes(response.service ?? {})
    ]
    return renderResult(lines, response, includeRaw)
  }

  const response = await listLocalServices(providerCtx(config))
  const services = Array.isArray(response.services) ? response.services : []
  const lines = [
    `services: ${services.length}`,
    "",
    ...(services.length > 0
      ? services.flatMap((service, index) => [`${index + 1}.`, ...summarizeService(service), ""])
      : ["no local services"]),
    "",
    "Only listed fields are authoritative. Use get_local_service for one service at a time."
  ]
  return renderResult(lines, response, includeRaw)
}

async function handlePublishArtifact(args, config, includeRaw) {
  const response = await publishArtifact({
    ...providerCtx(config),
    request: {
      service_id: resolvedServiceId(args),
      offer_id: args.offer_id,
      summary: args.summary,
      artifact_path: args.artifact_path,
      wasm_module_hex: args.wasm_module_hex,
      inline_source: args.inline_source,
      oci_reference: args.oci_reference,
      oci_digest: args.oci_digest,
      runtime: args.runtime,
      package_kind: args.package_kind,
      entrypoint_kind: args.entrypoint_kind,
      entrypoint: args.entrypoint,
      contract_version: args.contract_version,
      mounts: args.mounts,
      mode: args.mode,
      price_sats: args.price_sats,
      publication_state: args.publication_state,
      input_schema: args.input_schema,
      output_schema: args.output_schema
    }
  })
  return renderResult(summarizeMutationResponse(response), response, includeRaw)
}

async function handleTask(args, config, includeRaw) {
  if (args.wait) {
    const response = await waitTask({
      ...providerCtx(config),
      ...runtimeCtx(config),
      taskId: args.task_id,
      timeoutSecs: args.timeout_secs,
      pollIntervalSecs: args.poll_interval_secs
    })
    return renderResult(summarizeTask(response.task ?? {}), response, includeRaw)
  }

  const response = await getTask({
    ...providerCtx(config),
    ...runtimeCtx(config),
    taskId: args.task_id
  })
  return renderResult(summarizeTask(response.task ?? {}), response, includeRaw)
}

async function handleCompute(args, config, includeRaw) {
  const response = await runCompute({
    ...runtimeCtx(config),
    searchLimit: args.limit ?? config.defaultSearchLimit,
    trustedProviderUrl:
      resolvedProviderUrl(args) == null && resolvedProviderId(args) != null ? config.providerUrl : null,
    request: {
      provider_id: resolvedProviderId(args),
      provider_url: resolvedProviderUrl(args),
      input: args.input,
      artifact_path: args.artifact_path,
      wasm_module_hex: args.wasm_module_hex,
      inline_source: args.inline_source,
      oci_reference: args.oci_reference,
      oci_digest: args.oci_digest,
      runtime: args.runtime,
      package_kind: args.package_kind,
      entrypoint_kind: args.entrypoint_kind,
      entrypoint: args.entrypoint,
      contract_version: args.contract_version,
      mounts: args.mounts,
      timeout_secs: args.timeout_secs ?? 15
    }
  })
  const lines = response.task
    ? [...summarizeTask(response.task), `terminal: ${response.terminal === true}`]
    : [`status: ${response.status ?? "unknown"}`, `result: ${formatObject(response.result)}`]
  return renderResult(lines, response, includeRaw)
}

async function handleWalletBalance(args, config, includeRaw) {
  const response = await getWalletBalance(runtimeCtx(config))
  const lines = [
    `backend: ${response.backend ?? "unknown"}`,
    `mode: ${response.mode ?? "unknown"}`,
    `balance_known: ${response.balance_known === true}`,
    `balance_sats: ${response.balance_sats ?? "unknown"}`,
    `accepted_payment_methods: ${
      Array.isArray(response.accepted_payment_methods)
        ? response.accepted_payment_methods.join(", ") || "none"
        : "unknown"
    }`,
    `reservations: ${response.reservations === true}`,
    `receipts: ${response.receipts === true}`
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleSettlementActivity(args, config, includeRaw) {
  const response = await listSettlementActivity({
    ...runtimeCtx(config),
    limit: typeof args.limit === "number" ? args.limit : undefined
  })
  const items = Array.isArray(response.items) ? response.items : []
  const lines = [
    `count: ${items.length}`,
    `limit: ${response.limit ?? "unknown"}`,
    ""
  ]
  if (items.length === 0) {
    lines.push("no recent settlement activity")
  } else {
    for (const [index, item] of items.entries()) {
      lines.push(
        `${index + 1}.`,
        `  deal_id: ${item.deal_id}`,
        `  provider_id: ${item.provider_id}`,
        `  status: ${item.status}`,
        `  workload_kind: ${item.workload_kind ?? "unknown"}`,
        `  settlement_method: ${item.settlement_method ?? "unknown"}`,
        `  base_fee_msat: ${item.base_fee_msat ?? 0}`,
        `  success_fee_msat: ${item.success_fee_msat ?? 0}`,
        `  has_receipt: ${item.has_receipt === true}`,
        `  has_result: ${item.has_result === true}`,
        ...(item.error ? [`  error: ${item.error}`] : []),
        ""
      )
    }
  }
  return renderResult(lines, response, includeRaw)
}

async function handleDealPaymentIntent(args, config, includeRaw) {
  const dealId = typeof args.deal_id === "string" ? args.deal_id.trim() : ""
  if (dealId.length === 0) {
    throw new Error("deal_id is required for get_payment_intent")
  }
  const response = await getDealPaymentIntent({
    ...runtimeCtx(config),
    dealId
  })
  const intent = response.payment_intent ?? response.intent ?? response
  const lines = [
    `deal_id: ${dealId}`,
    `intent: ${formatObject(intent)}`
  ]
  return renderResult(lines, response, includeRaw)
}

const SUPPORTED_INSTALL_AGENTS = new Set(["claude-code", "codex", "openclaw"])
const SUPPORTED_INSTALL_RAILS = new Set(["lightning", "stripe", "x402"])

function renderInstallBlock({ targetAgent, paymentRail }) {
  // The public helper-script path is repo-local by design. Keep this in sync
  // with README.md, docs-site/src/content/docs/learn/quickstart.mdx, and the
  // landing-page configurator in docs-site/src/pages/index.astro.
  const paymentStep =
    paymentRail === "stripe"
      ? "cd froglet && FROGLET_STRIPE_SECRET_KEY=<stripe-test-secret-key> ./scripts/setup-payment.sh stripe"
      : paymentRail === "x402"
        ? "cd froglet && FROGLET_X402_WALLET_ADDRESS=<base-wallet-address> ./scripts/setup-payment.sh x402"
        : "cd froglet && ./scripts/setup-payment.sh lightning"
  const stepOne =
    "curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | sh"
  const stepTwo = "git clone https://github.com/armanas/froglet.git"
  const stepThree = `cd froglet && ./scripts/setup-agent.sh --target ${targetAgent}`
  const stepFour = paymentStep
  const stepFive = `cd froglet && set -a && . ./.froglet/payment/${paymentRail}.env && export FROGLET_HOST_READABLE_CONTROL_TOKEN=true && set +a && docker compose up --build -d`
  return [stepOne, stepTwo, stepThree, stepFour, stepFive]
}

async function handleInstallGuide(args, _config, includeRaw) {
  // Surface guidance for an LLM whose user has just asked to install Froglet
  // locally. The LLM is expected to execute the returned commands through
  // its own host shell (Claude Code's Bash, Codex's shell, etc.) — NOT
  // through the Froglet runtime, which has no way to touch the user's host
  // filesystem or docker socket.
  const rawTarget =
    typeof args.target_agent === "string" && args.target_agent.trim().length > 0
      ? args.target_agent.trim().toLowerCase()
      : "claude-code"
  if (!SUPPORTED_INSTALL_AGENTS.has(rawTarget)) {
    throw new Error(
      `target_agent must be one of: ${[...SUPPORTED_INSTALL_AGENTS].join(", ")}`
    )
  }
  const rawRail =
    typeof args.payment_rail === "string" && args.payment_rail.trim().length > 0
      ? args.payment_rail.trim().toLowerCase()
      : "lightning"
  if (!SUPPORTED_INSTALL_RAILS.has(rawRail)) {
    throw new Error(
      `payment_rail must be one of: ${[...SUPPORTED_INSTALL_RAILS].join(", ")}`
    )
  }

  const steps = renderInstallBlock({ targetAgent: rawTarget, paymentRail: rawRail })
  const payload = {
    target_agent: rawTarget,
    payment_rail: rawRail,
    steps,
    run_as: "user-host-shell",
    notes: [
      "Run these commands on the user's machine, via your host agent's shell execution (e.g. Claude Code's Bash tool). Do NOT route them through the Froglet runtime — Froglet cannot install itself on the user's host.",
      "Step 1 downloads and installs the signed froglet-node binary to ~/.local/bin.",
      "Step 2 clones the public froglet repo because the helper scripts, Compose file, and OpenClaw plugin live there.",
      `Step 3 writes the ${rawTarget} config so the agent can talk to the local Froglet.`,
      `Step 4 generates the ${rawRail} payment-rail env snippet under froglet/.froglet/payment/.`,
      "Step 5 loads that snippet, enables host-readable control tokens, and brings up the provider+runtime via docker compose.",
      "The repo-local steps intentionally start with `cd froglet &&` so they still work when your host shell asks for separate approvals per command.",
      "After step 5, the local stack listens on 127.0.0.1:8080 (provider) and 127.0.0.1:8081 (runtime); the agent config points there.",
      `${
        rawRail === "stripe"
          ? "Stripe: replace <stripe-test-secret-key> with your own Stripe test secret key before running step 4."
          : rawRail === "x402"
            ? "x402: replace <base-wallet-address> with your own Base wallet address before running step 4."
            : "Lightning: step 4 ships in mock mode by default; no wallet credentials required."
      }`
    ]
  }

  const lines = [
    `target_agent: ${rawTarget}`,
    `payment_rail: ${rawRail}`,
    `run_as: ${payload.run_as}`,
    "",
    "Commands to execute on the user's host (one per line):",
    ...steps.map((step, index) => `  ${index + 1}. ${step}`),
    "",
    "Notes:",
    ...payload.notes.map((note) => `  - ${note}`)
  ]
  return renderResult(lines, payload, includeRaw)
}

async function handleMarketplaceInvoke(args, config, includeRaw, { serviceId, input }) {
  // Thin wrapper: invoke the named marketplace.* service with the provided
  // input shape, letting the LLM caller optionally steer which marketplace
  // to hit (provider_id / provider_url). When neither is set we fall back
  // to the runtime's configured marketplace (FROGLET_MARKETPLACE_URL).
  const response = await invokeService({
    ...runtimeCtx(config),
    searchLimit: args.limit ?? config.defaultSearchLimit,
    request: {
      provider_id: resolvedProviderId(args),
      provider_url: resolvedProviderUrl(args),
      service_id: serviceId,
      input
    }
  })
  const effectiveResult =
    response.result !== undefined ? response.result : response.task?.result
  const lines = response.task
    ? [
        ...summarizeTask(response.task),
        `terminal: ${response.terminal === true}`,
        `result: ${formatObject(effectiveResult)}`
      ]
    : [`status: ${response.status ?? "unknown"}`, `result: ${formatObject(effectiveResult)}`]
  return renderResult(lines, response, includeRaw)
}

async function handleMarketplaceSearch(args, config, includeRaw) {
  return handleMarketplaceInvoke(args, config, includeRaw, {
    serviceId: "marketplace.search",
    input: {
      ...(typeof args.offer_kind === "string" && args.offer_kind.length > 0
        ? { offer_kind: args.offer_kind }
        : {}),
      ...(typeof args.runtime === "string" && args.runtime.length > 0
        ? { runtime: args.runtime }
        : {}),
      ...(typeof args.max_price_sats === "number"
        ? { max_price_sats: args.max_price_sats }
        : {}),
      ...(typeof args.cursor === "string" && args.cursor.length > 0
        ? { cursor: args.cursor }
        : {}),
      ...(typeof args.limit === "number" ? { limit: args.limit } : {})
    }
  })
}

async function handleMarketplaceProvider(args, config, includeRaw) {
  const providerId = typeof args.marketplace_provider_id === "string"
    ? args.marketplace_provider_id.trim()
    : ""
  if (providerId.length === 0) {
    throw new Error("marketplace_provider_id is required for marketplace_provider")
  }
  return handleMarketplaceInvoke(args, config, includeRaw, {
    serviceId: "marketplace.provider",
    input: { provider_id: providerId }
  })
}

async function handleMarketplaceReceipts(args, config, includeRaw) {
  const providerId = typeof args.marketplace_provider_id === "string"
    ? args.marketplace_provider_id.trim()
    : ""
  if (providerId.length === 0) {
    throw new Error("marketplace_provider_id is required for marketplace_receipts")
  }
  return handleMarketplaceInvoke(args, config, includeRaw, {
    serviceId: "marketplace.receipts",
    input: {
      provider_id: providerId,
      ...(typeof args.status === "string" && args.status.length > 0
        ? { status: args.status }
        : {}),
      ...(typeof args.cursor === "string" && args.cursor.length > 0
        ? { cursor: args.cursor }
        : {}),
      ...(typeof args.limit === "number" ? { limit: args.limit } : {})
    }
  })
}

async function handleMarketplaceStake(args, config, includeRaw) {
  const providerId = typeof args.marketplace_provider_id === "string"
    ? args.marketplace_provider_id.trim()
    : ""
  if (providerId.length === 0) {
    throw new Error("marketplace_provider_id is required for marketplace_stake")
  }
  if (typeof args.amount_msat !== "number" || !Number.isFinite(args.amount_msat) || args.amount_msat <= 0) {
    throw new Error("amount_msat must be a positive number for marketplace_stake")
  }
  return handleMarketplaceInvoke(args, config, includeRaw, {
    serviceId: "marketplace.stake",
    input: {
      provider_id: providerId,
      amount_msat: args.amount_msat
    }
  })
}

async function handleMarketplaceTopup(args, config, includeRaw) {
  const providerId = typeof args.marketplace_provider_id === "string"
    ? args.marketplace_provider_id.trim()
    : ""
  if (providerId.length === 0) {
    throw new Error("marketplace_provider_id is required for marketplace_topup")
  }
  if (typeof args.amount_msat !== "number" || !Number.isFinite(args.amount_msat) || args.amount_msat <= 0) {
    throw new Error("amount_msat must be a positive number for marketplace_topup")
  }
  return handleMarketplaceInvoke(args, config, includeRaw, {
    serviceId: "marketplace.topup",
    input: {
      provider_id: providerId,
      amount_msat: args.amount_msat
    }
  })
}

async function handleDealInvoiceBundle(args, config, includeRaw) {
  const dealId = typeof args.deal_id === "string" ? args.deal_id.trim() : ""
  if (dealId.length === 0) {
    throw new Error("deal_id is required for get_invoice_bundle")
  }
  const response = await getDealInvoiceBundle({
    ...providerCtx(config),
    dealId
  })
  const bundle = response.bundle ?? response.invoice_bundle ?? response
  const lines = [
    `deal_id: ${dealId}`,
    `bundle: ${formatObject(bundle)}`
  ]
  return renderResult(lines, response, includeRaw)
}

export async function dispatchFrogletAction(args, config, { includeRaw = false } = {}) {
  switch (args.action) {
    case "status":
      return handleStatus(args, config, includeRaw)
    case "discover_services":
      return handleDiscover(args, config, includeRaw)
    case "get_service":
      return handleGetService(args, config, includeRaw)
    case "invoke_service":
      return handleInvoke(args, config, includeRaw)
    case "list_local_services":
      return handleLocalServices(args, config, includeRaw)
    case "get_local_service":
      return handleLocalServices(args, config, includeRaw)
    case "publish_artifact":
      return handlePublishArtifact(args, config, includeRaw)
    case "get_task":
      return handleTask({ ...args, wait: false }, config, includeRaw)
    case "wait_task":
      return handleTask({ ...args, wait: true }, config, includeRaw)
    case "run_compute":
      return handleCompute(args, config, includeRaw)
    case "get_wallet_balance":
      return handleWalletBalance(args, config, includeRaw)
    case "list_settlement_activity":
      return handleSettlementActivity(args, config, includeRaw)
    case "get_payment_intent":
      return handleDealPaymentIntent(args, config, includeRaw)
    case "get_invoice_bundle":
      return handleDealInvoiceBundle(args, config, includeRaw)
    case "get_install_guide":
      return handleInstallGuide(args, config, includeRaw)
    case "marketplace_search":
      return handleMarketplaceSearch(args, config, includeRaw)
    case "marketplace_provider":
      return handleMarketplaceProvider(args, config, includeRaw)
    case "marketplace_receipts":
      return handleMarketplaceReceipts(args, config, includeRaw)
    case "marketplace_stake":
      return handleMarketplaceStake(args, config, includeRaw)
    case "marketplace_topup":
      return handleMarketplaceTopup(args, config, includeRaw)
    // Removed actions — return clear error messages
    case "tail_logs":
      throw new Error("Log tailing removed; use systemd journal directly")
    case "restart":
      throw new Error("Restart removed; use systemctl directly")
    case "list_projects":
    case "create_project":
    case "get_project":
    case "read_file":
    case "write_file":
    case "build_project":
    case "test_project":
    case "publish_project":
      throw new Error("Project authoring not available in current API")
    default:
      throw new Error(`Unknown Froglet action: ${args.action}`)
  }
}
