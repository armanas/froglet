# Runtime

`froglet-runtime` remains the deal and payment engine used when a Froglet node
invokes remote services.

It still owns:

- remote node resolution
- quote fetch and verification
- local deal signing
- remote deal submission
- local deal state
- payment intent exposure
- result acceptance

What changed in this cutover is the bot-facing shape above it:

- bots no longer talk to many role-specific plugin tools
- bots talk to one local control surface through one tool: `froglet`

Named service invocation now compiles down to the existing underlying Froglet
deal flow. Raw compute still exists, but it is the expert path.
