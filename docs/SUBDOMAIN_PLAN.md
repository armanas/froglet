# Subdomain Plan

Status: **approved** for `froglet.dev` as of 2026-04-17.

This public document is the canonical domain-ownership map for the public
Froglet surface. Detailed first-party DNS records, operator credentials,
provider-specific deployment notes, and live zone inventory are maintained
separately from the public protocol docs.

## Apex and subdomains

| Host | Purpose | Canonical source | Status |
|---|---|---|---|
| `froglet.dev` | Protocol landing page at `/` plus documentation under `/learn/*`, `/architecture/*`, and related routes. | `docs-site/` in this repo | **Live** — served by Cloudflare Worker `froglet-docs`; `/`, `/learn/quickstart/`, and `/sitemap-index.xml` verified on 2026-04-23 |
| `docs.froglet.dev` | Former alias of the apex. | Same `docs-site/` deployment as apex before launch cleanup | **Retired from advertised launch surface** — `froglet.dev` and `docs.froglet.dev` returned byte-identical HTML on 2026-04-24, so launch copy and monitoring now use only the apex. |
| `ai.froglet.dev` | Hosted Froglet provider environment: the first-party reference protocol instance that clients can point at. | First-party hosted deployment | **Live** — first-party Lightsail container service fronted by Cloudflare; `/health`, `/v1/feed`, and `/v1/openapi.yaml` serving signed content |
| `marketplace.froglet.dev` | Default public read marketplace for providers, offers, and receipts. | Default public marketplace deployment | **Live** — served by the `marketplace-api` + `indexer` stack in `froglet-services`; surfaces the hosted node's descriptors and offers at `/v1/providers` and `/v1/offers` |
| `froglet.dev/status/` | Public status snapshot for the first-party hosted services. | `docs-site/` status page | Planned in the launch-prep branch; verify after deploy. |
| `try.froglet.dev` | Hosted trial gateway with a shared session-token pool and 15-minute TTL. | Cloudflare Worker in `froglet-services/ops/cloudflare-workers/try-gate/` fronting the same Lightsail node as `ai.` | Worker-backed ingress. The public contract is `try.` only; the upstream session/demo routes on `ai.` are worker-gated, not public fallback endpoints. |

## Why the split

- The **apex** is the protocol landing. The `docs-site/` project already
  renders a hero + CTA view at `/` and docs under `/learn/*`; serving the
  whole thing at apex keeps one deployment, one build, one canonical URL, and
  one monitoring target. `docs.froglet.dev` was byte-identical to the apex and
  is no longer advertised as a separate public surface.
- The **hosted instance** is `ai.froglet.dev` (not the apex) so that running
  a first-party reference Froglet is obviously "a thing the protocol owns
  the URL for," not "the protocol itself."
- The **marketplace** is `marketplace.froglet.dev` so it is clearly
  addressable as a distinct service — anyone running their own marketplace can
  point at their own host without
  any assumption that the marketplace is "the" marketplace.
- `try.froglet.dev` stays in its own subdomain because the hosted-trial
  lifecycle (rate limiting, 15-minute TTL cleanup, shared-pool slot
  recycling) has a different operational boundary than the protocol core.
  The worker source lives in `froglet-services/ops/cloudflare-workers/try-gate/`
  rather than this public repo. The trial is authentication-only: there is
  no per-session cryptographic identity, no email verification, and no
  human-account conversion path (self-hosting is the only route to
  persistent identity and paid deals).

## Email-authentication baseline

Before any subdomain sends email, the email-sending domain needs:

- **SPF** — `v=spf1 include:<provider> -all` where `<provider>` is whichever
  transactional-email service is used for outbound (Postmark, SES, Resend,
  etc.).
- **DKIM** — the provider's DKIM record.
- **DMARC** — start with `p=quarantine; rua=mailto:dmarc-reports@froglet.dev`
  and tighten to `p=reject` after a month of clean DMARC reports.

Initial addresses:

- `hello@froglet.dev` — general contact.
- `security@froglet.dev` — published in `SECURITY.md` and the README for
  vulnerability reports.
- `dmarc-reports@froglet.dev` — aggregate reports landing endpoint.

All three can route to the same inbox to start; the distinction is purely
public-facing. No reply-from address is needed until the project actually
sends email.

## Operational details

The public decisions above are stable. The following operational details are
intentionally no longer documented here:

- live DNS record inventory
- nameserver and zone metadata
- provider-specific DNS automation
- first-party deploy order and cutover steps
- operator credential storage and alert routing

Those details are maintained separately from the public kernel/runtime repo.

## Deployment-order dependencies

1. DNS authority goes live on the chosen provider.
2. Cloudflare Worker deploy from this repo using `docs-site/wrangler.jsonc`.
3. Attach `froglet.dev` to that deployment.
4. `ai.froglet.dev` — first-party hosted provider deployment.
5. `marketplace.froglet.dev` — marketplace read API deployment.
6. `froglet.dev/status/` — public status snapshot deployment.

## What is not in scope here

- **Brand clearance / trademark** — Froglet is an open-source protocol. The
  lightweight registry-coherence check is documented separately.
- **Vanity redirects** (`froglet.io`, `froglet.app`, etc.) — not purchased.
  If someone else registers them, we live with it.
- **Country-specific TLDs** — not in scope.

## Revision history

- 2026-04-17: Document created. `froglet.dev` purchased; DNS and hosting
  decisions pending.
- 2026-04-19: First-party DNS and operator details moved out of this public
  copy so it only keeps the stable public-domain map.
- 2026-04-20: Public docs deploy path standardized on Cloudflare Workers via
  `docs-site/wrangler.jsonc`; the stale GitHub Pages workflow was removed.
- 2026-04-22: `ai.froglet.dev` and `marketplace.froglet.dev` flipped to
  **Live**; the `try.froglet.dev` contract was tightened so `try.` is the
  only public session/demo ingress and the upstream origin routes are
  worker-gated rather than public fallback endpoints. Email-claim mention
  removed from the `try.` justification to match the MVP scope
  (authentication-only pool, no account conversion).
- 2026-04-23: `docs-site/` deployed to Cloudflare Worker `froglet-docs`,
  with `froglet.dev` live through the default resolver. The initial
  `docs.froglet.dev` failure was pinned to ProtonVPN DNS returning only AAAA
  for the alias; after disconnecting ProtonVPN, ordinary curl verified `/`,
  `/learn/quickstart/`, and `/sitemap-index.xml`.
- 2026-04-24: `froglet.dev/` and `docs.froglet.dev/` returned byte-identical
  homepage HTML. Launch copy, monitoring, and Worker routing were narrowed to
  the apex canonical host.
