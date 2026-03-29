import http from "node:http"
import https from "node:https"
import { readFile } from "node:fs/promises"

import { parseCliArgs } from "./common.mjs"

function selectClient(protocol) {
  if (protocol === "http:") {
    return http
  }
  if (protocol === "https:") {
    return https
  }
  throw new Error(`unsupported target protocol ${protocol}`)
}

async function main() {
  const { values } = parseCliArgs({
    listen: { type: "string" },
    target: { type: "string" },
    cert: { type: "string" },
    key: { type: "string" },
  })
  if (!values.listen || !values.target || !values.cert || !values.key) {
    throw new Error("--listen, --target, --cert, and --key are required")
  }

  const targetUrl = new URL(values.target)
  const [host, portRaw] = values.listen.split(":")
  const port = Number.parseInt(portRaw, 10)
  if (!host || !Number.isInteger(port)) {
    throw new Error(`invalid --listen value ${values.listen}`)
  }

  const [cert, key] = await Promise.all([readFile(values.cert), readFile(values.key)])
  const client = selectClient(targetUrl.protocol)

  const server = https.createServer({ cert, key }, (request, response) => {
    const requestUrl = new URL(request.url ?? "/", values.target)
    const chunks = []
    request.on("data", (chunk) => chunks.push(chunk))
    request.on("error", (error) => {
      response.writeHead(502, { "Content-Type": "application/json" })
      response.end(JSON.stringify({ error: `request stream error: ${error.message}` }))
    })
    request.on("end", () => {
      const upstream = client.request(
        {
          protocol: targetUrl.protocol,
          hostname: targetUrl.hostname,
          port: targetUrl.port,
          method: request.method,
          path: `${requestUrl.pathname}${requestUrl.search}`,
          headers: request.headers,
          rejectUnauthorized: false,
        },
        (upstreamResponse) => {
          response.writeHead(upstreamResponse.statusCode ?? 502, upstreamResponse.headers)
          upstreamResponse.pipe(response)
        }
      )
      upstream.on("error", (error) => {
        response.writeHead(502, { "Content-Type": "application/json" })
        response.end(JSON.stringify({ error: `upstream request failed: ${error.message}` }))
      })
      if (chunks.length > 0) {
        upstream.write(Buffer.concat(chunks))
      }
      upstream.end()
    })
  })

  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(port, host, resolve)
  })
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
