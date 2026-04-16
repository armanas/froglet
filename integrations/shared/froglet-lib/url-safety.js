// URL safety for LLM-controlled provider URLs.
//
// Defends against SSRF and DNS-rebinding attacks on any URL that flows in from
// untrusted sources such as MCP tool arguments. The operator-configured
// runtimeUrl / providerUrl channel already has its own strict validation in
// `shared.js::normalizeBaseUrl` and allows loopback for local development —
// this module is intentionally stricter and should NOT be reused for that
// channel.
//
// Policy (LLM-controlled surface):
// - scheme must be https (or a `.onion` hostname; see note below)
// - hostname must not be a loopback, RFC1918, link-local, documentation,
//   broadcast, or cloud-metadata address
// - DNS names are resolved and every returned IP is classified; reject if
//   any address is local
// - IPv6 zone IDs (`%...`) are treated as link-local and rejected
// - credentials in URL (`user:pw@host`) are rejected
//
// Tor / onion: routing onion addresses requires a SOCKS proxy, which in turn
// requires a production dependency we intentionally do not add here. This
// module rejects `.onion` hostnames on the LLM-controlled path and points the
// caller at the Rust runtime, which handles Tor natively. If future tiering
// adds `FROGLET_EGRESS_MODE=strict` or similar, the onion branch can be
// re-enabled via an explicit SOCKS dispatcher.
//
// IP-pinning: the validator returns the resolved `pinnedAddress` along with
// the normalized URL. Callers use `pinnedHttpsRequest` below to issue the
// actual request with DNS pinned to that IP, closing the DNS-rebinding TOCTOU
// window entirely. TLS SNI / cert validation continues to use the original
// hostname so https certificate binding is preserved.

import { isIP } from "node:net"
import { promises as dnsPromises } from "node:dns"
import { request as httpsRequest } from "node:https"
import { request as httpRequest } from "node:http"

const LOOPBACK_HOSTS = new Set([
  "localhost",
  "localhost.",
  "localhost.localdomain",
])

/**
 * Classify an IPv4 address literal against the set of ranges we refuse to
 * talk to on the LLM-controlled surface. Returns true if the address is
 * private/loopback/link-local/documentation/broadcast/unspecified/metadata.
 *
 * Must match `froglet/src/provider_resolution.rs::ip_v4_targets_local_network`.
 */
function ipV4TargetsLocalNetwork(ip) {
  const parts = ip.split(".").map((octet) => Number.parseInt(octet, 10))
  if (parts.length !== 4 || parts.some((n) => !Number.isFinite(n) || n < 0 || n > 255)) {
    return false
  }
  const [a, b, c, d] = parts
  // Private (RFC1918)
  if (a === 10) return true
  if (a === 172 && b >= 16 && b <= 31) return true
  if (a === 192 && b === 168) return true
  // Loopback
  if (a === 127) return true
  // Link-local (RFC3927); includes the AWS/GCP metadata 169.254.169.254
  if (a === 169 && b === 254) return true
  // Broadcast
  if (a === 255 && b === 255 && c === 255 && d === 255) return true
  // Documentation (RFC5737)
  if (a === 192 && b === 0 && c === 2) return true
  if (a === 198 && b === 51 && c === 100) return true
  if (a === 203 && b === 0 && c === 113) return true
  // Unspecified
  if (a === 0 && b === 0 && c === 0 && d === 0) return true
  return false
}

/**
 * Classify an IPv6 address literal. Matches the IPv6 arm of
 * `froglet/src/provider_resolution.rs::ip_targets_local_network`.
 */
function ipV6TargetsLocalNetwork(ip) {
  const lower = ip.toLowerCase()
  // Loopback
  if (lower === "::1") return true
  // Unspecified
  if (lower === "::") return true
  // IPv4-mapped (::ffff:x.x.x.x) — classify the embedded v4
  const mapped = lower.match(/^::ffff:(\d+\.\d+\.\d+\.\d+)$/)
  if (mapped) return ipV4TargetsLocalNetwork(mapped[1])
  // Collapse leading zeros in the first group for range checks
  const firstGroup = lower.split(":")[0]
  const firstNum = Number.parseInt(firstGroup, 16)
  if (Number.isFinite(firstNum)) {
    // Unique Local Address (ULA) fc00::/7
    if ((firstNum & 0xfe00) === 0xfc00) return true
    // Link-local fe80::/10
    if ((firstNum & 0xffc0) === 0xfe80) return true
    // Multicast ff00::/8
    if ((firstNum & 0xff00) === 0xff00) return true
  }
  return false
}

/**
 * Returns true if `ip` — an IPv4 or IPv6 address literal — targets a local,
 * private, or metadata endpoint that must not be reached from the
 * LLM-controlled surface.
 */
export function ipTargetsLocalNetwork(ip) {
  const family = isIP(ip)
  if (family === 4) return ipV4TargetsLocalNetwork(ip)
  if (family === 6) return ipV6TargetsLocalNetwork(ip)
  return false
}

function stripIpv6Brackets(host) {
  if (host.startsWith("[") && host.endsWith("]")) {
    return host.slice(1, -1)
  }
  return host
}

/**
 * Validate an LLM-controlled URL and produce the resources needed for a
 * DNS-pinned request. Rejects http (except via Tor onion, which this module
 * currently refuses with a clear error) and any URL whose host resolves to a
 * private / loopback / link-local / metadata address.
 *
 * @param {string} value
 * @param {string} label - human-readable field name used in error messages
 * @param {{ _deps?: { lookup?: Function } }} [opts] - test hook for DNS
 * @returns {Promise<{
 *   normalizedUrl: string,
 *   hostname: string,
 *   port: number,
 *   pinnedAddress: string,
 *   family: 4 | 6,
 * }>}
 */
export async function validateProviderUrl(value, label, opts = {}) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${label} must be a non-empty URL`)
  }
  const raw = value.trim()

  let parsed
  try {
    parsed = new URL(raw)
  } catch (error) {
    throw new Error(`${label} is not a valid URL: ${error.message}`)
  }

  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error(
      `${label} must use https (got ${parsed.protocol || "no scheme"})`
    )
  }

  if (parsed.username !== "" || parsed.password !== "") {
    throw new Error(`${label} must not contain credentials in the URL`)
  }

  if (parsed.hostname === "") {
    throw new Error(`${label} must include a hostname`)
  }

  const hostname = stripIpv6Brackets(parsed.hostname)

  if (hostname.endsWith(".onion")) {
    throw new Error(
      `${label} points at a Tor onion address; access onion providers through the Rust runtime, not the ${label} override`
    )
  }

  // After the onion branch, LLM-controlled URLs must be https.
  if (parsed.protocol !== "https:") {
    throw new Error(`${label} must use https (got ${parsed.protocol})`)
  }

  // IPv6 zone IDs indicate link-local scope; reject outright.
  if (hostname.includes("%")) {
    throw new Error(`${label} contains an IPv6 zone id; link-local addresses are not allowed`)
  }

  if (LOOPBACK_HOSTS.has(hostname.toLowerCase())) {
    throw new Error(`${label} points at a loopback host (${hostname})`)
  }

  const port = parsed.port
    ? Number.parseInt(parsed.port, 10)
    : parsed.protocol === "https:"
      ? 443
      : 80

  const literalFamily = isIP(hostname)
  if (literalFamily !== 0) {
    if (ipTargetsLocalNetwork(hostname)) {
      throw new Error(
        `${label} resolves to a local or private address (${hostname})`
      )
    }
    return {
      normalizedUrl: parsed.toString().replace(/\/$/, ""),
      hostname,
      port,
      pinnedAddress: hostname,
      family: literalFamily,
    }
  }

  const lookup = opts?._deps?.lookup ?? dnsPromises.lookup
  const results = await lookup(hostname, { all: true, verbatim: true })
  if (!Array.isArray(results) || results.length === 0) {
    throw new Error(`${label} could not be resolved to any address`)
  }

  for (const entry of results) {
    if (!entry || typeof entry.address !== "string") {
      throw new Error(`${label} returned a malformed DNS result`)
    }
    if (ipTargetsLocalNetwork(entry.address)) {
      throw new Error(
        `${label} resolves to a local or private address (${entry.address})`
      )
    }
  }

  // Prefer IPv4 when available (broader reachability); fall back to first.
  const chosen =
    results.find((entry) => entry.family === 4) ?? results[0]

  return {
    normalizedUrl: parsed.toString().replace(/\/$/, ""),
    hostname,
    port,
    pinnedAddress: chosen.address,
    family: chosen.family,
  }
}

/**
 * Perform an https (or http) request with DNS pinned to `pinnedAddress`. This
 * closes the DNS-rebinding TOCTOU window between validation and fetch by
 * guaranteeing the socket connects to the address the validator classified.
 *
 * The `servername` is set to the original hostname so TLS SNI and certificate
 * validation continue to work. Returns the same `{ status, payload }` shape as
 * the stock `jsonRequest` helper in `froglet-client.js`.
 */
export async function pinnedJsonRequest(url, {
  method = "GET",
  headers = {},
  jsonBody,
  timeoutMs,
  expectedStatuses = [200],
  pinnedAddress,
  family,
}) {
  if (typeof pinnedAddress !== "string" || pinnedAddress.length === 0) {
    throw new Error("pinnedJsonRequest requires pinnedAddress")
  }
  const parsed = new URL(url)
  const isHttps = parsed.protocol === "https:"
  const port = parsed.port
    ? Number.parseInt(parsed.port, 10)
    : isHttps
      ? 443
      : 80
  const bodyBuffer =
    jsonBody !== undefined ? Buffer.from(JSON.stringify(jsonBody)) : null
  const requestFn = isHttps ? httpsRequest : httpRequest
  const requestHeaders = {
    Accept: "application/json",
    ...(bodyBuffer ? { "Content-Type": "application/json" } : {}),
    ...headers,
    Host: parsed.host,
    ...(bodyBuffer ? { "Content-Length": String(bodyBuffer.length) } : {}),
  }

  return new Promise((resolve, reject) => {
    let timer = null
    const req = requestFn({
      method,
      host: pinnedAddress,
      port,
      path: `${parsed.pathname}${parsed.search}`,
      headers: requestHeaders,
      servername: parsed.hostname,
      // Belt-and-suspenders: override DNS lookup too so any library-driven
      // re-resolution inside the stack stays pinned to pinnedAddress.
      lookup: (_hostname, _options, cb) =>
        cb(null, pinnedAddress, family ?? (isIP(pinnedAddress) || 4)),
    })

    if (timeoutMs) {
      timer = setTimeout(() => {
        req.destroy(new Error(`Request to ${url} timed out after ${timeoutMs}ms`))
      }, timeoutMs)
    }

    req.on("error", (err) => {
      if (timer) clearTimeout(timer)
      reject(err)
    })

    req.on("response", (response) => {
      const chunks = []
      response.on("data", (chunk) => chunks.push(chunk))
      response.on("end", () => {
        if (timer) clearTimeout(timer)
        const body = Buffer.concat(chunks).toString("utf8")
        let payload = null
        if (body.length > 0) {
          try {
            payload = JSON.parse(body)
          } catch (error) {
            const preview = body.slice(0, 200)
            reject(new Error(
              `Expected JSON from ${url}, got invalid payload: ${error.message}; body=${JSON.stringify(preview)}`
            ))
            return
          }
        }
        if (!expectedStatuses.includes(response.statusCode)) {
          reject(new Error(
            `Request to ${url} failed with ${response.statusCode}: ${JSON.stringify(payload)}`
          ))
          return
        }
        resolve({ status: response.statusCode, payload })
      })
      response.on("error", (err) => {
        if (timer) clearTimeout(timer)
        reject(err)
      })
    })

    if (bodyBuffer) {
      req.write(bodyBuffer)
    }
    req.end()
  })
}
