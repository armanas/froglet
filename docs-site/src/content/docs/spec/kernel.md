---
title: Kernel Specification
description: The normative protocol specification.
---

:::caution
This is a summary. The full normative specification is in [`docs/KERNEL.md`](https://github.com/armanas/froglet/blob/main/docs/KERNEL.md) in the froglet repository.
:::

## Scope

Froglet v1 is a small economic primitive for short-lived, bounded, fixed-price resource deals. A deal may represent a predefined service, a data service, or open-ended compute. These are product-layer distinctions over the same signed economic primitive.

## Global constants

- `schema_version`: always `froglet/v1`
- Hashes: lowercase hex SHA-256
- Timestamps: Unix seconds
- Canonical JSON: RFC 8785 JCS
- Identities: 32-byte secp256k1 x-only public keys (lowercase hex)
- Signatures: 64-byte BIP340 Schnorr (lowercase hex)

## Six artifact types

1. **Descriptor** — provider identity and capabilities
2. **Offer** — specific service with pricing and execution profile
3. **Quote** — priced workload for a specific requester (ephemeral)
4. **Deal** — requester commitment (signed by requester)
5. **InvoiceBundle** — Lightning payment instructions (two legs)
6. **Receipt** — terminal proof of execution and settlement

## Settlement methods

- `none` — free execution, no payment
- `lightning.base_fee_plus_success_fee.v1` — two-leg Lightning settlement

## What stays out

The kernel does not hardwire: marketplace, discovery, transport, storage engine, execution runtime, ranking/broker logic, deployment topology, or long-running sessions.
