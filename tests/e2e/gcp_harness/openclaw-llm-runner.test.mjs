import assert from "node:assert/strict"
import test from "node:test"

import { applyCuratedFixtureArgs } from "./openclaw-llm-runner.mjs"

test("applyCuratedFixtureArgs fills missing fields for global fixtures", () => {
  const args = { action: "invoke_service", service_id: "svc-1" }
  const merged = applyCuratedFixtureArgs(args, {
    provider_id: "provider-1",
    input: { marker: "expected" },
  })

  assert.equal(merged.provider_id, "provider-1")
  assert.deepEqual(merged.input, { marker: "expected" })
  assert.equal(merged.service_id, "svc-1")
})

test("applyCuratedFixtureArgs preserves model-supplied values for global fixtures", () => {
  const args = {
    action: "invoke_service",
    service_id: "svc-1",
    input: { marker: "from-model" },
  }
  const merged = applyCuratedFixtureArgs(args, {
    provider_id: "provider-1",
    input: { marker: "from-fixture" },
  })

  assert.equal(merged.provider_id, "provider-1")
  assert.deepEqual(merged.input, { marker: "from-model" })
})

test("applyCuratedFixtureArgs only overrides targeted action fixtures", () => {
  const waitArgs = { action: "wait_task", task_id: "task-1" }
  const invokeArgs = { action: "invoke_service", service_id: "svc-1" }
  const fixtureArgs = {
    action: "invoke_service",
    provider_id: "provider-1",
    input: { marker: "expected" },
  }

  const untouched = applyCuratedFixtureArgs(waitArgs, fixtureArgs)
  const targeted = applyCuratedFixtureArgs(invokeArgs, fixtureArgs)

  assert.deepEqual(untouched, { action: "wait_task", task_id: "task-1" })
  assert.equal(targeted.provider_id, "provider-1")
  assert.deepEqual(targeted.input, { marker: "expected" })
})
