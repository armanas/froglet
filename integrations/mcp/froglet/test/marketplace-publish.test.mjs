import { describe, it } from "node:test"
import assert from "node:assert/strict"

import { validatePublishInput, serviceToml } from "../../../shared/froglet-lib/marketplace-publish.js"

describe("marketplace_publish: validatePublishInput", () => {
  const publicUrlDeps = {
    _deps: {
      marketplaceUrl: {
        lookup: async () => [{ address: "93.184.216.34", family: 4 }]
      },
      selfUrl: {
        lookup: async () => [{ address: "93.184.216.34", family: 4 }]
      }
    }
  }
  function ok(input) {
    return validatePublishInput(input, publicUrlDeps)
  }
  async function err(input, pattern) {
    await assert.rejects(() => validatePublishInput(input, publicUrlDeps), pattern)
  }

  it("accepts the minimum viable input", async () => {
    const r = await ok({ name: "translator", source_inline: "print('hi')" })
    assert.equal(r.name, "translator")
    assert.equal(r.runtime, "python")
    assert.equal(r.packageKind, "inline_source")
    assert.equal(r.hosting.kind, "tor")
    assert.equal(r.settlement.method, "none")
    assert.equal(r.marketplaceUrl, "https://marketplace.froglet.dev")
    assert.equal(r.entrypoint, "handler.py")
    assert.match(r.summary, /translator/)
  })

  it("carries price_sats and currency=usd for a stripe service", async () => {
    const r = await ok({ name: "paid-svc", source_inline: "x", settlement: { method: "stripe" }, price_sats: 500 })
    assert.equal(r.settlement.method, "stripe")
    assert.equal(r.priceSats, 500)
    assert.equal(r.currency, "usd")
    const toml = serviceToml(r)
    assert.match(toml, /method = "stripe"/)
    assert.match(toml, /currency = "usd"/)
    assert.match(toml, /sats = 500/)
  })

  it("scaffolds sat currency for free/lightning services", async () => {
    const free = serviceToml(await ok({ name: "free-svc", source_inline: "x" }))
    assert.match(free, /currency = "sat"/)
    assert.match(free, /sats = 0/)
  })

  it("uses summary when provided", async () => {
    const r = await ok({
      name: "x",
      source_inline: "x",
      summary: "Custom summary"
    })
    assert.equal(r.summary, "Custom summary")
  })

  it("rejects empty name", async () => {
    await err({ name: "", source_inline: "x" }, /name .* is invalid/)
  })

  it("rejects uppercase name", async () => {
    await err({ name: "MyService", source_inline: "x" }, /name .* is invalid/)
  })

  it("rejects name with underscore", async () => {
    await err({ name: "with_underscore", source_inline: "x" }, /name .* is invalid/)
  })

  it("rejects name with trailing hyphen", async () => {
    await err({ name: "trailing-", source_inline: "x" }, /name .* is invalid/)
  })

  it("rejects missing source_inline", async () => {
    await err({ name: "translator" }, /source_inline is required/)
  })

  it("rejects empty source_inline", async () => {
    await err({ name: "translator", source_inline: "" }, /source_inline is required/)
  })

  it("rejects unsupported runtime in v1A", async () => {
    await err(
      { name: "translator", source_inline: "x", runtime: "wasm" },
      /runtime .* not supported in v1A/
    )
  })

  it("rejects unsupported package_kind in v1A", async () => {
    await err(
      { name: "translator", source_inline: "x", package_kind: "oci_image" },
      /package_kind .* not supported in v1A/
    )
  })

  it("rejects unsupported hosting kind", async () => {
    await err(
      { name: "translator", source_inline: "x", hosting: { kind: "managed" } },
      /hosting.kind .* not supported in v1A/
    )
  })

  it("requires hosting.url when kind = self", async () => {
    await err(
      { name: "translator", source_inline: "x", hosting: { kind: "self" } },
      /hosting.url is required when hosting.kind = 'self'/
    )
  })

  it("accepts hosting.kind = self with url", async () => {
    const r = await ok({
      name: "translator",
      source_inline: "x",
      hosting: { kind: "self", url: "https://my-host.fly.dev" }
    })
    assert.equal(r.hosting.kind, "self")
    assert.equal(r.hosting.url, "https://my-host.fly.dev")
  })

  it("rejects unsupported settlement.method", async () => {
    await err(
      {
        name: "translator",
        source_inline: "x",
        settlement: { method: "paypal" }
      },
      /settlement.method .* not supported/
    )
  })

  it("accepts settlement.method = lightning", async () => {
    const r = await ok({
      name: "translator",
      source_inline: "x",
      settlement: { method: "lightning" }
    })
    assert.equal(r.settlement.method, "lightning")
  })

  it("accepts settlement.method = stripe", async () => {
    const r = await ok({
      name: "stripe-svc",
      source_inline: "print('hi')",
      settlement: { method: "stripe" }
    })
    assert.equal(r.settlement.method, "stripe")
  })

  it("accepts custom marketplace_url", async () => {
    const r = await ok({
      name: "translator",
      source_inline: "x",
      marketplace_url: "https://my-marketplace.example.com/"
    })
    assert.equal(r.marketplaceUrl, "https://my-marketplace.example.com")
  })

  it("accepts custom entrypoint", async () => {
    const r = await ok({
      name: "translator",
      source_inline: "x",
      entrypoint: "main.py"
    })
    assert.equal(r.entrypoint, "main.py")
  })

  it("accepts nested relative entrypoint", async () => {
    const r = await ok({
      name: "translator",
      source_inline: "x",
      entrypoint: "src/main.py"
    })
    assert.equal(r.entrypoint, "src/main.py")
  })

  it("rejects entrypoint path traversal and absolute paths", async () => {
    await err(
      { name: "translator", source_inline: "x", entrypoint: "../owned.py" },
      /entrypoint must not contain path traversal/
    )
    await err(
      { name: "translator", source_inline: "x", entrypoint: "src/../owned.py" },
      /entrypoint must not contain path traversal/
    )
    await err(
      { name: "translator", source_inline: "x", entrypoint: "/tmp/owned.py" },
      /entrypoint must be a relative path/
    )
  })

  it("rejects invalid marketplace and self-host URLs", async () => {
    await err(
      { name: "translator", source_inline: "x", marketplace_url: "https://m.example/\n[price]" },
      /marketplace_url is not a valid URL/
    )
    await err(
      { name: "translator", source_inline: "x", marketplace_url: "file:///tmp/market" },
      /marketplace_url must use https:\/\//
    )
    await err(
      { name: "translator", source_inline: "x", marketplace_url: "http://marketplace.example" },
      /marketplace_url must use https:\/\//
    )
    await err(
      {
        name: "translator",
        source_inline: "x",
        hosting: { kind: "self", url: "https://user:pass@example.com" }
      },
      /hosting.url must not contain credentials/
    )
    await err(
      { name: "translator", source_inline: "x", marketplace_url: "https://127.0.0.1" },
      /marketplace_url must be a public https:\/\/ URL: .*local or private address/
    )
    await err(
      {
        name: "translator",
        source_inline: "x",
        hosting: { kind: "self", url: "https://[::ffff:7f00:1]" }
      },
      /hosting.url must be a public https:\/\/ URL: .*local or private address/
    )
  })

  it("escapes TOML strings for user-controlled text", async () => {
    const toml = serviceToml(await ok({
      name: "translator",
      source_inline: "x",
      summary: "safe summary\"\n[price]\nsats = 999"
    }))
    assert.match(toml, /summary = "safe summary\\"\\n\[price\]\\nsats = 999"/)
    assert.equal((toml.match(/^\[price\]$/gm) ?? []).length, 1)
  })
})
