# Marketplace Arbiter / Claims-Court Service

Status: **design stub**. The authoritative design notes for the arbiter
service live in [TODO.md Order 80](../TODO.md). This file exists so future
cross-references resolve and so there is a stable location for the full
mechanism-design writeup to land when implementation begins.

## Scope

The arbiter is the Froglet marketplace's enforcement surface for cheating,
grief-filing, and adjudicator capture. It is a **marketplace-layer service**,
not a kernel change. Kernel artifacts (offers, descriptors, receipts) and
settlement drivers remain unchanged; the arbiter operates on the signed
artifacts they produce.

For the full specification — handler surface, deposit model, panel
selection, appeal mechanism, slashing hook, and the intentionally-unresolved
sybil-resistance ceiling — see [TODO.md Order 80](../TODO.md). This
document will replace that entry's content verbatim once the service
implementation lands in `froglet-services/services/marketplace-arbiter`.

## Interaction with adjacent specs

- **[IDENTITY_ATTESTATION.md](IDENTITY_ATTESTATION.md)** — the arbiter uses
  the `IdentityAttestation` credentials defined there to gate adjudicator
  eligibility at high-value dispute tiers. That is the single concrete
  dependency between the two specs.
- **[SERVICE_BINDING.md](SERVICE_BINDING.md)** — the arbiter follows the
  same service-binding model as every other marketplace service; the
  `invoke_service` path is the generic escape hatch.
- **[KERNEL.md](KERNEL.md)** — the arbiter does not modify the kernel. Its
  handlers only accept, index, and produce signed artifacts that conform to
  the existing envelope shape.
- **[MARKETPLACE_SPLIT.md](MARKETPLACE_SPLIT.md)** — the arbiter lives in
  `froglet-services`, not in the public `froglet` repo. The public repo
  reserves the spec and the interface expectations only.

## Why this is a stub, not the full spec

Writing the full mechanism design here before implementation begins would
freeze economic parameters (deposit tiers, stake floors, fee split, appeal
multiplier) whose defensible values depend on data we do not yet have:
observed grief-filing attempts, adjudicator throughput, and the real cost
of running an adjudicator node. The TODO entry captures the structure;
concrete numbers land with the implementation.
