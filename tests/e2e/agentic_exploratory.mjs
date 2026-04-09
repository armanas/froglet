import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

import {
  defaultModel,
  parseCliArgs,
  requireApiKey,
  writeJson,
} from "./gcp_harness/common.mjs"
import { runExploratorySession } from "./gcp_harness/openclaw-llm-runner.mjs"
import {
  bootstrapLocalFixtures,
  buildLocalExploratoryFixtures,
  buildLocalExploratoryScenarioSet,
  loadLocalFrogletTool,
  localResultsPath,
} from "../../integrations/openclaw/froglet/test/openai-responses-smoke.mjs"

const e2eDir = fileURLToPath(new URL("./", import.meta.url))
const repoRoot = path.resolve(e2eDir, "../..")

async function runLocalExploratory({ out } = {}) {
  requireApiKey()
  const tool = loadLocalFrogletTool()
  const fixtures = await bootstrapLocalFixtures(tool, {
    prefix: `oa-explore-${Date.now()}`,
  })
  const exploratory = await runExploratorySession(
    tool,
    buildLocalExploratoryScenarioSet(),
    buildLocalExploratoryFixtures(fixtures)
  )
  const outputPath = out ?? localResultsPath("openclaw-exploratory-local.json")
  await writeJson(outputPath, {
    generated_at: new Date().toISOString(),
    model: defaultModel,
    repo_root: repoRoot,
    fixtures,
    exploratory,
  })
  return { outputPath, exploratory }
}

async function main() {
  const { values } = parseCliArgs({
    out: { type: "string", short: "o" },
  })
  const { exploratory } = await runLocalExploratory({ out: values.out })
  const severe = exploratory.anomalies.filter((entry) =>
    ["critical", "high"].includes(String(entry.severity ?? "").toLowerCase())
  )
  if (severe.length > 0) {
    process.exitCode = 1
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error)
    process.exitCode = 1
  })
}
