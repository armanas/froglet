---
title: Marketplace
description: Froglet can integrate with marketplaces while keeping the public contract stable.
---

Froglet keeps marketplace integration public, but it does not bundle the
first-party marketplace implementation in this repo anymore.

What stays public:

- Providers can register with an external marketplace.
- Runtimes can search and look up providers through an external marketplace.
- `FROGLET_MARKETPLACE_URL` remains the integration point for that behavior.
- Public Froglet feed and artifact APIs remain the ingest boundary used by the
  default marketplace implementation.
- Kernel, quote, deal, receipt, and settlement semantics do not change when a
  marketplace is involved.

What changed:

- The default marketplace still exists, but its implementation lives outside
  this public repo.
- This repo now documents marketplaces only at the contract level.
- Marketplace remains a Froglet integration point, not a bundled public
  service implementation.

The approved split boundary is recorded in
[`docs/MARKETPLACE_SPLIT.md`](https://github.com/armanas/froglet/blob/main/docs/MARKETPLACE_SPLIT.md).
