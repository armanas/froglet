# Froglet Runtime

Status: non-normative supporting document

This document covers the localhost bot-facing runtime.
The runtime is a product surface layered on top of the kernel in [`../SPEC.md`](../SPEC.md).
The current supported product boundary for that surface is defined in [`BOT_RUNTIME_ALPHA.md`](BOT_RUNTIME_ALPHA.md).

## 1. Happy Path

The intended runtime workflow is:

- search
- quote
- deal
- wait
- accept or reject
- receipt

The runtime should hide relay policy, transport routing, and raw invoice parsing on the happy path, while still allowing advanced callers to inspect them.

## 2. Local Authentication

The localhost runtime must require local authentication for privileged requests.
Binding to localhost is not a sufficient trust boundary.

## 3. Runtime-local Handles

The runtime may expose local handles such as:

- `deal_id`
- payment-intent identifiers
- local archive/export identifiers
- polling cursors

Those are implementation details.
The kernel evidence chain is still anchored by `artifact_hash`, not by runtime handles.

## 4. Runtime-local Deal States

The runtime may expose local statuses that are useful operationally but are not canonical protocol states.

Two important v1 examples are:

- `payment_pending`
- `result_ready`

Their projections are:

- `payment_pending` -> canonical `deal_state = opened`
- `result_ready` -> canonical `deal_state = admitted`, `execution_state = succeeded`, `settlement_state = funds_locked`

Runtime-local states must never appear inside signed artifacts.

## 5. Wallet-facing Helpers

The runtime may expose higher-level wallet helpers rather than raw invoice-bundle parsing.

Examples:

- payment intents derived from a validated `invoice_bundle`
- release-preimage helpers
- wallet inspection
- descriptor inspection

These helpers are encouraged because they simplify bot integration, but they are not part of the kernel.

## 6. Compatibility Endpoints

Reference implementations may retain compatibility helpers such as:

- `events.query`
- `execute.wasm`
- async job endpoints

When Lightning is the active settlement backend and a workload has a non-zero price, those helpers should reject inline payment shortcuts and direct callers to the `Quote -> Deal -> Receipt` flow instead.

## 7. Product Boundary

Long-lived remote-agent execution, leases, checkpoints, and session models are future layers on top of the same economic primitive.
They are not reasons to widen the version 1 kernel.
