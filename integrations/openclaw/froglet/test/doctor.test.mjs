import assert from "node:assert/strict"
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import process from "node:process"
import test from "node:test"

const doctorPath = path.resolve(import.meta.dirname, "..", "scripts", "doctor.mjs")

async function writeConfig(tempDir, fileName, pluginConfig) {
  const configPath = path.join(tempDir, fileName)
  const document = {
    plugins: {
      load: { paths: [path.join(tempDir, "plugin")] },
      entries: {
        froglet: {
          enabled: true,
          config: pluginConfig
        }
      }
    }
  }
  await writeFile(configPath, JSON.stringify(document, null, 2), "utf8")
  return configPath
}

async function runDoctor(args) {
  const child = process.execPath
  const { spawn } = await import("node:child_process")
  return new Promise((resolve, reject) => {
    const proc = spawn(child, [doctorPath, ...args], {
      stdio: ["ignore", "pipe", "pipe"]
    })
    let stdout = ""
    let stderr = ""
    proc.stdout.on("data", (chunk) => {
      stdout += chunk
    })
    proc.stderr.on("data", (chunk) => {
      stderr += chunk
    })
    proc.on("error", reject)
    proc.on("close", (code) => resolve({ code, stdout, stderr }))
  })
}

test("doctor validates the new single-tool config shape", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-token\n", "utf8")
    await writeFile(path.join(tempDir, "plugin"), "ok\n", "utf8")
    const configPath = await writeConfig(tempDir, "openclaw.json", {
      hostProduct: "openclaw",
      baseUrl: "http://127.0.0.1:9191",
      authTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    const result = await runDoctor(["--config", configPath, "--target", "openclaw"])
    assert.equal(result.code, 0)
    const payload = JSON.parse(result.stdout)
    assert.equal(payload.overall_status, "warning")
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor can run a live status probe", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  const server = http.createServer((req, res) => {
    if (req.url === "/v1/froglet/status" && req.headers.authorization === "Bearer froglet-token") {
      res.writeHead(200, { "content-type": "application/json" })
      res.end(
        JSON.stringify({
          node_id: "node-1",
          runtime: { healthy: true },
          provider: { healthy: true }
        })
      )
      return
    }
    res.writeHead(404).end()
  })
  try {
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
    const address = server.address()
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-token\n", "utf8")
    await writeFile(path.join(tempDir, "plugin"), "ok\n", "utf8")
    const configPath = await writeConfig(tempDir, "openclaw-live.json", {
      hostProduct: "openclaw",
      baseUrl: `http://127.0.0.1:${address.port}`,
      authTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    const result = await runDoctor([
      "--config",
      configPath,
      "--target",
      "openclaw",
      "--check-runtime"
    ])
    assert.equal(result.code, 0)
    const payload = JSON.parse(result.stdout)
    assert.equal(payload.overall_status, "ok")
  } finally {
    server.close()
    await rm(tempDir, { recursive: true, force: true })
  }
})
