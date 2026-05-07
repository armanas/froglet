# Marketplace

Froglet supports marketplace-based discovery without making any marketplace the
source of truth for the protocol.

## Public model

- Providers can register signed descriptors and offers with a marketplace.
- Runtimes can search for providers and look up provider details through a
  marketplace.
- `FROGLET_MARKETPLACE_URL` is the public integration point for that behavior.
- A default public marketplace exists at `https://marketplace.froglet.dev`
  for runtime discovery through `/v1/providers` and `/v1/offers`.
- Provider self-registration is available at `POST /v1/registrations`. It
  requires the provider to advertise the same URL being registered and publish
  a valid signed descriptor plus at least one bound offer in `/v1/feed`.
- HTTPS clearnet registration requires a public HTTPS origin with public
  DNS/IP resolution.
- Tor registration uses `{"provider_url":"http://<v3>.onion","transport":"tor"}`
  and requires marketplace/indexer Tor SOCKS access.
- Froglet-managed `*.providers.froglet.dev` clearnet names are claimed through
  `marketplace_domain_claim` and `marketplace_domain_complete`; DNS automation
  creates a DNS-only record or returns `pending_operator_dns`.
- Raw IPs, localhost/private/link-local URLs, and self-signed clearnet
  endpoints are not accepted for the public marketplace MVP path.
- The MVP arbiter is a separate operator-run service for public complaints and
  provider suspension. See [ARBITER.md](ARBITER.md). Suspension is marketplace
  policy, not a protocol-level validity rule.
- Public Froglet feed and artifact endpoints remain the canonical signed inputs
  that marketplaces consume.
- Kernel, quote, deal, receipt, and settlement semantics do not change when a
  marketplace is involved.

## Direct path without a marketplace

Marketplaces are optional. When the requester already knows a provider URL, the
requester can deal directly with that provider without any marketplace hop.

## Reader expectation

From a public-reader perspective, Froglet has one simple story:

1. Run a provider or runtime node.
2. Point it at a marketplace with `FROGLET_MARKETPLACE_URL` when you want
   discovery or registration.
3. Use the default public marketplace for runtime discovery and provider
   self-registration through HTTPS, Tor, or Froglet-managed subdomain
   registration.

The marketplace helps providers find each other. It does not replace the signed
artifacts, and it does not become the protocol root of truth.
