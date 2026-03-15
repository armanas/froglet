# Higher-Layer Checklist

This checklist tracks marketplace and addon work that is intentionally separate
from the Froglet core freeze checklist.

Status legend:

- [x] done
- [~] current focus
- [ ] pending

## 1. Boundary and Inputs

- [x] Decide that higher-layer marketplace services are external Froglet
  consumers, not protocol actors
- [x] Decide that ownership-style linkage does not require a kernel change now
- [ ] Define the stable external inputs these services consume
- [ ] Lock the initial ingest contract to public APIs and signed artifacts
- [ ] Define extraction criteria for moving this work into its own repository

Recommended initial inputs:

- `/v1/feed`
- `/v1/artifacts/:hash`
- descriptor artifacts
- offer artifacts
- curated-list artifacts
- receipt summaries and/or receipt artifacts
- optional Nostr summaries as secondary signals

## 2. Minimal Indexer

- [ ] Define indexer ingest loop over Froglet artifact feeds
- [ ] Verify artifacts before projection
- [ ] Store descriptors, offers, curated lists, and receipt summaries in an
  indexer-owned store
- [ ] Reconcile descriptor/offer supersession and expiry
- [ ] Expose a simple search API over indexed providers and offers

## 3. Catalog Layer

- [ ] Define catalog projection shape for providers, services, and tags
- [ ] Support curated-list overlays without making them canonical
- [ ] Define trust labels such as direct, curated, domain-linked, or verified
- [ ] Support human-facing notes, categories, and service summaries

## 4. Ownership / Issuer Layer

- [ ] Define profile concepts: provider, issuer, operator, beneficiary, listing
- [ ] Define self-asserted ownership profile shape
- [ ] Define stronger proof levels such as domain-linked or third-party attested
- [ ] Decide how ownership claims are rendered without affecting core artifact
  verification

## 5. Broker and Reputation

- [ ] Define broker role for quote aggregation and routing
- [ ] Define reputation inputs from signed receipts and observed availability
- [ ] Decide what is scored versus what is merely displayed
- [ ] Keep ranking policy out of Froglet core

## 6. Remote-Agent and Other Addons

- [ ] Keep long-running workflow orchestration outside the kernel
- [ ] Decide whether remote-agent orchestration belongs with marketplace or in a
  separate repo/service family
- [ ] Define any additional addon directories that should later split out on
  their own

## 7. Extraction

- [ ] Move this directory into its own repo once the first service boundaries
  are stable
- [ ] Leave only references and compatibility notes behind in the Froglet repo
