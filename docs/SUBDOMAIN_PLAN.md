# Subdomain Plan

Status: **approved** for `froglet.dev` as of 2026-04-17.

The primary domain `froglet.dev` is owned. DNS is not yet authoritative
pending the cloud-provider decision. This document is the canonical decision
record for which subdomain serves what, what the apex does, and what the
email-authentication posture is.

## Apex and subdomains

| Host | Purpose | Source | Status |
|---|---|---|---|
| `froglet.dev` | Protocol landing page at `/`, documentation under `/learn/*`, `/architecture/*`, etc. Same Astro build as the docs. The apex serves the hero + CTA view that lives in `docs-site/src/pages/index.astro`. | `docs-site/` in this repo | Not yet provisioned |
| `docs.froglet.dev` | Alias of the apex for readers who reach for the `docs.*` form directly. Same build, same content; served from the same deployment via Cloudflare Workers hostname routing. Optional — if a separate pure-docs surface is ever split out, this becomes its canonical home. | Same `docs-site/` deployment as apex | Not yet provisioned |
| `ai.froglet.dev` | Hosted Froglet provider environment — the reference protocol instance that an LLM / MCP client can point at. | Hosted `froglet-node` on the chosen cloud | Waits on the hosting-provider decision |
| `marketplace.froglet.dev` | Marketplace read API (providers, offers, receipts). | `froglet-services/services/marketplace-api` | Waits on hosting + Postgres |
| `status.froglet.dev` | Public status page for the `ai.*` and `marketplace.*` instances. | A hosted status service (e.g., Statuspage, Instatus) or a self-hosted minimal page | Waits on hosting |
| `try.froglet.dev` | Hosted trial gateway — temporary 15-minute identity, free-only deals, optional email-claim lifecycle. **Separate private repo.** | Not in this repo (see [HOSTED_TRIAL.md](HOSTED_TRIAL.md)) | Out of scope for this repo |

## Why the split

- The **apex** is the protocol landing. The `docs-site/` project already
  renders a hero + CTA view at `/` and docs under `/learn/*`; serving the
  whole thing at apex keeps one deployment, one build, one URL the README
  can link to without version skew. `docs.froglet.dev` is an alias of the
  same deployment rather than a separate site, so the canonical URL for a
  docs page is `froglet.dev/learn/quickstart/` with
  `docs.froglet.dev/learn/quickstart/` as a working mirror.
- The **hosted instance** is `ai.froglet.dev` (not the apex) so that running
  a first-party reference Froglet is obviously "a thing the protocol owns
  the URL for," not "the protocol itself."
- The **marketplace** is `marketplace.froglet.dev` so it is clearly
  addressable as a distinct service — anyone forking `froglet-services`
  and running their own marketplace can point at their own host without
  any assumption that the marketplace is "the" marketplace.
- `try.froglet.dev` stays in its own subdomain and its own private repo
  because the hosted-trial lifecycle (rate limiting, TTL cleanup, audit
  logging, email verification, human-account conversion) has a different
  operational boundary than the protocol core and does not belong in the
  public repo.

## Email-authentication baseline

Before any subdomain sends email, the email-sending domain needs:

- **SPF** — `v=spf1 include:<provider> -all` where `<provider>` is whichever
  transactional-email service is used for outbound (Postmark, SES, Resend,
  etc.). Decision pending cloud choice.
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

## DNS authority

Decision pending. Candidates:

- **Cloudflare Registrar + DNS** — free DNS, DNSSEC, good at edge
  rate-limiting which matters for the hosted surface. Default choice unless
  the hosting cloud has strong reasons to keep DNS there (e.g., Route53
  for AWS-first deployments).
- **Registrar-native DNS** (Porkbun / Namecheap / whoever sold the domain) —
  simpler, fewer moving parts, fine for pure-DNS use until edge features
  are needed.

The registrar choice lives outside this repo; once DNS authority is picked,
record the provider here so the deploy automation in `scripts/` can target
it directly.

## Deployment-order dependencies

1. DNS authority goes live on the chosen provider.
2. `docs.froglet.dev` — earliest to stand up; depends only on Astro build
   + DNS. Cloudflare Workers build from this repo.
3. Apex `froglet.dev` — a minimal landing page (static HTML is fine).
4. `ai.froglet.dev` — blocked on the cloud choice + TLS reverse proxy
   (TODO Order 53) + the actual `froglet-node` hosted deploy (Order 19).
5. `marketplace.froglet.dev` — blocked on hosting for `froglet-services` +
   Postgres.
6. `status.froglet.dev` — blocked on hosting for the status page service.

## What is not in scope here

- **Brand clearance / trademark** — Froglet is an open-source protocol, not
  a company. [TODO.md Order 50](../TODO.md) covers the lightweight
  name-registry coherence check; a full trademark clearance only matters if
  a commercial entity is formed, and that entity would use a different
  name.
- **Vanity redirects** (`froglet.io`, `froglet.app`, etc.) — not purchased.
  If someone else registers them, we live with it.
- **Country-specific TLDs** — not in scope.

## Revision history

- 2026-04-17: Document created. `froglet.dev` purchased; DNS and hosting
  decisions pending.
