import { execFile as execFileCallback, spawn } from "node:child_process"
import { promisify } from "node:util"
import { access, mkdir, readFile, rm, writeFile } from "node:fs/promises"
import path from "node:path"

import {
  frogletStatus,
  getLocalService,
  publishArtifact,
} from "../../../integrations/shared/froglet-lib/froglet-client.js"
import { parseCliArgs, readJson, repoRoot, sleep, writeJson } from "./common.mjs"

const execFile = promisify(execFileCallback)

async function runCommand(command, args, options = {}) {
  try {
    const result = await execFile(command, args, {
      cwd: options.cwd,
      env: options.env,
      maxBuffer: 8 * 1024 * 1024,
    })
    return {
      stdout: result.stdout.trim(),
      stderr: result.stderr.trim(),
    }
  } catch (error) {
    if (options.allowFailure) {
      return {
        stdout: String(error.stdout ?? "").trim(),
        stderr: String(error.stderr ?? "").trim(),
        failed: true,
      }
    }
    throw new Error(
      `${command} ${args.join(" ")} failed: ${String(error.stderr ?? error.message ?? error)}`
    )
  }
}

async function fileExists(filePath) {
  try {
    await access(filePath)
    return true
  } catch {
    return false
  }
}

async function ensurePublishArtifact(context, request) {
  try {
    return await publishArtifact({
      ...context,
      request,
    })
  } catch (error) {
    const message = String(error.message)
    if (!message.includes("409") && !message.includes("already exists")) {
      throw error
    }
    return {
      service: {
        service_id: request.service_id,
      },
    }
  }
}

async function localService(context, serviceId) {
  const response = await getLocalService({
    ...context,
    serviceId,
  })
  return response.service
}

let validWasmHexCache = null

async function validWasmHex() {
  if (validWasmHexCache !== null) {
    return validWasmHexCache
  }
  validWasmHexCache = (await readFile(
    path.join(repoRoot, "integrations", "openclaw", "froglet", "test", "fixtures", "valid-wasm.hex"),
    "utf8"
  )).trim()
  return validWasmHexCache
}

async function seedInlinePythonService(context, request) {
  await ensurePublishArtifact(context, {
    offer_id: request.service_id,
    runtime: "python",
    package_kind: "inline_source",
    entrypoint_kind: "handler",
    entrypoint: "handler",
    contract_version: "froglet.python.handler_json.v1",
    ...request,
  })
  if (request.publication_state === "hidden") {
    return {
      service_id: request.service_id,
      offer_id: request.service_id,
      publication_state: "hidden",
      price_sats: request.price_sats ?? 0,
    }
  }
  return localService(context, request.service_id)
}

async function seedInlineWasmService(context, request) {
  await ensurePublishArtifact(context, {
    offer_id: request.service_id,
    runtime: "wasm",
    package_kind: "inline_module",
    entrypoint_kind: "handler",
    entrypoint: "run",
    contract_version: "froglet.wasm.run_json.v1",
    wasm_module_hex: await validWasmHex(),
    ...request,
  })
  if (request.publication_state === "hidden") {
    return {
      service_id: request.service_id,
      offer_id: request.service_id,
      publication_state: "hidden",
      price_sats: request.price_sats ?? 0,
    }
  }
  return localService(context, request.service_id)
}

async function seedProviderFree(context, prefix) {
  const services = {}

  services.free_static = await seedInlinePythonService(context, {
    service_id: `${prefix}-free-static`,
    summary: "Free static JSON service",
    inline_source:
      "def handler(event, context):\n    return {\"message\": \"pong\", \"provider\": \"free\"}\n",
    price_sats: 0,
    publication_state: "active",
  })

  services.free_python_inline = await seedInlinePythonService(context, {
    service_id: `${prefix}-free-python`,
    summary: "Free inline Python echo service",
    inline_source:
      "def handler(event, context):\n    return {\"provider\": \"free\", \"input\": event}\n",
    price_sats: 0,
    publication_state: "active",
  })

  services.wat_project = await seedInlineWasmService(context, {
    service_id: `${prefix}-wat-hello`,
    summary: "WAT project fixture",
    price_sats: 0,
    publication_state: "active",
  })

  services.hidden = await seedInlinePythonService(context, {
    service_id: `${prefix}-hidden`,
    summary: "Hidden inline Python fixture",
    inline_source:
      "def handler(event, context):\n    return {\"hidden\": True, \"input\": event}\n",
    price_sats: 0,
    publication_state: "hidden",
  })

  services.data_echo = await seedInlinePythonService(context, {
    service_id: `${prefix}-data-echo`,
    summary: "Data-style echo service",
    inline_source:
      "def handler(event, context):\n    return event\n",
    price_sats: 0,
    publication_state: "active",
  })

  services.shared_collision = await seedInlinePythonService(context, {
    service_id: `${prefix}-shared`,
    summary: "Duplicate service id on provider-free",
    inline_source:
      "def handler(event, context):\n    return {\"provider\": \"free\", \"duplicate\": True}\n",
    price_sats: 0,
    publication_state: "active",
  })

  return services
}

async function ensureLocalRegistryImage(prefix) {
  const workingDir = path.join(repoRoot, "_tmp", "gcp-harness", "oci-container")
  await mkdir(workingDir, { recursive: true })
  await writeFile(
    path.join(workingDir, "main.py"),
    [
      "import json",
      "import os",
      "import sys",
      "",
      "payload = json.load(sys.stdin)",
      "context = json.loads(os.environ.get(\"FROGLET_CONTEXT\", \"{}\"))",
      "json.dump({\"via\": \"oci-container\", \"input\": payload, \"context\": context}, sys.stdout, separators=(\",\", \":\"))",
      "",
    ].join("\n"),
    "utf8"
  )
  await writeFile(
    path.join(workingDir, "Dockerfile"),
    [
      "FROM python:3.11-alpine",
      "WORKDIR /app",
      "COPY main.py /app/main.py",
      "ENTRYPOINT [\"python\", \"/app/main.py\"]",
      "",
    ].join("\n"),
    "utf8"
  )

  await runCommand("docker", ["rm", "-f", "froglet-harness-registry"], { allowFailure: true })
  await runCommand("docker", [
    "run",
    "-d",
    "--restart",
    "unless-stopped",
    "--name",
    "froglet-harness-registry",
    "-p",
    "127.0.0.1:5000:5000",
    "registry:2",
  ])

  const image = `127.0.0.1:5000/froglet/${prefix}-echo-json:latest`
  await runCommand("docker", ["build", "-t", image, workingDir], { cwd: workingDir })
  await runCommand("docker", ["push", image])
  const digestRef = (
    await runCommand("docker", ["image", "inspect", image, "--format", "{{index .RepoDigests 0}}"])
  ).stdout
  const digest = digestRef.split("@sha256:")[1]
  if (!digest) {
    throw new Error(`failed to resolve pushed image digest from ${digestRef}`)
  }
  return {
    reference: image,
    digest,
  }
}

async function ensureWasmFixture() {
  const fixtureScript = path.join(repoRoot, "tests", "e2e", "gcp_harness", "serve-oci-wasm-fixture.mjs")
  const infoPath = path.join(repoRoot, "_tmp", "gcp-harness", "oci-wasm-fixture.json")
  const pidPath = path.join(repoRoot, "_tmp", "gcp-harness", "oci-wasm-fixture.pid")
  await mkdir(path.dirname(infoPath), { recursive: true })

  if (await fileExists(pidPath)) {
    const pid = Number.parseInt((await readFile(pidPath, "utf8")).trim(), 10)
    if (Number.isInteger(pid)) {
      try {
        process.kill(pid, 0)
      } catch {
        await rm(pidPath, { force: true })
      }
    }
  }

  if (!(await fileExists(pidPath))) {
    const child = spawn(
      process.execPath,
      [
        fixtureScript,
        "--listen",
        "127.0.0.1:5001",
        "--module-hex-path",
        path.join(repoRoot, "integrations", "openclaw", "froglet", "test", "fixtures", "valid-wasm.hex"),
        "--out",
        infoPath,
      ],
      {
        detached: true,
        stdio: "ignore",
      }
    )
    child.unref()
    await writeFile(pidPath, `${child.pid}\n`, "utf8")
  }

  for (let attempt = 0; attempt < 50; attempt += 1) {
    if (await fileExists(infoPath)) {
      return JSON.parse(await readFile(infoPath, "utf8"))
    }
    await sleep(200)
  }
  throw new Error("timed out waiting for OCI Wasm fixture info")
}

async function seedProviderPaid(context, prefix) {
  const services = {}
  const ociContainer = await ensureLocalRegistryImage(prefix)
  const ociWasm = await ensureWasmFixture()

  services.priced = await seedInlinePythonService(context, {
    service_id: `${prefix}-priced`,
    summary: "Priced inline Python service",
    inline_source:
      "def handler(event, context):\n    return {\"priced\": True, \"input\": event}\n",
    price_sats: 25,
    publication_state: "active",
  })

  services.async_echo = await seedInlinePythonService(context, {
    service_id: `${prefix}-async`,
    summary: "Async inline Python service",
    inline_source:
      "import time\n\n" +
      "def handler(event, context):\n" +
      "    time.sleep(float(event.get(\"delay_ms\", 1000)) / 1000.0)\n" +
      "    return {\"async\": True, \"echo\": event}\n",
    price_sats: 0,
    publication_state: "active",
    mode: "async",
  })

  await ensurePublishArtifact(context, {
    service_id: `${prefix}-oci-wasm`,
    offer_id: `${prefix}-oci-wasm`,
    runtime: "wasm",
    package_kind: "oci_image",
    oci_reference: ociWasm.oci_reference,
    oci_digest: ociWasm.oci_digest,
    price_sats: 0,
    publication_state: "active",
    summary: "OCI-backed Wasm service",
  })
  services.oci_wasm = await localService(context, `${prefix}-oci-wasm`)

  await ensurePublishArtifact(context, {
    service_id: `${prefix}-oci-container`,
    offer_id: `${prefix}-oci-container`,
    runtime: "container",
    package_kind: "oci_image",
    oci_reference: ociContainer.reference,
    oci_digest: ociContainer.digest,
    price_sats: 0,
    publication_state: "active",
    summary: "OCI-backed container service",
  })
  services.oci_container = await localService(context, `${prefix}-oci-container`)

  services.shared_collision = await seedInlinePythonService(context, {
    service_id: `${prefix}-shared`,
    summary: "Duplicate service id on provider-paid",
    inline_source:
      "def handler(event, context):\n    return {\"provider\": \"paid\", \"duplicate\": True}\n",
    price_sats: 0,
    publication_state: "active",
  })

  return {
    services,
    fixtures: {
      oci_container: ociContainer,
      oci_wasm: {
        reference: ociWasm.oci_reference,
        digest: ociWasm.oci_digest,
      },
    },
  }
}

async function main() {
  const { values } = parseCliArgs({
    inventory: { type: "string", short: "i" },
    role: { type: "string", short: "r" },
    out: { type: "string", short: "o" },
  })
  if (!values.inventory || !values.role || !values.out) {
    throw new Error("--inventory, --role, and --out are required")
  }

  const inventory = await readJson(values.inventory)
  const role = inventory.roles?.[values.role]
  if (!role) {
    throw new Error(`missing role ${values.role} in inventory`)
  }
  const context = {
    providerUrl: role.provider_local_url,
    runtimeUrl: role.runtime_url,
    providerAuthTokenPath: role.token_paths.provider_control,
    runtimeAuthTokenPath: role.token_paths.runtime_auth,
    requestTimeoutMs: 20_000,
  }
  const status = await frogletStatus(context)
  const prefix = (inventory.run_id ?? "gcp").replace(/[^a-zA-Z0-9-]/g, "-").slice(0, 12)

  let seeded
  if (values.role === "froglet-provider-free") {
    seeded = {
      provider_id: status.node_id,
      provider_public_url: role.provider_public_url,
      services: await seedProviderFree(context, prefix),
    }
  } else if (values.role === "froglet-provider-paid") {
    seeded = {
      provider_id: status.node_id,
      provider_public_url: role.provider_public_url,
      ...(await seedProviderPaid(context, prefix)),
    }
  } else {
    throw new Error(`unsupported seed role ${values.role}`)
  }

  await writeJson(values.out, seeded)
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
