import assert from "node:assert/strict"
import test from "node:test"

import {
  assertAgentTranscript,
  assertContainsAll,
  assertContainsInOrder,
  extractJsonSection,
  normalizeLines
} from "./matrix-assertions.mjs"

test("extractJsonSection parses labeled JSON payloads", () => {
  const payload = extractJsonSection(
    "wallet_balance_response_json:\n{\"backend\":\"lightning\",\"balance_sats\":21}\n",
    "wallet_balance_response_json:"
  )

  assert.equal(payload.backend, "lightning")
  assert.equal(payload.balance_sats, 21)
})

test("assertAgentTranscript validates ordered runtime summaries", () => {
  const transcript = [
    "runtime_url: http://127.0.0.1:8081",
    "returned_nodes: 1",
    "1.",
    "node_id: provider-1",
    "status: active",
    "provider_id: provider-1",
    "offer_id=execute.compute",
    "terminal: false"
  ].join("\n")

  const result = assertAgentTranscript(transcript, {
    mustContain: ["runtime_url: http://127.0.0.1:8081", "provider_id: provider-1"],
    mustContainOrdered: ["returned_nodes: 1", "node_id: provider-1", "offer_id=execute.compute"],
    mustNotContain: ["unexpected failure"]
  })

  assert.equal(result.lines[0], "runtime_url: http://127.0.0.1:8081")
})

test("assertContains helpers fail on missing content", () => {
  assert.doesNotThrow(() =>
    assertContainsAll("alpha beta gamma", ["alpha", "gamma"], "content check")
  )
  assert.doesNotThrow(() =>
    assertContainsInOrder("alpha beta gamma", ["alpha", "beta", "gamma"], "order check")
  )
  assert.deepEqual(normalizeLines("a\nb\r\nc"), ["a", "b", "c"])
})
