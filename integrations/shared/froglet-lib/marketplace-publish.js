// marketplace_publish MCP action.
//
// Phase 3 of the agent-grade publish plan. Wraps the `froglet-node publish`
// CLI subcommand (Phase 2) so an LLM can take a service from a one-sentence
// user intent to a live marketplace offer in one MCP call.
//
// Architectural rule: this handler delegates to the SAME `froglet-node publish`
// surface a human would type, by writing manifests to a temp directory and
// shelling out. That way the MCP path can never diverge from the CLI path —
// one bug, one fix. The shelling-out is intentional, not a shortcut.

import { execFile } from "node:child_process"
import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { promisify } from "node:util"

const execFileAsync = promisify(execFile)

const PUBLISH_TIMEOUT_MS = 5 * 60 * 1000 // 5 min covers indexer-wait worst case
const VALID_NAME = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/
const VALID_RUNTIMES = new Set(["python"])
const VALID_PACKAGE_KINDS = new Set(["inline_source"])
const VALID_HOSTING = new Set(["local", "tor", "self"])
const VALID_SETTLEMENT = new Set(["none", "lightning", "stripe"])

/**
 * Validate the MCP-shaped publish input. Throws on the first problem with a
 * message the LLM can act on directly ("set X to Y") rather than a generic
 * "invalid input".
 */
export function validatePublishInput(args) {
  const name = (args.name ?? "").trim()
  if (!VALID_NAME.test(name)) {
    throw new Error(
      `marketplace_publish: name ${JSON.stringify(args.name)} is invalid; ` +
        "must be 1-63 lowercase ASCII letters, digits, or interior hyphens"
    )
  }

  const runtime = args.runtime ?? "python"
  if (!VALID_RUNTIMES.has(runtime)) {
    throw new Error(
      `marketplace_publish: runtime ${JSON.stringify(runtime)} not supported in v1A; ` +
        "use 'python' (WASM + OCI runtimes land in Phase 1B)"
    )
  }

  const packageKind = args.package_kind ?? "inline_source"
  if (!VALID_PACKAGE_KINDS.has(packageKind)) {
    throw new Error(
      `marketplace_publish: package_kind ${JSON.stringify(packageKind)} not supported in v1A; ` +
        "use 'inline_source'"
    )
  }

  if (typeof args.source_inline !== "string" || args.source_inline.length === 0) {
    throw new Error(
      "marketplace_publish: source_inline is required (string of Python source); " +
        "WASM + OCI source forms are Phase 1B"
    )
  }

  const hostingKind = args.hosting?.kind ?? "tor"
  if (!VALID_HOSTING.has(hostingKind)) {
    throw new Error(
      `marketplace_publish: hosting.kind ${JSON.stringify(hostingKind)} not supported in v1A; ` +
        "use 'local' | 'tor' | 'self' (Managed + Fly land in Phase 1B)"
    )
  }
  if (hostingKind === "self" && typeof args.hosting?.url !== "string") {
    throw new Error(
      "marketplace_publish: hosting.url is required when hosting.kind = 'self'"
    )
  }

  const settlementMethod = args.settlement?.method ?? "none"
  if (!VALID_SETTLEMENT.has(settlementMethod)) {
    throw new Error(
      `marketplace_publish: settlement.method ${JSON.stringify(settlementMethod)} not supported; ` +
        "use 'none' (free), 'lightning' (paid via Lightning), or 'stripe' (paid via Stripe MPP)"
    )
  }

  const priceSats = Number.isInteger(args.price_sats) ? args.price_sats : 0
  if (priceSats < 0) {
    throw new Error("marketplace_publish: price_sats must be a non-negative integer")
  }
  // The manifest validator requires currency="usd" for stripe settlement and
  // "sat"/absent for lightning/none. Stripe "sats" are therefore USD cents.
  const currency = settlementMethod === "stripe" ? "usd" : "sat"

  return {
    name,
    summary: typeof args.summary === "string" ? args.summary : `Froglet service ${name}`,
    runtime,
    packageKind,
    entrypoint: args.entrypoint ?? "handler.py",
    sourceInline: args.source_inline,
    hosting: {
      kind: hostingKind,
      url: args.hosting?.url,
    },
    settlement: { method: settlementMethod },
    priceSats,
    currency,
    marketplaceUrl: typeof args.marketplace_url === "string"
      ? args.marketplace_url
      : "https://marketplace.froglet.dev",
  }
}

function projectToml({ name, marketplaceUrl }) {
  return [
    `schema_version = "froglet/v1"`,
    ``,
    `[project]`,
    `name = "${name}"`,
    ``,
    `[project.marketplace]`,
    `url = "${marketplaceUrl}"`,
    ``,
    `[project.defaults]`,
    `runtime = "python"`,
    `hosting = "tor"`,
    `settlement = "none"`,
    ``,
  ].join("\n")
}

export function serviceToml(input) {
  const { name, summary, runtime, packageKind, entrypoint, hosting, settlement, priceSats, currency, marketplaceUrl } = input
  const lines = [
    `schema_version = "froglet-service/v3"`,
    ``,
    `project_id = "${name}"`,
    `service_id = "${name}"`,
    `summary = ${JSON.stringify(summary)}`,
    ``,
    `runtime = "${runtime}"`,
    `package_kind = "${packageKind}"`,
    `entrypoint_kind = "script"`,
    `entrypoint = "${entrypoint}"`,
    `contract_version = "froglet.python.handler_json.v1"`,
    ``,
    `[hosting]`,
    `default = "${hosting.kind}"`,
  ]
  if (hosting.kind === "self") {
    lines.push(``, `[hosting.self]`, `url = "${hosting.url}"`)
  }
  lines.push(``, `[settlement]`, `method = "${settlement.method}"`)
  lines.push(``, `[marketplace]`, `url = "${marketplaceUrl}"`)
  lines.push(``, `[price]`, `sats = ${priceSats ?? 0}`, `currency = "${currency ?? "sat"}"`, ``)
  return lines.join("\n")
}

/**
 * Run `froglet-node publish --json` against a freshly-materialised service
 * directory and return the parsed JSON output. Cleans up the temp dir on
 * success and failure; the source code is preserved in the daemon's offer
 * artifact regardless.
 *
 * `frogletNodeBinary` defaults to "froglet-node" so the caller's PATH wins.
 * Override via the FROGLET_NODE_BIN env var for tests / non-PATH installs.
 */
export async function runMarketplacePublish(args, options = {}) {
  const input = validatePublishInput(args)
  const binary = options.frogletNodeBinary || process.env.FROGLET_NODE_BIN || "froglet-node"

  const workDir = await mkdtemp(join(tmpdir(), "froglet-publish-"))
  try {
    await writeFile(join(workDir, "froglet.toml"), projectToml(input))
    await writeFile(join(workDir, "froglet-service.toml"), serviceToml(input))
    await writeFile(join(workDir, input.entrypoint), input.sourceInline)

    const flags = ["publish", "--json"]
    if (input.hosting.kind !== "local") {
      // The default is taken from the manifest; only pass --host when the
      // MCP caller wants to override (we always set it explicitly here so
      // the behaviour is deterministic).
      flags.push("--host", input.hosting.kind)
    } else {
      flags.push("--host", "local")
    }
    flags.push("--marketplace", input.marketplaceUrl)

    const { stdout, stderr } = await execFileAsync(binary, flags, {
      cwd: workDir,
      timeout: PUBLISH_TIMEOUT_MS,
      maxBuffer: 4 * 1024 * 1024,
      env: process.env,
    })
    if (stderr && stderr.length > 0) {
      // The CLI uses stderr for human-readable warnings; surface them so the
      // LLM can mention them in its response without parsing the JSON.
      // eslint-disable-next-line no-console
      console.error(`marketplace_publish stderr: ${stderr.trim()}`)
    }
    return JSON.parse(stdout)
  } catch (error) {
    const exitCode = error?.code
    const stderr = error?.stderr ? String(error.stderr).trim() : ""
    const stdout = error?.stdout ? String(error.stdout).trim() : ""
    const detail = stderr || stdout || error?.message || "unknown"
    const wrapped = new Error(
      `marketplace_publish: froglet-node publish failed (exit=${exitCode}): ${detail}`
    )
    wrapped.cause = error
    throw wrapped
  } finally {
    await rm(workDir, { recursive: true, force: true }).catch(() => {})
  }
}
