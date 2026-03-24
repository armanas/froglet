import assert from "node:assert/strict"
import { test } from "node:test"

import {
  createPaymentIntentPhase,
  finalizePaymentPhase,
  normalizeSmokePhase
} from "../../../../_tmp/js/sandbox-smoke.mjs"

function jsonText(label, payload) {
  return `${label}\n${JSON.stringify(payload, null, 2)}`
}

function makeTools(handlers, calls) {
  return new Map(
    Object.entries(handlers).map(([name, handler]) => [
      name,
      {
        definition: {
          execute: async (toolInputLabel, args) => {
            calls.push({ name, toolInputLabel, args })
            return handler(toolInputLabel, args)
          }
        }
      }
    ])
  )
}

test("normalizeSmokePhase accepts the supported values", () => {
  assert.equal(normalizeSmokePhase(undefined), "full")
  assert.equal(normalizeSmokePhase("full"), "full")
  assert.equal(normalizeSmokePhase("create-payment"), "create-payment")
  assert.equal(normalizeSmokePhase("finalize-payment"), "finalize-payment")
})

test("normalizeSmokePhase rejects unsupported values", () => {
  assert.throws(() => normalizeSmokePhase("other"), /unsupported FROGLET_SMOKE_PHASE/)
})

test("createPaymentIntentPhase stops after payment intent and emits machine-readable data", async () => {
  const calls = []
  const tools = makeTools(
    {
      froglet_search: () =>
        ({ content: [{ text: jsonText("search_response_json:", { nodes: [
          {
            descriptor: {
              node_id: "provider-1",
              transports: { clearnet_url: "https://provider.example" }
            }
          }
        ] }) }] }),
      froglet_get_provider: () =>
        ({ content: [{ text: jsonText("provider_response_json:", {
          descriptor: { payload: { provider_id: "provider-1" } },
          offers: [{ payload: { offer_id: "execute.compute" } }]
        }) }] }),
      froglet_buy: () =>
        ({ content: [{ text: jsonText("buy_response_json:", {
          provider_url: "https://provider.example",
          deal: { deal_id: "deal-1", provider_id: "provider-1", status: "payment_pending" },
          payment_intent_path: "/v1/runtime/deals/deal-1/payment-intent"
        }) }] }),
      froglet_payment_intent: () =>
        ({ content: [{ text: jsonText("payment_intent_response_json:", {
          payment_intent: {
            deal_id: "deal-1",
            backend: "lightning",
            mock_action: { endpoint_path: "/v1/runtime/deals/deal-1/mock-pay" }
          }
        }) }] })
    },
    calls
  )

  const originalLog = console.log
  console.log = () => {}
  try {
    const result = await createPaymentIntentPhase({
      tools,
      moduleHex: "00",
      expectedProviderUrl: "https://provider.example",
      rowId: "row-1"
    })

    assert.equal(result.phase, "create-payment")
    assert.equal(result.deal.deal_id, "deal-1")
    assert.equal(result.payment_intent.deal_id, "deal-1")
    assert.deepEqual(
      calls.map((entry) => entry.name),
      ["froglet_search", "froglet_get_provider", "froglet_buy", "froglet_payment_intent"]
    )
  } finally {
    console.log = originalLog
  }
})

test("finalizePaymentPhase uses the existing deal and completes acceptance", async () => {
  const calls = []
  const tools = makeTools(
    {
      froglet_payment_intent: () =>
        ({ content: [{ text: jsonText("payment_intent_response_json:", {
          payment_intent: {
            deal_id: "deal-1",
            backend: "lightning",
            mock_action: { endpoint_path: "/v1/runtime/deals/deal-1/mock-pay" }
          }
        }) }] }),
      froglet_mock_pay: () =>
        ({ content: [{ text: jsonText("mock_pay_response_json:", {
          deal: { deal_id: "deal-1", status: "payment_pending" }
        }) }] }),
      froglet_wait_deal: () =>
        ({ content: [{ text: jsonText("wait_response_json:", {
          deal: { deal_id: "deal-1", status: "result_ready", result_hash: "hash-1" }
        }) }] }),
      froglet_accept_result: () =>
        ({ content: [{ text: jsonText("accept_response_json:", {
          deal: { deal_id: "deal-1", status: "succeeded" }
        }) }] })
    },
    calls
  )

  const originalLog = console.log
  console.log = () => {}
  try {
    const result = await finalizePaymentPhase({
      tools,
      dealId: "deal-1"
    })

    assert.equal(result.phase, "finalize-payment")
    assert.equal(result.deal.status, "succeeded")
    assert.deepEqual(
      calls.map((entry) => entry.name),
      ["froglet_payment_intent", "froglet_mock_pay", "froglet_wait_deal", "froglet_accept_result"]
    )
  } finally {
    console.log = originalLog
  }
})
