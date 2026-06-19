import {
  discoverServices,
  completeProviderDomainClaim,
  createProviderDomainClaim,
  fileMarketplaceComplaint,
  frogletStatus,
  getDealInvoiceBundle,
  getDealPaymentIntent,
  getLocalService,
  getMarketplaceComplaint,
  getService,
  getTask,
  getWalletBalance,
  invokeService,
  listLocalServices,
  listSettlementActivity,
  publishArtifact,
  registerProviderOnMarketplace,
  runCompute,
  waitTask
} from "./froglet-client.js"
import { runMarketplacePublish } from "./marketplace-publish.js"
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

function marketplaceDeps(config) {
  return config?._deps?.marketplace
}

function clientDeps(config) {
  return config?._deps?.client
}

function marketplacePublishDeps(config) {
  return config?._deps?.marketplacePublish
    ?? config?._deps?.marketplace
    ?? config?._deps?.client
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

function publishTemplateRequest(args) {
  if (typeof args.template !== "string" || args.template.trim().length === 0) {
    return args
  }
  const template = args.template.trim()
  if (template !== "demo.add") {
    throw new Error("template must be demo.add")
  }
  return {
    ...args,
    service_id: resolvedServiceId(args) ?? "demo.add.local",
    offer_id: args.offer_id ?? "demo.add.local",
    summary: args.summary ?? "Free local demo service that adds two integers.",
    starter: args.starter ?? "{\"a\":7,\"b\":5}",
    runtime: args.runtime ?? "python",
    package_kind: args.package_kind ?? "inline_source",
    entrypoint_kind: args.entrypoint_kind ?? "handler",
    entrypoint: args.entrypoint ?? "handler",
    contract_version: args.contract_version ?? "froglet.python.handler_json.v1",
    inline_source:
      args.inline_source ??
      "def handler(event, context):\n    return {\"sum\": int(event[\"a\"]) + int(event[\"b\"])}\n",
    input_schema:
      args.input_schema ?? {
        type: "object",
        required: ["a", "b"],
        properties: {
          a: { type: "integer" },
          b: { type: "integer" }
        }
      },
    output_schema:
      args.output_schema ?? {
        type: "object",
        required: ["sum"],
        properties: {
          sum: { type: "integer" }
        }
      },
    price_sats: args.price_sats ?? 0,
    publication_state: args.publication_state ?? "active",
    mode: args.mode ?? "sync"
  }
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
    _deps: clientDeps(config),
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
    trustedProviderUrl:
      resolvedProviderUrl(args) == null && resolvedProviderId(args) != null ? config.providerUrl : null,
    _deps: clientDeps(config),
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
  const requestArgs = publishTemplateRequest(args)
  const response = await publishArtifact({
    ...providerCtx(config),
    request: {
      service_id: resolvedServiceId(requestArgs),
      offer_id: requestArgs.offer_id,
      summary: requestArgs.summary,
      starter: requestArgs.starter,
      artifact_path: requestArgs.artifact_path,
      wasm_module_hex: requestArgs.wasm_module_hex,
      inline_source: requestArgs.inline_source,
      oci_reference: requestArgs.oci_reference,
      oci_digest: requestArgs.oci_digest,
      runtime: requestArgs.runtime,
      package_kind: requestArgs.package_kind,
      entrypoint_kind: requestArgs.entrypoint_kind,
      entrypoint: requestArgs.entrypoint,
      contract_version: requestArgs.contract_version,
      mounts: requestArgs.mounts,
      capabilities: requestArgs.capabilities,
      mode: requestArgs.mode,
      price_sats: requestArgs.price_sats,
      publication_state: requestArgs.publication_state,
      input_schema: requestArgs.input_schema,
      output_schema: requestArgs.output_schema
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
    _deps: clientDeps(config),
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
      capabilities: args.capabilities,
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

const SUPPORTED_INSTALL_AGENTS = new Set(["claude-code", "codex", "openclaw", "manual"])
const SETUP_AGENT_TARGETS = new Set(["claude-code", "codex", "openclaw"])
const LOCAL_MCP_AGENT_TARGETS = new Set(["claude-code", "codex"])
const SUPPORTED_INSTALL_RAILS = new Set([
  "none",
  "lightning-mock",
  "lightning-lnd-rest",
  "stripe-test",
  "stripe-live",
  "x402",
  // Backward-compatible aliases accepted on input; tool schemas should prefer
  // the explicit values above.
  "lightning",
  "stripe"
])
const SUPPORTED_LIGHTNING_MODES = new Set(["mock", "lnd_rest"])
const SUPPORTED_INSTALL_FOOTPRINTS = new Set(["docker", "binary", "source"])
const SUPPORTED_INSTALL_ROLES = new Set(["consumer", "provider", "both"])
const SUPPORTED_NETWORK_MODES = new Set(["clearnet", "tor", "dual"])
const SUPPORTED_USE_CASE_PROFILES = new Set([
  "consumer",
  "provider",
  "evidence",
  "payments",
  "batch",
  "gpu"
])

function normalizeChoice(args, field, defaultValue, supported) {
  const raw =
    typeof args[field] === "string" && args[field].trim().length > 0
      ? args[field].trim().toLowerCase()
      : defaultValue
  if (!supported.has(raw)) {
    throw new Error(`${field} must be one of: ${[...supported].join(", ")}`)
  }
  return raw
}

function optionalString(args, field) {
  return typeof args[field] === "string" && args[field].trim().length > 0
    ? args[field].trim()
    : undefined
}

function shellSingleQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`
}

function normalizePaymentRail(args) {
  if (typeof args.payment_rail !== "string" || args.payment_rail.trim().length === 0) {
    return null
  }
  const raw = args.payment_rail.trim().toLowerCase()
  if (!SUPPORTED_INSTALL_RAILS.has(raw)) {
    throw new Error(`${"payment_rail"} must be one of: none, lightning-mock, lightning-lnd-rest, stripe-test, stripe-live, x402`)
  }
  if (raw === "lightning") {
    return args.lightning_mode === "lnd_rest" ? "lightning-lnd-rest" : "lightning-mock"
  }
  if (raw === "stripe") {
    return "stripe-test"
  }
  return raw
}

function normalizeInstallProfile(args) {
  const paymentRail = normalizePaymentRail(args)
  const profile = {
    targetAgent: normalizeChoice(args, "target_agent", "claude-code", SUPPORTED_INSTALL_AGENTS),
    paymentRail,
    lightningMode: paymentRail === "lightning-lnd-rest" ? "lnd_rest" : "mock",
    footprint: normalizeChoice(args, "footprint", "docker", SUPPORTED_INSTALL_FOOTPRINTS),
    role: normalizeChoice(args, "role", "both", SUPPORTED_INSTALL_ROLES),
    networkMode: normalizeChoice(args, "network_mode", "clearnet", SUPPORTED_NETWORK_MODES),
    marketplaceUrl: optionalString(args, "marketplace_url"),
    useCase: optionalString(args, "use_case")
  }
  if (profile.marketplaceUrl && !/^https?:\/\/[^\s]+$/.test(profile.marketplaceUrl)) {
    throw new Error("marketplace_url must be an http:// or https:// URL")
  }
  if (args.lightning_mode && !["lightning", "lightning-mock", "lightning-lnd-rest"].includes(String(args.payment_rail ?? ""))) {
    throw new Error("lightning_mode only applies when payment_rail is lightning-mock or lightning-lnd-rest")
  }
  return profile
}

function paymentEnvName(paymentRail) {
  if (paymentRail === "lightning-mock" || paymentRail === "lightning-lnd-rest") {
    return "lightning.env"
  }
  if (paymentRail === "stripe-test" || paymentRail === "stripe-live") {
    return "stripe.env"
  }
  return `${paymentRail}.env`
}

function renderPaymentStep({ paymentRail, lightningMode }) {
  if (paymentRail === "none") {
    return "cd froglet && mkdir -p .froglet/payment && printf '%s\\n' 'FROGLET_PAYMENT_BACKEND=none' > .froglet/payment/none.env"
  }
  if (paymentRail === "stripe-test") {
    return "cd froglet && FROGLET_STRIPE_SECRET_KEY=<stripe-test-secret-key> ./scripts/setup-payment.sh stripe"
  }
  if (paymentRail === "stripe-live") {
    return "cd froglet && FROGLET_STRIPE_SECRET_KEY=<stripe-live-secret-key> FROGLET_STRIPE_LIVE_CONFIRM=fresh ./scripts/setup-payment.sh stripe"
  }
  if (paymentRail === "x402") {
    return "cd froglet && FROGLET_X402_WALLET_ADDRESS=<base-wallet-address> ./scripts/setup-payment.sh x402"
  }
  if (paymentRail === "lightning-lnd-rest" || lightningMode === "lnd_rest") {
    return "cd froglet && FROGLET_LIGHTNING_REST_URL=<lnd-rest-url> FROGLET_LIGHTNING_MACAROON_PATH=<macaroon-path> FROGLET_LIGHTNING_TLS_CERT_PATH=<tls-cert-path-if-needed> ./scripts/setup-payment.sh lightning --mode lnd_rest"
  }
  return "cd froglet && ./scripts/setup-payment.sh lightning"
}

function renderComposeEnv(profile) {
  const vars = []
  if (profile.networkMode !== "clearnet") {
    vars.push(`FROGLET_NETWORK_MODE=${profile.networkMode}`)
  }
  if (profile.marketplaceUrl) {
    vars.push(`FROGLET_MARKETPLACE_URL=${shellSingleQuote(profile.marketplaceUrl)}`)
  }
  return vars.length > 0 ? `${vars.join(" ")} ` : ""
}

function renderInstallBlock(profile) {
  if (!profile.paymentRail) {
    return []
  }
  // The default public path is intentionally no-clone. It installs the signed
  // node binary, starts provider/runtime from published GHCR images, and emits
  // MCP setup JSON. Repo-local helper scripts are contributor/source mode.
  if (profile.footprint === "docker") {
    const vars = [
      profile.targetAgent !== "claude-code" ? `FROGLET_AGENT_TARGET=${profile.targetAgent}` : null,
      profile.networkMode !== "clearnet" ? `FROGLET_NETWORK_MODE=${profile.networkMode}` : null,
      profile.marketplaceUrl ? `FROGLET_MARKETPLACE_URL=${shellSingleQuote(profile.marketplaceUrl)}` : null
    ].filter(Boolean)
    const prefix = vars.length > 0 ? `${vars.join(" ")} ` : ""
    return [`${prefix}curl -fsSL https://froglet.dev/agent | bash`]
  }
  const stepOne =
    "curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | sh"
  if (profile.footprint === "binary") {
    return [stepOne]
  }
  const steps = ["git clone https://github.com/armanas/froglet.git"]
  if (profile.footprint === "source") {
    steps.push("cd froglet && cargo build --release --bin froglet-node -p froglet")
  } else {
    steps.unshift(stepOne)
  }
  if (LOCAL_MCP_AGENT_TARGETS.has(profile.targetAgent)) {
    steps.push("cd froglet && npm ci --prefix integrations/mcp/froglet")
  }
  if (SETUP_AGENT_TARGETS.has(profile.targetAgent)) {
    steps.push(`cd froglet && ./scripts/setup-agent.sh --target ${profile.targetAgent}`)
  }
  steps.push(renderPaymentStep(profile))
  const envName = paymentEnvName(profile.paymentRail)
  const startCommand =
    profile.footprint === "source"
      ? `cd froglet && set -a && . ./.froglet/payment/${envName} && set +a && ${renderComposeEnv(profile)}./target/release/froglet-node`
      : `cd froglet && set -a && . ./.froglet/payment/${envName} && export FROGLET_HOST_READABLE_CONTROL_TOKEN=true && set +a && ${renderComposeEnv(profile)}docker compose up --build -d`
  steps.push(startCommand)
  return steps
}

function installQuestions(args) {
  const questions = []
  if (!args.target_agent) {
    questions.push("Which agent should be configured: claude-code, codex, openclaw, or manual?")
  }
  if (!args.footprint) {
    questions.push("Which install footprint: binary, docker, or source?")
  }
  if (!args.role) {
    questions.push("Is the user primarily a consumer, provider, or both?")
  }
  if (!args.payment_rail) {
    questions.push("Which payment mode: none, lightning-mock, lightning-lnd-rest, stripe-test, stripe-live, or x402?")
  }
  if (!args.network_mode) {
    questions.push("Which network mode: clearnet, tor, or dual?")
  }
  if (!args.use_case) {
    questions.push("What first use case should be implemented after install?")
  }
  return questions
}

function requiredInstallInputs(profile) {
  if (!profile.paymentRail) {
    return [
      "Payment choice is required before commands are generated: none, lightning-mock, lightning-lnd-rest, stripe-test, stripe-live, or x402."
    ]
  }
  if (profile.paymentRail === "stripe-test") {
    return ["Stripe test-mode secret key (`sk_test_...`)."]
  }
  if (profile.paymentRail === "stripe-live") {
    return ["Stripe live-mode secret key (`sk_live_...`) and a literal fresh approval before live payment proof."]
  }
  if (profile.paymentRail === "x402") {
    return ["Base wallet address (`0x...`)."]
  }
  if (profile.paymentRail === "lightning-lnd-rest") {
    return [
      "LND REST URL.",
      "LND macaroon path.",
      "LND TLS certificate path when the endpoint uses a private CA."
    ]
  }
  return ["No payment secret is required for this profile."]
}

function installPrerequisites(profile) {
  const prerequisites = ["curl"]
  if (profile.footprint === "source") {
    prerequisites.push("git")
  }
  if (profile.footprint === "docker") {
    prerequisites.push("Docker with Compose v2")
  }
  if (profile.footprint === "source") {
    prerequisites.push("Rust toolchain with cargo")
  }
  if (profile.footprint !== "docker" && SETUP_AGENT_TARGETS.has(profile.targetAgent)) {
    prerequisites.push("Node.js 18+ with npm for the local MCP server")
  }
  if (profile.networkMode !== "clearnet") {
    prerequisites.push("Tor installed and reachable by the Froglet node")
  }
  return prerequisites
}

function validationChecks(profile) {
  const checks = []
  if (!profile.paymentRail) {
    checks.push("Choose a payment mode before running install commands.")
    return checks
  }
  if (profile.footprint === "binary") {
    checks.push("Run `froglet-node --help` to confirm the signed binary is installed.")
    return checks
  }
  if (profile.footprint === "source") {
    checks.push("Confirm the foreground `froglet-node` process starts without errors.")
  } else if (profile.footprint === "docker") {
    checks.push("Run `docker compose -f ~/.froglet/agent/compose.yaml ps` and confirm provider/runtime are healthy.")
  } else {
    checks.push("Run `docker compose ps` and confirm provider/runtime are healthy.")
  }
  checks.push("Run `curl http://127.0.0.1:8080/health`.")
  checks.push("Run `curl http://127.0.0.1:8081/health`.")
  checks.push("Use the Froglet `status` action against the local MCP config.")
  if (profile.paymentRail === "stripe-test") {
    checks.push("Confirm `setup-payment.sh stripe` reported `livemode=false`.")
  } else if (profile.paymentRail === "stripe-live") {
    checks.push("Confirm `setup-payment.sh stripe` reported `livemode=true` after a literal fresh approval.")
  } else if (profile.paymentRail === "x402") {
    checks.push("Confirm the x402 facilitator `/verify` probe returned an expected HTTP status.")
  } else if (profile.paymentRail === "lightning-lnd-rest") {
    checks.push("Confirm the LND REST `/v1/getinfo` probe succeeded.")
  }
  return checks
}

function postInstallPlaybooks() {
  return [
    "consumer-first: list services, invoke a free local service, inspect receipt/feed evidence.",
    "provider-first: publish a small service, inspect descriptor/offer, then invoke it locally.",
    "evidence-first: witness a URL, hash-verify a pinned asset, or notarize a content hash.",
    "payments-first: run a mock Lightning paid deal before entering real Stripe/x402/LND credentials.",
    "batch-first: use existing async task status primitives for one long task; multi-item batch fan-out remains Order 44 work.",
    "gpu-first: require provider-advertised GPU capability and hardware verification before accepting GPU work.",
    "network-first: keep loopback first, then move to clearnet, Tor, or dual after health checks pass."
  ]
}

function inferUseCaseProfile(args) {
  if (typeof args.workload_profile === "string" && args.workload_profile.trim().length > 0) {
    const profile = args.workload_profile.trim().toLowerCase()
    if (!SUPPORTED_USE_CASE_PROFILES.has(profile)) {
      throw new Error(`workload_profile must be one of: ${[...SUPPORTED_USE_CASE_PROFILES].join(", ")}`)
    }
    return profile
  }
  const text = `${args.use_case ?? ""}`.toLowerCase()
  if (/\bgpu\b|cuda|nvidia|accelerat/.test(text)) return "gpu"
  if (/\bbatch\b|queue|long[- ]?running|async|fan[- ]?out/.test(text)) return "batch"
  if (/pay|stripe|x402|lightning|invoice|settle/.test(text)) return "payments"
  if (/witness|hash|notari[sz]e|receipt|attest|prove/.test(text)) return "evidence"
  if (/publish|provider|offer|service/.test(text)) return "provider"
  return "consumer"
}

function useCaseSteps(profile) {
  const common = [
    "Call `status` and do not continue unless provider_healthy and runtime_healthy are true.",
    "If status fails, call `plan_install` instead of guessing local URLs or token paths."
  ]
  if (profile === "provider") {
    return [
      ...common,
      "HEADLINE PATH for publishing user-described services: call `marketplace_publish` with `{name, source_inline, hosting:{kind:'tor'|'local'|'self'}}`. The handler shells out to `froglet-node publish --json`, which scaffolds manifests, builds the artifact, signs, registers, and verifies in one call. Returns provider_id, public_url, offer_hash, marketplace_offer_url, and invoke_command.",
      "Hosting choices: 'tor' (default, public via auto hidden service — requires daemon FROGLET_NETWORK_MODE=tor), 'local' (private dev, no marketplace registration), 'self' (user-supplied URL via hosting.url). Managed + Fly land in Phase 1B.",
      "Settlement supports 'none' (free), 'lightning' (hold-invoice escrow paid to the node's Lightning wallet; requires Lightning backend + currency='sat'), and 'stripe' (Stripe MPP / Shared Payment Tokens; requires Stripe backend + currency='usd'). Pass settlement.method and the matching price.currency for a paid service.",
      "For the first-run verification on a fresh node, call `publish_artifact` with `template: \"demo.add\"` to publish the canonical local demo without writing source. This is a sanity check; real services go through `marketplace_publish`.",
      "After `marketplace_publish` returns, call `list_local_services`/`get_local_service` to confirm the offer fields and `invoke_service` (using the returned invoke_command) to verify end-to-end."
    ]
  }
  if (profile === "evidence") {
    return [
      ...common,
      "Select one user-owned URL, file hash, or artifact hash; do not use arbitrary third-party targets without authorization.",
      "Use `discover_services`/`get_service` or a local service to choose witness, hash-verify, or notarize behavior.",
      "Call `invoke_service`, then report status, result hash, receipt presence, and feed/artifact evidence."
    ]
  }
  if (profile === "payments") {
    return [
      ...common,
      "Call `get_wallet_balance` and `list_settlement_activity` before a paid run.",
      "Start with Lightning mock unless the user explicitly supplied Stripe test, x402, or LND REST credentials.",
      "After execution, call `get_payment_intent` or `get_invoice_bundle` when a deal id is available, then report settlement state."
    ]
  }
  if (profile === "batch") {
    return [
      ...common,
      "Use `run_compute` or `invoke_service` for the smallest single long-running job first.",
      "If the response returns a task_id or non-terminal task, call `get_task`/`wait_task` for progress and completion.",
      "Do not claim multi-item batch fan-out, retries, or paid async settlement until Order 44 lands and is verified."
    ]
  }
  if (profile === "gpu") {
    return [
      ...common,
      "Confirm the provider advertises GPU capability in its descriptor/service metadata before routing GPU work.",
      "If no provider advertises GPU, report GPU unavailable rather than falling back silently.",
      "A single-node Docker GPU path has been verified on GCP T4; full provider scheduling, quota, marketplace routing, and fallback semantics remain Order 45 work."
    ]
  }
  return [
    ...common,
    "Call `list_local_services` or `discover_services` depending on whether the user wants local or marketplace-backed work.",
    "Call `get_local_service`/`get_service` for the chosen service before invoking it.",
    "Call `invoke_service`, then report status, result, receipt evidence, and what remains unproven."
  ]
}

function useCaseBoundaries(profile) {
  const boundaries = [
    "The no-install hosted proof is not an MCP action; use https://froglet.dev/llms.txt for that.",
    "Do not enter payment secrets, expose clearnet/Tor endpoints, or run install commands without user approval."
  ]
  if (profile === "batch") {
    boundaries.push("Current actionable surface can observe async task progress; true batch submission and fan-out is not yet implemented.")
  }
  if (profile === "gpu") {
    boundaries.push("Current actionable surface can plan GPU work and require advertised capability; one self-hosted GCP T4 Docker workload has been verified, but provider scheduling and production capacity are not yet proved.")
  }
  return boundaries
}

async function handlePlanUseCase(args, _config, includeRaw) {
  const profile = inferUseCaseProfile(args)
  const payload = {
    workload_profile: profile,
    use_case: optionalString(args, "use_case") ?? null,
    readiness_checks: [
      "local Froglet provider/runtime reachable",
      "MCP token paths configured",
      "service metadata inspected before invocation",
      ...(profile === "gpu" ? ["provider advertises GPU capability", "real GPU host or cloud quota available"] : []),
      ...(profile === "batch" ? ["single async task path verified before multi-item orchestration"] : [])
    ],
    next_actions: useCaseSteps(profile),
    boundaries: useCaseBoundaries(profile)
  }
  const lines = [
    `workload_profile: ${profile}`,
    `use_case: ${payload.use_case ?? "not specified"}`,
    "",
    "Readiness checks:",
    ...payload.readiness_checks.map((check) => `  - ${check}`),
    "",
    "Next actions:",
    ...payload.next_actions.map((step, index) => `  ${index + 1}. ${step}`),
    "",
    "Boundaries:",
    ...payload.boundaries.map((boundary) => `  - ${boundary}`)
  ]
  return renderResult(lines, payload, includeRaw)
}

async function handleInstallGuide(args, _config, includeRaw) {
  // Surface guidance for an LLM whose user has just asked to install Froglet
  // locally. The LLM is expected to execute the returned commands through
  // its own host shell (Claude Code's Bash, Codex's shell, etc.) — NOT
  // through the Froglet runtime, which has no way to touch the user's host
  // filesystem or docker socket.
  const profile = normalizeInstallProfile(args)
  if (!profile.paymentRail) {
    const payload = {
      status: "decision_required",
      decision: "payment_rail",
      options: ["none", "lightning-mock", "lightning-lnd-rest", "stripe-test", "stripe-live", "x402"],
      recommended_for_first_install: "none",
      reason:
        "Froglet setup no longer assumes a payment rail. Pick none for the first demo service; choose a paid rail only when the user has the required wallet or Stripe inputs."
    }
    return renderResult([
      "status: decision_required",
      "decision: payment_rail",
      "options: none, lightning-mock, lightning-lnd-rest, stripe-test, stripe-live, x402",
      "recommended_for_first_install: none",
      `reason: ${payload.reason}`
    ], payload, includeRaw)
  }
  const steps = renderInstallBlock(profile)
  const payload = {
    target_agent: profile.targetAgent,
    payment_rail: profile.paymentRail,
    lightning_mode: profile.lightningMode,
    footprint: profile.footprint,
    role: profile.role,
    network_mode: profile.networkMode,
    marketplace_url: profile.marketplaceUrl,
    use_case: profile.useCase,
    steps,
    run_as: "user-host-shell",
    required_inputs: requiredInstallInputs(profile),
    validation_checks: validationChecks(profile),
    post_install_playbooks: postInstallPlaybooks(),
    notes: [
      "Run these commands on the user's machine, via your host agent's shell execution (e.g. Claude Code's Bash tool). Do NOT route them through the Froglet runtime — Froglet cannot install itself on the user's host.",
      profile.footprint === "source"
        ? "Source footprint clones the public repo and builds froglet-node with cargo instead of downloading the signed binary."
        : profile.footprint === "docker"
          ? "The agent bootstrap path downloads the signed froglet-node binary, writes ~/.froglet/agent/compose.yaml, and starts provider/runtime from published GHCR images without cloning the repo."
          : "The installer command downloads and installs the signed froglet-node binary to ~/.local/bin.",
      profile.footprint === "binary"
        ? "Binary footprint stops after froglet-node is installed; no repo-local helper scripts or Docker stack are started."
        : profile.footprint === "source"
          ? "The repo clone is contributor/developer mode because helper scripts, local Compose, and the OpenClaw plugin live there."
          : "Docker footprint is the no-clone user path. Agent configuration uses the published froglet-mcp image.",
      profile.footprint !== "docker" && LOCAL_MCP_AGENT_TARGETS.has(profile.targetAgent)
        ? "The npm step installs the local MCP server dependencies used by the generated Claude Code/Codex config."
        : "This profile does not require installing the local MCP server dependencies before setup-agent.",
      profile.footprint !== "docker" && SETUP_AGENT_TARGETS.has(profile.targetAgent)
        ? `The agent setup step writes the ${profile.targetAgent} config so the agent can talk to local Froglet.`
        : profile.footprint === "docker" && SETUP_AGENT_TARGETS.has(profile.targetAgent)
          ? `The bootstrap writes a ${profile.targetAgent} MCP config backed by the published froglet-mcp Docker image.`
        : "Manual target selected: do not run setup-agent; show the MCP config docs instead.",
      profile.footprint === "docker"
        ? "Bootstrap configures the first local stack with FROGLET_PAYMENT_BACKEND=none. Configure real payments later through the MCP-guided payment flow after local health checks pass."
        : `The payment step generates the ${profile.paymentRail} env snippet under froglet/.froglet/payment/.`,
      profile.footprint === "docker"
        ? "The bootstrap starts provider+runtime from published images and writes ~/.froglet/agent/compose.yaml for inspection and restart."
        : "The final step starts the selected footprint; binary-only installs stop after the signed binary is present.",
      profile.footprint === "source"
        ? "The repo-local steps intentionally start with `cd froglet &&` so they still work when your host shell asks for separate approvals per command."
        : "No repo-local step is required for the default Docker footprint.",
      profile.footprint === "docker"
        ? "After the final step, the local stack listens on 127.0.0.1:8080 (provider) and 127.0.0.1:8081 (runtime); the agent config points there."
        : "For binary/source installs, confirm the actual node role and listen addresses before expecting MCP health checks to pass.",
      `Network mode: ${profile.networkMode}. Keep loopback first; expose clearnet or Tor only after local health checks pass.`,
      `Role intent: ${profile.role}. Docker compose starts provider and runtime together; split roles are a direct froglet-node concern.`,
      `${
        profile.paymentRail === "stripe-test"
          ? "Stripe: replace <stripe-test-secret-key> with your own Stripe test secret key before running the payment step."
          : profile.paymentRail === "stripe-live"
            ? "Stripe live: replace <stripe-live-secret-key> only after a fresh operator approval; run the tiny live payment/refund proof before claiming live fiat."
          : profile.paymentRail === "x402"
            ? "x402: replace <base-wallet-address> with your own Base wallet address before running the payment step."
            : profile.paymentRail === "lightning-lnd-rest"
              ? "Lightning LND REST: provide your own REST URL, macaroon path, and TLS cert path when needed before running the payment step."
              : profile.paymentRail === "lightning-mock"
                ? "Lightning: mock mode needs no wallet credentials; use lightning_mode=lnd_rest only when the user already has LND REST credentials."
                : "No payment rail: the local node runs without payment credentials."
      }`
    ]
  }

  const lines = [
    `target_agent: ${profile.targetAgent}`,
    `payment_rail: ${profile.paymentRail}`,
    `lightning_mode: ${profile.lightningMode}`,
    `footprint: ${profile.footprint}`,
    `role: ${profile.role}`,
    `network_mode: ${profile.networkMode}`,
    `marketplace_url: ${profile.marketplaceUrl ?? "none"}`,
    `use_case: ${profile.useCase ?? "not specified"}`,
    `run_as: ${payload.run_as}`,
    "",
    "Commands to execute on the user's host (one per line):",
    ...steps.map((step, index) => `  ${index + 1}. ${step}`),
    "",
    "Required inputs:",
    ...payload.required_inputs.map((input) => `  - ${input}`),
    "",
    "Validation checks:",
    ...payload.validation_checks.map((check) => `  - ${check}`),
    "",
    "Post-install playbooks:",
    ...payload.post_install_playbooks.map((playbook) => `  - ${playbook}`),
    "",
    "Notes:",
    ...payload.notes.map((note) => `  - ${note}`)
  ]
  return renderResult(lines, payload, includeRaw)
}

async function handlePlanInstall(args, _config, includeRaw) {
  const profile = normalizeInstallProfile(args)
  if (!profile.paymentRail) {
    const payload = {
      status: "decision_required",
      decision: "payment_rail",
      options: ["none", "lightning-mock", "lightning-lnd-rest", "stripe-test", "stripe-live", "x402"],
      recommended_for_first_install: "none",
      prerequisites: installPrerequisites(profile),
      questions_to_ask_before_running_commands: installQuestions(args),
      safety:
        "Ask before running install scripts, starting Docker, entering secrets, or exposing clearnet/Tor endpoints. Do not transmit secrets into hosted services."
    }
    return renderResult([
      "status: decision_required",
      "decision: payment_rail",
      "options: none, lightning-mock, lightning-lnd-rest, stripe-test, stripe-live, x402",
      "recommended_for_first_install: none",
      "",
      "Questions to ask before running commands:",
      ...payload.questions_to_ask_before_running_commands.map((question) => `  - ${question}`),
      "",
      "Prerequisites:",
      ...payload.prerequisites.map((item) => `  - ${item}`),
      "",
      `Safety: ${payload.safety}`
    ], payload, includeRaw)
  }
  const steps = renderInstallBlock(profile)
  const payload = {
    install_profile: {
      target_agent: profile.targetAgent,
      footprint: profile.footprint,
      role: profile.role,
      payment_rail: profile.paymentRail,
      lightning_mode: profile.lightningMode,
      network_mode: profile.networkMode,
      marketplace_url: profile.marketplaceUrl ?? null,
      use_case: profile.useCase ?? null
    },
    questions_to_ask_before_running_commands: installQuestions(args),
    prerequisites: installPrerequisites(profile),
    required_inputs: requiredInstallInputs(profile),
    commands_preview: steps,
    validation_checks: validationChecks(profile),
    post_install_playbooks: postInstallPlaybooks(),
    safety:
      "Ask before running install scripts, starting Docker, entering secrets, or exposing clearnet/Tor endpoints. Do not transmit secrets into hosted services."
  }

  const lines = [
    "install_profile:",
    `  target_agent: ${profile.targetAgent}`,
    `  footprint: ${profile.footprint}`,
    `  role: ${profile.role}`,
    `  payment_rail: ${profile.paymentRail}`,
    `  lightning_mode: ${profile.lightningMode}`,
    `  network_mode: ${profile.networkMode}`,
    `  marketplace_url: ${profile.marketplaceUrl ?? "none"}`,
    `  use_case: ${profile.useCase ?? "not specified"}`,
    "",
    "Questions to ask before running commands:",
    ...(payload.questions_to_ask_before_running_commands.length > 0
      ? payload.questions_to_ask_before_running_commands.map((question) => `  - ${question}`)
      : ["  - None; this profile is fully specified."]),
    "",
    "Prerequisites:",
    ...payload.prerequisites.map((item) => `  - ${item}`),
    "",
    "Required inputs:",
    ...payload.required_inputs.map((item) => `  - ${item}`),
    "",
    "Commands preview:",
    ...steps.map((step, index) => `  ${index + 1}. ${step}`),
    "",
    "Validation checks:",
    ...payload.validation_checks.map((check) => `  - ${check}`),
    "",
    "Post-install playbooks:",
    ...payload.post_install_playbooks.map((playbook) => `  - ${playbook}`),
    "",
    `Safety: ${payload.safety}`
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
    _deps: clientDeps(config),
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

async function handleMarketplaceRegister(args, config, includeRaw) {
  const response = await registerProviderOnMarketplace({
    marketplaceUrl: firstDefined(args.marketplace_url, config.marketplaceUrl),
    providerUrl: config.providerUrl,
    requestTimeoutMs: config.requestTimeoutMs,
    request: {
      provider_url: resolvedProviderUrl(args),
      registration_transport: args.registration_transport
    },
    _deps: marketplaceDeps(config)
  })
  const lines = [
    `status: ${response.status ?? "unknown"}`,
    `provider_id: ${response.provider_id ?? "unknown"}`,
    `provider_url: ${response.provider_url ?? "unknown"}`,
    `transport: ${response.transport ?? args.registration_transport ?? "clearnet"}`,
    `descriptor_hash: ${response.descriptor_hash ?? "unknown"}`,
    `offers_seen: ${response.offers_seen ?? 0}`,
    `already_registered: ${response.already_registered === true}`
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleMarketplaceDomainClaim(args, config, includeRaw) {
  const providerId = resolvedProviderId(args)
  if (typeof providerId !== "string" || providerId.trim().length === 0) {
    throw new Error("provider_id is required for marketplace_domain_claim")
  }
  if (typeof args.public_ip !== "string" || args.public_ip.trim().length === 0) {
    throw new Error("public_ip is required for marketplace_domain_claim")
  }
  const response = await createProviderDomainClaim({
    marketplaceUrl: firstDefined(args.marketplace_url, config.marketplaceUrl),
    ...providerCtx(config),
    requestTimeoutMs: config.requestTimeoutMs,
    request: {
      provider_id: providerId.trim(),
      public_ip: args.public_ip.trim(),
      ...(typeof args.requested_slug === "string" && args.requested_slug.trim().length > 0
        ? { requested_slug: args.requested_slug.trim() }
        : {})
    },
    _deps: marketplaceDeps(config)
  })
  const lines = [
    `status: ${response.status ?? "unknown"}`,
    `claim_id: ${response.claim_id ?? "unknown"}`,
    `provider_id: ${response.provider_id ?? providerId}`,
    `hostname: ${response.hostname ?? "unknown"}`,
    `public_ip: ${response.public_ip ?? args.public_ip}`,
    `expires_at: ${response.expires_at ?? "unknown"}`,
    "next: call marketplace_domain_complete with claim_id and signing_message"
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleMarketplaceDomainComplete(args, config, includeRaw) {
  const claimId = typeof args.claim_id === "string" ? args.claim_id.trim() : ""
  if (claimId.length === 0) {
    throw new Error("claim_id is required for marketplace_domain_complete")
  }
  const signingMessage =
    typeof args.signing_message === "string" ? args.signing_message.trim() : ""
  if (signingMessage.length === 0) {
    throw new Error("signing_message is required for marketplace_domain_complete")
  }
  const response = await completeProviderDomainClaim({
    marketplaceUrl: firstDefined(args.marketplace_url, config.marketplaceUrl),
    ...providerCtx(config),
    claimId,
    signingMessage,
    _deps: marketplaceDeps(config)
  })
  const lines = [
    `status: ${response.status ?? "unknown"}`,
    `provider_id: ${response.provider_id ?? response.signed_provider_id ?? "unknown"}`,
    `hostname: ${response.hostname ?? "unknown"}`,
    `public_ip: ${response.public_ip ?? "unknown"}`,
    `dns_record_id: ${response.dns_record_id ?? "none"}`,
    `dns_required: ${response.dns_required === true}`
  ]
  return renderResult(lines, response, includeRaw)
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
  throw new Error(
    "marketplace_stake is not available: marketplace-node no longer publishes marketplace.stake; use marketplace_provider for current provider details."
  )
}

async function handleMarketplaceTopup(args, config, includeRaw) {
  throw new Error(
    "marketplace_topup is not available: marketplace-node no longer publishes marketplace.topup; use marketplace_provider for current provider details."
  )
}

async function handleMarketplaceFileComplaint(args, config, includeRaw) {
  const providerId = typeof args.marketplace_provider_id === "string"
    ? args.marketplace_provider_id.trim()
    : ""
  if (providerId.length === 0) {
    throw new Error("marketplace_provider_id is required for marketplace_file_complaint")
  }
  const dealId = typeof args.deal_id === "string" ? args.deal_id.trim() : ""
  if (dealId.length === 0) {
    throw new Error("deal_id is required for marketplace_file_complaint")
  }
  const reason = typeof args.reason === "string" ? args.reason.trim() : ""
  if (reason.length === 0) {
    throw new Error("reason is required for marketplace_file_complaint")
  }
  const response = await fileMarketplaceComplaint({
    arbiterUrl: firstDefined(args.marketplace_arbiter_url, config.marketplaceArbiterUrl),
    requestTimeoutMs: config.requestTimeoutMs,
    request: {
      provider_id: providerId,
      deal_id: dealId,
      reason,
      ...(typeof args.receipt_hash === "string" && args.receipt_hash.trim().length > 0
        ? { receipt_hash: args.receipt_hash.trim() }
        : {}),
      ...(typeof args.complainant_id === "string" && args.complainant_id.trim().length > 0
        ? { complainant_id: args.complainant_id.trim() }
        : {}),
      ...(args.evidence !== undefined ? { evidence: args.evidence } : {})
    },
    _deps: marketplaceDeps(config)
  })
  const lines = [
    `complaint_id: ${response.complaint_id ?? "unknown"}`,
    `status: ${response.status ?? "unknown"}`,
    `provider_id: ${response.provider_id ?? providerId}`,
    `deal_id: ${response.deal_id ?? dealId}`,
    `created_at: ${response.created_at ?? "unknown"}`
  ]
  return renderResult(lines, response, includeRaw)
}

async function handleMarketplaceGetComplaint(args, config, includeRaw) {
  const complaintId = typeof args.complaint_id === "string" ? args.complaint_id.trim() : ""
  if (complaintId.length === 0) {
    throw new Error("complaint_id is required for marketplace_get_complaint")
  }
  const response = await getMarketplaceComplaint({
    arbiterUrl: firstDefined(args.marketplace_arbiter_url, config.marketplaceArbiterUrl),
    requestTimeoutMs: config.requestTimeoutMs,
    complaintId,
    _deps: marketplaceDeps(config)
  })
  const complaint = response.complaint ?? {}
  const verdicts = Array.isArray(response.verdicts) ? response.verdicts : []
  const lines = [
    `complaint_id: ${complaint.complaint_id ?? complaintId}`,
    `status: ${complaint.status ?? "unknown"}`,
    `provider_id: ${complaint.provider_id ?? "unknown"}`,
    `deal_id: ${complaint.deal_id ?? "unknown"}`,
    `verdicts: ${verdicts.length}`
  ]
  if (verdicts.length > 0) {
    const latest = verdicts[0]
    lines.push(`latest_verdict: ${latest.verdict ?? "unknown"}`)
    lines.push(`latest_remedy: ${latest.remedy ?? "unknown"}`)
  }
  return renderResult(lines, response, includeRaw)
}

async function handleMarketplacePublish(args, config, includeRaw) {
  // Delegates to `froglet-node publish --json` via the publish engine. The
  // engine handles the full build → host → sign → register pipeline; the
  // MCP tool's job is just to materialise the manifests + handler.py from
  // structured input and parse the JSON the CLI emits.
  const response = await runMarketplacePublish(args, {
    _deps: marketplacePublishDeps(config),
  })
  const lines = [
    `status: ${response?.warnings?.length ? "published with warnings" : "published"}`,
    `provider_id: ${response.provider_id}`,
    `public_url: ${response.public_url}`,
    `offer_hash: ${response.offer_hash}`
  ]
  if (response.marketplace_offer_url) {
    lines.push(`marketplace_offer_url: ${response.marketplace_offer_url}`)
  }
  if (response.status_url) {
    lines.push(`status_url: ${response.status_url}`)
  }
  if (response.invoke_command) {
    lines.push(`invoke: ${response.invoke_command}`)
  }
  if (Array.isArray(response.warnings) && response.warnings.length > 0) {
    lines.push(`warnings: ${response.warnings.length}`)
    for (const w of response.warnings) {
      lines.push(`  - ${JSON.stringify(w)}`)
    }
  }
  return renderResult(lines, response, includeRaw)
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
    case "plan_install":
      return handlePlanInstall(args, config, includeRaw)
    case "get_install_guide":
      return handleInstallGuide(args, config, includeRaw)
    case "plan_use_case":
      return handlePlanUseCase(args, config, includeRaw)
    case "marketplace_search":
      return handleMarketplaceSearch(args, config, includeRaw)
    case "marketplace_register":
      return handleMarketplaceRegister(args, config, includeRaw)
    case "marketplace_domain_claim":
      return handleMarketplaceDomainClaim(args, config, includeRaw)
    case "marketplace_domain_complete":
      return handleMarketplaceDomainComplete(args, config, includeRaw)
    case "marketplace_provider":
      return handleMarketplaceProvider(args, config, includeRaw)
    case "marketplace_receipts":
      return handleMarketplaceReceipts(args, config, includeRaw)
    case "marketplace_stake":
      return handleMarketplaceStake(args, config, includeRaw)
    case "marketplace_topup":
      return handleMarketplaceTopup(args, config, includeRaw)
    case "marketplace_file_complaint":
      return handleMarketplaceFileComplaint(args, config, includeRaw)
    case "marketplace_get_complaint":
      return handleMarketplaceGetComplaint(args, config, includeRaw)
    case "marketplace_publish":
      return handleMarketplacePublish(args, config, includeRaw)
    // Removed actions — return clear error messages
    case "run_hosted_proof":
      throw new Error("run_hosted_proof is not part of the installed MCP surface. Use https://froglet.dev/llms.txt for the no-install hosted proof, or use local MCP actions against a configured Froglet node.")
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
