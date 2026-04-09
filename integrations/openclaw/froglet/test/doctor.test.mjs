import assert from "node:assert/strict"
import { mkdtemp, rm, writeFile } from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import process from "node:process"
import test from "node:test"
import { fileURLToPath } from "node:url"

import { checkApis } from "../scripts/doctor.mjs"

const doctorPath = fileURLToPath(new URL("../scripts/doctor.mjs", import.meta.url))

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

test("doctor validates the new dual-URL config shape", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-token\n", "utf8")
    await writeFile(path.join(tempDir, "plugin"), "ok\n", "utf8")
    const configPath = await writeConfig(tempDir, "openclaw.json", {
      hostProduct: "openclaw",
      providerUrl: "http://127.0.0.1:8080",
      runtimeUrl: "http://127.0.0.1:8081",
      providerAuthTokenPath: tokenPath,
      runtimeAuthTokenPath: tokenPath,
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

test("doctor validates legacy baseUrl/authTokenPath config as fallback", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-token\n", "utf8")
    await writeFile(path.join(tempDir, "plugin"), "ok\n", "utf8")
    const configPath = await writeConfig(tempDir, "openclaw-legacy.json", {
      hostProduct: "openclaw",
      baseUrl: "http://127.0.0.1:8080",
      authTokenPath: tokenPath,
      requestTimeoutMs: 1000,
      defaultSearchLimit: 10,
      maxSearchLimit: 50
    })
    const result = await runDoctor(["--config", configPath, "--target", "openclaw"])
    assert.equal(result.code, 0)
    const payload = JSON.parse(result.stdout)
    // auth_token check may be warning (plugin dir doesn't exist) but must not be error
    assert.ok(["ok", "warning"].includes(payload.overall_status))
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor checkApis probes provider and runtime health", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  const previousFetch = global.fetch
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-token\n", "utf8")
    global.fetch = async (url, options = {}) => {
      const urlStr = String(url)
      const auth = options.headers?.Authorization
      if (urlStr === "http://127.0.0.1:8080/health") {
        return new Response(JSON.stringify({ healthy: true }))
      }
      if (urlStr === "http://127.0.0.1:8080/v1/node/capabilities" && auth === "Bearer froglet-token") {
        return new Response(JSON.stringify({ compute_offer_ids: ["execute.compute"] }))
      }
      if (urlStr === "http://127.0.0.1:8080/v1/node/identity" && auth === "Bearer froglet-token") {
        return new Response(JSON.stringify({ node_id: "node-1" }))
      }
      if (urlStr === "http://127.0.0.1:8081/health") {
        return new Response(JSON.stringify({ status: "ok" }))
      }
      throw new Error(`unexpected URL ${urlStr}`)
    }

    const status = await checkApis(
      "http://127.0.0.1:8080",
      tokenPath,
      "http://127.0.0.1:8081",
      1000
    )
    assert.equal(status.node_id, "node-1")
    assert.equal(status.healthy, true)
    assert.equal(status.provider_healthy, true)
    assert.equal(status.runtime_healthy, true)
  } finally {
    global.fetch = previousFetch
    await rm(tempDir, { recursive: true, force: true })
  }
})
