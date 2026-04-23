# Marketplace

Froglet supports marketplace-based discovery without making any marketplace the
source of truth for the protocol.

## Public model

- Providers can register signed descriptors and offers with a marketplace.
- Runtimes can search for providers and look up provider details through a
  marketplace.
- `FROGLET_MARKETPLACE_URL` is the public integration point for that behavior.
- A default public read marketplace exists at `https://marketplace.froglet.dev`
  for runtime discovery through `/v1/providers` and `/v1/offers`.
- Provider auto-registration requires a write-capable marketplace endpoint;
  the default public marketplace is read-only for this release.
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
3. Use the default public marketplace for runtime discovery, or another
   compatible write-capable marketplace endpoint when you need provider
   registration.

The marketplace helps providers find each other. It does not replace the signed
artifacts, and it does not become the protocol root of truth.
