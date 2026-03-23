import assert from "node:assert/strict"
import { mkdtemp, rm, writeFile } from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import test from "node:test"

import { createProject, publishArtifact, publishProject } from "../lib/froglet-client.js"

async function withTokenPath(fn) {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "froglet-client-"))
  try {
    const tokenPath = path.join(tempDir, "froglet.token")
    await writeFile(tokenPath, "froglet-test-token\n", "utf8")
    await fn(tokenPath)
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
}

test("createProject accepts HTTP 201 responses", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    global.fetch = async () =>
      new Response(JSON.stringify({ project: { project_id: "lol" } }), {
        status: 201,
        headers: { "Content-Type": "application/json" }
      })
    try {
      const response = await createProject({
        baseUrl: "http://127.0.0.1:9191",
        authTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        request: { name: "lol", result_json: "lol" }
      })
      assert.equal(response.project.project_id, "lol")
    } finally {
      global.fetch = previousFetch
    }
  })
})

test("publish endpoints accept HTTP 201 responses", async () => {
  await withTokenPath(async (tokenPath) => {
    const previousFetch = global.fetch
    let callCount = 0
    global.fetch = async () => {
      callCount += 1
      return new Response(JSON.stringify({ status: "passed" }), {
        status: 201,
        headers: { "Content-Type": "application/json" }
      })
    }
    try {
      const projectResponse = await publishProject({
        baseUrl: "http://127.0.0.1:9191",
        authTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        projectId: "lol"
      })
      const artifactResponse = await publishArtifact({
        baseUrl: "http://127.0.0.1:9191",
        authTokenPath: tokenPath,
        requestTimeoutMs: 1000,
        request: {
          service_id: "lol",
          offer_id: "lol",
          execution_kind: "wasm_inline",
          wasm_module_hex: "0061736d01000000"
        }
      })
      assert.equal(projectResponse.status, "passed")
      assert.equal(artifactResponse.status, "passed")
      assert.equal(callCount, 2)
    } finally {
      global.fetch = previousFetch
    }
  })
})
