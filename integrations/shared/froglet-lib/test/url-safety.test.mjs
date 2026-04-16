import assert from "node:assert/strict"
import test from "node:test"

import {
  ipTargetsLocalNetwork,
  validateProviderUrl,
} from "../url-safety.js"

// Stub DNS lookup used via the `_deps` hook so tests stay hermetic.
function stubLookup(entries) {
  return async (_hostname, _options) => entries
}

test("ipTargetsLocalNetwork classifies RFC1918 and loopback IPv4", () => {
  assert.equal(ipTargetsLocalNetwork("10.0.0.1"), true)
  assert.equal(ipTargetsLocalNetwork("172.16.0.1"), true)
  assert.equal(ipTargetsLocalNetwork("172.31.255.255"), true)
  assert.equal(ipTargetsLocalNetwork("192.168.1.1"), true)
  assert.equal(ipTargetsLocalNetwork("127.0.0.1"), true)
  assert.equal(ipTargetsLocalNetwork("169.254.169.254"), true)
  assert.equal(ipTargetsLocalNetwork("169.254.1.1"), true)
  assert.equal(ipTargetsLocalNetwork("0.0.0.0"), true)
  assert.equal(ipTargetsLocalNetwork("255.255.255.255"), true)
})

test("ipTargetsLocalNetwork classifies documentation IPv4", () => {
  assert.equal(ipTargetsLocalNetwork("192.0.2.1"), true)
  assert.equal(ipTargetsLocalNetwork("198.51.100.1"), true)
  assert.equal(ipTargetsLocalNetwork("203.0.113.1"), true)
})

test("ipTargetsLocalNetwork allows public IPv4", () => {
  assert.equal(ipTargetsLocalNetwork("8.8.8.8"), false)
  assert.equal(ipTargetsLocalNetwork("1.1.1.1"), false)
  assert.equal(ipTargetsLocalNetwork("93.184.216.34"), false)
  // 172.15/16 and 172.32/16 are public — boundary check on the RFC1918 range
  assert.equal(ipTargetsLocalNetwork("172.15.0.1"), false)
  assert.equal(ipTargetsLocalNetwork("172.32.0.1"), false)
})

test("ipTargetsLocalNetwork classifies IPv6 loopback, ULA, link-local, multicast", () => {
  assert.equal(ipTargetsLocalNetwork("::1"), true)
  assert.equal(ipTargetsLocalNetwork("::"), true)
  assert.equal(ipTargetsLocalNetwork("fc00::1"), true)
  assert.equal(ipTargetsLocalNetwork("fd00::1"), true)
  assert.equal(ipTargetsLocalNetwork("fe80::1"), true)
  assert.equal(ipTargetsLocalNetwork("ff02::1"), true)
  // IPv4-mapped to a private v4 address
  assert.equal(ipTargetsLocalNetwork("::ffff:10.0.0.1"), true)
  assert.equal(ipTargetsLocalNetwork("::ffff:169.254.169.254"), true)
})

test("ipTargetsLocalNetwork allows public IPv6", () => {
  assert.equal(ipTargetsLocalNetwork("2606:4700::1"), false)
  assert.equal(ipTargetsLocalNetwork("2001:db8::1"), false)
  // IPv4-mapped to a public v4 address
  assert.equal(ipTargetsLocalNetwork("::ffff:8.8.8.8"), false)
})

test("validateProviderUrl rejects empty, non-string, malformed input", async () => {
  await assert.rejects(
    () => validateProviderUrl("", "provider_url"),
    /must be a non-empty URL/
  )
  await assert.rejects(
    () => validateProviderUrl(undefined, "provider_url"),
    /must be a non-empty URL/
  )
  await assert.rejects(
    () => validateProviderUrl(42, "provider_url"),
    /must be a non-empty URL/
  )
  await assert.rejects(
    () => validateProviderUrl("not a url", "provider_url"),
    /is not a valid URL/
  )
})

test("validateProviderUrl rejects non-https schemes", async () => {
  await assert.rejects(
    () => validateProviderUrl("http://example.com", "provider_url"),
    /must use https/
  )
  await assert.rejects(
    () => validateProviderUrl("ftp://example.com", "provider_url"),
    /must use https/
  )
  await assert.rejects(
    () => validateProviderUrl("file:///etc/passwd", "provider_url"),
    /must use https/
  )
  await assert.rejects(
    () => validateProviderUrl("javascript:alert(1)", "provider_url"),
    /must use https/
  )
})

test("validateProviderUrl rejects embedded credentials", async () => {
  await assert.rejects(
    () => validateProviderUrl("https://user:password@example.com", "provider_url"),
    /must not contain credentials/
  )
})

test("validateProviderUrl rejects loopback hostnames", async () => {
  await assert.rejects(
    () => validateProviderUrl("https://localhost", "provider_url"),
    /loopback host/
  )
  await assert.rejects(
    () => validateProviderUrl("https://localhost.localdomain:8443", "provider_url"),
    /loopback host/
  )
})

test("validateProviderUrl rejects private IPv4 literals", async () => {
  for (const url of [
    "https://127.0.0.1",
    "https://10.0.0.1:8443",
    "https://192.168.1.1",
    "https://172.16.0.1",
    "https://169.254.169.254",
    "https://169.254.1.1:80",
  ]) {
    await assert.rejects(
      () => validateProviderUrl(url, "provider_url"),
      /local or private address/,
      `expected ${url} to be rejected`
    )
  }
})

test("validateProviderUrl rejects private IPv6 literals", async () => {
  for (const url of [
    "https://[::1]",
    "https://[fc00::1]",
    "https://[fe80::1]",
    "https://[ff02::1]",
  ]) {
    await assert.rejects(
      () => validateProviderUrl(url, "provider_url"),
      /local or private address/,
      `expected ${url} to be rejected`
    )
  }
})

test("validateProviderUrl rejects IPv6 zone ids", async () => {
  // Node's URL parser may reject `[fe80::1%25eth0]` as invalid syntax, or
  // parse it and preserve the zone id. Either rejection is acceptable as
  // long as the address is not treated as a public host.
  await assert.rejects(
    () =>
      validateProviderUrl("https://[fe80::1%25eth0]", "provider_url"),
    /IPv6 zone id|local or private|not a valid URL/
  )
})

test("validateProviderUrl rejects .onion addresses", async () => {
  await assert.rejects(
    () =>
      validateProviderUrl(
        "https://abcdefghijklmnop.onion",
        "provider_url"
      ),
    /onion/
  )
  await assert.rejects(
    () => validateProviderUrl("http://abcdefghijklmnop.onion", "provider_url"),
    /onion/
  )
})

test("validateProviderUrl rejects hostnames that resolve to private addresses", async () => {
  await assert.rejects(
    () =>
      validateProviderUrl("https://rebind.example", "provider_url", {
        _deps: {
          lookup: stubLookup([{ address: "127.0.0.1", family: 4 }]),
        },
      }),
    /local or private address/
  )
  // Any private address among mixed results also rejects (partial-match attack).
  await assert.rejects(
    () =>
      validateProviderUrl("https://mixed.example", "provider_url", {
        _deps: {
          lookup: stubLookup([
            { address: "93.184.216.34", family: 4 },
            { address: "169.254.169.254", family: 4 },
          ]),
        },
      }),
    /local or private address/
  )
})

test("validateProviderUrl rejects empty DNS responses", async () => {
  await assert.rejects(
    () =>
      validateProviderUrl("https://void.example", "provider_url", {
        _deps: { lookup: stubLookup([]) },
      }),
    /could not be resolved/
  )
})

test("validateProviderUrl accepts public literal IPv4", async () => {
  const result = await validateProviderUrl(
    "https://93.184.216.34",
    "provider_url"
  )
  assert.equal(result.normalizedUrl, "https://93.184.216.34")
  assert.equal(result.hostname, "93.184.216.34")
  assert.equal(result.pinnedAddress, "93.184.216.34")
  assert.equal(result.family, 4)
  assert.equal(result.port, 443)
})

test("validateProviderUrl accepts public literal IPv6", async () => {
  const result = await validateProviderUrl(
    "https://[2606:4700::1]:8443",
    "provider_url"
  )
  assert.equal(result.pinnedAddress, "2606:4700::1")
  assert.equal(result.family, 6)
  assert.equal(result.port, 8443)
})

test("validateProviderUrl accepts public hostnames and pins the chosen IP", async () => {
  const result = await validateProviderUrl("https://public.example.com", "provider_url", {
    _deps: {
      lookup: stubLookup([
        { address: "2606:4700::1", family: 6 },
        { address: "93.184.216.34", family: 4 },
      ]),
    },
  })
  assert.equal(result.hostname, "public.example.com")
  // Prefers IPv4 when available.
  assert.equal(result.pinnedAddress, "93.184.216.34")
  assert.equal(result.family, 4)
  assert.equal(result.normalizedUrl, "https://public.example.com")
})

test("validateProviderUrl strips trailing slash in normalizedUrl", async () => {
  const result = await validateProviderUrl(
    "https://public.example.com/",
    "provider_url",
    {
      _deps: {
        lookup: stubLookup([{ address: "93.184.216.34", family: 4 }]),
      },
    }
  )
  assert.equal(result.normalizedUrl, "https://public.example.com")
})
