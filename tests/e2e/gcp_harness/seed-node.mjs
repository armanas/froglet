import { execFile as execFileCallback, spawn } from "node:child_process"
import { promisify } from "node:util"
import { access, mkdir, readFile, rm, writeFile } from "node:fs/promises"
import path from "node:path"

import {
  createProject,
  frogletStatus,
  getLocalService,
  publishArtifact,
  publishProject,
  writeProjectFile,
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

async function ensureProject(context, request) {
  try {
    return await createProject({
      ...context,
      request,
    })
  } catch (error) {
    const message = String(error.message)
    if (!message.includes("409") && !message.includes("already exists")) {
      throw error
    }
    return {
      project: {
        project_id: request.project_id ?? request.service_id,
        service_id: request.service_id ?? request.project_id,
      },
    }
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

async function seedProviderFree(context, prefix) {
  const services = {}

  await ensureProject(context, {
    project_id: `${prefix}-free-static`,
    service_id: `${prefix}-free-static`,
    name: `${prefix}-free-static`,
    summary: "Free static JSON service",
    result_json: { message: "pong", provider: "free" },
    price_sats: 0,
    publication_state: "active",
  })
  services.free_static = await localService(context, `${prefix}-free-static`)

  await ensureProject(context, {
    project_id: `${prefix}-free-python`,
    service_id: `${prefix}-free-python`,
    name: `${prefix}-free-python`,
    summary: "Free inline Python echo service",
    runtime: "python",
    package_kind: "inline_source",
    entrypoint_kind: "handler",
    entrypoint: "handler",
    contract_version: "froglet.python.handler_json.v1",
    inline_source:
      "def handler(event, context):\n    return {\"provider\": \"free\", \"input\": event}\n",
    price_sats: 0,
    publication_state: "active",
  })
  services.free_python_inline = await localService(context, `${prefix}-free-python`)

  await ensureProject(context, {
    project_id: `${prefix}-wat-hello`,
    service_id: `${prefix}-wat-hello`,
    name: `${prefix}-wat-hello`,
    summary: "WAT project fixture",
    starter: "hello_world",
    publication_state: "hidden",
  })
  await publishProject({
    ...context,
    projectId: `${prefix}-wat-hello`,
  })
  services.wat_project = await localService(context, `${prefix}-wat-hello`)

  await ensurePublishArtifact(context, {
    service_id: `${prefix}-hidden`,
    offer_id: `${prefix}-hidden`,
    summary: "Hidden inline Python fixture",
    runtime: "python",
    package_kind: "inline_source",
    entrypoint_kind: "handler",
    entrypoint: "handler",
    contract_version: "froglet.python.handler_json.v1",
    inline_source:
      "def handler(event, context):\n    return {\"hidden\": True, \"input\": event}\n",
    price_sats: 0,
    publication_state: "hidden",
  })
  services.hidden = await localService(context, `${prefix}-hidden`)

  await ensureProject(context, {
    project_id: `${prefix}-data-echo`,
    service_id: `${prefix}-data-echo`,
    name: `${prefix}-data-echo`,
    summary: "Data-style echo service",
    starter: "echo_json",
    price_sats: 0,
    publication_state: "active",
  })
  services.data_echo = await localService(context, `${prefix}-data-echo`)

  await ensureProject(context, {
    project_id: `${prefix}-shared`,
    service_id: `${prefix}-shared`,
    name: `${prefix}-shared`,
    summary: "Duplicate service id on provider-free",
    result_json: { provider: "free", duplicate: true },
    price_sats: 0,
    publication_state: "active",
  })
  services.shared_collision = await localService(context, `${prefix}-shared`)

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

  await ensureProject(context, {
    project_id: `${prefix}-priced`,
    service_id: `${prefix}-priced`,
    name: `${prefix}-priced`,
    summary: "Priced inline Python service",
    runtime: "python",
    package_kind: "inline_source",
    entrypoint_kind: "handler",
    entrypoint: "handler",
    contract_version: "froglet.python.handler_json.v1",
    inline_source:
      "def handler(event, context):\n    return {\"priced\": True, \"input\": event}\n",
    price_sats: 25,
    publication_state: "active",
  })
  services.priced = await localService(context, `${prefix}-priced`)

  await ensureProject(context, {
    project_id: `${prefix}-async`,
    service_id: `${prefix}-async`,
    name: `${prefix}-async`,
    summary: "Async inline Python service",
    runtime: "python",
    package_kind: "inline_source",
    entrypoint_kind: "handler",
    entrypoint: "handler",
    contract_version: "froglet.python.handler_json.v1",
    inline_source:
      "import time\n\n" +
      "def handler(event, context):\n" +
      "    time.sleep(float(event.get(\"delay_ms\", 1000)) / 1000.0)\n" +
      "    return {\"async\": True, \"echo\": event}\n",
    price_sats: 0,
    publication_state: "active",
    mode: "async",
  })
  services.async_echo = await localService(context, `${prefix}-async`)

  await ensurePublishArtifact(context, {
    service_id: `${prefix}-oci-wasm`,
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
    runtime: "container",
    package_kind: "oci_image",
    oci_reference: ociContainer.reference,
    oci_digest: ociContainer.digest,
    price_sats: 0,
    publication_state: "active",
    summary: "OCI-backed container service",
  })
  services.oci_container = await localService(context, `${prefix}-oci-container`)

  await ensureProject(context, {
    project_id: `${prefix}-shared`,
    service_id: `${prefix}-shared`,
    name: `${prefix}-shared`,
    summary: "Duplicate service id on provider-paid",
    result_json: { provider: "paid", duplicate: true },
    price_sats: 0,
    publication_state: "active",
  })
  services.shared_collision = await localService(context, `${prefix}-shared`)

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
    baseUrl: role.operator_url,
    authTokenPath: role.token_paths.provider_control,
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
