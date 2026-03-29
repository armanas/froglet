import { createHash } from "node:crypto"
import http from "node:http"
import { mkdir, readFile, writeFile } from "node:fs/promises"
import path from "node:path"

import { parseCliArgs } from "./common.mjs"

function sha256Hex(buffer) {
  return createHash("sha256").update(buffer).digest("hex")
}

function sendJson(response, statusCode, payload) {
  response.writeHead(statusCode, { "Content-Type": "application/json" })
  response.end(JSON.stringify(payload))
}

async function main() {
  const { values } = parseCliArgs({
    listen: { type: "string" },
    "module-hex-path": { type: "string" },
    out: { type: "string" },
  })
  if (!values.listen || !values["module-hex-path"] || !values.out) {
    throw new Error("--listen, --module-hex-path, and --out are required")
  }

  const [host, portRaw] = values.listen.split(":")
  const port = Number.parseInt(portRaw, 10)
  if (!host || !Number.isInteger(port)) {
    throw new Error(`invalid --listen value ${values.listen}`)
  }

  const moduleHex = (await readFile(values["module-hex-path"], "utf8")).trim()
  const moduleBytes = Buffer.from(moduleHex, "hex")
  const image = "module"
  const reference = "latest"
  const layerDigest = `sha256:${sha256Hex(moduleBytes)}`

  const server = http.createServer((request, response) => {
    const url = new URL(request.url ?? "/", `http://${request.headers.host ?? values.listen}`)
    if (request.method === "GET" && url.pathname === "/token") {
      sendJson(response, 200, {
        token: "froglet-harness-token",
        access_token: "froglet-harness-token",
        expires_in: 3600,
      })
      return
    }

    const manifestPath = `/v2/${image}/manifests/${reference}`
    if (request.method === "GET" && url.pathname === manifestPath) {
      sendJson(response, 200, {
        schemaVersion: 2,
        mediaType: "application/vnd.oci.image.manifest.v1+json",
        config: {
          mediaType: "application/vnd.oci.image.config.v1+json",
          size: 2,
          digest: `sha256:${"00".repeat(32)}`,
        },
        layers: [
          {
            mediaType: "application/wasm",
            size: moduleBytes.length,
            digest: layerDigest,
          },
        ],
      })
      return
    }

    const blobPath = `/v2/${image}/blobs/${layerDigest}`
    if (request.method === "GET" && url.pathname === blobPath) {
      response.writeHead(200, { "Content-Type": "application/wasm" })
      response.end(moduleBytes)
      return
    }

    sendJson(response, 404, { error: "not_found", path: url.pathname })
  })

  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(port, host, resolve)
  })

  const info = {
    listen: values.listen,
    oci_reference: `http://${values.listen}/${image}:${reference}`,
    oci_digest: sha256Hex(moduleBytes),
  }
  await mkdir(path.dirname(values.out), { recursive: true })
  await writeFile(values.out, `${JSON.stringify(info, null, 2)}\n`, "utf8")
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
