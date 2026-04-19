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
| `ai.froglet.dev` | Hosted Froglet provider environment — the reference protocol instance that an LLM / MCP client can point at. | Hosted `froglet-node` on AWS Lightsail Container Service (us-east-1, power=small), fronted by Cloudflare proxy + Lightsail-issued ACM cert. | **Live (placeholder)** — serving `nginx:alpine` welcome page while we cut the first Froglet image tag. |
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

**Chosen 2026-04-19: DNS delegated to Cloudflare.** Registration stays at
Namecheap for now; transfer to Cloudflare Registrar is a later, optional
cleanup (60-day ICANN post-purchase lock applies anyway).

Live state:

- **Nameservers:** `giancarlo.ns.cloudflare.com`, `irena.ns.cloudflare.com`
- **Zone ID:** `ff6367e195a95ebe1a1acb066f8b09a6`
- **Zone status:** Active, Free plan
- **Automation:** [scripts/cloudflare_dns.sh](../scripts/cloudflare_dns.sh)
  reads a Keychain-stored API token (`security add-generic-password -a
  froglet -s cloudflare-dns-token`) and exposes `verify / zone / list /
  create / delete / upsert` subcommands. Token scope is `Edit zone DNS`
  limited to `froglet.dev`; a separate, broader token will be needed when
  Workers / Pages deploys land.

### Records currently provisioned

Inherited from Namecheap during Cloudflare's onboarding scan; none have been
added or removed yet.

| Type | Name | Content | Notes |
| --- | --- | --- | --- |
| A | `froglet.dev` | `192.64.119.73` | Namecheap parking page (Cloudflare-proxied). Replace when docs-site deploys (Order 18). |
| CNAME | `www.froglet.dev` | `parkingpage.namecheap.com` | Namecheap parking. Replace or remove when docs-site deploys. |
| MX | `froglet.dev` | `eforward1..5.registrar-servers.com` | Namecheap email forwarding (5 priority-distributed MX). Keep until an outbound email provider (Postmark / Resend / SES / Cloudflare Email Routing) is chosen. To actually receive mail, the Namecheap dashboard needs a forwarding rule (Domain List → `froglet.dev` → "Redirect Email"). |
| TXT | `froglet.dev` | `v=spf1 include:spf.efwd.registrar-servers.com ~all` | SPF authorising the Namecheap forwarder. Must be rewritten when we pick an outbound email provider. |

### Records not yet provisioned

Blocked on upstream decisions, listed here so we don't forget them:

- **Apex A/AAAA or CNAME → docs deploy** — Order 18 deploys `docs-site/`
  to Cloudflare Workers / Pages. Once deployed, replace the Namecheap
  parking A record with the Workers-target CNAME (or A + AAAA).
- **`docs.froglet.dev` CNAME** — same target as the apex, mirrors the docs
  deployment.
- **`ai.froglet.dev` A/AAAA** — Order 19, blocked on the cloud-provider
  decision (GCP is on hold per 2026-04-19).
- **`marketplace.froglet.dev` A/AAAA** — Order 64, same cloud blocker.
- **`status.froglet.dev` CNAME** — Order 63, blocked on the status-page
  provider choice.
- **DKIM + DMARC** — blocked on the outbound email provider. DMARC can
  land with `p=none` (monitor only) before DKIM is generated; DKIM is
  provider-specific.

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
- 2026-04-19: DNS delegated to Cloudflare (registration stays at
  Namecheap). Zone live; 8 records inherited from Namecheap defaults.
  `scripts/cloudflare_dns.sh` added as the canonical automation surface.
- 2026-04-19: `ai.froglet.dev` stood up on AWS Lightsail Container Service
  (us-east-1, power=small). Lightsail-issued ACM certificate attached;
  Cloudflare proxied CNAME live; `nginx:alpine` placeholder serving until
  the first Froglet image tag is cut. Automation:
  [scripts/deploy_aws.sh](../scripts/deploy_aws.sh).
