import assert from "node:assert/strict"
import test from "node:test"
import { createServer } from "node:http"
import { once } from "node:events"

// End-to-end test that `request.provider_url` on the LLM-controlled surface
// is (a) rejected when it points at a private address and (b) correctly
// DNS-pinned when it points at a public host that resolves (per our stubbed
// lookup) to a loopback test server. The second case exercises the
// `pinnedJsonRequest` path — a rebinding DNS server could not redirect it
// elsewhere because the socket connects to the address the validator chose.

// Import the shared client lib's internal exports.
import {
  pinnedJsonRequest,
  validateProviderUrl,
} from "../../../shared/froglet-lib/url-safety.js"

async function withServer(handler, fn) {
  const server = createServer(handler)
  server.listen(0, "127.0.0.1")
  await once(server, "listening")
  const address = server.address()
  try {
    await fn(address.port)
  } finally {
    server.close()
    await once(server, "close")
  }
}

test("validateProviderUrl rejects http://169.254.169.254 without any outbound request", async () => {
  let outboundRequests = 0
  await withServer(
    (_req, res) => {
      outboundRequests += 1
      res.end("{}")
    },
    async () => {
      await assert.rejects(
        () =>
          validateProviderUrl(
            "http://169.254.169.254/latest/meta-data/",
            "request.provider_url"
          ),
        /https/
      )
    }
  )
  assert.equal(
    outboundRequests,
    0,
    "no outbound request should be issued for a rejected URL"
  )
})

test("pinnedJsonRequest connects only to the pinned address even if hostname disagrees", async () => {
  await withServer(
    (req, res) => {
      // Hostname in the request URL is unroutable, but the socket connects to
      // 127.0.0.1 because the pin overrode the dispatcher's lookup.
      res.setHeader("content-type", "application/json")
      res.end(JSON.stringify({ ok: true, path: req.url, host: req.headers.host }))
    },
    async (port) => {
      const result = await pinnedJsonRequest(`http://does-not-resolve.example:${port}/ping`, {
        method: "GET",
        timeoutMs: 2000,
        pinnedAddress: "127.0.0.1",
        family: 4,
      })
      assert.equal(result.status, 200)
      assert.equal(result.payload.ok, true)
      assert.equal(result.payload.path, "/ping")
      assert.equal(
        result.payload.host,
        `does-not-resolve.example:${port}`,
        "Host header should preserve the original hostname for TLS SNI / virtual hosts"
      )
    }
  )
})
