# Ownership and Issuer Notes

This document captures the current decision about ownership-style linkage.

## 1. Current Position

No Froglet kernel change is needed now.

The current identity model is sufficient for the core:

- `provider_id` is the operational Froglet protocol identity
- linked identities can associate that provider with other public identities for
  limited scopes

## 2. Why Ownership Stays Higher-Layer

Ownership can mean different things:

- brand association
- operating control
- beneficial ownership
- payout beneficiary
- issuer identity

Those meanings should not be collapsed into the kernel artifact chain unless a
clear interoperability need appears.

## 3. Recommended Model

Model these concepts separately:

- provider: the Froglet key that signs protocol artifacts
- operator: the party running the service
- issuer: the party making public ownership or organizational claims
- beneficiary: the party intended to receive economic benefit
- listing: the market-level object exposed to users

## 4. Proof Levels

Recommended proof levels for future higher-layer services:

- self-asserted
- domain-linked
- marketplace-verified
- third-party-attested

These are presentation and trust-policy concerns above Froglet core.

## 5. Constraint

Do not change quote, deal, receipt, or settlement semantics to express
ownership-style claims at this stage.

If ownership claims are added later, prefer descriptor-linked or catalog-level
attestations first.
