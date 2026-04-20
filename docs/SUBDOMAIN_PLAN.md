# Subdomain Plan

Status: **approved** for `froglet.dev` as of 2026-04-17.

This public document is the canonical domain-ownership map for the public
Froglet surface. Detailed first-party DNS records, operator credentials,
provider-specific deployment notes, and live zone inventory are maintained
separately from the public protocol docs.

## Apex and subdomains

| Host | Purpose | Canonical source | Status |
|---|---|---|---|
| `froglet.dev` | Protocol landing page at `/` plus documentation under `/learn/*`, `/architecture/*`, and related routes. | `docs-site/` in this repo | Provisioning; public host not live yet |
| `docs.froglet.dev` | Alias of the apex for readers who reach for the `docs.*` form directly. Same build and content as the apex deployment. | Same `docs-site/` deployment as apex | Not yet provisioned |
| `ai.froglet.dev` | Hosted Froglet provider environment: the first-party reference protocol instance that clients can point at. | First-party hosted deployment | Edge hostname exists; Froglet app not live yet |
| `marketplace.froglet.dev` | Default public marketplace for providers, offers, and receipts. | Default public marketplace deployment | Planned |
| `status.froglet.dev` | Public status page for the first-party hosted services. | First-party hosted status deployment | Planned |
| `try.froglet.dev` | Hosted trial gateway with temporary identity and lifecycle controls. | First-party hosted gateway | Out of scope for this repo |

## Why the split

- The **apex** is the protocol landing. The `docs-site/` project already
  renders a hero + CTA view at `/` and docs under `/learn/*`; serving the
  whole thing at apex keeps one deployment, one build, and one canonical URL
  once the public host is provisioned. `docs.froglet.dev` is an alias of the
  same deployment rather than a separate site, so the canonical URL for a
  docs page is intended to be `froglet.dev/learn/quickstart/` with
  `docs.froglet.dev/learn/quickstart/` as the mirror after provisioning.
- The **hosted instance** is `ai.froglet.dev` (not the apex) so that running
  a first-party reference Froglet is obviously "a thing the protocol owns
  the URL for," not "the protocol itself."
- The **marketplace** is `marketplace.froglet.dev` so it is clearly
  addressable as a distinct service — anyone running their own marketplace can
  point at their own host without
  any assumption that the marketplace is "the" marketplace.
- `try.froglet.dev` stays in its own subdomain because the hosted-trial
  lifecycle (rate limiting, TTL cleanup, audit
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

Those details are maintained separately from the public kernel/runtime repo.

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
