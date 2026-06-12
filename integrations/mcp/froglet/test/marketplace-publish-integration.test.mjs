// Integration tests for marketplace_publish.
//
// These verify the JS handler end-to-end by pointing FROGLET_NODE_BIN at
// a stub script that records its argv + cwd + cwd contents, then emits
// canned JSON on stdout. This proves:
//
//   1. The right manifests + handler.py are materialised in the temp dir
//   2. The CLI is invoked with the right `publish --json [--host X] [--marketplace URL]` flags
//   3. The stdout JSON is parsed and returned as a structured result
//   4. Errors from non-zero exit are wrapped with a useful message
//
// A real-daemon end-to-end test lives in the Rust publish-engine crate
// behind `#[ignore]`; that one talks to marketplace.froglet.dev. This
// CI-friendly stub test does not need network or a daemon.

import { describe, it, before, after } from "node:test"
import assert from "node:assert/strict"
import { execFile as execFileCb } from "node:child_process"
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"

import { runMarketplacePublish } from "../../../shared/froglet-lib/marketplace-publish.js"

const execFileAsync = promisify(execFileCb)
const __dirname = dirname(fileURLToPath(import.meta.url))

let stubDir
let stubBinary
const publicUrlDeps = {
  marketplaceUrl: {
    lookup: async () => [{ address: "93.184.216.34", family: 4 }]
  },
  selfUrl: {
    lookup: async () => [{ address: "93.184.216.34", family: 4 }]
  }
}

// Build a stub `froglet-node` script that, when invoked, writes its
// argv + cwd + cwd file contents to a file at $FROGLET_STUB_LOG and
// then emits the JSON in $FROGLET_STUB_RESPONSE to stdout. Exit code
// is read from $FROGLET_STUB_EXIT (default 0).
//
// We use a POSIX shell script (no node interpreter required, no PATH
// games). The tests set the env vars per case.
async function writeStubBinary() {
  stubDir = await mkdtemp(join(tmpdir(), "froglet-publish-stub-"))
  stubBinary = join(stubDir, "froglet-node")
  const script = `#!/usr/bin/env bash
set -u
LOG="\${FROGLET_STUB_LOG:-/dev/null}"
{
  echo "argv: $@"
  echo "cwd: $(pwd)"
  echo "---files---"
  for f in froglet.toml froglet-service.toml handler.py; do
    if [ -f "$f" ]; then
      echo "=== $f ==="
      cat "$f"
    fi
  done
} >>"$LOG" 2>&1

if [ -n "\${FROGLET_STUB_STDERR:-}" ]; then
  echo "$FROGLET_STUB_STDERR" >&2
fi

if [ -n "\${FROGLET_STUB_RESPONSE:-}" ]; then
  echo "$FROGLET_STUB_RESPONSE"
fi
exit "\${FROGLET_STUB_EXIT:-0}"
`
  await writeFile(stubBinary, script, { mode: 0o755 })
}

before(async () => {
  await writeStubBinary()
})

after(async () => {
  if (stubDir) {
    await rm(stubDir, { recursive: true, force: true })
  }
})

describe("marketplace_publish: stub-binary integration", () => {
  it("materialises manifests + handler.py and parses canned JSON output", async () => {
    const logFile = join(stubDir, `log-${Date.now()}-${Math.random()}.txt`)
    const canned = {
      provider_id: "deadbeef".repeat(8),
      public_url: "http://abc123.onion",
      offer_hash: "0123abcd".repeat(8),
      marketplace_offer_url: "https://marketplace.froglet.dev/v1/offers/0123abcd",
      invoke_command: "froglet-node invoke translator '{}'",
      status_url: "https://marketplace.froglet.dev/v1/providers/deadbeef",
      warnings: []
    }
    process.env.FROGLET_NODE_BIN = stubBinary
    process.env.FROGLET_STUB_LOG = logFile
    process.env.FROGLET_STUB_RESPONSE = JSON.stringify(canned)
    process.env.FROGLET_STUB_EXIT = "0"
    delete process.env.FROGLET_STUB_STDERR

    const result = await runMarketplacePublish(
      {
        name: "translator",
        summary: "EN→ES translator",
        source_inline: "def handle(p):\n    return p\n",
        hosting: { kind: "tor" },
        marketplace_url: "https://marketplace.froglet.dev"
      },
      { _deps: publicUrlDeps }
    )

    assert.deepEqual(result, canned)

    // Inspect what the stub recorded: argv must include the publish
    // subcommand + --json, plus the host and marketplace flags.
    const log = await readFile(logFile, "utf8")
    assert.match(log, /argv: publish --json --host tor --marketplace https:\/\/marketplace\.froglet\.dev/)

    // The temp dir must have contained all three files with the right
    // shape. (cwd is captured in the log too; we just check contents.)
    assert.match(log, /=== froglet\.toml ===\s+schema_version = "froglet\/v1"/)
    assert.match(log, /=== froglet-service\.toml ===\s+schema_version = "froglet-service\/v3"/)
    assert.match(log, /service_id = "translator"/)
    assert.match(log, /summary = "EN→ES translator"/)
    assert.match(log, /default = "tor"/)
    assert.match(log, /=== handler\.py ===\s+def handle/)
  })

  it("passes --host local when hosting.kind is local", async () => {
    const logFile = join(stubDir, `log-local-${Date.now()}.txt`)
    process.env.FROGLET_NODE_BIN = stubBinary
    process.env.FROGLET_STUB_LOG = logFile
    process.env.FROGLET_STUB_RESPONSE = JSON.stringify({
      provider_id: "local",
      public_url: "http://127.0.0.1:8080",
      offer_hash: "h",
      invoke_command: "x",
      warnings: []
    })
    process.env.FROGLET_STUB_EXIT = "0"

    await runMarketplacePublish(
      {
        name: "local-svc",
        source_inline: "x = 1\n",
        hosting: { kind: "local" }
      },
      { _deps: publicUrlDeps }
    )

    const log = await readFile(logFile, "utf8")
    assert.match(log, /argv: publish --json --host local --marketplace/)
  })

  it("writes [hosting.self] url when hosting.kind = self", async () => {
    const logFile = join(stubDir, `log-self-${Date.now()}.txt`)
    process.env.FROGLET_NODE_BIN = stubBinary
    process.env.FROGLET_STUB_LOG = logFile
    process.env.FROGLET_STUB_RESPONSE = JSON.stringify({
      provider_id: "x",
      public_url: "https://my-host.fly.dev",
      offer_hash: "h",
      invoke_command: "x",
      warnings: []
    })
    process.env.FROGLET_STUB_EXIT = "0"

    await runMarketplacePublish(
      {
        name: "self-svc",
        source_inline: "x = 1\n",
        hosting: { kind: "self", url: "https://my-host.fly.dev" }
      },
      { _deps: publicUrlDeps }
    )

    const log = await readFile(logFile, "utf8")
    assert.match(log, /argv: publish --json --host self --marketplace/)
    assert.match(log, /\[hosting\.self\]\s+url = "https:\/\/my-host\.fly\.dev"/)
  })

  it("wraps non-zero exit codes with stderr detail", async () => {
    process.env.FROGLET_NODE_BIN = stubBinary
    process.env.FROGLET_STUB_LOG = join(stubDir, "log-err.txt")
    process.env.FROGLET_STUB_RESPONSE = ""
    process.env.FROGLET_STUB_STDERR = "manifest: bad entrypoint"
    process.env.FROGLET_STUB_EXIT = "3"

    await assert.rejects(
      runMarketplacePublish(
        {
          name: "broken",
          source_inline: "x = 1\n",
          hosting: { kind: "tor" }
        },
        { _deps: publicUrlDeps }
      ),
      (e) => {
        assert.match(e.message, /froglet-node publish failed/)
        assert.match(e.message, /manifest: bad entrypoint/)
        return true
      }
    )
  })

  it("cleans up the temp dir even on failure", async () => {
    // The temp dir is created inside the handler. We can't observe it
    // from outside (it's randomly-named) but we can assert that the
    // os.tmpdir() doesn't grow per call by listing before/after.
    const { readdir } = await import("node:fs/promises")
    const before = (await readdir(tmpdir())).filter((n) => n.startsWith("froglet-publish-"))
    process.env.FROGLET_NODE_BIN = stubBinary
    process.env.FROGLET_STUB_LOG = "/dev/null"
    process.env.FROGLET_STUB_RESPONSE = "{}"
    process.env.FROGLET_STUB_EXIT = "1"
    process.env.FROGLET_STUB_STDERR = "boom"

    await assert.rejects(
      runMarketplacePublish(
        {
          name: "cleanup-test",
          source_inline: "x = 1\n",
          hosting: { kind: "tor" }
        },
        { _deps: publicUrlDeps }
      )
    )
    const after = (await readdir(tmpdir())).filter((n) => n.startsWith("froglet-publish-"))
    assert.deepEqual(
      after,
      before,
      `expected temp dirs to be cleaned up; new dirs: ${JSON.stringify(after.filter((d) => !before.includes(d)))}`
    )
  })
})
