import { describe, it } from "node:test"
import assert from "node:assert/strict"

import { validatePublishInput } from "../../../shared/froglet-lib/marketplace-publish.js"

describe("marketplace_publish: validatePublishInput", () => {
  function ok(input) {
    return validatePublishInput(input)
  }
  function err(input, pattern) {
    assert.throws(() => validatePublishInput(input), pattern)
  }

  it("accepts the minimum viable input", () => {
    const r = ok({ name: "translator", source_inline: "print('hi')" })
    assert.equal(r.name, "translator")
    assert.equal(r.runtime, "python")
    assert.equal(r.packageKind, "inline_source")
    assert.equal(r.hosting.kind, "tor")
    assert.equal(r.settlement.method, "none")
    assert.equal(r.marketplaceUrl, "https://marketplace.froglet.dev")
    assert.equal(r.entrypoint, "handler.py")
    assert.match(r.summary, /translator/)
  })

  it("uses summary when provided", () => {
    const r = ok({
      name: "x",
      source_inline: "x",
      summary: "Custom summary"
    })
    assert.equal(r.summary, "Custom summary")
  })

  it("rejects empty name", () => {
    err({ name: "", source_inline: "x" }, /name .* is invalid/)
  })

  it("rejects uppercase name", () => {
    err({ name: "MyService", source_inline: "x" }, /name .* is invalid/)
  })

  it("rejects name with underscore", () => {
    err({ name: "with_underscore", source_inline: "x" }, /name .* is invalid/)
  })

  it("rejects name with trailing hyphen", () => {
    err({ name: "trailing-", source_inline: "x" }, /name .* is invalid/)
  })

  it("rejects missing source_inline", () => {
    err({ name: "translator" }, /source_inline is required/)
  })

  it("rejects empty source_inline", () => {
    err({ name: "translator", source_inline: "" }, /source_inline is required/)
  })

  it("rejects unsupported runtime in v1A", () => {
    err(
      { name: "translator", source_inline: "x", runtime: "wasm" },
      /runtime .* not supported in v1A/
    )
  })

  it("rejects unsupported package_kind in v1A", () => {
    err(
      { name: "translator", source_inline: "x", package_kind: "oci_image" },
      /package_kind .* not supported in v1A/
    )
  })

  it("rejects unsupported hosting kind", () => {
    err(
      { name: "translator", source_inline: "x", hosting: { kind: "managed" } },
      /hosting.kind .* not supported in v1A/
    )
  })

  it("requires hosting.url when kind = self", () => {
    err(
      { name: "translator", source_inline: "x", hosting: { kind: "self" } },
      /hosting.url is required when hosting.kind = 'self'/
    )
  })

  it("accepts hosting.kind = self with url", () => {
    const r = ok({
      name: "translator",
      source_inline: "x",
      hosting: { kind: "self", url: "https://my-host.fly.dev" }
    })
    assert.equal(r.hosting.kind, "self")
    assert.equal(r.hosting.url, "https://my-host.fly.dev")
  })

  it("rejects settlement.method != none in v1", () => {
    err(
      {
        name: "translator",
        source_inline: "x",
        settlement: { method: "lightning" }
      },
      /settlement.method .* not supported in v1/
    )
  })

  it("accepts custom marketplace_url", () => {
    const r = ok({
      name: "translator",
      source_inline: "x",
      marketplace_url: "https://my-marketplace.example.com"
    })
    assert.equal(r.marketplaceUrl, "https://my-marketplace.example.com")
  })

  it("accepts custom entrypoint", () => {
    const r = ok({
      name: "translator",
      source_inline: "x",
      entrypoint: "main.py"
    })
    assert.equal(r.entrypoint, "main.py")
  })
})
