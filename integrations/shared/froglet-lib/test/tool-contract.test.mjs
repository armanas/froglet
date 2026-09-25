import assert from "node:assert/strict"
import test from "node:test"

import { frogletToolInputSchema } from "../../../mcp/froglet/lib/tools.js"
import { frogletToolParameters } from "../../../openclaw/froglet/lib/froglet-tool.js"

const config = { maxSearchLimit: 50 }

test("MCP and OpenClaw expose the same canonical action and payment contracts", () => {
  const mcp = frogletToolInputSchema(config).properties
  const openclaw = frogletToolParameters(config).properties

  assert.deepEqual(openclaw.action.enum, mcp.action.enum)
  assert.deepEqual(openclaw.payment_rail.enum, mcp.payment_rail.enum)
  assert.deepEqual(openclaw.lightning_mode.enum, mcp.lightning_mode.enum)
  for (const name of ["attested", "attestation_kind", "attestation_dns_zone"]) {
    assert.deepEqual(openclaw[name], mcp[name])
  }
})
