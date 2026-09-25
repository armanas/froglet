# Provider onboarding

Two surfaces share one implementation:

- **Agent-driven**: Claude Code, Codex, or another MCP host calls the native
  `marketplace_publish` action with an authored `project_dir`.
- **Human-driven**: `froglet-node init` plus `froglet-node publish` in a shell.

Every public publication is deliberately two-call. The first call builds the
exact package and returns a non-mutating consent summary. The second rebuilds
once and proceeds only with the exact user-approved `consent_hash`. Both
surfaces run the same `froglet-publish-engine` implementation: build exact plan
→ approval → rebuild and provider-private verification → signed immutable
revision → prepare the approved transport → exact registration → conditional
marketplace/requester canaries. Local-only publication does not open a
transport or register a listing.

---

## The agent-driven flow (plan, then exact approval)

```
User: "Publish the service in /home/me/translator through the relay."

Agent call 1:
  {"action":"marketplace_publish",
   "project_dir":"/home/me/translator",
   "host":"relay"}

→ Returns status=approval_required, consent_hash, and the exact package,
  schema, capability, limit, provider, endpoint, relay, price, and commerce
  disclosure. Nothing has been published.

Agent shows that summary to the user.

Agent call 2, only after approval:
  {"action":"marketplace_publish",
   "project_dir":"/home/me/translator",
   "host":"relay",
   "consent_hash":"<exact approved hash>"}

→ Returns the signed Publication Revision and public URL. An exact offer URL,
  marketplace activation evidence, and independent requester canary are present
  only when the marketplace activates the exact candidate; policy-held
  candidates return pending-review evidence instead.
```

The first call never posts to provider control, prepares hosting, or submits a
marketplace candidate. Private source/data/fixture bytes remain private and
are represented by exact hashes in consent. If anything material changes, the
second call computes a different hash and stops before publication.

**Native MCP input shape:**

```json
{
  "action": "marketplace_publish",
  "project_dir": "/absolute/path/containing/froglet-service.toml",
  "host": "local|relay|tor|self",
  "marketplace_url": "https://marketplace.froglet.dev",
  "consent_hash": "<omit on plan; exact approved hash on call 2>"
}
```

Native authoring supports read-only JSON/CSV/SQLite data, embedded WAT/Wasm,
and resolver-free locked Python. Digest-pinned OCI execution is an advanced
isolated-worker path. Settlement is authored in the manifest: `none` is the
default; `lightning` and `stripe` require explicit paid terms and rail setup.

---

## The human-driven flow

For when you're typing directly, not driving through an LLM:

```bash
# 1. Run the immutable-release download + digest check from the Quickstart,
#    review its non-mutating plan, then run its exact approved execute command.
#    https://froglet.dev/learn/quickstart/

# 2. Scaffold a new service:
froglet-node init my-translator
cd my-translator

# 3. Edit handler.py to do real work:
$EDITOR handler.py

# 4. Produce the non-mutating public plan:
froglet-node publish --host relay --plan --json

# 5. Review it, then approve the exact returned hash:
froglet-node publish --host relay --approve-consent <hash> --json
```

`froglet-node init` writes four files: `froglet.toml` (project),
`froglet-service.toml` (per-service, v4 schema), `handler.py` (Python
skeleton), and `.gitignore`. See [docs/MANIFEST.md](./MANIFEST.md) for
the manifest spec.

`froglet-node publish` reads both manifests and runs the same engine pipeline
as native MCP. It accepts:

- `--host local|relay|tor|self` to override `[hosting] default`
- `--marketplace URL` to override the manifest's marketplace
- `--plan` for the non-mutating consent summary
- `--approve-consent HASH` for the exact approved public plan
- `--json` to emit machine-readable output

Other useful subcommands:

- `froglet-node build` — validate manifests + build the artifact, no
  publish. Quick sanity check.
- `froglet-node whoami` — print identity + daemon transport info
- `froglet-node print-identity` / `sign-message` — identity utilities

---

## Hosting backends

The v4 authoring contract exposes provider-neutral choices. Local, Relay, Tor,
and self-hosted are executable today; managed authoring is validatable but its
publish-engine deployment orchestration is not yet implemented. Relay is the
dependency-minimal public design when a live operator endpoint is configured.

### Relay (`--host relay`) — public default design

The node dials outbound WSS and receives an identity-derived HTTPS origin. No
inbound port, user-owned DNS account, or local certificate is needed. The relay
terminates TLS, can observe plaintext, and applies quotas, so those facts are
approval-bound. Bootstrap plans the official URL and suffix dormant by default,
opening no WSS without an exact durable grant. Public publication still fails
before approval unless the configured relay reports `status=up`, the exact
public URL, and provider identity. Consult the implementation evidence matrix
before claiming the first-party relay deployment is online. This preflight
proves the configured node-to-relay session only; it does not itself prove
public DNS, trusted TLS, or external Internet ingress.

### Tor (`--host tor`)

The daemon spawns a Tor hidden service; the `.onion` URL is your public
address. No DNS, no TLS, no port-forwarding, works behind any NAT.
Planning is read-only and fail-closed: capabilities must report Tor enabled,
the daemon's 64-hex provider identity, and an exact credential-free
`http://<56-char-v3>.onion` origin. That identity and endpoint enter the consent
hash, and the prepared endpoint must match the approved origin.

Requires:

```bash
FROGLET_NETWORK_MODE=tor froglet-node   # or "dual" for clearnet+tor
```

(`froglet-node` with no subcommand runs the daemon.)

Strongest no-DNS posture. Tradeoff: higher latency (~500ms+), clients
must speak Tor.

### Local (`--host local`, Phase 1A)

Private development. Service binds to `127.0.0.1:8080`, never registers
with the marketplace. Use during development; promote to Tor or
self-hosted when ready.

### Self-hosted (`--host self`, Phase 1A)

You deploy the daemon somewhere with a public HTTPS URL (Fly, Render,
Railway, your VPS) and supply the URL in the manifest. The CLI and MCP
front-ends reject loopback, private-network, `.local`, `.internal`, and
plain-public-HTTP URLs before publish. The shared publish engine independently
requires a credential-free HTTPS root origin with no path, query, or fragment;
it also rejects `.onion` and local/private numeric IP literals. DNS resolution
and external reachability remain the marketplace canary's responsibility. The
marketplace's `/v1/registrations` then verifies that the URL serves `/v1/feed`
with a signed descriptor plus an offer matching your provider key.

```toml
[hosting]
default = "self"

[hosting.self]
url = "https://my-translator.fly.dev"
```

### Managed (`--host managed`, adapter pending)

V4 represents managed hosting with non-empty provider-neutral `target` and
`profile` fields. Provider-specific account, region, DNS, and platform state
belong behind the operator adapter, not in the service manifest. The current
publish engine returns a not-implemented error for this choice; successful
manifest validation is not deployment proof. See
[PUBLICATION_CONTRACT.md](./PUBLICATION_CONTRACT.md#service-manifest-v4-and-hosting-portability).

### Fly (v3 compatibility only)

A v3 `hosting.default = "fly"` manifest remains readable and emits a
deprecation warning. V4 rejects Fly as a first-class authoring choice. Migrate
to `managed` and select Fly, if desired, behind a future deployment adapter.

---

## Pricing and currency

The `[price]` section in `froglet-service.toml` accepts the legacy whole-unit
amount plus optional explicit fee legs:

- `sats` — the price integer (default `0` = free)
- `currency` — the unit for that integer (default `"sat"`)
- `base_fee_msat` — optional explicit base-fee leg
- `success_fee_msat` — optional explicit success-fee leg; when present it must
  equal `sats * 1000`

**Allowed values for `currency`:**

| Value | Unit | Settled via |
|---|---|---|
| `"sat"` (default) | satoshis | Lightning rail |
| `"usd"` | US cents (e.g. `500` = $5.00) | Stripe rail |

```toml
# Lightning-priced: 1000 satoshis
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

The existing signed offer retains its Kernel-compatible fee schedule and exact
settlement method. The higher-layer provider service record exposes currency
alongside both fee legs, and a verified Publication Revision binds unambiguous
currency-aware minor units. The field name `price.sats` remains a compatibility
name on the Stripe rail; treat it as whole price units and let `currency`
disambiguate. See [PUBLICATION_CONTRACT.md](./PUBLICATION_CONTRACT.md#public-provider-service-pricing).

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
  complete; the issuance service is not yet built (tracked as Order 81
  in `TODO.md`).

---

## What's deliberately not hidden

This document used to describe a one-call publish flow. Public mutation is now
explicitly separated from planning: an agent selects a transport, but it cannot
consume the returned approval hash until the user approves the exact summary.

If you're an operator running your own marketplace, the engine talks
to a configured `froglet-node` daemon (`FROGLET_DAEMON_URL`, default
`http://127.0.0.1:8080`) and registers against the manifest's
`marketplace_url`. Marketplace URLs must be public HTTPS or approved onion
transport; local/private marketplace URLs are reserved for explicit dev/test
paths. There is no first-party lock-in.
