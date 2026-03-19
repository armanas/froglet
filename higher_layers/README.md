# Higher-Layer Index

This directory is the staging area for product-layer work that sits above the
public Froglet kernel, runtime, SDKs, and conformance surfaces.

The canonical boundary document is [REPO_STRATEGY.md](REPO_STRATEGY.md).

## Service Directories

- [marketplace/README.md](marketplace/README.md): staged notes for the split
  between the public reference-discovery surface and later private
  marketplace/catalog layers
- [indexer/README.md](indexer/README.md): ingest and projection notes for
  signed artifacts and feeds
- [broker/README.md](broker/README.md): org-purchase, funding, and billing
  notes for higher-layer broker flows
- [trust/README.md](trust/README.md): ranking, reputation, ownership, and
  issuer-overlay notes
- [operator/README.md](operator/README.md): hosted control-plane and admin
  tooling notes
- [openclaw/README.md](openclaw/README.md): first-party OpenClaw integration
  notes that should stay out of the public plugin boundary

## Legacy Planning Notes

The older flat planning files remain in this directory only as compatibility
notes while the service directories become the canonical home:

- [MARKETPLACE.md](MARKETPLACE.md)
- [EXECUTION_PLAN.md](EXECUTION_PLAN.md)
- [OWNERSHIP.md](OWNERSHIP.md)
- [CHECKLIST.md](CHECKLIST.md)
- [DECISIONS.md](DECISIONS.md)

Focused active note retained beside the service directories:

- [BROKER_SPONSORED_ORG_PURCHASE.md](BROKER_SPONSORED_ORG_PURCHASE.md)

## Related Core Docs

- [../docs/IMPLEMENTATION_CHECKLIST.md](../docs/IMPLEMENTATION_CHECKLIST.md)
- [../docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md)
- [../docs/REMOTE_AGENT_LAYER.md](../docs/REMOTE_AGENT_LAYER.md)
