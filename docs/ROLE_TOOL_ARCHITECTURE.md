# Froglet Service Tool Architecture

This is the implemented cutover architecture.

## Product Model

- OpenClaw and NemoClaw expose one tool: `froglet`
- a Froglet node can both consume and provide
- a published thing is either:
  - a named service
  - raw compute
- marketplace is not a special product; it is just Froglet services published by
  another Froglet node

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
   - `summary`
   - `execution_kind`
   - `abi_version`
   - `mode`
   - price and publication state
   - optional schemas
   - binding information needed to compile a service invocation into a normal
     Froglet workload

3. Local control API
   - `/v1/froglet/*`
   - project authoring
   - build/test/publish
   - discovery and invocation
   - logs and restart

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
