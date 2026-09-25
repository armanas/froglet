---
name: froglet
description: Use when a user asks what Froglet can prove, whether they should install Froglet, or how to implement a Froglet-backed use case.
---

# Froglet

Use the Froglet MCP server as the source of truth for a local or self-hosted
Froglet node. Do not invent API calls or install commands. The no-install demo
proof lives at https://froglet.dev/llms.txt, not in this installed plugin.

For the native catalog-sharing journey, use `froglet-node mcp` with Codex or
Claude Code on macOS arm64 or Linux x86_64/arm64. Read
https://froglet.dev/learn/share-services/. Call `prepare_service` to inspect and
materialize an explicitly selected snapshot, then use the canonical two-call
`marketplace_publish` consent flow. Use `doctor`, `check_updates`, and
`open_status` for recovery. Native `invoke_service` accepts remote provider and
service identities and enforces free-only remote calls. Do not install Node/npm
or invent manifests for this journey. Check tools/list to distinguish native
capabilities from the optional JavaScript compatibility actions below.
Installation and exact public publication need separate approvals. Keep source
metadata private, expose only selected snapshot bytes, and never republish on a
source change without a new preview and approval. A shell probe is not evidence
that the actual agent connected. Keep local verification, public reachability,
marketplace activation, and recipient execution distinct. The candidate is not
a released, clean-machine-qualified product until those checks are recorded.

JavaScript compatibility flow:

1. Call `status` first. If the provider/runtime are unreachable or token paths are missing, explain the local configuration gap instead of falling back to a demo.
2. If the user wants to install locally, call `plan_install` before shell commands. Ask for missing choices: agent host, Docker versus local binary, provider/requester/both role, clearnet versus Tor, payment rail, marketplace URL, and first use case.
3. Present the complete immutable plan, hashes, filesystem/process impact, exact command preview, and approval hash to the user. Stop and wait for an explicit approval response; never relay the approval hash automatically in the same turn.
4. Only after that approval, pass the exact returned `release_tag` and `install_approval_hash` to `get_install_guide`, then execute its shell commands through the host agent shell, not through the Froglet runtime.
5. Once Froglet is running and `status` is healthy, call `plan_use_case` for the user's first workflow before execution. This is required for batch or GPU requests so unsupported boundaries are stated before work starts.
6. Use `list_local_services`, `discover_services`, `get_service`, `invoke_service`, `run_compute`, `publish_artifact`, and settlement/marketplace actions to implement the concrete workflow only after the plan is clear.

Boundaries:

- This plugin is local/actionable-first. It is not a hosted demo wrapper.
- Paid rails, persistent identity, service publication, marketplace write flows, long-running jobs, batch execution, and GPU execution require a configured local or self-hosted Froglet node.
- Batch fan-out and GPU scheduling are not proven by the hosted demo. Require local evidence before claiming they work.
- Chat-only LLMs that cannot make HTTP POST requests with Bearer auth should use the `llms.txt` fallback wording and point the user to an agentic client or `curl`.
