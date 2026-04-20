# Marketplace Arbiter / Claims-Court Service

Status: **design stub**. This file reserves the public design surface for a
marketplace-layer arbitration service and gives future docs a stable
cross-reference target.

## Scope

The arbiter is the Froglet marketplace's enforcement surface for cheating,
grief-filing, and adjudicator capture. It is a **marketplace-layer service**,
not a kernel change. Kernel artifacts (offers, descriptors, receipts) and
settlement drivers remain unchanged; the arbiter operates on the signed
artifacts they produce.

The full mechanism design is intentionally still open. This page keeps the
public interface expectations visible without freezing deposit tiers, panel
selection rules, or appeal economics before there is operational data.

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
- **[MARKETPLACE.md](MARKETPLACE.md)** — the arbiter is a marketplace-layer
  service. It uses the same public marketplace integration surface as other
  marketplace services.

## Why this is a stub, not the full spec

Writing the full mechanism design here before implementation begins would
freeze economic parameters (deposit tiers, stake floors, fee split, appeal
multiplier) whose defensible values depend on data we do not yet have:
observed grief-filing attempts, adjudicator throughput, and the real cost
of running an adjudicator node. The TODO entry captures the structure;
concrete numbers land with the implementation.
