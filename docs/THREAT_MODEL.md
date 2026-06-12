# Threat Model

What Froglet protects, where the trust boundaries are, what happens when keys
leak, and which risks are accepted deliberately. This document describes the
reference node and the first-party hosted deployments; the kernel's
cryptographic guarantees are specified in [KERNEL.md](KERNEL.md) and frozen
per [VERSIONING.md](VERSIONING.md).

## Assets

| Asset | Where it lives | Compromise impact |
|---|---|---|
| Node identity seed (secp256k1) | `data/identity/node_identity_seed`, mode 0600; optionally `FROGLET_IDENTITY_SEED_HEX` | Full impersonation — see [key compromise](#key-compromise-blast-radius-and-runbook) |
| Nostr publication seed | separate file under `data/identity/` | Forged feed publications under the linked identity |
| Payment credentials | buyer Stripe secret key, phoenixd credentials, LND macaroons (env/config) | Direct spend of wallet funds, independent of identity |
| Success-fee preimages | requester node state, per deal | Leaking a preimage settles that success fee unconditionally |
| Signed artifacts / receipts | node DB, public feeds, archives | Integrity is cryptographic; the threat is *loss*, not forgery |
| Hosted service availability | `ai.froglet.dev`, `marketplace.froglet.dev`, `try.froglet.dev`, `arbiter.froglet.dev` | Free-tier outage; no custody of user funds |

## Trust boundaries

1. **Requester ↔ provider (untrusted peers).** Nothing a peer sends is
   trusted until its signature and hash chain verify locally. Quotes are
   re-verified by the requester (signer, ids, workload hash) before any money
   moves. A provider can lie about *future* behavior; it cannot forge the
   evidence trail.
2. **Node ↔ upstream HTTP.** All server-side fetches of remote content go
   through SSRF-protected fetch: DNS pinning, private/link-local address
   blocking (fail-closed; test-only escape hatch), response-size caps, and
   request timeouts. `.onion` destinations route via the configured Tor SOCKS
   proxy. The same policy is shared with the closed-source indexer.
3. **Execution sandbox.** WASM workloads run under wasmtime with fuel,
   memory, output-size, and wall-clock limits from the offer's execution
   profile. The process runtime applies Landlock + seccomp confinement on
   Linux. Sandbox escape ⇒ provider-node compromise; treat provider hosts as
   hostile-workload runners and isolate accordingly.
4. **Agent ↔ daemon (MCP/OpenClaw).** Local agents authenticate with token
   files and are *semi-trusted*: they may initiate paid deals. Requester
   spend caps (`FROGLET_REQUESTER_*`) bound the financial blast radius of a
   runaway or prompt-injected agent; paid deals are refused when no budget is
   configured.
5. **Marketplace ingest.** The indexer verifies BIP340 signatures on every
   artifact before projection; unverifiable artifacts never become listings.
   Registration requires HTTPS (or Tor) feeds and is rate-limited at the edge.
6. **Operator admin surfaces.** Arbiter verdicts and feed-source approval
   require bearer tokens. These are operator policy levers — they affect
   hosted listings, never artifact validity.

## Key compromise: blast radius and runbook

There is **no protocol-level key revocation in `froglet/v1`**. An attacker
holding a node identity seed can sign artifacts as that identity indefinitely.
What a seed does *not* grant: wallet funds (separate credentials) and
already-signed history (timestamps and chains are immutable).

If a provider seed leaks:

1. **Stop the node** and take the compromised identity out of service.
2. **Generate a fresh identity** (new seed ⇒ new provider id) and republish
   descriptors and offers under it.
3. **Re-register on the marketplace** under the new identity; ask the
   operator (or use the complaint flow at `arbiter.froglet.dev`) to suspend
   the compromised identity's listings.
4. **Rotate linked identities** (Nostr publication key) and any payment
   credentials that shared the host.
5. **Establish the compromise window** from your last known-good artifact
   timestamp; everything signed after it is suspect. Requesters can be
   pointed at the new identity's attestation (DNS/OAuth domain claim) to
   re-anchor trust — re-attesting the same domain under the new key is the
   recovery path attestation exists for.

**Roadmap:** a revocation/tombstone artifact type is an additive `froglet/v1`
extension candidate per [VERSIONING.md](VERSIONING.md) — designed, not
scheduled.

## Payment-specific threats

- **Runaway buyer agent:** bounded by per-deal and cumulative spend caps
  (fail-closed when unconfigured), plus requester-side re-checks of quoted
  totals against the caller's `max_price_sats`.
- **Provider quotes ≠ provider charges:** impossible by construction on
  Lightning — invoices are bound to the signed quote amounts and the success
  fee settles only on the requester's preimage release.
- **Preimage theft:** equivalent to authorizing the success fee. Preimages
  live only in requester node state; protect the node host.
- **Wrong results for pay:** *not prevented.* A receipt makes outcomes
  attributable, not correct. Mitigations are economic and documented in
  [Trust & Economics](https://froglet.dev/learn/economics/): small deals,
  attestation filters, spot-check re-execution, the arbiter complaint path.
- **Stripe rail:** test-mode only; SPT validation fails closed; secrets are
  redacted from debug output. No production Stripe claims until live-mode
  transcripts exist (see PAYMENT_MATRIX.md).

## Denial of service

- Node-wide: request body cap (1 MiB), per-route timeouts, concurrency limits
  on provider/runtime/hosted-trial route groups.
- Hosted trial: per-origin quotas (window-based, derived from
  forwarded-for/CF headers behind the edge) on the public demo surface.
- Marketplace: Cloudflare edge rate limits (e.g. registration: 5/min/IP) and
  bounded registration/domain-claim concurrency.
- **Accepted gap:** fine-grained per-IP rate limiting on *all* node routes is
  backlog (tracked for tower_governor adoption); hosted deployments rely on
  the edge for this today. Volumetric DoS against the free tier is explicitly
  out of scope of the bounty/disclosure policy.

## Accepted risks (alpha)

Stated so they are decisions, not surprises:

1. **Seeds are plaintext-on-disk (0600) with in-memory zeroization** — no
   at-rest encryption yet. Run nodes on encrypted disks; keychain/HSM
   integration is future work.
2. **Base fees are non-refundable on execution failure** — by design;
   documented in the settlement docs and priced into the fee model.
3. **Marketplace enforcement is operator-adjudicated** — see
   [ARBITER.md](ARBITER.md); decentralized adjudication is post-MVP.
4. **External uptime alerting for hosted services is paused** — accepted
   alpha risk, documented in MONITORING.md.
5. **No protocol revocation** — runbook above is the mitigation until a
   tombstone artifact ships.
