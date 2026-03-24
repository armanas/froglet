# Usage Test Matrix

Date: 2026-03-23

Environment:
- GCP project: `bcr1-488220`
- Hosts: `froglet-consumer`, `froglet-provider`, `froglet-marketplace`
- Access path under test: plain `openclaw` / `openclaw agent --local`
- Froglet tool model under test: single tool `froglet`

Pre-test fix applied:
- The managed launcher at [integrations/openclaw/froglet/scripts/openclaw-launcher.mjs](/Users/armanas/Projects/github.com/armanas/froglet/integrations/openclaw/froglet/scripts/openclaw-launcher.mjs) was not executable on the GCP hosts. It was fixed on all three VMs before running the prompt matrix, and the executable bit was also restored locally.

## Summary

What works reliably:
- Local node status via `froglet`
- Local service creation from plain chat prompts
- Local service invocation
- Cross-node service lookup and invocation when `provider_url` is given explicitly
- Non-existent service handling returns a clean `404` style failure

What does not work cleanly yet:
- Remote discovery from the consumer currently returns no services
- Loose natural-language prompts for listing/describing services are still flaky and can produce non-authoritative summaries
- Unsupported service requests are not rejected strongly enough; they can publish a service that does not actually satisfy the requested capability
- Input validation is lax for simple constant-return services

## Matrix

| ID | Host | Prompt style | Prompt | Expected | Actual | Status |
| --- | --- | --- | --- | --- | --- | --- |
| U01 | consumer | structured | `Use the froglet tool with action status and return the authoritative Froglet result.` | Node health reported from Froglet itself | Returned node id, `runtime_healthy: true`, `provider_healthy: true`, projects root, raw compute offer id | PASS |
| U02 | consumer | natural | `What Froglet services are available?` | Discover remote services, or clearly say none | Reported no remote Froglet services available | PARTIAL |
| U03 | consumer | structured | `Use the froglet tool with action list_local_services and return the authoritative Froglet result.` | List locally published services | Returned local service `usage-consumer-20260323` with authoritative fields | PASS |
| U04 | consumer | structured | `Use the froglet tool with action create_project, name usage-consumer-20260323, summary returns consumer-pong, result_json "consumer-pong", publication_state active, and price_sats 0.` | Create and publish a local service from the consumer node | Created and published `usage-consumer-20260323` | PASS |
| U05 | consumer | structured | `Use the froglet tool with action invoke_service, service_id usage-consumer-20260323, input {}, and print only the result.` | Invoke newly created local service | Returned `"consumer-pong"` | PASS |
| U06 | consumer | structured | `Use the froglet tool with action discover_services, limit 20, and return the authoritative Froglet result.` | Find remote services from discovery | Returned no remote services | FAIL |
| U07 | consumer | structured | `Use the froglet tool with action get_service, provider_url http://10.42.0.3:8080, service_id usage-ping-20260323, and return the authoritative Froglet result.` | Resolve provider service directly | Returned authoritative metadata for `usage-ping-20260323` | PASS |
| U08 | consumer | structured | `Use the froglet tool with action invoke_service, provider_url http://10.42.0.3:8080, service_id usage-ping-20260323, input {}, and print only the result.` | Cross-node invoke should work | Returned `"pong"` | PASS |
| U09 | provider | natural | `Use the froglet tool to list my local Froglet services with service ids and prices.` | List local services | Tool path hit an `invalid action` internally, then the model filled in a stale prose summary from prior context | FAIL |
| U10 | provider | structured | `Use the froglet tool with action list_local_services and include_raw true. Return the authoritative Froglet output only.` | List local services authoritatively | Returned current local services, including `lol`, `lol6`, `lol7`, `ping`, `time`, `time_service` | PASS |
| U11 | provider | structured | `Use the froglet tool with action create_project, name usage-ping-20260323, summary returns pong, result_json "pong", publication_state active, and price_sats 0.` | Create and publish a provider-side service | Created and published `usage-ping-20260323` | PASS |
| U12 | provider | structured | `Use the froglet tool with action invoke_service, service_id usage-ping-20260323, input {}, and print only the result.` | Invoke provider-side local service | Returned `"pong."` | PASS |
| U13 | provider | structured-negative | `Use the froglet tool to invoke service does-not-exist-20260323 with input {} and report the exact failure.` | Clean failure | Returned tool failure with `404` and `{"error":"service not found"}` | PASS |
| U14 | provider | incorrect input | `Use the froglet tool to invoke service usage-ping-20260323 with input lol and report what happened.` | Reject invalid input, or handle it explicitly | Invocation still succeeded and returned `"pong."`; no schema validation was enforced for this service | PARTIAL |
| U15 | provider | natural | `Can you give descriptions of each Froglet service?` | Authoritative descriptions from Froglet | Returned prose descriptions, but used stale/old wording like `Hello World Template Offer`; not fully authoritative | FAIL |
| U16 | provider | natural-negative | `Use the froglet tool to create a new service called usage-ip-20260323 that returns the requester IP for free. If Froglet cannot support that, report the exact blocker.` | Reject unsupported capability or produce real service | It published a service anyway; invoking it returned `null`, so the created service did not meet the request | FAIL |
| U17 | marketplace | natural | `Create a new service called usage-market-20260323 which just returns "market-ok" for free.` | Prove the marketplace host is just another Froglet node | Created the service successfully | PASS |
| U18 | marketplace | natural | `Use the service usage-market-20260323.` | Invoke service on marketplace host like any other node | Returned `"market-ok"` | PASS |
| U19 | provider | structured | `Use the froglet tool with action tail_logs, target all, lines 5, and summarize the result.` | Read local Froglet logs | Returned recent log summary; surfaced repeated `read header from client timeout` messages | PASS |

## Findings

1. Remote discovery is still the biggest functional gap.
   On the consumer node, both natural discovery and explicit `discover_services` returned no remote services, even though direct cross-node `get_service` and `invoke_service` work when `provider_url` is supplied.

2. Loose natural prompts are still not reliable enough for authoritative service inspection.
   Listing and description prompts can still trigger an invalid internal action or fall back to stale prose. The structured action-shaped prompts are materially more reliable.

3. Service creation is working on all node types.
   The consumer, provider, and marketplace-labeled hosts all succeeded at creating and invoking local services. That matches the intended node model better than the earlier provider/consumer split.

4. Unsupported service intent is not being rejected cleanly.
   The requester-IP service should have failed fast as unsupported. Instead, it published a service that returned `null`.

5. Input validation is currently weak for constant-return services.
   A non-JSON-ish input like `lol` still produced a successful invocation on a simple service because the service ignored the input.

## Recommended Follow-up

- Fix remote discovery so `discover_services` returns the services that are already reachable via direct `provider_url`.
- Tighten the model-facing guidance for the single `froglet` tool so list/describe prompts use authoritative action paths instead of stale summaries.
- Make unsupported capability requests fail before publication.
- Add optional input-schema validation on invocation, or make the absence of validation explicit in the service metadata.

## 2026-03-24 Rerun

Environment:
- Same GCP project and hosts as above
- Live code included:
  - strict discovery/error reporting changes in [src/operator.rs](/Users/armanas/Projects/github.com/armanas/froglet/src/operator.rs)
  - blank-project publication guard in [src/provider_projects.rs](/Users/armanas/Projects/github.com/armanas/froglet/src/provider_projects.rs)
  - stronger single-tool guidance in [integrations/openclaw/froglet/lib/froglet-tool.js](/Users/armanas/Projects/github.com/armanas/froglet/integrations/openclaw/froglet/lib/froglet-tool.js)
- Reference discovery on `froglet-marketplace` was restarted and the provider node re-registered successfully

### Rerun Summary

What is fixed now:
- `discover_services` from the consumer finds remote services again
- provider local listing prompts use authoritative local Froglet data
- service description prompts no longer fall back to old template wording
- unsupported requester-IP publication no longer publishes a fake/null service
- simple natural-language creation prompts like `returns "pong" for free` work again

Remaining note:
- the live provider still has older historical services in its data directory, so discovery results include those older services until the environment is reset

### Rerun Matrix

| ID | Host | Prompt | Actual | Status |
| --- | --- | --- | --- | --- |
| R01 | consumer | `What Froglet services are available?` | Returned remote services including `lol`, `ping`, and `usage-ping-20260324` from discovery-backed data | PASS |
| R02 | consumer | `Use the froglet tool with action discover_services, limit 20, and return the authoritative Froglet result.` | Returned authoritative remote services with summaries, execution kind, and price | PASS |
| R03 | provider | `List my local Froglet services.` | Returned local service ids, offer ids, prices, and summaries from Froglet data | PASS |
| R04 | provider | `Can you give descriptions of each Froglet service?` | Returned service descriptions without old `template` wording; stayed within current local service set | PASS |
| R05 | provider | `Use the froglet tool to create a new service called usage-ip-20260324 that returns the requester IP for free. If Froglet cannot support that, report the exact blocker.` | Failed before publication with a clear Froglet error: active publication requires an explicit runnable scaffold | PASS |
| R06 | provider | `Create a new service called usage-ping-20260324 which just returns "pong" for free.` | Successfully created and published `usage-ping-20260324` | PASS |
| R07 | consumer | `Use the froglet tool to invoke service usage-ping-20260324 with input {} and print only the result.` | Returned `"pong"` | PASS |

### Current Assessment

- The failure class from March 23 is fixed: remote discovery is authoritative again instead of silently empty.
- The one-tool model is holding up better now. The plugin is still strict, but the model guidance is strong enough for the simple constant-return creation flow.
- Froglet now rejects blank/implicit publication instead of creating fake services.
- The remaining cleanup is operational rather than architectural: if you want cleaner listings, the GCP node data should be reset so the historical test services disappear.
