import assert from "node:assert/strict"
import { mkdtemp, rm, writeFile } from "node:fs/promises"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import test from "node:test"

import { collectDoctorResults, runDoctorCli } from "../scripts/doctor.mjs"

function makeOpenClawConfig({ pluginPath, runtimeUrl, tokenPath, overrides = {} }) {
  return {
    plugins: {
      load: {
        paths: [pluginPath]
      },
      entries: {
        froglet: {
          enabled: true,
          config: {
            runtimeUrl,
            runtimeAuthTokenPath: tokenPath,
            requestTimeoutMs: 1000,
            defaultSearchLimit: 10,
            maxSearchLimit: 50,
            ...overrides
          }
        }
      }
    }
  }
}

function makeNemoclawConfig({ runtimeUrl = "https://consumer.example", tokenPath = "/sandbox/.openclaw/froglet-runtime.token" } = {}) {
  return {
    plugins: {
      load: {
        paths: ["/sandbox/froglet/integrations/openclaw/froglet"]
      },
      entries: {
        froglet: {
          enabled: true,
          config: {
            runtimeUrl,
            runtimeAuthTokenPath: tokenPath,
            requestTimeoutMs: 15000,
            defaultSearchLimit: 10,
            maxSearchLimit: 50
          }
        }
      }
    }
  }
}

async function withRuntimeServer(handler, fn) {
  const server = http.createServer(handler)
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
  const address = server.address()
  const runtimeUrl = `http://127.0.0.1:${address.port}`
  try {
    await fn(runtimeUrl)
  } finally {
    await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())))
  }
}

async function writeConfig(tempDir, fileName, document) {
  const configPath = path.join(tempDir, fileName)
  await writeFile(configPath, JSON.stringify(document, null, 2), "utf8")
  return configPath
}

test("valid OpenClaw config passes structural validation", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const configPath = await writeConfig(
      tempDir,
      "openclaw.json",
      makeOpenClawConfig({
        pluginPath: tempDir,
        runtimeUrl: "http://127.0.0.1:8081",
        tokenPath
      })
    )

    const result = await collectDoctorResults({
      configPath,
      target: "openclaw"
    })

    assert.equal(result.overallStatus, "ok")
    assert.deepEqual(
      result.checks.filter((check) => check.status === "error"),
      []
    )
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("valid NemoClaw config passes structural validation with manual follow-up warnings", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const configPath = await writeConfig(tempDir, "nemoclaw.json", makeNemoclawConfig())
    const result = await collectDoctorResults({
      configPath,
      target: "nemoclaw"
    })

    assert.equal(result.overallStatus, "warning")
    assert.equal(
      result.checks.some((check) => check.id === "manual_nemoclaw_checks" && check.status === "warning"),
      true
    )
    assert.deepEqual(
      result.checks.filter((check) => check.status === "error"),
      []
    )
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor fails when required Froglet runtime keys are missing", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const configPath = await writeConfig(tempDir, "missing-runtime-url.json", {
      plugins: {
        load: { paths: [tempDir] },
        entries: {
          froglet: {
            enabled: true,
            config: {
              runtimeAuthTokenPath: path.join(tempDir, "auth.token")
            }
          }
        }
      }
    })

    const result = await collectDoctorResults({
      configPath,
      target: "openclaw"
    })

    assert.equal(result.overallStatus, "error")
    assert.match(result.checks.find((check) => check.id === "froglet_config").summary, /runtimeUrl/)
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor rejects non-loopback HTTP runtime URLs", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const configPath = await writeConfig(
      tempDir,
      "bad-runtime-url.json",
      makeOpenClawConfig({
        pluginPath: tempDir,
        runtimeUrl: "http://10.0.0.5:8081",
        tokenPath
      })
    )

    const result = await collectDoctorResults({
      configPath,
      target: "openclaw"
    })

    assert.equal(result.overallStatus, "error")
    assert.match(
      result.checks.find((check) => check.id === "froglet_config").summary,
      /https:\/\/ or loopback http:\/\//
    )
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor allows loopback HTTP runtime URLs", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    const configPath = await writeConfig(
      tempDir,
      "loopback-runtime-url.json",
      makeOpenClawConfig({
        pluginPath: tempDir,
        runtimeUrl: "http://127.0.0.1:8081",
        tokenPath
      })
    )

    const result = await collectDoctorResults({
      configPath,
      target: "openclaw"
    })

    assert.equal(result.overallStatus, "ok")
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor runtime check succeeds against a healthy runtime", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")

    await withRuntimeServer((req, res) => {
      if (
        req.url === "/v1/runtime/wallet/balance" &&
        req.headers.authorization === "Bearer froglet-test-token"
      ) {
        res.writeHead(200, { "content-type": "application/json" })
        res.end(JSON.stringify({ backend: "lightning", balance_sats: 21 }))
        return
      }
      res.writeHead(401, { "content-type": "application/json" })
      res.end(JSON.stringify({ error: "unauthorized" }))
    }, async (runtimeUrl) => {
      const configPath = await writeConfig(
        tempDir,
        "healthy-runtime.json",
        makeOpenClawConfig({
          pluginPath: tempDir,
          runtimeUrl,
          tokenPath
        })
      )

      const result = await collectDoctorResults({
        configPath,
        target: "openclaw",
        checkRuntime: true
      })

      assert.equal(result.overallStatus, "ok")
      assert.equal(
        result.checks.find((check) => check.id === "runtime_health").status,
        "ok"
      )
    })
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor runtime check fails cleanly on 401 responses", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")

    await withRuntimeServer((req, res) => {
      res.writeHead(401, { "content-type": "application/json" })
      res.end(JSON.stringify({ error: "unauthorized" }))
    }, async (runtimeUrl) => {
      const configPath = await writeConfig(
        tempDir,
        "runtime-401.json",
        makeOpenClawConfig({
          pluginPath: tempDir,
          runtimeUrl,
          tokenPath
        })
      )

      const result = await collectDoctorResults({
        configPath,
        target: "openclaw",
        checkRuntime: true
      })

      assert.equal(result.overallStatus, "error")
      assert.match(
        result.checks.find((check) => check.id === "runtime_health").summary,
        /failed with 401/
      )
    })
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor runtime check fails cleanly on 404 responses", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")

    await withRuntimeServer((req, res) => {
      res.writeHead(404, { "content-type": "application/json" })
      res.end(JSON.stringify({ error: "not found" }))
    }, async (runtimeUrl) => {
      const configPath = await writeConfig(
        tempDir,
        "runtime-404.json",
        makeOpenClawConfig({
          pluginPath: tempDir,
          runtimeUrl,
          tokenPath
        })
      )

      const result = await collectDoctorResults({
        configPath,
        target: "openclaw",
        checkRuntime: true
      })

      assert.equal(result.overallStatus, "error")
      assert.match(
        result.checks.find((check) => check.id === "runtime_health").summary,
        /failed with 404/
      )
    })
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor runtime check fails cleanly on timeouts", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const tokenPath = path.join(tempDir, "auth.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")

    await withRuntimeServer(async (_req, _res) => {
      await new Promise((resolve) => setTimeout(resolve, 50))
    }, async (runtimeUrl) => {
      const configPath = await writeConfig(
        tempDir,
        "runtime-timeout.json",
        makeOpenClawConfig({
          pluginPath: tempDir,
          runtimeUrl,
          tokenPath,
          overrides: { requestTimeoutMs: 1000 }
        })
      )

      const result = await collectDoctorResults({
        configPath,
        target: "openclaw",
        checkRuntime: true
      })

      assert.equal(result.overallStatus, "error")
      assert.match(
        result.checks.find((check) => check.id === "runtime_health").summary,
        /timed out/
      )
    })
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})

test("doctor CLI exits zero for NemoClaw warnings and one for real errors", async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-doctor-"))
  try {
    const goodConfig = await writeConfig(tempDir, "nemoclaw.json", makeNemoclawConfig())
    const stdout = { chunks: [], write(chunk) { this.chunks.push(String(chunk)) } }
    const stderr = { chunks: [], write(chunk) { this.chunks.push(String(chunk)) } }

    const warningCode = await runDoctorCli(
      ["--config", goodConfig, "--target", "nemoclaw"],
      stdout,
      stderr
    )
    assert.equal(warningCode, 0)
    assert.match(stdout.chunks.join(""), /overall_status=warning/)

    const badConfig = await writeConfig(
      tempDir,
      "bad.json",
      makeOpenClawConfig({
        pluginPath: tempDir,
        runtimeUrl: "http://10.0.0.5:8081",
        tokenPath: path.join(tempDir, "auth.token")
      })
    )
    const errorCode = await runDoctorCli(
      ["--config", badConfig, "--target", "openclaw"],
      { chunks: [], write(chunk) { this.chunks.push(String(chunk)) } },
      { chunks: [], write(chunk) { this.chunks.push(String(chunk)) } }
    )
    assert.equal(errorCode, 1)
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
})
