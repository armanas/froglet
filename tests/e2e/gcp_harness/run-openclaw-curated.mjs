import { readFileSync } from "node:fs"

import {
  defaultModel,
  loadFrogletTool,
  parseCliArgs,
  readJson,
  requireApiKey,
  writeJson,
} from "./common.mjs"
import { runCuratedSuite } from "./openclaw-llm-runner.mjs"

async function main() {
  const { values } = parseCliArgs({
    inventory: { type: "string", short: "i" },
    scenarios: { type: "string", short: "s" },
    "provider-url": { type: "string" },
    "runtime-url": { type: "string" },
    "provider-auth-token-path": { type: "string" },
    "runtime-auth-token-path": { type: "string" },
    "base-url": { type: "string" },
    "auth-token-path": { type: "string" },
    out: { type: "string", short: "o" },
  })
  const providerUrl = values["provider-url"] ?? values["base-url"]
  const runtimeUrl = values["runtime-url"] ?? values["base-url"]
  const providerAuthTokenPath =
    values["provider-auth-token-path"] ?? values["auth-token-path"]
  const runtimeAuthTokenPath =
    values["runtime-auth-token-path"] ?? values["auth-token-path"]
  if (
    !values.inventory ||
    !values.scenarios ||
    !providerUrl ||
    !runtimeUrl ||
    !providerAuthTokenPath ||
    !runtimeAuthTokenPath ||
    !values.out
  ) {
    throw new Error(
      "--inventory, --scenarios, --out, and either split provider/runtime URLs + token paths or legacy --base-url/--auth-token-path are required"
    )
  }

  requireApiKey()
  const [inventory, scenarioSet] = await Promise.all([
    readJson(values.inventory),
    readJson(values.scenarios),
  ])

  const tool = loadFrogletTool({
    providerUrl,
    runtimeUrl,
    providerAuthTokenPath,
    runtimeAuthTokenPath,
    requestTimeoutMs: 20_000,
  })
  const fixtures = {
    validWasmHex: readFileSync(
      new URL("../../../integrations/openclaw/froglet/test/fixtures/valid-wasm.hex", import.meta.url),
      "utf8"
    ).trim(),
  }

  const curated = await runCuratedSuite(
    tool,
    scenarioSet.openclaw?.curated ?? [],
    fixtures,
    { run_id: inventory.run_id }
  )

  await writeJson(values.out, {
    generated_at: new Date().toISOString(),
    model: defaultModel,
    run_id: inventory.run_id,
    ...curated,
  })
  if (curated.failed > 0) {
    process.exitCode = 1
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
