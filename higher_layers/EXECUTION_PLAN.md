# Higher-Layer Execution Plan

This plan turns the public/private repo strategy into an execution order that
can be followed while work is still temporarily incubated in this repository.

## Phase 0: Boundary Lock

Goal:

- lock the public/private split before implementation pressure distorts it

Deliverables:

- public docs that define what stays open
- public docs that define what may remain closed
- ignored `private/` incubation layout
- explicit rule that private higher-layer work consumes only public Froglet
  interfaces

Status:

- current focus

## Phase 1: Public Foundation

Goal:

- make the public Froglet core easy to adopt and integrate against

Public deliverables:

- official Docker packaging for `froglet`
- optional Docker packaging for the reference `marketplace`
- documented public API surfaces for provider, runtime, and signed-artifact
  verification
- open OpenClaw integration targeting public and supported Froglet surfaces

Boundary rule:

- OpenClaw integration remains public even if it connects to closed first-party
  marketplace services later

Exit criteria:

- a third party can run Froglet, understand the protocol, and integrate through
  public tooling without needing access to private service code

## Phase 2: Private Marketplace Alpha

Goal:

- build the first closed-source commercial layer without changing the Froglet
  trust model

Private deliverables:

- marketplace product backend
- initial catalog/search projections
- seed listing policy and metadata overlays
- basic operator/admin tooling for the marketplace

Rules:

- ingest only through public APIs, signed artifacts, or documented contracts
- do not depend on Froglet SQLite internals
- do not require kernel changes for marketplace-only needs

Exit criteria:

- the marketplace can ingest and serve useful search/catalog data while
  remaining an ordinary Froglet consumer

## Phase 3: Private Indexer and Broker Layers

Goal:

- expand the commercial layer into richer network intelligence

Private deliverables:

- indexer-owned storage and projections
- broker and routing experiments
- ranking/reputation pipelines
- ownership / issuer overlays

Rules:

- keep routing, ranking, and reputation out of `SPEC.md`
- prefer signed outputs or attributable statements when higher-layer trust
  claims matter

Exit criteria:

- higher-layer services add value through data and policy, not through hidden
  protocol extensions

## Phase 4: Extraction

Goal:

- move private higher-layer services out of this repo once boundaries are
  stable

Move-out targets:

- `private/marketplace/`
- `private/indexer/`
- `private/broker/`
- other private higher-layer directories as they become real services

Extraction trigger:

- interface is documented
- data ownership is clear
- service cadence no longer matches the public core repo
- in-repo incubation would otherwise create coupling pressure

Exit criteria:

- the public Froglet repo remains cleanly focused on protocol, runtime, SDKs,
  and reference implementations
