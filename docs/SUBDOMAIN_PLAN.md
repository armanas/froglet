# Subdomain Plan

Status: **approved** for `froglet.dev` as of 2026-04-17.

This public document is the canonical domain-ownership map for the public
Froglet surface. Detailed first-party DNS records, operator credentials,
provider-specific deployment notes, and live zone inventory now live in the
private services/operator workspace.

## Apex and subdomains

| Host | Purpose | Canonical source | Status |
|---|---|---|---|
| `froglet.dev` | Protocol landing page at `/` plus documentation under `/learn/*`, `/architecture/*`, and related routes. | `docs-site/` in this repo | Planned |
| `docs.froglet.dev` | Alias of the apex for readers who reach for the `docs.*` form directly. Same build and content as the apex deployment. | Same `docs-site/` deployment as apex | Planned |
| `ai.froglet.dev` | Hosted Froglet provider environment: the first-party reference protocol instance that clients can point at. | Private services/operator workspace | Live operator-managed service |
| `marketplace.froglet.dev` | Marketplace read API for providers, offers, and receipts. | `froglet-services` | Planned |
| `status.froglet.dev` | Public status page for the first-party hosted services. | Private services/operator workspace | Planned |
| `try.froglet.dev` | Hosted trial gateway with temporary identity and lifecycle controls. | Separate private repo (see [HOSTED_TRIAL.md](HOSTED_TRIAL.md)) | Out of scope for this repo |

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

Those details are part of the private services/operator workspace, not the
public kernel/runtime repo.

## Deployment-order dependencies

1. DNS authority goes live on the chosen provider.
2. Cloudflare Worker preview deploy from this repo using
   `docs-site/wrangler.jsonc`.
3. Attach both `froglet.dev` and `docs.froglet.dev` to that same deployment
   so the apex remains canonical and `docs.*` is only a mirror.
4. `ai.froglet.dev` — first-party hosted provider deployment.
5. `marketplace.froglet.dev` — marketplace read API deployment.
6. `status.froglet.dev` — public status page deployment.

## What is not in scope here

- **Brand clearance / trademark** — Froglet is an open-source protocol, not
  a company. The lightweight registry-coherence check is documented
  separately; a full trademark clearance only matters if a commercial
  entity is formed, and that entity would use a different name.
- **Vanity redirects** (`froglet.io`, `froglet.app`, etc.) — not purchased.
  If someone else registers them, we live with it.
- **Country-specific TLDs** — not in scope.

## Revision history

- 2026-04-17: Document created. `froglet.dev` purchased; DNS and hosting
  decisions pending.
- 2026-04-19: First-party DNS and operator details moved into the private
  services/operator workspace so this public copy only keeps the stable
  public-domain map.
- 2026-04-20: Public docs deploy path standardized on Cloudflare Workers via
  `docs-site/wrangler.jsonc`; the stale GitHub Pages workflow was removed.
