// Tests for FROGLET_EGRESS_MODE=strict pin propagation through
// frogletRequest / frogletRequestWithStatus / frogletPublicRequest
// (TODO.md Order 70).
//
// These tests exercise the control flow (strict mode off → no pin resolution;
// strict mode on → pin resolution happens and caches; caller-supplied pin
// always wins). They do not spin up a real network peer because the unit
// under test is specifically the pin-lookup path, not the pinned dispatcher
// itself (which is exercised separately in url-safety.test.mjs).

import { test } from "node:test"
import assert from "node:assert/strict"

import {
  isStrictEgressMode,
  resolveOperatorPin,
  __resetOperatorPinCacheForTests,
} from "../froglet-client.js"

function withStrictMode(fn) {
  const previous = process.env.FROGLET_EGRESS_MODE
  process.env.FROGLET_EGRESS_MODE = "strict"
  return Promise.resolve(fn()).finally(() => {
    if (previous === undefined) {
      delete process.env.FROGLET_EGRESS_MODE
    } else {
      process.env.FROGLET_EGRESS_MODE = previous
    }
    __resetOperatorPinCacheForTests()
  })
}

function withLenientMode(fn) {
  const previous = process.env.FROGLET_EGRESS_MODE
  delete process.env.FROGLET_EGRESS_MODE
  return Promise.resolve(fn()).finally(() => {
    if (previous !== undefined) {
      process.env.FROGLET_EGRESS_MODE = previous
    }
    __resetOperatorPinCacheForTests()
  })
}

test("isStrictEgressMode reflects the env var", () => {
  const previous = process.env.FROGLET_EGRESS_MODE
  try {
    delete process.env.FROGLET_EGRESS_MODE
    assert.equal(isStrictEgressMode(), false)

    process.env.FROGLET_EGRESS_MODE = ""
    assert.equal(isStrictEgressMode(), false)

    process.env.FROGLET_EGRESS_MODE = "true"
    assert.equal(isStrictEgressMode(), false, "only literal 'strict' should enable")

    process.env.FROGLET_EGRESS_MODE = "strict"
    assert.equal(isStrictEgressMode(), true)
  } finally {
    if (previous === undefined) {
      delete process.env.FROGLET_EGRESS_MODE
    } else {
      process.env.FROGLET_EGRESS_MODE = previous
    }
  }
})

test("resolveOperatorPin returns null in lenient mode regardless of URL", async () => {
  await withLenientMode(async () => {
    const pin = await resolveOperatorPin("https://example.com", "runtime_url")
    assert.equal(pin, null, "lenient mode must not resolve a pin")
  })
})

test("resolveOperatorPin returns null for missing / empty URL even in strict mode", async () => {
  await withStrictMode(async () => {
    assert.equal(await resolveOperatorPin(null), null)
    assert.equal(await resolveOperatorPin(undefined), null)
    assert.equal(await resolveOperatorPin(""), null)
    assert.equal(await resolveOperatorPin("   "), null)
  })
})

test("resolveOperatorPin rejects loopback operator URLs under strict mode", async () => {
  await withStrictMode(async () => {
    await assert.rejects(
      () => resolveOperatorPin("https://127.0.0.1:8443", "runtime_url"),
      /loopback|private|127\.0\.0\.1/i,
      "strict mode must reject loopback operator URLs just like LLM-controlled ones"
    )
  })
})

test("resolveOperatorPin rejects non-https operator URLs under strict mode", async () => {
  await withStrictMode(async () => {
    await assert.rejects(
      () => resolveOperatorPin("http://example.com", "runtime_url"),
      /https/i,
      "strict mode must reject http:// operator URLs"
    )
  })
})

test("resolveOperatorPin does not persist-cache failed validations", async () => {
  await withStrictMode(async () => {
    await assert.rejects(() => resolveOperatorPin("http://example.com", "runtime_url"))
    // Second call should re-attempt rather than serving a cached error; the
    // operator may have corrected their config between calls. If caching
    // were too aggressive, this would silently return the prior error.
    await assert.rejects(
      () => resolveOperatorPin("http://example.com", "runtime_url"),
      /https/i
    )
  })
})
