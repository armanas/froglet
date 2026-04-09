# Froglet Service Tool Architecture

This is the implemented cutover architecture.

## Product Model

- OpenClaw and NemoClaw expose one tool: `froglet`
- external agent hosts can expose the same surface through the MCP server
- a Froglet node can both consume and provide
- a published thing is either:
  - a named service
  - a data-service binding
  - a direct compute offer
- named/data service discovery flows through `discover_services`
- open-ended compute uses the direct compute offer and `run_compute`
- marketplace is not a special product; it is just Froglet services published by
  another Froglet node
- identity is part of the core signed protocol; marketplace ranking, incentive,
  and trust policy are higher-layer behavior

## Layers

1. Froglet protocol
   - identity
   - signed artifacts
   - quotes
   - deals
   - receipts
   - payment and settlement

2. Service manifest layer
   - `service_id`
   - `offer_id`
   - `offer_kind`
   - `resource_kind`
   - `summary`
   - `runtime`
   - `package_kind`
   - `entrypoint_kind`
   - `entrypoint`
   - `mode`
   - price and publication state
   - optional schemas
   - binding information needed to compile a service invocation into a normal
     Froglet workload

3. Provider and runtime APIs
   - `/v1/provider/*` — catalog, deals, artifacts, settlement
   - `/v1/runtime/*` — search, deals, payments
   - `/v1/node/*` — jobs, events, capabilities

4. Plugin
   - one plugin id: `froglet`
   - one tool: `froglet`

## Why This Fixes The Earlier Failure

The old system published named offers but still required the bot to infer raw
compute payloads manually. That is why a discovered service like `hello-live`
was not naturally callable.

The new system adds an explicit service manifest and a service invocation path.
`invoke_service` resolves the service manifest, builds the correct underlying
workload spec, and then uses the unchanged Froglet deal protocol underneath.
