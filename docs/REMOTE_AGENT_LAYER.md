# Remote Agent Layer

Status: planned later, post-alpha product layer for batch and orchestration work

This document defines how Froglet should grow into fuller remote-agent execution without widening the version 1 economic kernel.

## 1. Goal

The goal is to support richer agent behavior:

- batch processing
- longer-running tasks
- multi-step workflows
- resumable sessions
- tool selection and retries
- user-visible progress

The wrong way to get there would be to mutate the v1 kernel into a session protocol.
The right way is to keep the kernel small and compose richer behavior on top of it.

## 2. What Must Stay Fixed

The following remain the stable kernel:

- signed `Descriptor`, `Offer`, `Quote`, `Deal`, and `Receipt`
- exact hashing and signing rules
- exact settlement commitments
- exact terminal receipt semantics
- canonical `compute.wasm.v1` workload identity

Remote-agent execution must not require:

- mutable in-flight deals
- streaming receipt updates inside signed kernel artifacts
- long-lived leased kernel sessions
- new kernel trust roles
- adapter-specific settlement shortcuts

If a remote-agent design requires any of those, it is pushing product concerns into the wrong layer.

## 3. Proposed Layering

The remote-agent layer should be a product/service layer above the runtime:

- the kernel remains the unit of economic commitment
- the runtime remains the localhost bot/operator surface
- the remote-agent layer becomes an orchestrator that composes many short kernel deals

The orchestrator may be:

- a local controller process beside the bot
- a provider-managed service
- a requester-managed service

But it is not itself part of the v1 kernel contract.

## 4. Recommended Model

The recommended model is:

- one economically meaningful step equals one Froglet deal
- a larger remote task becomes a workflow of deals
- workflow state is local product state, not kernel state

That means:

- progress is tracked in a workflow transcript
- retries create new deals
- partial results are retained as local evidence or referenced outputs
- final user-visible success is a workflow judgment built from multiple terminal receipts

This keeps receipts honest.
Each receipt proves one bounded interaction, not an open-ended session claim.

## 5. What the Remote-Agent Layer May Add

Without changing the kernel, the remote-agent layer may add:

- workflow IDs
- step graphs
- prompts or task plans
- local checkpoints
- resumable orchestrator state
- richer result references
- policy about retries, fallbacks, and tool choice
- user-facing progress events

Those are all product-level structures.
They should live in:

- runtime-local state
- archive bundles
- separate higher-layer services

They should not be promoted into signed kernel artifacts until real interoperability pressure proves they belong there.

## 6. What Should Count as a Session

A “session” in the future product should mean:

- a controller-level grouping of deals
- a state machine owned by the runtime or orchestrator
- a retained transcript over many receipts

It should not mean:

- a single open Froglet deal with mutable scope
- a single payment lock stretched across arbitrary runtime
- a single receipt standing in for many steps

The kernel is intentionally better at short, bounded commitments than at long mutable sessions.
The remote-agent layer should respect that instead of fighting it.

## 7. Settlement Guidance

For longer workflows:

- settle each economically meaningful step independently
- prefer short hold windows
- stage larger jobs into smaller deals

Do not:

- keep one success-fee hold open for an unbounded interactive session
- let a remote-agent protocol depend on hidden wallet state instead of terminal receipts

If a future workflow needs staged payments or leases, that should be evaluated as a post-v1 extension on top of the same kernel, not backported into v1.

## 8. Evidence and Audit

The remote-agent layer should produce:

- a workflow transcript
- links to the underlying deal IDs and receipt hashes
- optional archive exports for each step
- local progress or checkpoint records

The authoritative economic evidence still remains:

- signed deals
- signed receipts
- retained archive evidence

Workflow summaries are helpful, but they are derived views.

## 9. What to Build Later

When work resumes on this layer, the first implementation target should be a local orchestrator profile:

1. define a workflow transcript format outside the kernel
2. map each step to an ordinary Froglet deal
3. retain step archives and terminal receipts
4. expose workflow status through runtime-local APIs

Only after that should Froglet evaluate:

- chunked work protocols
- staged payment policies
- leases or reservations across many steps
- checkpoint and resume semantics across providers

## 10. Boundary Rule

The boundary rule is simple:

- if it changes hashes, signatures, quote/deal/receipt meaning, or offline verification, it belongs in the kernel and should be treated as expensive
- if it changes workflow UX, orchestration, retries, checkpoints, or progress reporting, it belongs in the remote-agent layer and should stay out of the kernel

That is how Froglet can support fuller agent products later without corrupting the version 1 primitive.
