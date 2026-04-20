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
- Kernel, quote, deal, receipt, and settlement semantics do not change when a
  marketplace is involved.

The default public marketplace is `https://marketplace.froglet.dev`.
Marketplaces remain integration points, not protocol roots of truth.
