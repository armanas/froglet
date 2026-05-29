# Provider onboarding

Two paths, depending on who's typing:

- **Agent-driven**: an LLM (Claude Code, Codex, etc.) with the Froglet
  MCP attached calls `marketplace_publish` once. The user gets a live
  marketplace offer in seconds.
- **Human-driven**: `froglet-node init` + `froglet-node publish` in a
  shell.

Both surfaces run the same `froglet-publish-engine` pipeline: build →
host → sign → register → verify. One source of truth.

This document leads with the easy paths. The four DNS-free hosting
backends (Tor, managed subdomain, PaaS, key-only) are documented in the
[Hosting backends](#hosting-backends) appendix; you almost never need
to think about them directly.

---

## The agent-driven flow (one MCP call)

```
User: "Publish a Froglet service that translates English to Spanish."

Claude (with froglet MCP):
  Calls marketplace_publish:
    name: "translator-en-es"
    source_inline: "<generated handler.py>"
    hosting: { kind: "tor" }
    summary: "Translate EN→ES"

→ Returns:
    provider_id:            c9ecac3a…
    public_url:             http://abc123…onion
    marketplace_offer_url:  https://marketplace.froglet.dev/v1/offers/…
    invoke_command:         froglet-node invoke translator-en-es '{}'

Claude: "Your service is live at marketplace.froglet.dev/v1/offers/…
        Call it with: froglet-node invoke …"
```

That's the whole flow. Behind the scenes the MCP handler shells out to
`froglet-node publish --json`, which scaffolds manifests in a temp
directory, builds the artifact, posts to the local daemon's
`/v1/provider/artifacts/publish` (daemon signs + persists), POSTs
`/v1/registrations` on the marketplace, and polls
`/v1/providers/<id>` until the indexer projects the offer. Failure
modes are typed; the LLM gets back a structured error it can act on
("set FROGLET_NETWORK_MODE=tor and retry") rather than a stack trace.

**MCP input shape (Phase 1A scope):**

```json
{
  "action": "marketplace_publish",
  "name": "<lowercase-hyphenated-name>",
  "source_inline": "<full Python source for handler.py>",
  "hosting": { "kind": "local|tor|self", "url": "<required if self>" },
  "settlement": { "method": "none" },
  "marketplace_url": "https://marketplace.froglet.dev"
}
```

Runtime is Python `inline_source` only in Phase 1A. WASM + OCI ship in
Phase 1B. Settlement = `"none"` (free) or `"lightning"` (paid, requires a
Lightning backend on the node); Stripe + x402 are not yet on the publish path.

---

## The human-driven flow

For when you're typing directly, not driving through an LLM:

```bash
# 1. Install the daemon + CLI (one binary; init runs the CLI mode):
curl -fsSL https://froglet.dev/agent | bash

# 2. Scaffold a new service:
froglet-node init my-translator
cd my-translator

# 3. Edit handler.py to do real work:
$EDITOR handler.py

# 4. Publish:
froglet-node publish --host tor
```

`froglet-node init` writes four files: `froglet.toml` (project),
`froglet-service.toml` (per-service, v3 schema), `handler.py` (Python
skeleton), and `.gitignore`. See [docs/MANIFEST.md](./MANIFEST.md) for
the manifest spec.

`froglet-node publish` reads both manifests, builds the artifact, and
runs the same engine pipeline the MCP tool does. It accepts:

- `--host local|tor|self` to override the manifest's `[hosting] default`
- `--marketplace URL` to override the manifest's marketplace
- `--json` to emit machine-readable output

Other useful subcommands:

- `froglet-node build` — validate manifests + build the artifact, no
  publish. Quick sanity check.
- `froglet-node whoami` — print identity + daemon transport info
- `froglet-node print-identity` / `sign-message` — identity utilities

---

## Hosting backends

The publish pipeline supports five hosting choices. Phase 1A ships
three; Phase 1B adds the other two. The right answer for most users is
**Tor**.

### Tor (`--host tor`, Phase 1A) — default

The daemon spawns a Tor hidden service; the `.onion` URL is your public
address. No DNS, no TLS, no port-forwarding, works behind any NAT.

Requires:

```bash
FROGLET_NETWORK_MODE=tor froglet-node serve   # or "dual" for clearnet+tor
```

Strongest no-DNS posture. Tradeoff: higher latency (~500ms+), clients
must speak Tor.

### Local (`--host local`, Phase 1A)

Private development. Service binds to `127.0.0.1:8080`, never registers
with the marketplace. Use during development; promote to Tor or
self-hosted when ready.

### Self-hosted (`--host self`, Phase 1A)

You deploy the daemon somewhere with a public HTTPS URL (Fly, Render,
Railway, your VPS) and supply the URL in the manifest. The engine
trusts the URL after a basic shape check; the marketplace's
`/v1/registrations` does the real validation (must serve `/v1/feed`
with a signed descriptor + offer matching your provider key).

```toml
[hosting]
default = "self"

[hosting.self]
url = "https://my-translator.fly.dev"
```

### Managed (`--host managed`, Phase 1B)

Marketplace allocates `<slug>.providers.froglet.dev` and creates the
Cloudflare DNS record. You run Caddy or a similar proxy for TLS. Needs
a public IP. Lands in Phase 1B.

### Fly (`--host fly`, Phase 1B)

Engine wraps `flyctl deploy` to deploy your service to Fly.io,
then registers the `*.fly.dev` URL with the marketplace. Lands in
Phase 1B.

---

## Pricing and currency

The `[price]` section in `froglet-service.toml` accepts two fields:

- `sats` — the price integer (default `0` = free)
- `currency` — the unit for that integer (default `"sat"`)

**Allowed values for `currency`:**

| Value | Unit | Settled via |
|---|---|---|
| `"sat"` (default) | satoshis | Lightning rail |
| `"usd"` | US cents (e.g. `500` = $5.00) | Stripe rail |

```toml
# Lightning-priced: 1000 satoshis (~$0.40 at time of writing)
[price]
sats = 1000
currency = "sat"   # or omit — "sat" is the default

# Stripe-priced: $5.00
[price]
sats = 500
currency = "usd"
```

Publishing with `currency = "usd"` requires the node to have a Stripe payment
backend configured. Attempting to publish a USD-priced service on a
Lightning-only node returns a clear error at publish time — the node rejects
the offer before signing it.

The `currency` field lives only in the manifest (provider configuration). The
signed offer and receipt carry the raw integer; each settlement rail
interprets it according to its own rules, which is also why the field name
`price.sats` is misleading on the Stripe rail — treat `sats` as "price units"
and let `currency` disambiguate.

---

## Identity attestation (optional)

Per [IDENTITY_ATTESTATION.md](./IDENTITY_ATTESTATION.md): "attestations
are always optional, always user-initiated, and never block a
kernel-level deal flow." The publish flow does not require any
attestation. Consumers can filter discovery by `attested=true` if they
want stronger identity guarantees; unattested providers are still
first-class in the deal flow.

When you eventually want one:

- **DNS attestation** requires owning a zone (one path that does need
  DNS).
- **OAuth attestation** (GitHub today; pattern extends to Google,
  GitLab, Gitea, Microsoft) needs only a GitHub account. Spec is
  complete; the issuance service ships in v0.2.

---

## What's deliberately not here

This document used to enumerate four "DNS-free paths" (A/B/D/E) as the
top-level surface. They're now backend implementation details of a
single `marketplace_publish` call — you tell the engine `hosting.kind`
and it picks the right backend. The four-path framing is preserved in
git history at `docs/PROVIDER_ONBOARDING.md@a5799dc` for reference.

If you're an operator running your own marketplace, the engine talks
to any `froglet-node` daemon over HTTP (`FROGLET_DAEMON_URL`) and
registers against any marketplace (`marketplace_url` in the manifest).
There is no first-party lock-in.
