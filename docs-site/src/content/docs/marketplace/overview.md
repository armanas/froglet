---
title: Marketplace
description: Froglet can integrate with marketplaces while keeping the public contract stable.
---

Froglet keeps marketplace integration public while preserving one simple
reader-facing model: use the default public marketplace or point at any other
compatible marketplace endpoint.

What stays public:

- Providers can register with an external marketplace.
- Runtimes can search and look up providers through an external marketplace.
- `FROGLET_MARKETPLACE_URL` remains the integration point for that behavior.
- Public Froglet feed and artifact APIs remain the ingest boundary used by the
  default public marketplace.
- The MVP arbiter is a separate operator-run complaint and suspension service;
  enforcement is marketplace policy, not kernel validity.
- Kernel, quote, deal, receipt, and settlement semantics do not change when a
  marketplace is involved.

The default public marketplace is `https://marketplace.froglet.dev`.
It accepts provider self-registration at `POST /v1/registrations` when the
provider publishes a valid signed descriptor plus at least one bound offer in
`/v1/feed`.

Accepted registration transports:

| Transport | Request | Requirement |
|-----------|---------|-------------|
| HTTPS clearnet | `{"provider_url":"https://example.com"}` | public HTTPS origin, public DNS/IP, capabilities advertise the same clearnet URL |
| Tor | `{"provider_url":"http://<v3>.onion","transport":"tor"}` | Tor v3 onion URL, capabilities advertise the same onion URL, marketplace/indexer Tor SOCKS access |
| Froglet-managed subdomain | MCP `marketplace_domain_claim` then `marketplace_domain_complete` | provider identity signs the claim, DNS-only `*.providers.froglet.dev` record is created or returned as `pending_operator_dns` |

Raw IPs, private URLs, localhost, link-local addresses, and self-signed
clearnet URLs are not accepted for the public marketplace path.
Marketplaces remain integration points, not protocol roots of truth.
