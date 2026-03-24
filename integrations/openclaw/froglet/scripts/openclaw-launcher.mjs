#!/usr/bin/env node

import { access, readFile } from "node:fs/promises"
import { constants as fsConstants } from "node:fs"
import os from "node:os"
import path from "node:path"
import process from "node:process"
import readline from "node:readline/promises"
import { fileURLToPath } from "node:url"
import { spawn } from "node:child_process"

const launcherPath = fileURLToPath(import.meta.url)
const sessionId = `froglet-chat-${os.hostname().replace(/[^a-zA-Z0-9_.-]/g, "-")}`

function parseEnvFile(contents) {
  const values = {}
  for (const rawLine of contents.split(/\r?\n/u)) {
    const line = rawLine.trim()
    if (line.length === 0 || line.startsWith("#")) {
      continue
    }
    const normalized = line.startsWith("export ") ? line.slice("export ".length).trim() : line
    const separatorIndex = normalized.indexOf("=")
    if (separatorIndex <= 0) {
      continue
    }
    const key = normalized.slice(0, separatorIndex).trim()
    let value = normalized.slice(separatorIndex + 1).trim()
    if (
      (value.startsWith("\"") && value.endsWith("\"")) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1)
    }
    if (key.length > 0) {
      values[key] = value
    }
  }
  return values
}

async function loadManagedEnv() {
  const candidateFiles = [
    process.env.FROGLET_OPENCLAW_ENV_FILE,
    path.join(os.homedir(), ".config", "froglet", "openclaw.env"),
    path.join(path.dirname(launcherPath), "openclaw.env")
  ].filter(Boolean)

  const loaded = {}
  for (const candidate of candidateFiles) {
    try {
      const contents = await readFile(candidate, "utf8")
      Object.assign(loaded, parseEnvFile(contents))
    } catch (error) {
      if (error?.code !== "ENOENT") {
        throw error
      }
    }
  }
  return {
    ...loaded,
    ...process.env
  }
}

async function fileExecutable(targetPath) {
  try {
    await access(targetPath, fsConstants.X_OK)
    return true
  } catch {
    return false
  }
}

async function resolveUpstreamBinary(env) {
  const candidates = [
    env.FROGLET_OPENCLAW_UPSTREAM_BIN,
    env.OPENCLAW_UPSTREAM_BIN,
    path.join(path.dirname(launcherPath), "openclaw.real"),
    "/usr/bin/openclaw",
    "/usr/local/bin/openclaw",
    "/opt/homebrew/bin/openclaw"
  ].filter(Boolean)

  for (const candidate of candidates) {
    const resolved = path.resolve(candidate)
    if (resolved === path.resolve(launcherPath)) {
      continue
    }
    if (await fileExecutable(resolved)) {
      return resolved
    }
  }

  throw new Error(
    "could not locate the upstream OpenClaw binary; set FROGLET_OPENCLAW_UPSTREAM_BIN"
  )
}

async function runUpstream(upstreamBinary, args, env) {
  const child = spawn(upstreamBinary, args, {
    stdio: "inherit",
    env
  })
  const exitCode = await new Promise((resolve, reject) => {
    child.on("error", reject)
    child.on("exit", (code, signal) => {
      if (signal) {
        reject(new Error(`OpenClaw exited from signal ${signal}`))
      } else {
        resolve(code ?? 0)
      }
    })
  })
  return exitCode
}

async function runLocalChat(upstreamBinary, env) {
  const timeoutSecs = env.FROGLET_OPENCLAW_TIMEOUT_SECS ?? "120"
  const agentId = env.FROGLET_OPENCLAW_AGENT ?? "main"
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout
  })

  process.stdout.write(`Local Froglet chat on ${os.hostname()}. Type /exit to quit.\n`)
  try {
    while (true) {
      let answer
      try {
        answer = await rl.question("you> ")
      } catch (error) {
        if (error?.code === "ERR_USE_AFTER_CLOSE") {
          break
        }
        throw error
      }
      const prompt = answer.trim()
      if (prompt.length === 0) {
        continue
      }
      if (prompt === "/exit") {
        break
      }
      const args = [
        "agent",
        "--agent",
        agentId,
        "--local",
        "--timeout",
        timeoutSecs,
        "-m",
        prompt,
        "--session-id",
        sessionId
      ]
      const exitCode = await runUpstream(upstreamBinary, args, env)
      if (exitCode !== 0) {
        process.stdout.write(`\n[openclaw agent exited ${exitCode}]\n`)
      }
    }
  } finally {
    rl.close()
  }
  return 0
}

const launcherEnv = await loadManagedEnv()
const upstreamBinary = await resolveUpstreamBinary(launcherEnv)
const args = process.argv.slice(2)
const exitCode =
  args.length === 0
    ? await runLocalChat(upstreamBinary, launcherEnv)
    : await runUpstream(upstreamBinary, args, launcherEnv)
process.exitCode = exitCode
