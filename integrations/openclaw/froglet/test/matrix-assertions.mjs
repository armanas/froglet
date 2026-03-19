import assert from "node:assert/strict"

export function extractJsonSection(text, label) {
  const marker = `${label}\n`
  const index = text.indexOf(marker)
  assert.notEqual(index, -1, `missing ${label} in tool output`)
  return JSON.parse(text.slice(index + marker.length))
}

export function normalizeLines(text) {
  return String(text)
    .split(/\r?\n/)
    .map((line) => line.trimEnd())
}

export function assertContainsAll(text, needles, message = "missing expected text") {
  const haystack = String(text)
  for (const needle of needles) {
    assert.ok(haystack.includes(needle), `${message}: ${needle}`)
  }
}

export function assertContainsInOrder(text, needles, message = "missing ordered text") {
  const haystack = String(text)
  let cursor = 0
  for (const needle of needles) {
    const index = haystack.indexOf(needle, cursor)
    assert.notEqual(index, -1, `${message}: ${needle}`)
    cursor = index + needle.length
  }
}

export function assertToolSummary(text, expectations) {
  if (expectations.title) {
    assertContainsAll(text, [expectations.title], "missing tool summary title")
  }
  if (Array.isArray(expectations.contains)) {
    assertContainsAll(text, expectations.contains, "missing tool summary content")
  }
  if (Array.isArray(expectations.ordered)) {
    assertContainsInOrder(text, expectations.ordered, "missing ordered tool summary content")
  }
}

export function assertAgentTranscript(text, expectations = {}) {
  const lines = normalizeLines(text)
  const normalized = lines.join("\n")

  if (expectations.mustContain) {
    assertContainsAll(normalized, expectations.mustContain, "agent transcript missing expected content")
  }

  if (expectations.mustContainOrdered) {
    assertContainsInOrder(
      normalized,
      expectations.mustContainOrdered,
      "agent transcript missing ordered content"
    )
  }

  if (expectations.mustNotContain) {
    for (const needle of expectations.mustNotContain) {
      assert.ok(!normalized.includes(needle), `agent transcript unexpectedly contained ${needle}`)
    }
  }

  return { lines, normalized }
}
