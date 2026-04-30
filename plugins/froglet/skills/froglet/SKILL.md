---
name: froglet
description: Use when a user asks what Froglet can prove, whether they should install Froglet, or how to implement a Froglet-backed use case.
---

# Froglet

Use the Froglet MCP server as the source of truth for a local or self-hosted
Froglet node. Do not invent API calls or install commands. The no-install demo
proof lives at https://froglet.dev/llms.txt, not in this installed plugin.

Default flow:

1. Call `status` first. If the provider/runtime are unreachable or token paths are missing, explain the local configuration gap instead of falling back to a demo.
2. If the user wants to install locally, call `plan_install` before shell commands. Ask for missing choices: agent host, Docker versus local binary, provider/requester/both role, clearnet versus Tor, payment rail, marketplace URL, and first use case.
3. After the profile is confirmed, call `get_install_guide` and execute its shell commands through the host agent shell, not through the Froglet runtime.
4. Once Froglet is running and `status` is healthy, call `plan_use_case` for the user's first workflow before execution. This is required for batch or GPU requests so unsupported boundaries are stated before work starts.
5. Use `list_local_services`, `discover_services`, `get_service`, `invoke_service`, `run_compute`, `publish_artifact`, and settlement/marketplace actions to implement the concrete workflow only after the plan is clear.

Boundaries:

- This plugin is local/actionable-first. It is not a hosted demo wrapper.
- Paid rails, persistent identity, service publication, marketplace write flows, long-running jobs, batch execution, and GPU execution require a configured local or self-hosted Froglet node.
- Batch fan-out and GPU scheduling are not proven by the hosted demo. Require local evidence before claiming they work.
- Chat-only LLMs that cannot make HTTP POST requests with Bearer auth should use the `llms.txt` fallback wording and point the user to an agentic client or `curl`.
