# Froglet TODO / Roadmap Notes

This file preserves roadmap order references used by code comments, tests, and
design stubs. It is not the kernel specification. `docs/KERNEL.md` remains the
authoritative source for canonical artifact payloads, hashing, signing bytes,
state transitions, and settlement bindings.

## Order 70 - Strict Egress Pin Propagation

Status: implemented with regression coverage in
`integrations/shared/froglet-lib/test/egress-mode.test.mjs`.

Track strict egress behavior through the shared Froglet JavaScript client:
`frogletRequest`, `frogletRequestWithStatus`, and `frogletPublicRequest` must
resolve operator pins when `FROGLET_EGRESS_MODE=strict`, cache the resolved pin,
and honor caller-supplied pins over cached lookups.

Remaining work: keep this coverage aligned as additional agent integrations
delegate requests through `integrations/shared/froglet-lib/`.

## Order 81 - Identity Attestation Issuing Service

Status: protocol payload and validator exist in
`froglet-protocol/src/protocol/identity_attestation.rs`; service issuance
flows remain to be built.

Implement the marketplace attestation service that issues DNS and OAuth/OIDC
identity attestations:

- DNS flow: create a challenge, verify the DNS evidence, issue the signed
  `identity_attestation/v1` artifact, and schedule re-verification.
- OAuth/OIDC flow: verify provider identity evidence, bind it to the Froglet
  subject key, issue the signed artifact, and schedule re-verification.
- Revocation and expiry handling: consumers must reject expired attestations;
  failed re-verification should invalidate the attestation before expiry.

Out of scope for this order: W3C Verifiable Credentials and proof-of-personhood.

## Arbiter Mechanism Design

Status: stubbed in `docs/ARBITER.md`.

The arbiter design still needs data-backed economic parameters before it should
be promoted from stub to implementation spec. Open parameters include deposit
tiers, stake floors, fee split, appeal multiplier, adjudicator throughput, and
grief-filing cost assumptions.

Implementation should update `docs/ARBITER.md` with concrete values and tests
once those parameters are chosen.
