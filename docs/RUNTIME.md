# Runtime

`froglet-runtime` remains the deal and payment engine used when a Froglet node
invokes remote Froglet resources.

It still owns:

- remote node resolution
- quote fetch and verification
- local deal signing
- remote deal submission
- local deal state
- payment intent exposure
- result acceptance

A single Froglet node may both publish local resources and invoke remote ones.
`provider` and `requester` remain per-deal roles, not node classes.

What changed in this cutover is the bot-facing shape above it:

- bots no longer talk to many role-specific plugin tools
- bots talk to one local control surface through one tool: `froglet`

Named services, data services, and open-ended compute all compile down to the
same underlying Froglet deal flow.

At the product surface:

- named and data-service bindings are discovered and invoked through service
  metadata
- open-ended compute uses the provider's direct compute offer
- bounded async execution is exposed through task polling
- longer-running orchestration, batch workflows, and checkpoint/resume remain a
  higher-layer concern described in `REMOTE_AGENT_LAYER.md`

Current implementation note:

- the checked-in execution profiles are current reference implementations
- the intended product boundary is a generic execution primitive that can back
  named services, data services, and open-ended compute
